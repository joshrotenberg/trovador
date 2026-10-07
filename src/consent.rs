//! Which existing sessions trovador may continue.
//!
//! Starting a new session is always allowed. Continuing one adds a turn to someone's
//! conversation, so it is gated. The state lives in `~/.trovador/consent.json`, not in the
//! index: the index is derived and can be deleted at any time, and this file is the only
//! record of which sessions trovador started.
//!
//! The MCP surface can read a check but never change the policy; that is done from the CLI
//! by the person at the machine.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::archive::Backend;
use crate::paths::Paths;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// No existing session may be continued.
    Deny,
    /// Only sessions trovador started.
    #[default]
    Spawned,
    /// Sessions trovador started, plus sessions approved one by one.
    Approved,
    /// Every session not explicitly denied.
    All,
}

impl std::str::FromStr for Level {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "deny" => Ok(Level::Deny),
            "spawned" => Ok(Level::Spawned),
            "approved" => Ok(Level::Approved),
            "all" => Ok(Level::All),
            other => Err(format!(
                "unknown consent level \"{other}\"; have: deny, spawned, approved, all"
            )),
        }
    }
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Deny => "deny",
            Level::Spawned => "spawned",
            Level::Approved => "approved",
            Level::All => "all",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Approved,
    Denied,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spawned {
    backend: Backend,
    session_id: String,
    at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionRecord {
    backend: Backend,
    session_id: String,
    decision: Decision,
    at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    level: Level,
    spawned: Vec<Spawned>,
    decisions: Vec<DecisionRecord>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConsentError {
    #[error(
        "consent state at {path} is invalid ({reason}); continuation is denied until it is repaired or removed, and nothing will overwrite it"
    )]
    Invalid { path: String, reason: String },
    #[error("consent state at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("session id must be 1 to 1024 characters without NUL")]
    BadId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// No state file yet; the default level applies.
    Default,
    File,
    /// The file could not be read or parsed. Everything is denied.
    Invalid,
}

/// Whether one session may be continued, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Check {
    pub backend: Backend,
    pub session_id: String,
    pub level: Level,
    pub source: Source,
    /// Started by trovador.
    pub spawned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    pub allowed: bool,
    pub explanation: String,
    /// CLI commands that would change the outcome. Run by a person, not by an agent.
    pub next_steps: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Status {
    pub path: String,
    pub level: Level,
    pub source: Source,
    pub spawned: usize,
    pub approved: usize,
    pub denied: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn path(paths: &Paths) -> PathBuf {
    paths.home.join("consent.json")
}

fn lock_path(paths: &Paths) -> PathBuf {
    paths.home.join("consent.lock")
}

fn now() -> String {
    humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string()
}

fn check_id(id: &str) -> Result<(), ConsentError> {
    if id.is_empty() || id.len() > 1024 || id.contains('\0') {
        return Err(ConsentError::BadId);
    }
    Ok(())
}

fn load(paths: &Paths) -> Result<(State, Source), ConsentError> {
    let p = path(paths);
    let raw = match fs::read_to_string(&p) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((
                State {
                    version: 1,
                    ..Default::default()
                },
                Source::Default,
            ));
        }
        Err(e) => {
            return Err(ConsentError::Invalid {
                path: p.display().to_string(),
                reason: e.to_string(),
            });
        }
    };
    let state: State = serde_json::from_str(&raw).map_err(|e| ConsentError::Invalid {
        path: p.display().to_string(),
        reason: e.to_string(),
    })?;
    if state.version != 1 {
        return Err(ConsentError::Invalid {
            path: p.display().to_string(),
            reason: format!("unsupported version {}", state.version),
        });
    }
    Ok((state, Source::File))
}

/// Read, change, and write the state under an exclusive lock, atomically and private to
/// the user. Invalid state is never overwritten: the person repairs or removes it.
fn update(paths: &Paths, change: impl FnOnce(&mut State) -> bool) -> Result<State, ConsentError> {
    let io = |source| ConsentError::Io {
        path: path(paths).display().to_string(),
        source,
    };
    fs::create_dir_all(&paths.home).map_err(io)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(paths))
        .map_err(io)?;
    lock.lock().map_err(io)?;
    let (mut state, _) = load(paths)?;
    if change(&mut state) {
        write_private(&path(paths), &state).map_err(io)?;
    }
    Ok(state)
}

fn write_private(target: &Path, state: &State) -> std::io::Result<()> {
    let tmp = target.with_extension(format!("json.{}.tmp", std::process::id()));
    {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f: File = opts.open(&tmp)?;
        f.write_all(serde_json::to_string_pretty(state)?.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, target)
}

fn same(b: Backend, id: &str) -> impl Fn(&Backend, &String) -> bool + '_ {
    move |rb, rid| *rb == b && rid == id
}

/// Records that trovador started this session. Called as soon as the id is known, before
/// the run finishes, so a run that times out can still be continued (spike#108).
pub fn record_spawned(paths: &Paths, backend: Backend, id: &str) -> Result<(), ConsentError> {
    check_id(id)?;
    update(paths, |s| {
        let is = same(backend, id);
        if s.spawned.iter().any(|r| is(&r.backend, &r.session_id)) {
            return false;
        }
        s.spawned.push(Spawned {
            backend,
            session_id: id.to_string(),
            at: now(),
        });
        true
    })
    .map(|_| ())
}

pub fn set_level(paths: &Paths, level: Level) -> Result<Status, ConsentError> {
    update(paths, |s| {
        let changed = s.level != level;
        s.level = level;
        changed
    })?;
    Ok(status(paths))
}

pub fn set_decision(
    paths: &Paths,
    backend: Backend,
    id: &str,
    decision: Option<Decision>,
) -> Result<Check, ConsentError> {
    check_id(id)?;
    update(paths, |s| {
        let is = same(backend, id);
        let before = s.decisions.len();
        s.decisions.retain(|r| !is(&r.backend, &r.session_id));
        if let Some(d) = decision {
            s.decisions.push(DecisionRecord {
                backend,
                session_id: id.to_string(),
                decision: d,
                at: now(),
            });
            return true;
        }
        s.decisions.len() != before
    })?;
    Ok(check(paths, backend, id))
}

pub fn status(paths: &Paths) -> Status {
    let p = path(paths).display().to_string();
    match load(paths) {
        Ok((s, source)) => Status {
            path: p,
            level: s.level,
            source,
            spawned: s.spawned.len(),
            approved: s
                .decisions
                .iter()
                .filter(|d| d.decision == Decision::Approved)
                .count(),
            denied: s
                .decisions
                .iter()
                .filter(|d| d.decision == Decision::Denied)
                .count(),
            error: None,
        },
        Err(e) => Status {
            path: p,
            level: Level::Deny,
            source: Source::Invalid,
            spawned: 0,
            approved: 0,
            denied: 0,
            error: Some(e.to_string()),
        },
    }
}

/// Never fails: unreadable state is a denial that says why.
pub fn check(paths: &Paths, backend: Backend, id: &str) -> Check {
    let target = format!("{id} --backend {backend}");
    let (state, source) = match load(paths) {
        Ok(loaded) => loaded,
        Err(e) => {
            return Check {
                backend,
                session_id: id.to_string(),
                level: Level::Deny,
                source: Source::Invalid,
                spawned: false,
                decision: None,
                allowed: false,
                explanation: e.to_string(),
                next_steps: vec!["trovador consent status".into()],
            };
        }
    };
    let is = same(backend, id);
    let spawned = state.spawned.iter().any(|r| is(&r.backend, &r.session_id));
    let decision = state
        .decisions
        .iter()
        .find(|r| is(&r.backend, &r.session_id))
        .map(|r| r.decision);
    let who = format!("{backend}/{id}");
    let (allowed, explanation, next_steps) = match (decision, state.level) {
        (Some(Decision::Denied), _) => (
            false,
            format!("{who} is explicitly denied"),
            vec![format!("trovador consent clear {target}")],
        ),
        (_, Level::Deny) => (
            false,
            "the consent level is deny".to_string(),
            vec!["trovador consent policy spawned".into()],
        ),
        (_, Level::All) => (true, "the consent level is all".to_string(), vec![]),
        _ if spawned => (true, format!("{who} was started by trovador"), vec![]),
        (Some(Decision::Approved), Level::Approved) => {
            (true, format!("{who} is explicitly approved"), vec![])
        }
        (Some(Decision::Approved), Level::Spawned) => (
            false,
            format!("{who} is approved, but the level is spawned, which ignores approvals"),
            vec!["trovador consent policy approved".into()],
        ),
        (None, Level::Spawned) => (
            false,
            format!("{who} was not started by trovador and the level is spawned"),
            vec![
                format!("trovador consent approve {target}"),
                "trovador consent policy approved".into(),
            ],
        ),
        (None, Level::Approved) => (
            false,
            format!("{who} was neither started by trovador nor approved"),
            vec![format!("trovador consent approve {target}")],
        ),
    };
    Check {
        backend,
        session_id: id.to_string(),
        level: state.level,
        source,
        spawned,
        decision,
        allowed,
        explanation,
        next_steps,
    }
}

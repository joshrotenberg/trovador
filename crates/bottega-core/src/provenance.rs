//! Which sessions the toolkit started.
//!
//! tosi records each session it starts here. trovador reads the record so that list and
//! search include those sessions, although automated sessions (`claude -p`, `codex exec`)
//! are otherwise left out by default. It is provenance only: past sessions are read-only
//! history, and nothing here decides whether one may be continued. The record lives in
//! `~/.trovador/spawned.json`, not in the index, because the index is derived and can be
//! deleted at any time.

use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Backend;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Started {
    backend: Backend,
    session_id: String,
    at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    spawned: Vec<Started>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProvenanceError {
    #[error(
        "the record of sessions the toolkit started, {path}, is invalid ({reason}); it is never overwritten, so repair or remove it"
    )]
    Invalid { path: String, reason: String },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("session id must be 1 to 1024 characters without NUL")]
    BadId,
}

/// What `trovador doctor` reports about the record.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Status {
    pub path: String,
    /// Sessions the toolkit started.
    pub spawned: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn path(paths: &Path) -> PathBuf {
    crate::state::provenance_path(paths)
}

fn lock_path(paths: &Path) -> PathBuf {
    paths.join("spawned.lock")
}

/// Before consent was removed, the same records were the `spawned` list in
/// `consent.json`. They are carried over the first time this record is read.
fn legacy_path(paths: &Path) -> PathBuf {
    paths.join("consent.json")
}

fn now() -> String {
    humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string()
}

fn check_id(id: &str) -> Result<(), ProvenanceError> {
    if id.is_empty() || id.len() > 1024 || id.contains('\0') {
        return Err(ProvenanceError::BadId);
    }
    Ok(())
}

fn legacy(paths: &Path) -> Vec<Started> {
    let Ok(raw) = fs::read_to_string(legacy_path(paths)) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    v.get("spawned")
        .and_then(|s| serde_json::from_value::<Vec<Started>>(s.clone()).ok())
        .unwrap_or_default()
}

fn load(paths: &Path) -> Result<State, ProvenanceError> {
    let p = path(paths);
    let invalid = |reason: String| ProvenanceError::Invalid {
        path: p.display().to_string(),
        reason,
    };
    match fs::read_to_string(&p) {
        Ok(raw) => {
            let state: State = serde_json::from_str(&raw).map_err(|e| invalid(e.to_string()))?;
            if state.version != 1 {
                return Err(invalid(format!("unsupported version {}", state.version)));
            }
            Ok(state)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State {
            version: 1,
            spawned: legacy(paths),
        }),
        Err(e) => Err(invalid(e.to_string())),
    }
}

/// Read, change, and write the record under an exclusive lock, private to the user. An
/// invalid record is never overwritten.
fn update(paths: &Path, change: impl FnOnce(&mut State) -> bool) -> Result<(), ProvenanceError> {
    let io = |source| ProvenanceError::Io {
        path: path(paths).display().to_string(),
        source,
    };
    crate::private::private_dir(paths).map_err(io)?;
    let lock = crate::private::private_file()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(paths))
        .map_err(io)?;
    lock.lock().map_err(io)?;
    let mut state = load(paths)?;
    // A record carried over from consent.json is written out the first time anything
    // changes, so it no longer depends on the old file.
    let migrating = !path(paths).exists() && !state.spawned.is_empty();
    if change(&mut state) || migrating {
        let json = serde_json::to_vec_pretty(&state).map_err(|e| io(e.into()))?;
        crate::private::write_private(&path(paths), &json).map_err(io)?;
    }
    Ok(())
}

/// Records that the toolkit started this session.
pub fn record_spawned(paths: &Path, backend: Backend, id: &str) -> Result<(), ProvenanceError> {
    check_id(id)?;
    update(paths, |s| add(s, backend, id, &now()))
}

fn add(s: &mut State, backend: Backend, id: &str, at: &str) -> bool {
    if s.spawned
        .iter()
        .any(|r| r.backend == backend && r.session_id == id)
    {
        return false;
    }
    s.spawned.push(Started {
        backend,
        session_id: id.to_string(),
        at: at.to_string(),
    });
    true
}

/// Adds every session in `incoming`, as one update. Every id is checked first, so a bad
/// one changes nothing. Returns how many were added and how many were already present; a
/// dry run reports the same counts without writing.
pub fn record_all(
    paths: &Path,
    incoming: &[(Backend, String)],
    dry_run: bool,
) -> Result<(usize, usize), ProvenanceError> {
    for (_, id) in incoming {
        check_id(id)?;
    }
    let apply = |s: &mut State| {
        let at = now();
        let mut added = 0;
        for (backend, id) in incoming {
            if add(s, *backend, id, &at) {
                added += 1;
            }
        }
        (added, incoming.len() - added)
    };
    if dry_run {
        return Ok(apply(&mut load(paths)?));
    }
    let mut counts = (0, 0);
    update(paths, |s| {
        counts = apply(s);
        counts.0 > 0
    })?;
    Ok(counts)
}

/// Whether the toolkit started this session. False when the record cannot be read.
pub fn is_spawned(paths: &Path, backend: Backend, id: &str) -> bool {
    load(paths).is_ok_and(|s| {
        s.spawned
            .iter()
            .any(|r| r.backend == backend && r.session_id == id)
    })
}

/// Every session the toolkit started, as `backend/id`. Empty when the record cannot be read.
pub fn spawned_keys(paths: &Path) -> Vec<String> {
    load(paths)
        .map(|s| {
            s.spawned
                .iter()
                .map(|r| format!("{}/{}", r.backend, r.session_id))
                .collect()
        })
        .unwrap_or_default()
}

pub fn status(paths: &Path) -> Status {
    let p = path(paths).display().to_string();
    match load(paths) {
        Ok(s) => Status {
            path: p,
            spawned: s.spawned.len(),
            error: None,
        },
        Err(e) => Status {
            path: p,
            spawned: 0,
            error: Some(e.to_string()),
        },
    }
}

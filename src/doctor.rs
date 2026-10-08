//! What `trovador doctor` checks: the first thing to run on a new machine.

use std::path::{Path, PathBuf};
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::archive::{self, Backend};
use crate::consent;
use crate::index::{self, IndexStatus};
use crate::limits::Limits;
use crate::sessions::Ctx;

/// Printed by the CLI only, so it is serialized but never read back.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub version: String,
    /// False when any problem is an error.
    pub ok: bool,
    pub problems: Vec<Problem>,
    pub providers: Vec<Provider>,
    pub archives: Vec<Archive>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexStatus>,
    pub consent: consent::Status,
    pub home: String,
    pub limits: Limits,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Problem {
    /// `error`: something trovador relies on is broken. `warn`: a feature is unavailable.
    pub severity: Severity,
    pub what: String,
    /// What to do about it.
    pub fix: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warn,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Provider {
    pub backend: Backend,
    /// The binary trovador would run, when it can be found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The first line of `--version`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Archive {
    pub name: String,
    pub path: String,
    pub readable: bool,
    pub sessions: usize,
}

/// Where the binary is: the configured override, else the first match on PATH.
fn resolve(configured: Option<&Path>, name: &str) -> Option<PathBuf> {
    match configured {
        Some(p) => p.is_file().then(|| p.to_path_buf()),
        None => std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|p| p.is_file())
        }),
    }
}

async fn provider(backend: Backend, configured: Option<&Path>) -> Provider {
    let name = backend.as_str();
    let Some(path) = resolve(configured, name) else {
        return Provider {
            backend,
            path: configured.map(|p| p.display().to_string()),
            version: None,
            error: Some(match configured {
                Some(p) => format!("{} does not exist", p.display()),
                None => format!("{name} is not on PATH"),
            }),
        };
    };
    let run = tokio::process::Command::new(&path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let (version, error) = match tokio::time::timeout(Duration::from_secs(10), run).await {
        Ok(Ok(out)) if out.status.success() => (
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty()),
            None,
        ),
        Ok(Ok(out)) => (None, Some(format!("--version exited with {}", out.status))),
        Ok(Err(e)) => (None, Some(e.to_string())),
        Err(_) => (None, Some("--version did not answer within 10 s".into())),
    };
    Provider {
        backend,
        path: Some(path.display().to_string()),
        version,
        error,
    }
}

pub async fn report(ctx: &Ctx) -> Report {
    let mut problems = Vec::new();
    let p = &ctx.paths;

    let providers = vec![
        provider(Backend::Claude, p.claude_bin.as_deref()).await,
        provider(Backend::Codex, p.codex_bin.as_deref()).await,
    ];
    for pr in &providers {
        if let Some(e) = &pr.error {
            problems.push(Problem {
                severity: Severity::Warn,
                what: format!("{}: {e}", pr.backend),
                fix: format!(
                    "ask and summarize need {0}; install it, or set TROVADOR_{1}_BIN. Reading and search work without it",
                    pr.backend,
                    pr.backend.as_str().to_uppercase()
                ),
            });
        }
    }

    let archives: Vec<Archive> = [
        (
            "claude",
            &p.claude_archive,
            archive::scan_claude(&p.claude_archive).len(),
        ),
        (
            "codex",
            &p.codex_archive,
            archive::scan_codex(&p.codex_archive, false).len(),
        ),
        (
            "codex archived",
            &p.codex_archived,
            archive::scan_codex(&p.codex_archived, true).len(),
        ),
    ]
    .into_iter()
    .map(|(name, path, sessions)| Archive {
        name: name.into(),
        path: path.display().to_string(),
        readable: std::fs::read_dir(path).is_ok(),
        sessions,
    })
    .collect();
    if archives.iter().all(|a| !a.readable) {
        problems.push(Problem {
            severity: Severity::Error,
            what: "no session archive can be read".into(),
            fix: "check TROVADOR_CLAUDE_ARCHIVE and TROVADOR_CODEX_ARCHIVE, or run claude or codex once".into(),
        });
    }

    let index = crate::sessions::index_status(ctx);
    if index.is_none() {
        problems.push(Problem {
            severity: Severity::Warn,
            what: if index::db_path(p).exists() {
                "the index was built by another version and will be rebuilt".into()
            } else {
                "no search index yet".into()
            },
            fix: "run trovador index once; searches keep it current afterwards".into(),
        });
    }

    let consent = consent::status(p);
    if consent.source == consent::Source::Invalid {
        problems.push(Problem {
            severity: Severity::Error,
            what: format!(
                "consent state cannot be read: {}",
                consent.error.clone().unwrap_or_default()
            ),
            fix: format!(
                "repair or remove {}; until then no existing session can be continued",
                consent.path
            ),
        });
    }

    Report {
        version: env!("CARGO_PKG_VERSION").into(),
        ok: !problems.iter().any(|p| p.severity == Severity::Error),
        problems,
        providers,
        archives,
        index,
        consent,
        home: p.home.display().to_string(),
        limits: ctx.limits.clone(),
    }
}

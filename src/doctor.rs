//! What `trovador doctor` checks: the first thing to run on a new machine.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::archive;
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
    pub archives: Vec<Archive>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexStatus>,
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
pub struct Archive {
    pub name: String,
    pub path: String,
    pub readable: bool,
    pub sessions: usize,
}

pub fn report(ctx: &Ctx) -> Report {
    let mut problems = Vec::new();
    let p = &ctx.paths;
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

    Report {
        version: env!("CARGO_PKG_VERSION").into(),
        ok: !problems.iter().any(|p| p.severity == Severity::Error),
        problems,
        archives,
        index,
        home: p.home.display().to_string(),
        limits: ctx.limits.clone(),
    }
}

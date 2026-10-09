//! A one-time import of what spike (`agent-sessions`) kept under `~/.agent-sessions`.
//!
//! Read only: nothing under the spike directory is changed, so spike keeps working and
//! removing trovador later leaves spike exactly as it was.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::archive::{self, Backend};
use crate::consent::{self, Decision, Level};
use crate::sessions::Ctx;
use crate::summary::{self, Summary};

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ImportReport {
    pub from: String,
    pub dry_run: bool,
    /// spike's level, and whether it was adopted. It is adopted only when trovador had no
    /// consent state of its own, so an import never loosens or tightens a policy that a
    /// person already chose here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    pub level_adopted: bool,
    pub spawned_added: usize,
    pub spawned_present: usize,
    pub decisions_added: usize,
    /// spike had a decision for a session trovador also has one for; trovador's was kept.
    pub decisions_kept_trovador: usize,
    pub summaries_imported: usize,
    pub summaries_present: usize,
    /// The session changed after spike summarized it, or is gone.
    pub summaries_stale: usize,
    pub summaries_unreadable: usize,
    /// Everything else spike kept, by kind, with entry counts. Not imported.
    pub not_imported: BTreeMap<String, usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{0} is not a spike state directory (no consent.json or summaries/)")]
    NotSpike(String),
    #[error("spike consent state at {path} cannot be read: {reason}")]
    Consent { path: String, reason: String },
    #[error(transparent)]
    Write(#[from] consent::ConsentError),
}

pub fn default_from() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".agent-sessions")
}

#[derive(Deserialize)]
struct SpikeConsent {
    level: Level,
    #[serde(default)]
    spawned: Vec<SpikeRecord>,
    #[serde(default)]
    decisions: Vec<SpikeDecision>,
}

#[derive(Deserialize)]
struct SpikeRecord {
    backend: Backend,
    session_id: String,
}

#[derive(Deserialize)]
struct SpikeDecision {
    backend: Backend,
    session_id: String,
    decision: Decision,
}

pub fn import(ctx: &Ctx, from: &Path, dry_run: bool) -> Result<ImportReport, ImportError> {
    let consent_file = from.join("consent.json");
    let summaries_dir = from.join("summaries");
    if !consent_file.is_file() && !summaries_dir.is_dir() {
        return Err(ImportError::NotSpike(from.display().to_string()));
    }
    let mut report = ImportReport {
        from: from.display().to_string(),
        dry_run,
        ..Default::default()
    };

    if consent_file.is_file() {
        let bad = |reason: String| ImportError::Consent {
            path: consent_file.display().to_string(),
            reason,
        };
        let raw = std::fs::read_to_string(&consent_file).map_err(|e| bad(e.to_string()))?;
        let spike: SpikeConsent = serde_json::from_str(&raw).map_err(|e| bad(e.to_string()))?;
        report.level = Some(spike.level);
        let incoming = consent::Incoming {
            level: Some(spike.level),
            spawned: spike
                .spawned
                .into_iter()
                .map(|r| (r.backend, r.session_id))
                .collect(),
            decisions: spike
                .decisions
                .into_iter()
                .map(|d| (d.backend, d.session_id, d.decision))
                .collect(),
        };
        let merged = consent::merge(&ctx.paths, &incoming, dry_run).map_err(|e| match e {
            consent::ConsentError::BadId => bad(format!(
                "a spawned or decision record has an invalid session id ({e}); nothing was imported"
            )),
            other => other.into(),
        })?;
        report.level_adopted = merged.level_adopted;
        report.spawned_added = merged.spawned_added;
        report.spawned_present = merged.spawned_present;
        report.decisions_added = merged.decisions_added;
        report.decisions_kept_trovador = merged.decisions_kept;
    }

    if let Ok(files) = std::fs::read_dir(&summaries_dir) {
        for f in files.flatten() {
            let path = f.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            match import_summary(ctx, &path, dry_run) {
                Some(Outcome::Imported) => report.summaries_imported += 1,
                Some(Outcome::Present) => report.summaries_present += 1,
                Some(Outcome::Stale) => report.summaries_stale += 1,
                None => report.summaries_unreadable += 1,
            }
        }
    }

    for kind in [
        "standing.json",
        "pads",
        "blobs",
        "windows",
        "runners",
        "reports",
    ] {
        let p = from.join(kind);
        let n = if p.is_dir() {
            std::fs::read_dir(&p).map(|d| d.count()).unwrap_or(0)
        } else if p.is_file() {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| v.as_object().map(|o| o.len()))
                .unwrap_or(1)
        } else {
            continue;
        };
        report
            .not_imported
            .insert(kind.trim_end_matches(".json").to_string(), n);
    }
    Ok(report)
}

enum Outcome {
    Imported,
    Present,
    Stale,
}

#[derive(Deserialize)]
struct SpikeSummary {
    session_id: String,
    backend: Backend,
    project: Option<String>,
    /// Milliseconds since the epoch: the source file's mtime when spike summarized it.
    source_mtime: f64,
    #[serde(default)]
    chunks: usize,
    #[serde(default)]
    transcript_chars: usize,
    #[serde(default)]
    cost_usd: f64,
    created: String,
    summary: String,
}

fn import_summary(ctx: &Ctx, path: &Path, dry_run: bool) -> Option<Outcome> {
    let raw = std::fs::read_to_string(path).ok()?;
    let s: SpikeSummary = serde_json::from_str(&raw).ok()?;
    let Ok(e) = archive::find_session(&ctx.paths, &s.session_id, Some(s.backend)) else {
        return Some(Outcome::Stale);
    };
    if summary::cached(ctx, &e).is_some() {
        return Some(Outcome::Present);
    }
    // spike treated a summary as current while the file had not been written since; the
    // same test here, at millisecond resolution because that is what spike stored.
    let now_ms = summary::mtime_ns(e.mtime) / 1_000_000;
    if (s.source_mtime as i64) < now_ms {
        return Some(Outcome::Stale);
    }
    if !dry_run {
        let imported = Summary {
            backend: e.backend,
            session_id: e.session_id.clone(),
            project: s.project,
            summary: s.summary,
            cached: false,
            cost_usd: s.cost_usd,
            model: "imported from spike".into(),
            chunks: s.chunks,
            chunks_summarized: s.chunks,
            transcript_chars: s.transcript_chars,
            created: s.created,
            source_size: e.size_bytes,
            source_mtime_ns: summary::mtime_ns(e.mtime),
            prompt_version: summary::PROMPT_VERSION,
            truncated: None,
        };
        summary::store(ctx, &imported).ok()?;
    }
    Some(Outcome::Imported)
}

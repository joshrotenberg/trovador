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
use crate::provenance;
use crate::sessions::Ctx;
use crate::summary::{self, Summary};

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ImportReport {
    pub from: String,
    pub dry_run: bool,
    /// Sessions spike started, added to the record of sessions the toolkit started.
    pub spawned_added: usize,
    pub spawned_present: usize,
    /// spike's consent level and per-session decisions, which have no counterpart here:
    /// past sessions are read-only history. Counted, not imported.
    pub decisions_ignored: usize,
    pub summaries_imported: usize,
    pub summaries_present: usize,
    /// The session changed, is gone, or is a forked Codex child with inherited context.
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
    Write(#[from] provenance::ProvenanceError),
}

pub fn default_from() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".agent-sessions")
}

#[derive(Deserialize)]
struct SpikeConsent {
    #[serde(default)]
    spawned: Vec<SpikeRecord>,
    #[serde(default)]
    decisions: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct SpikeRecord {
    backend: Backend,
    session_id: String,
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
        let spawned: Vec<(Backend, String)> = spike
            .spawned
            .into_iter()
            .map(|r| (r.backend, r.session_id))
            .collect();
        let (added, present) =
            provenance::record_all(&ctx.paths, &spawned, dry_run).map_err(|e| match e {
                provenance::ProvenanceError::BadId => bad(format!(
                    "a spawned record has an invalid session id ({e}); nothing was imported"
                )),
                other => other.into(),
            })?;
        report.spawned_added = added;
        report.spawned_present = present;
        report.decisions_ignored = spike.decisions.len();
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
    #[serde(default)]
    fork_aware: bool,
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
    // Issue #35: stamping spike's old projection with today's prompt version would
    // make inherited parent turns look current. Keep normal imports reusable.
    if !s.fork_aware && crate::codex::is_forked_child(&e) {
        return Some(Outcome::Stale);
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
            fork_aware: s.fork_aware,
            truncated: None,
        };
        summary::store(ctx, &imported).ok()?;
    }
    Some(Outcome::Imported)
}

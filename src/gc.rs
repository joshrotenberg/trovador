//! Removing what trovador keeps that will never be used again.
//!
//! Only derived files are touched: cached summaries and the run log. Consent is never
//! touched, because it is the only record of which sessions trovador started.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::archive::{self, Backend};
use crate::runlog;
use crate::sessions::Ctx;
use crate::summary::{self, Summary};

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct GcReport {
    /// Nothing was removed; this is what `--apply` would do.
    pub dry_run: bool,
    pub older_than: String,
    pub summaries: SummaryCounts,
    pub runs: RunCounts,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SummaryCounts {
    pub kept: usize,
    /// The session's file is gone.
    pub session_gone: usize,
    /// The session changed after it was summarized, so this summary is never served again.
    pub stale: usize,
    /// Made before the cutoff.
    pub old: usize,
    /// Files that are not a summary trovador wrote. Left alone.
    pub unreadable: usize,
    pub bytes_removed: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RunCounts {
    pub kept: usize,
    pub removed: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

pub fn gc(ctx: &Ctx, older_than: Duration, apply: bool) -> std::io::Result<GcReport> {
    let cutoff = SystemTime::now() - older_than;
    let cutoff_text = humantime::format_rfc3339_millis(cutoff).to_string();
    let mut report = GcReport {
        dry_run: !apply,
        older_than: humantime::format_duration(older_than).to_string(),
        ..Default::default()
    };

    let entries: HashMap<(Backend, String), archive::ArchiveEntry> = archive::scan_all(&ctx.paths)
        .into_iter()
        .map(|e| ((e.backend, e.session_id.clone()), e))
        .collect();
    let dir = summary::dir(ctx);
    for item in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = item.path();
        let size = item.metadata().map(|m| m.len()).unwrap_or(0);
        let Some(s) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Summary>(&t).ok())
        else {
            report.summaries.unreadable += 1;
            continue;
        };
        let reason = match entries.get(&(s.backend, s.session_id.clone())) {
            None => Some(&mut report.summaries.session_gone),
            Some(e) if summary::cached(ctx, e).is_none() => Some(&mut report.summaries.stale),
            Some(_) if s.created < cutoff_text => Some(&mut report.summaries.old),
            Some(_) => None,
        };
        match reason {
            Some(count) => {
                *count += 1;
                report.summaries.bytes_removed += size;
                if apply {
                    std::fs::remove_file(&path)?;
                }
            }
            None => report.summaries.kept += 1,
        }
    }

    let log = runlog::path(&ctx.paths);
    // Held across the read and the rewrite, so a run that ends meanwhile waits rather than
    // appending to the file about to be replaced.
    let _lock = if apply {
        Some(runlog::lock(&ctx.paths)?)
    } else {
        None
    };
    if let Ok(text) = std::fs::read_to_string(&log) {
        report.runs.bytes_before = text.len() as u64;
        let mut kept = String::with_capacity(text.len());
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let at = serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| v.get("at").and_then(|a| a.as_str()).map(str::to_string));
            // A line without a readable time is kept: gc only drops what it knows is old.
            if at.is_some_and(|a| a < cutoff_text) {
                report.runs.removed += 1;
            } else {
                report.runs.kept += 1;
                kept.push_str(line);
                kept.push('\n');
            }
        }
        report.runs.bytes_after = kept.len() as u64;
        if apply && report.runs.removed > 0 {
            rewrite(&log, &kept)?;
        }
    }
    Ok(report)
}

/// Replaces a file atomically. The caller holds the log's lock.
fn rewrite(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("jsonl.{}.tmp", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(tmp, path)
}

//! Summaries of sessions, made by a model and cached against the source file.
//!
//! A summary is how a session too large to resume can still be asked about, and how a
//! person or agent gets the gist of one without reading it. Each costs real tokens, so the
//! cache is keyed tightly: the source file's size and mtime, and the prompt version. A hit
//! costs nothing.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::archive::{self, ArchiveEntry, Backend, LookupError};
use crate::output::{self, Truncated};
use crate::sessions::Ctx;
use crate::transcript::{self, Projection, RecordKind};

/// Bumped when the prompts change, so old summaries are not served as if current.
pub(crate) const PROMPT_VERSION: u32 = 1;

const FINAL_SHAPE: &str = "Cover: Goal; What happened; Key decisions; Current state; Open items. Under 300 words, no preamble.";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Summary {
    pub backend: Backend,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub summary: String,
    /// Served from the cache, at no cost.
    pub cached: bool,
    /// What making it cost, as the provider reported it. Zero for a cache hit.
    pub cost_usd: f64,
    pub model: String,
    /// Chunks the transcript was cut into.
    pub chunks: usize,
    /// Chunks actually summarized. Fewer than `chunks` when the transcript exceeded the
    /// chunk limit; the middle was left out.
    pub chunks_summarized: usize,
    pub transcript_chars: usize,
    pub created: String,
    /// The source file this summary describes.
    pub source_size: u64,
    pub source_mtime_ns: i64,
    pub prompt_version: u32,
    /// Set when `summary` is one part of a summary longer than `response_max_chars`. The
    /// cache always holds the whole text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<Truncated>,
}

#[derive(Debug, thiserror::Error)]
pub enum SummaryError {
    #[error(transparent)]
    Lookup(#[from] LookupError),
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("summarizer failed: {0}")]
    Provider(String),
}

pub fn dir(ctx: &Ctx) -> PathBuf {
    ctx.paths.home.join("summaries")
}

fn cache_path(ctx: &Ctx, backend: Backend, id: &str) -> PathBuf {
    dir(ctx).join(format!("{backend}-{id}.json"))
}

pub(crate) fn mtime_ns(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// The cached summary, only if it still describes the file as it is now.
pub fn cached(ctx: &Ctx, e: &ArchiveEntry) -> Option<Summary> {
    let raw = std::fs::read_to_string(cache_path(ctx, e.backend, &e.session_id)).ok()?;
    let s: Summary = serde_json::from_str(&raw).ok()?;
    let fresh = s.source_size == e.size_bytes
        && s.source_mtime_ns == mtime_ns(e.mtime)
        && s.prompt_version == PROMPT_VERSION;
    fresh.then_some(Summary {
        cached: true,
        cost_usd: 0.0,
        ..s
    })
}

pub(crate) fn store(ctx: &Ctx, s: &Summary) -> std::io::Result<()> {
    crate::paths::private_dir(&ctx.paths.home)?;
    crate::paths::private_dir(&dir(ctx))?;
    // Another process may be summarizing the same session at the same moment; the write
    // goes through a temp file named for this process.
    crate::paths::write_private(
        &cache_path(ctx, s.backend, &s.session_id),
        serde_json::to_string_pretty(s)
            .unwrap_or_default()
            .as_bytes(),
    )
}

/// The transcript as text, without omitted records, which carry nothing to summarize.
fn render(e: &ArchiveEntry, ctx: &Ctx) -> std::io::Result<String> {
    let mut out = String::new();
    for r in Projection::open(e, &ctx.limits)? {
        // Reasoning headings say little about what happened and would raise every
        // summary's cost.
        if matches!(r.kind, RecordKind::Omitted | RecordKind::Reasoning) {
            continue;
        }
        out.push_str(&transcript::render(&r));
        out.push('\n');
    }
    Ok(out)
}

fn chunk(text: &str, size: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest.len().min(size);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        // A size smaller than the next character's UTF-8 width backed off to 0 and never
        // advanced (#20). Such a chunk holds that one character instead.
        if end == 0 {
            end = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        chunks.push(&rest[..end]);
        rest = &rest[end..];
    }
    chunks
}

/// Which chunks to summarize when there are more than `max`: the first and last halves,
/// in order. The start says what the session set out to do and the end where it got to.
fn pick(n: usize, max: usize) -> Vec<usize> {
    if n <= max {
        return (0..n).collect();
    }
    let head = max.div_ceil(2);
    let tail = max - head;
    (0..head).chain(n - tail..n).collect()
}

/// One part of a summary, at most `response_max_chars` of it from character `offset`. What
/// both doors return. Callers that need the whole text use [`summarize`].
pub fn part(s: Summary, offset: usize, limits: &crate::limits::Limits) -> Summary {
    let id = s.session_id.clone();
    let backend = s.backend;
    let text = output::source(&s.summary, limits);
    let (summary, truncated) = output::part(&text, offset, limits.response_max_chars, |n| {
        format!(
            "summarize session_id={id} backend={backend} offset={n}; CLI: trovador summarize {id} --backend {backend} --offset {n}. Served from the cache at no cost while the session is unchanged"
        )
    });
    Summary {
        summary,
        truncated,
        ..s
    }
}

pub async fn summarize(
    ctx: &Ctx,
    id: &str,
    backend: Option<Backend>,
    force: bool,
    model: Option<&str>,
) -> Result<Summary, SummaryError> {
    let e = archive::find_session(&ctx.paths, id, backend)?;
    if !force && let Some(hit) = cached(ctx, &e) {
        return Ok(hit);
    }
    let text = {
        let (entry, ctx) = (e.clone(), ctx.clone());
        tokio::task::spawn_blocking(move || render(&entry, &ctx))
            .await
            .map_err(|err| SummaryError::Provider(err.to_string()))?
            .map_err(|source| SummaryError::Io {
                path: e.file.display().to_string(),
                source,
            })?
    };
    let model = model
        .map(str::to_string)
        .unwrap_or_else(|| ctx.limits.summary_model.clone());
    let timeout = Duration::from_secs(ctx.limits.claude_timeout_s as u64);
    let chunks = chunk(&text, ctx.limits.summary_chunk_chars.max(1));
    let picked = pick(chunks.len(), ctx.limits.summary_max_chunks.max(1));
    let mut cost = 0.0;

    let summary = if picked.len() <= 1 {
        let body = chunks.first().copied().unwrap_or("(empty session)");
        let (s, c) = oneshot(
            ctx,
            &format!("Summarize this agent session transcript. {FINAL_SHAPE}\n\n---\n{body}"),
            &model,
            timeout,
        )
        .await
        .map_err(SummaryError::Provider)?;
        cost += c;
        s
    } else {
        let total = picked.len();
        let gap = (chunks.len() > total).then(|| {
            format!(
                " Parts {} to {} of the original were left out.",
                total.div_ceil(2) + 1,
                chunks.len() - total / 2
            )
        });
        let mut partials = Vec::with_capacity(total);
        // A few at a time: each is a full CLI start, not a cheap request.
        for group in picked.chunks(ctx.limits.fanout.max(1)) {
            let runs = group.iter().map(|&i| {
                let prompt = format!(
                    "This is part {} of {} of one agent session transcript, in order. Summarize THIS PART in at most 10 tight bullets: goals, actions, decisions, outcomes, open threads. No preamble.\n\n---\n{}",
                    i + 1,
                    chunks.len(),
                    chunks[i]
                );
                let model = model.clone();
                async move { oneshot(ctx, &prompt, &model, timeout).await }
            });
            for r in futures::future::join_all(runs).await {
                let (s, c) = r.map_err(SummaryError::Provider)?;
                cost += c;
                partials.push(s);
            }
        }
        let merged: String = partials
            .iter()
            .zip(&picked)
            .map(|(p, i)| format!("PART {}:\n{p}", i + 1))
            .collect::<Vec<_>>()
            .join("\n\n");
        let (s, c) = oneshot(
            ctx,
            &format!(
                "These are sequential partial summaries of one agent session.{} Merge them into a single summary. {FINAL_SHAPE}\n\n---\n{merged}",
                gap.unwrap_or_default()
            ),
            &model,
            timeout,
        )
        .await
        .map_err(SummaryError::Provider)?;
        cost += c;
        s
    };

    let meta = archive::read_meta(&e, &ctx.limits);
    let s = Summary {
        backend: e.backend,
        session_id: e.session_id.clone(),
        project: meta.project().map(str::to_string),
        summary,
        cached: false,
        cost_usd: cost,
        model,
        chunks: chunks.len(),
        chunks_summarized: picked.len(),
        transcript_chars: text.len(),
        created: humantime::format_rfc3339_millis(SystemTime::now()).to_string(),
        source_size: e.size_bytes,
        source_mtime_ns: mtime_ns(e.mtime),
        prompt_version: PROMPT_VERSION,
        truncated: None,
    };
    // A summary that cannot be cached is still returned; it just costs again next time.
    let _ = store(ctx, &s);
    Ok(s)
}

fn depth() -> usize {
    std::env::var("TROVADOR_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Writes the end record of a summary run whose future is dropped before it writes its own:
/// a cancelled or expired MCP task, or a client that went away. Dropping the future kills
/// the provider, and without this the log showed a start with no end.
struct Unfinished<'a> {
    ctx: &'a Ctx,
    record: Option<serde_json::Value>,
    started: std::time::Instant,
}

impl Unfinished<'_> {
    fn finish(mut self) {
        self.record = None;
    }
}

impl Drop for Unfinished<'_> {
    fn drop(&mut self) {
        if let Some(mut record) = self.record.take() {
            record["ms"] = serde_json::json!(self.started.elapsed().as_millis() as u64);
            crate::runlog::append(&self.ctx.paths, record);
        }
    }
}

/// One Claude call whose session is not kept: no transcript is written, so it never shows
/// up in the archive or as something to continue.
async fn oneshot(
    ctx: &Ctx,
    prompt: &str,
    model: &str,
    timeout: Duration,
) -> Result<(String, f64), String> {
    use claude_wrapper::{Claude, QueryCommand};
    use serde_json::json;

    let depth = depth();
    if depth >= ctx.limits.max_depth {
        return Err(format!(
            "spawn depth {depth} has reached TROVADOR_MAX_DEPTH={}",
            ctx.limits.max_depth
        ));
    }
    let mut builder = Claude::builder()
        .timeout(timeout)
        .process_group(true)
        .env("TROVADOR_DEPTH", (depth + 1).to_string());
    if let Some(bin) = &ctx.paths.claude_bin {
        builder = builder.binary(bin);
    }
    let claude = builder.build().map_err(|e| e.to_string())?;
    let cmd = QueryCommand::new(prompt.to_string())
        .prompt_via_stdin(true)
        .model(model)
        .no_session_persistence()
        .hermetic()
        // Reading the transcript in the prompt is the whole job; no tools are needed.
        .max_turns(1);
    let started = std::time::Instant::now();
    let run_id = uuid::Uuid::new_v4().to_string();
    crate::runlog::append(
        &ctx.paths,
        json!({"phase": "start", "run_id": run_id, "backend": "claude", "verb": "summarize"}),
    );
    let unfinished = Unfinished {
        ctx,
        record: Some(
            json!({"phase": "end", "run_id": run_id, "backend": "claude",
            "verb": "summarize", "outcome": "cancelled"}),
        ),
        started,
    };
    let result = cmd.execute_json(&claude).await;
    unfinished.finish();
    let outcome = match &result {
        Ok(r) if !r.is_error => "ok",
        Ok(_) => "error",
        Err(claude_wrapper::Error::Timeout { .. }) => "timeout",
        Err(_) => "failed",
    };
    crate::runlog::append(
        &ctx.paths,
        json!({"phase": "end", "run_id": run_id, "backend": "claude", "verb": "summarize",
        "outcome": outcome, "ms": started.elapsed().as_millis() as u64}),
    );
    match result {
        Ok(r) if r.is_error => Err(r.result),
        Ok(r) => Ok((r.result, r.cost_usd.unwrap_or(0.0))),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_the_limit_keeps_the_start_and_the_end_in_order() {
        assert_eq!(pick(3, 8), vec![0, 1, 2]);
        assert_eq!(pick(20, 8), vec![0, 1, 2, 3, 16, 17, 18, 19]);
        assert_eq!(pick(20, 5), vec![0, 1, 2, 18, 19]);
        assert_eq!(pick(20, 1), vec![0]);
    }

    #[test]
    fn chunks_always_advance_even_when_smaller_than_a_character() {
        for text in ["abc", "a\u{1F642}z", "\u{1F642}\u{1F642}", "\u{e9}t\u{e9}"] {
            for size in 1..=5 {
                let c = chunk(text, size);
                assert_eq!(c.concat(), text, "{text:?} at {size}");
                assert!(c.iter().all(|p| !p.is_empty()), "{text:?} at {size}");
                // Within size whenever a whole character fits.
                assert!(
                    c.iter().all(|p| p.len() <= size || p.chars().count() == 1),
                    "{text:?} at {size}: {c:?}"
                );
            }
        }
    }

    #[test]
    fn chunks_never_split_a_character() {
        let s = "ab\u{e9}cd";
        let c = chunk(s, 3);
        assert_eq!(c.concat(), s);
        assert!(c.iter().all(|p| p.len() <= 3));
    }
}

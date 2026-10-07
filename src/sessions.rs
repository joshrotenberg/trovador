//! The session projections both doors return. The CLI and the MCP tool call these and
//! serialize the result unchanged; neither computes its own version of a row.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::time::Duration;

use crate::archive::{self, ArchiveEntry, Backend, LookupError, SessionMeta};
use crate::index::{self, Index, IndexError, Match, RefreshOpts, RefreshReport, SearchOpts};
use crate::limits::Limits;
use crate::paths::Paths;
use crate::transcript::{self, TranscriptPage};

/// What every operation needs: where things are and how much to return.
#[derive(Clone, Debug)]
pub struct Ctx {
    pub paths: Paths,
    pub limits: Limits,
}

impl Ctx {
    pub fn from_env() -> Self {
        Self {
            paths: Paths::from_env(),
            limits: Limits::from_env(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Lookup(#[from] LookupError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error("invalid query: {0}")]
    Query(String),
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SessionRow {
    pub backend: Backend,
    pub session_id: String,
    pub modified: String,
    pub size_kb: u64,
    /// The directory a resume runs from: where the session lives now, else where it started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(flatten)]
    pub meta: SessionMeta,
    pub resumable: bool,
    /// Codex archived this thread. It is still readable here.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub archived: bool,
    /// The MCP resource for this session.
    pub resource: String,
    /// What to paste in a terminal to pick the session up.
    pub open_with: String,
}

pub fn row(e: &ArchiveEntry, meta: SessionMeta, limits: &Limits) -> SessionRow {
    let project = meta.project().map(str::to_string);
    SessionRow {
        backend: e.backend,
        session_id: e.session_id.clone(),
        modified: e.modified(),
        size_kb: e.size_bytes.div_ceil(1024),
        open_with: archive::open_with(e.backend, project.as_deref(), &e.session_id),
        resource: format!("session://{}/{}", e.backend, e.session_id),
        resumable: archive::resumability(e, limits).resumable,
        archived: e.archived,
        project,
        meta,
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct ListOpts {
    /// Only this backend.
    pub backend: Option<Backend>,
    /// Only sessions whose project path contains this text.
    pub project: Option<String>,
    /// How many to return, newest first.
    pub limit: Option<usize>,
    /// Only sessions with this origin (`cli`, `claude-desktop`, `sdk-cli`, `exec`, ...).
    pub origin: Option<String>,
    /// Include sessions started by another session. Off by default: approval reviewers and
    /// spawned agents outnumber the sessions a person started.
    #[serde(default)]
    pub include_subagents: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListResult {
    /// Sessions in the archives searched, before any filter.
    pub total_sessions: usize,
    /// Sessions whose metadata was read to answer this list.
    pub examined: usize,
    pub returned: usize,
    /// More matching sessions may exist beyond this list. With a project filter that is a
    /// possibility rather than a count: the list stops reading once it has enough.
    pub truncated: bool,
    pub sessions: Vec<SessionRow>,
}

/// Newest first. Metadata is read only for sessions that might be returned, and the scan
/// stops once the list is full (spike#90: the list loop once read every head regardless).
pub fn list(ctx: &Ctx, opts: &ListOpts) -> ListResult {
    let all: Vec<ArchiveEntry> = archive::scan_all(&ctx.paths)
        .into_iter()
        .filter(|e| opts.backend.is_none_or(|b| e.backend == b))
        .collect();
    let limit = opts.limit.unwrap_or(ctx.limits.list_default).max(1);
    // Metadata from the index where the file has not changed since it was indexed. The
    // index is only read here, never created: listing must not leave state behind.
    let cached = Index::open_existing(&ctx.paths)
        .map(|i| i.meta_snapshot())
        .unwrap_or_default();
    let needle = opts.project.as_deref().filter(|p| !p.is_empty());
    let mut sessions = Vec::new();
    let mut examined = 0;
    let mut truncated = false;
    for e in &all {
        if sessions.len() == limit {
            truncated = true;
            break;
        }
        examined += 1;
        let meta = cached_meta(&cached, e).unwrap_or_else(|| archive::read_meta(e, &ctx.limits));
        if meta.subagent && !opts.include_subagents {
            continue;
        }
        if let Some(o) = opts.origin.as_deref()
            && meta.origin.as_deref() != Some(o)
        {
            continue;
        }
        if let Some(n) = needle {
            let hit = [meta.cwd.as_deref(), meta.relocated_cwd.as_deref()]
                .into_iter()
                .flatten()
                .any(|p| p.contains(n));
            if !hit {
                continue;
            }
        }
        sessions.push(row(e, meta, &ctx.limits));
    }
    ListResult {
        total_sessions: all.len(),
        examined,
        returned: sessions.len(),
        truncated,
        sessions,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub row: SessionRow,
    pub file: String,
    pub size_bytes: u64,
    /// Why the session cannot be resumed, when it cannot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_blocked_by: Option<String>,
    pub transcript_resource: String,
    /// Whether trovador may add a turn to this session, and what would change that.
    pub consent: crate::consent::Check,
}

pub fn show(ctx: &Ctx, id: &str, backend: Option<Backend>) -> Result<SessionDetail, Error> {
    let e = archive::find_session(&ctx.paths, id, backend)?;
    let meta = archive::read_meta(&e, &ctx.limits);
    let r = row(&e, meta, &ctx.limits);
    Ok(SessionDetail {
        file: e.file.display().to_string(),
        size_bytes: e.size_bytes,
        resume_blocked_by: archive::resumability(&e, &ctx.limits).reason,
        transcript_resource: format!("{}/transcript", r.resource),
        consent: crate::consent::check(&ctx.paths, e.backend, &e.session_id),
        row: r,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TranscriptResult {
    pub backend: Backend,
    pub session_id: String,
    #[serde(flatten)]
    pub page: TranscriptPage,
}

pub fn transcript(
    ctx: &Ctx,
    id: &str,
    backend: Option<Backend>,
    offset: usize,
    limit: Option<usize>,
) -> Result<TranscriptResult, Error> {
    let e = archive::find_session(&ctx.paths, id, backend)?;
    let page = transcript::page(&e, offset, limit, &ctx.limits).map_err(|source| Error::Io {
        path: e.file.display().to_string(),
        source,
    })?;
    Ok(TranscriptResult {
        backend: e.backend,
        session_id: e.session_id,
        page,
    })
}

type MetaCache = HashMap<(Backend, String), (u64, i64, SessionMeta)>;

fn cached_meta(cache: &MetaCache, e: &ArchiveEntry) -> Option<SessionMeta> {
    let (size, mtime, meta) = cache.get(&(e.backend, e.session_id.clone()))?;
    let now = e
        .mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    (*size == e.size_bytes && *mtime == now).then(|| meta.clone())
}

// --- search ----------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    #[serde(flatten)]
    pub row: SessionRow,
    /// Matching records in this session.
    pub match_count: usize,
    /// The best-ranked matches, each with the record index to open the transcript at.
    pub matches: Vec<Match>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Coverage {
    /// What bringing the index up to date did before this query ran.
    pub refresh: RefreshReport,
    /// Sessions with metadata but no indexed text, by origin. Codex `guardian` approval
    /// reviewers are excluded because their text is a copy of the parent session's.
    pub text_excluded: HashMap<String, usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchResult {
    pub query: String,
    /// `terms`, `fts`, or `regex`.
    pub mode: String,
    /// The FTS5 expression or regular expression that ran.
    pub expression: String,
    /// Sessions with at least one match, before `limit`.
    pub sessions_matched: usize,
    pub returned: usize,
    pub hits: Vec<SearchHit>,
    /// True only when every session this search covers was indexed and current when the
    /// query ran. A search that is not exhaustive proves nothing by finding nothing.
    pub exhaustive: bool,
    /// Present when there are no hits: whether that absence means the text is not there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub absence_is_proof: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub coverage: Coverage,
}

/// Brings the index up to date within the search budget, then queries it.
pub fn search(ctx: &Ctx, opts: &SearchOpts) -> Result<SearchResult, Error> {
    if opts.query.trim().is_empty() {
        return Err(Error::Query("query is empty".into()));
    }
    if opts.regex && opts.fts {
        return Err(Error::Query("choose regex or fts, not both".into()));
    }
    if opts.regex {
        regex::Regex::new(&opts.query).map_err(|e| Error::Query(e.to_string()))?;
    }
    let mut idx = Index::open(&ctx.paths)?;
    let refresh = idx.refresh(
        &ctx.paths,
        &ctx.limits,
        &RefreshOpts {
            budget: Some(Duration::from_millis(ctx.limits.index_budget_ms as u64)),
        },
    )?;
    let limit = opts.limit.unwrap_or(ctx.limits.search_default).max(1);
    let per_session = opts
        .per_session
        .unwrap_or(ctx.limits.per_session_default)
        .max(1);
    let raw = idx.search(opts, limit, per_session).map_err(|e| match e {
        // A malformed FTS5 expression is the caller's to fix, not an index failure.
        IndexError::Sql(err) if opts.fts => Error::Query(err.to_string()),
        other => other.into(),
    })?;

    let entries: HashMap<(Backend, String), ArchiveEntry> = archive::scan_all(&ctx.paths)
        .into_iter()
        .map(|e| ((e.backend, e.session_id.clone()), e))
        .collect();
    let hits: Vec<SearchHit> = raw
        .hits
        .into_iter()
        .filter_map(|h| {
            let e = entries.get(&(h.backend, h.session_id.clone()))?;
            Some(SearchHit {
                row: row(e, h.meta, &ctx.limits),
                match_count: h.match_count,
                matches: h.matches,
            })
        })
        .collect();

    let text_excluded = idx.text_excluded()?;
    let exhaustive = refresh.pending == 0 && refresh.failed == 0;
    let excluded_note = (opts.include_subagents && !text_excluded.is_empty()).then(|| {
        let mut parts: Vec<String> = text_excluded
            .iter()
            .map(|(o, n)| format!("{n} {}", if o.is_empty() { "unknown" } else { o }))
            .collect();
        parts.sort();
        format!("text of {} sessions is not indexed", parts.join(", "))
    });
    let stale_note = (!exhaustive).then(|| {
        format!(
            "{} session files were not indexed within the {} ms budget and {} could not be read; run `trovador index` to finish",
            refresh.pending, ctx.limits.index_budget_ms, refresh.failed
        )
    });
    let note = match (stale_note, excluded_note) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    };
    Ok(SearchResult {
        query: opts.query.clone(),
        mode: if opts.regex {
            "regex"
        } else if opts.fts {
            "fts"
        } else {
            "terms"
        }
        .into(),
        expression: raw.expression,
        sessions_matched: raw.sessions_matched,
        returned: hits.len(),
        absence_is_proof: hits.is_empty().then_some(exhaustive),
        hits,
        exhaustive,
        note,
        coverage: Coverage {
            refresh,
            text_excluded,
        },
    })
}

/// Brings the index fully up to date, with no time budget.
pub fn reindex(ctx: &Ctx, rebuild: bool) -> Result<RefreshReport, Error> {
    if rebuild {
        // Deleting every row leaves the file at its old size (378 MB against 287 MB fresh,
        // and 15 s against 9 s, on 2026-10-07), so a rebuild starts from no file at all.
        let db = index::db_path(&ctx.paths);
        for suffix in ["", "-wal", "-shm"] {
            let mut p = db.clone().into_os_string();
            p.push(suffix);
            match std::fs::remove_file(std::path::PathBuf::from(p)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(Error::Io {
                        path: db.display().to_string(),
                        source,
                    });
                }
            }
        }
    }
    let mut idx = Index::open(&ctx.paths)?;
    Ok(idx.refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())?)
}

pub fn index_status(ctx: &Ctx) -> Option<index::IndexStatus> {
    Index::open_existing(&ctx.paths).and_then(|i| i.status().ok())
}

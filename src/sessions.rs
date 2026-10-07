//! The session projections both doors return. The CLI and the MCP tool call these and
//! serialize the result unchanged; neither computes its own version of a row.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::archive::{self, ArchiveEntry, Backend, LookupError, SessionMeta};
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
        let meta = archive::read_meta(e, &ctx.limits);
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

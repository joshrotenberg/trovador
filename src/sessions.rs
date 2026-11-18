//! The session projections both doors return. The CLI and the MCP tool call these and
//! serialize the result unchanged; neither computes its own version of a row.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::time::Duration;

use crate::archive::{self, ArchiveEntry, Backend, LookupError, SessionMeta};
use crate::index::{
    self, AUTOMATED_ORIGINS, Hidden, Index, IndexError, Match, RefreshOpts, RefreshReport,
    SearchOpts, Visibility,
};
use crate::limits::Limits;
use crate::paths::Paths;
use crate::revision;
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
    /// The caller's continuation names a revision the session no longer has.
    #[error(
        "session {backend}/{session_id} is not at revision {expected}; it is now at revision {current}. Offsets from the earlier read may not address the same records: restart from offset 0 without expected_revision"
    )]
    StaleRevision {
        backend: Backend,
        session_id: String,
        expected: String,
        current: String,
    },
    /// The file was rewritten while a page was being read, so the page could mix two
    /// versions. Nothing is returned for it.
    #[error("session {backend}/{session_id} was rewritten while it was being read; retry the call")]
    ChangedWhileReading {
        backend: Backend,
        session_id: String,
    },
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
    /// Only sessions whose project path contains this text (ASCII case-insensitive).
    pub project: Option<String>,
    /// How many to return, newest first.
    pub limit: Option<usize>,
    /// Only sessions with this origin (`cli`, `claude-desktop`, `sdk-cli`, `exec`, ...).
    pub origin: Option<String>,
    /// Include sessions started by another session. Off by default: approval reviewers and
    /// spawned agents outnumber the sessions a person started.
    #[serde(default)]
    pub include_subagents: bool,
    /// Include automated sessions (`claude -p`, `codex exec`) that trovador did not start.
    /// An explicit `origin` includes them too.
    #[serde(default)]
    pub include_automated: bool,
    /// Only sessions last written at or after this: an RFC 3339 time, a date, or a
    /// duration back from now such as `2d`.
    pub since: Option<String>,
    /// Only sessions last written before this, in the same forms as `since`.
    pub until: Option<String>,
}

/// `since` and `until` as times, or the error to return for a value that does not parse.
pub(crate) fn window(
    since: Option<&str>,
    until: Option<&str>,
) -> Result<(Option<std::time::SystemTime>, Option<std::time::SystemTime>), Error> {
    let now = std::time::SystemTime::now();
    let parse = |v: Option<&str>, name: &str| {
        v.filter(|s| !s.trim().is_empty())
            .map(|s| {
                crate::timespec::parse(s, now).map_err(|e| Error::Query(format!("{name}: {e}")))
            })
            .transpose()
    };
    Ok((parse(since, "since")?, parse(until, "until")?))
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
    /// Automated sessions trovador did not start, left out among those examined.
    pub hidden_automated: usize,
    pub sessions: Vec<SessionRow>,
}

/// Whether the automated default leaves this session out.
fn automated_hidden(meta: &SessionMeta) -> bool {
    meta.origin
        .as_deref()
        .is_some_and(|o| AUTOMATED_ORIGINS.contains(&o))
}

/// Newest first. Metadata is read only for sessions that might be returned, and the scan
/// stops once the list is full (spike#90: the list loop once read every head regardless).
pub fn list(ctx: &Ctx, opts: &ListOpts) -> Result<ListResult, Error> {
    let (since, until) = window(opts.since.as_deref(), opts.until.as_deref())?;
    let all: Vec<ArchiveEntry> = archive::scan_all(&ctx.paths)
        .into_iter()
        .filter(|e| opts.backend.is_none_or(|b| e.backend == b))
        .collect();
    let limit = opts.limit.unwrap_or(ctx.limits.list_default).max(1);
    // Metadata from the index where the file has not changed since it was indexed. The
    // index is only read here, never created: listing must not leave state behind.
    let index = Index::open_existing(&ctx.paths, &ctx.limits);
    let cached = index
        .as_ref()
        .map(|i| i.meta_snapshot())
        .unwrap_or_default();
    let hide_automated = !opts.include_automated && opts.origin.is_none();
    let mut hidden_automated = 0;
    // Issue #31: match search's SQLite LIKE semantics, which fold ASCII case only.
    let needle = opts
        .project
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(str::to_ascii_lowercase);
    let mut sessions = Vec::new();
    let mut examined = 0;
    let mut truncated = false;
    for e in &all {
        if sessions.len() == limit {
            truncated = true;
            break;
        }
        // Newest first, so everything after this is older still.
        if since.is_some_and(|s| e.mtime < s) {
            break;
        }
        if until.is_some_and(|u| e.mtime >= u) {
            continue;
        }
        examined += 1;
        let meta = cached_meta(&cached, e)
            .or_else(|| {
                index
                    .as_ref()
                    .and_then(|i| i.read_meta_entry(e, &ctx.limits))
            })
            .unwrap_or_else(|| archive::read_meta(e, &ctx.limits));
        if meta.subagent && !opts.include_subagents {
            continue;
        }
        if let Some(o) = opts.origin.as_deref()
            && meta.origin.as_deref() != Some(o)
        {
            continue;
        }
        if hide_automated && automated_hidden(&meta) {
            hidden_automated += 1;
            continue;
        }
        if let Some(n) = needle.as_deref() {
            let hit = [meta.cwd.as_deref(), meta.relocated_cwd.as_deref()]
                .into_iter()
                .flatten()
                .any(|p| p.to_ascii_lowercase().contains(n));
            if !hit {
                continue;
            }
        }
        sessions.push(row(e, meta, &ctx.limits));
    }
    Ok(ListResult {
        total_sessions: all.len(),
        examined,
        returned: sessions.len(),
        truncated,
        hidden_automated,
        sessions,
    })
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
    let meta = Index::open_existing(&ctx.paths, &ctx.limits)
        .and_then(|i| i.read_meta_entry(&e, &ctx.limits))
        .unwrap_or_else(|| archive::read_meta(&e, &ctx.limits));
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
    /// The revision of the session this page was read from. Pass it as
    /// `expected_revision` when reading the next page.
    pub revision: String,
    /// Present when the session has grown since `revision`: this page is of the session as
    /// it was then, and `grown.current_revision` reads it as it is now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grown: Option<revision::Grown>,
    #[serde(flatten)]
    pub page: TranscriptPage,
}

/// What a read covers: the entry cut at the revision's size, the revision, and whether the
/// session has grown past it.
struct Read {
    entry: ArchiveEntry,
    rev: revision::Revision,
    grown: Option<revision::Grown>,
}

/// Names the revision a read will cover and refuses it when the caller expected a state the
/// session is not in. Done before the read so a stale continuation costs no projection.
/// A session that has only grown since the expected revision is read as it was.
fn begin_read(ctx: &Ctx, e: &ArchiveEntry, expected: Option<&str>) -> Result<Read, Error> {
    let io = |source| Error::Io {
        path: e.file.display().to_string(),
        source,
    };
    let (rev, grown) = match revision::check(e, &ctx.limits, expected).map_err(io)? {
        revision::Check::Current(rev) => (rev, None),
        revision::Check::Pinned {
            rev,
            current,
            bytes_added,
        } => (
            rev,
            Some(revision::Grown {
                current_revision: current.id,
                bytes_added,
            }),
        ),
        revision::Check::Stale { current } => {
            return Err(Error::StaleRevision {
                backend: e.backend,
                session_id: e.session_id.clone(),
                expected: expected.unwrap_or_default().to_string(),
                current: current.id,
            });
        }
    };
    let mut entry = e.clone();
    entry.size_bytes = rev.size;
    Ok(Read { entry, rev, grown })
}

/// Run after the read. The read is cut at the size the revision names, so an append during
/// it is harmless; a rewrite is not, and fails the call instead of labelling a mixed page
/// with the old revision.
fn end_read(read: &Read) -> Result<(), Error> {
    let e = &read.entry;
    let holds = read.rev.still_holds(e).map_err(|source| Error::Io {
        path: e.file.display().to_string(),
        source,
    })?;
    if holds {
        Ok(())
    } else {
        Err(Error::ChangedWhileReading {
            backend: e.backend,
            session_id: e.session_id.clone(),
        })
    }
}

/// [`transcript_at`] without a revision check.
pub fn transcript(
    ctx: &Ctx,
    id: &str,
    backend: Option<Backend>,
    offset: usize,
    limit: Option<usize>,
) -> Result<TranscriptResult, Error> {
    transcript_at(ctx, id, backend, offset, limit, None)
}

/// One page of a transcript. With `expected_revision`, the page is read only if the
/// session is still at that revision; otherwise [`Error::StaleRevision`] carries the
/// current one.
pub fn transcript_at(
    ctx: &Ctx,
    id: &str,
    backend: Option<Backend>,
    offset: usize,
    limit: Option<usize>,
    expected_revision: Option<&str>,
) -> Result<TranscriptResult, Error> {
    transcript_at_hooked(ctx, id, backend, offset, limit, expected_revision, || {})
}

/// [`transcript_at`] that runs `after_read` between the read and the final check. Public
/// only because integration tests are separate crates; they use it to change the file at
/// the one moment a rewrite during a read matters, which nothing else can place
/// deterministically. Not part of the supported API.
#[doc(hidden)]
pub fn transcript_at_hooked(
    ctx: &Ctx,
    id: &str,
    backend: Option<Backend>,
    offset: usize,
    limit: Option<usize>,
    expected_revision: Option<&str>,
    after_read: impl FnOnce(),
) -> Result<TranscriptResult, Error> {
    let e = archive::find_session(&ctx.paths, id, backend)?;
    let read = begin_read(ctx, &e, expected_revision)?;
    let page =
        transcript::page(&read.entry, offset, limit, &ctx.limits).map_err(|source| Error::Io {
            path: e.file.display().to_string(),
            source,
        })?;
    after_read();
    end_read(&read)?;
    Ok(TranscriptResult {
        backend: e.backend,
        session_id: e.session_id,
        revision: read.rev.id,
        grown: read.grown,
        page,
    })
}

/// [`text_at`] without a revision check.
pub fn text(
    ctx: &Ctx,
    id: &str,
    backend: Option<Backend>,
    offset: usize,
) -> Result<crate::text::SessionText, Error> {
    text_at(ctx, id, backend, offset, None)
}

/// One part of a session as plain text, for the caller to read or summarize. See
/// [`crate::text`]. With `expected_revision` it is read only if the session is still at
/// that revision, as for [`transcript_at`].
pub fn text_at(
    ctx: &Ctx,
    id: &str,
    backend: Option<Backend>,
    offset: usize,
    expected_revision: Option<&str>,
) -> Result<crate::text::SessionText, Error> {
    text_at_hooked(ctx, id, backend, offset, expected_revision, || {})
}

/// [`text_at`] with the same test-only hook as [`transcript_at_hooked`], public for the
/// same reason and not part of the supported API.
#[doc(hidden)]
pub fn text_at_hooked(
    ctx: &Ctx,
    id: &str,
    backend: Option<Backend>,
    offset: usize,
    expected_revision: Option<&str>,
    after_read: impl FnOnce(),
) -> Result<crate::text::SessionText, Error> {
    let e = archive::find_session(&ctx.paths, id, backend)?;
    let read = begin_read(ctx, &e, expected_revision)?;
    let mut part =
        crate::text::part(&read.entry, offset, &read.rev.id, &ctx.limits).map_err(|source| {
            Error::Io {
                path: e.file.display().to_string(),
                source,
            }
        })?;
    after_read();
    end_read(&read)?;
    part.grown = read.grown;
    Ok(part)
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
    /// Matching sessions the defaults left out, by reason.
    pub hidden: Hidden,
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
        crate::index::search_regex(&opts.query).map_err(|e| Error::Query(e.to_string()))?;
    }
    // Issue #28: certainly unmatchable terms must fail even if the index cannot open,
    // and must not spend the caller's refresh budget before reporting invalid input.
    if !opts.regex && !opts.fts {
        crate::index::validate_terms(&opts.query).map_err(|err| match err {
            IndexError::Query(message) => Error::Query(message),
            other => other.into(),
        })?;
    }
    let (since, until) = window(opts.since.as_deref(), opts.until.as_deref())?;
    let updated = (|| {
        let mut idx = Index::open(&ctx.paths, &ctx.limits)?;
        let refresh = idx
            .refresh(
                &ctx.paths,
                &ctx.limits,
                &RefreshOpts {
                    budget: Some(Duration::from_millis(ctx.limits.index_budget_ms as u64)),
                },
            )
            .map_err(|e| idx.classify_error(e))?;
        Ok::<_, IndexError>((idx, refresh))
    })();
    let (idx, mut refresh, readonly_note) = match updated {
        Ok((idx, refresh)) => (idx, refresh, None),
        Err(error) if error.is_permission() => {
            let idx = Index::open_read_only(&ctx.paths, &ctx.limits)?;
            let note = format!(
                "index {} could not be updated: {error}; searching the existing index read-only; sessions written since its last refresh are not covered",
                index::db_path(&ctx.paths).display()
            );
            (idx, RefreshReport::default(), Some(note))
        }
        Err(error) => return Err(error.into()),
    };
    let limit = opts.limit.unwrap_or(ctx.limits.search_default).max(1);
    let per_session = opts
        .per_session
        .unwrap_or(ctx.limits.per_session_default)
        .max(1);
    let vis = Visibility {
        archive_roots: Some(
            [
                &ctx.paths.claude_archive,
                &ctx.paths.codex_archive,
                &ctx.paths.codex_archived,
            ]
            .into_iter()
            .map(|root| root.display().to_string())
            .collect(),
        ),
        since_ns: since.map(crate::timespec::ns),
        until_ns: until.map(crate::timespec::ns),
        active_since_ns: (!opts.include_active && ctx.limits.active_window_s > 0).then(|| {
            let cutoff = std::time::SystemTime::now()
                - Duration::from_secs(ctx.limits.active_window_s as u64);
            crate::timespec::ns(cutoff)
        }),
        exclude_session: opts.exclude_session.clone().filter(|s| !s.is_empty()),
        automated_except: (!opts.include_automated && opts.origin.is_none()).then(Vec::new),
    };
    let raw = idx
        .search(opts, &vis, limit, per_session, ctx.limits.redact)
        .map_err(|e| search_error(e, opts))?;

    let entries: HashMap<(Backend, String), ArchiveEntry> = archive::scan_all(&ctx.paths)
        .into_iter()
        .map(|e| ((e.backend, e.session_id.clone()), e))
        .collect();
    if readonly_note.is_some() {
        refresh.sessions = entries.len();
    }
    let found = raw.hits.len();
    let hits: Vec<SearchHit> = raw
        .hits
        .into_iter()
        .filter_map(|h| {
            let e = entries.get(&(h.backend, h.session_id.clone()))?;
            // Issue #36: a complete final title without a newline is display metadata,
            // so search must use the same bounded read as list and show.
            let meta = idx.read_meta_entry(e, &ctx.limits).unwrap_or(h.meta);
            Some(SearchHit {
                row: row(e, meta, &ctx.limits),
                match_count: h.match_count,
                matches: h.matches,
            })
        })
        .collect();

    let mut hidden = raw.hidden.clone();
    // Matches whose file the scan could not see: in an unreadable root, or removed since
    // the index was last refreshed.
    hidden.unavailable = found - hits.len();
    let text_excluded = idx.text_excluded()?;
    let exhaustive = readonly_note.is_none()
        && refresh.pending == 0
        && refresh.failed == 0
        && refresh.unavailable_roots.is_empty();
    let excluded_note = (opts.include_subagents && !text_excluded.is_empty()).then(|| {
        let mut parts: Vec<String> = text_excluded
            .iter()
            .map(|(o, n)| format!("{n} {}", if o.is_empty() { "unknown" } else { o }))
            .collect();
        parts.sort();
        format!("text of {} sessions is not indexed", parts.join(", "))
    });
    let stale_note = (refresh.pending > 0 || refresh.failed > 0).then(|| {
        format!(
            "{} session files were not indexed within the {} ms budget and {} could not be read; run `trovador index` to finish",
            refresh.pending, ctx.limits.index_budget_ms, refresh.failed
        )
    });
    let unavailable_note = (!refresh.unavailable_roots.is_empty()).then(|| {
        format!(
            "archive roots could not be read: {}; their sessions were not checked, and {} matching sessions in them could not be returned",
            refresh.unavailable_roots.join(", "),
            hidden.unavailable
        )
    });
    let h = &hidden;
    let hidden_note = (h.active + h.automated + h.subagents + h.excluded > 0).then(|| {
        let mut parts = Vec::new();
        if h.active > 0 {
            parts.push(format!(
                "{} written in the last {} s (include_active)",
                h.active, ctx.limits.active_window_s
            ));
        }
        if h.automated > 0 {
            parts.push(format!("{} automated (include_automated)", h.automated));
        }
        if h.subagents > 0 {
            parts.push(format!("{} subagents (include_subagents)", h.subagents));
        }
        if h.excluded > 0 {
            parts.push("the excluded session".to_string());
        }
        format!(
            "matching sessions left out by default: {}",
            parts.join(", ")
        )
    });
    let query_note = if opts.fts && hits.is_empty() {
        crate::index::fts_absence_note(&opts.query)?
    } else {
        None
    };
    let query_is_proof = query_note.is_none();
    let note = [
        readonly_note,
        stale_note,
        unavailable_note,
        excluded_note,
        hidden_note,
        query_note,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let note = (!note.is_empty()).then(|| note.join("; "));
    idx.verify_snapshot()?;
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
        absence_is_proof: hits
            .is_empty()
            .then_some(exhaustive && !hidden.any() && query_is_proof),
        hidden,
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
    let mut idx = Index::open(&ctx.paths, &ctx.limits)?;
    Ok(idx.refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())?)
}

pub fn index_status(ctx: &Ctx) -> Option<index::IndexStatus> {
    Index::open_existing(&ctx.paths, &ctx.limits).and_then(|i| i.status().ok())
}

fn search_error(error: IndexError, opts: &SearchOpts) -> Error {
    match error {
        // A malformed FTS5 expression is the caller's to fix, not an index failure.
        IndexError::Sql(err) if opts.fts => Error::Query(err.to_string()),
        // Issue #33: callback failures remain invalid queries if validation diverges.
        // SQLite loses the callback kind; only tagged regex failures are query errors.
        IndexError::Sql(err)
            if opts.regex
                && (matches!(&err, rusqlite::Error::UserFunctionError(_))
                    || matches!(
                        &err,
                        rusqlite::Error::SqliteFailure(_, Some(message))
                                if message.starts_with(index::REGEXP_QUERY_ERROR_PREFIX)
                    )) =>
        {
            Error::Query(err.to_string())
        }
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_callback_failures_are_queries_but_database_failures_remain_index_errors() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.create_scalar_function(
            "regexp",
            2,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |ctx| {
                let re = ctx.get_or_create_aux(0, |v| {
                    crate::index::sql_search_regex(v.as_str().unwrap_or(""))
                })?;
                Ok(re.is_match(ctx.get_raw(1).as_str().unwrap_or("")))
            },
        )
        .unwrap();
        let opts = SearchOpts {
            regex: true,
            ..Default::default()
        };
        let err = conn
            .query_row("SELECT regexp(?1, 'question')", [r"\w{100}"], |r| {
                r.get::<_, bool>(0)
            })
            .unwrap_err();
        assert!(matches!(
            search_error(IndexError::Sql(err), &opts),
            Error::Query(_)
        ));
        let err = conn
            .query_row("SELECT * FROM missing", [], |r| r.get::<_, bool>(0))
            .unwrap_err();
        assert!(matches!(
            search_error(IndexError::Sql(err), &opts),
            Error::Index(_)
        ));
    }
}

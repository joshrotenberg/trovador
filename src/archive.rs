//! The provider archives: scanning, addressing, metadata, resumability.
//!
//! Both archives are read only. Nothing here writes under `~/.claude` or `~/.codex`.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::limits::Limits;
use crate::paths::Paths;

pub use crate::provider::Provider;

pub(crate) fn is_compressed(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "zst")
}

/// Offsets address decompressed JSONL so checkpoints survive either storage format.
pub(crate) fn open_archive(path: &Path, offset: u64) -> std::io::Result<Box<dyn BufRead + Send>> {
    let mut file = File::open(path)?;
    if is_compressed(path) {
        let mut reader = BufReader::new(zstd::stream::read::Decoder::new(file)?);
        // A zstd stream has no random access; discard decoded bytes to reach a checkpoint.
        std::io::copy(&mut reader.by_ref().take(offset), &mut std::io::sink())?;
        Ok(Box::new(reader))
    } else {
        file.seek(SeekFrom::Start(offset))?;
        Ok(Box::new(BufReader::new(file)))
    }
}

/// One session file, as found on disk, before anything inside it is read.
#[derive(Clone, Debug)]
pub struct ArchiveEntry {
    pub provider: Provider,
    pub session_id: String,
    pub file: PathBuf,
    pub size_bytes: u64,
    pub mtime: SystemTime,
    /// Codex moved this thread to `archived_sessions`.
    pub archived: bool,
}

impl ArchiveEntry {
    pub fn modified(&self) -> String {
        humantime::format_rfc3339_millis(self.mtime).to_string()
    }

    /// No line will be appended: Codex archived the thread, or compressed the rollout,
    /// which it does only to finished ones.
    pub(crate) fn finished(&self) -> bool {
        self.archived || is_compressed(&self.file)
    }
}

// --- scanning --------------------------------------------------------------

/// Claude keeps `<root>/<project>/<uuid>.jsonl`. Subagent transcripts live one level
/// deeper under `<session>/subagents/` and are deliberately not listed as sessions: they
/// cannot be resumed on their own.
pub fn scan_claude(root: &Path) -> Vec<ArchiveEntry> {
    scan_root(root, Provider::Claude, false, &mut Vec::new())
}

/// Codex nests active sessions by date and keeps archived sessions in a flat directory.
pub fn scan_codex(root: &Path, archived: bool) -> Vec<ArchiveEntry> {
    scan_root(root, Provider::Codex, archived, &mut Vec::new())
}

pub(crate) struct ScanFailure {
    pub path: PathBuf,
    pub kind: std::io::ErrorKind,
}

fn scan_root(
    root: &Path,
    provider: Provider,
    archived: bool,
    failures: &mut Vec<ScanFailure>,
) -> Vec<ArchiveEntry> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let items = match fs::read_dir(&dir) {
            Ok(items) => items,
            Err(error) => {
                failures.push(ScanFailure {
                    path: dir,
                    kind: error.kind(),
                });
                continue;
            }
        };
        for item in items {
            let item = match item {
                Ok(item) => item,
                Err(error) => {
                    failures.push(ScanFailure {
                        path: dir.clone(),
                        kind: error.kind(),
                    });
                    continue;
                }
            };
            let path = item.path();
            let ft = match item.file_type() {
                Ok(ft) => ft,
                Err(error) => {
                    failures.push(ScanFailure {
                        path,
                        kind: error.kind(),
                    });
                    continue;
                }
            };
            // Issue #8: a dangling project link is not an archive directory and must
            // not prevent cleanup of sessions that were actually deleted.
            if provider == Provider::Claude && depth == 0 && ft.is_symlink() && !path.is_dir() {
                continue;
            }
            if (provider == Provider::Claude && depth == 0 && (ft.is_dir() || ft.is_symlink()))
                || (provider == Provider::Codex && ft.is_dir())
            {
                stack.push((path, depth + 1));
                continue;
            }
            if provider == Provider::Claude && depth != 1 {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let id = match provider {
                Provider::Claude => name.strip_suffix(".jsonl"),
                Provider::Codex => trailing_uuid(name),
            };
            if let Some(id) = id {
                match entry(provider, id, &path, archived, failures) {
                    Ok(Some(e)) => out.push(e),
                    Ok(None) => {}
                    Err(error) => failures.push(ScanFailure {
                        path,
                        kind: error.kind(),
                    }),
                }
            }
        }
    }
    out
}

fn trailing_uuid(name: &str) -> Option<&str> {
    let stem = name
        .strip_suffix(".jsonl")
        .or_else(|| name.strip_suffix(".jsonl.zst"))?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    let shape_ok = id.char_indices().all(|(i, c)| match i {
        8 | 13 | 18 | 23 => c == '-',
        _ => c.is_ascii_hexdigit(),
    });
    shape_ok.then_some(id)
}

fn entry(
    provider: Provider,
    id: &str,
    path: &Path,
    archived: bool,
    failures: &mut Vec<ScanFailure>,
) -> std::io::Result<Option<ArchiveEntry>> {
    let md = fs::metadata(path)?;
    if !md.is_file() {
        return Ok(None);
    }
    let mtime = md.modified().unwrap_or_else(|error| {
        failures.push(ScanFailure {
            path: path.to_path_buf(),
            kind: error.kind(),
        });
        SystemTime::UNIX_EPOCH
    });
    Ok(Some(ArchiveEntry {
        provider,
        session_id: id.to_string(),
        file: path.to_path_buf(),
        size_bytes: md.len(),
        mtime,
        archived,
    }))
}

/// Every session in both archives, newest first, one file per session.
///
/// A Claude session moved between projects can leave its old file behind. The newest file
/// is the live one. Choosing it here, once, means listing, lookup, search, and the index
/// all describe the same file; when only the index chose, a list showed the session twice,
/// every prefix was ambiguous, and search returned a row mixing the two files (#28).
pub fn scan_all(paths: &Paths) -> Vec<ArchiveEntry> {
    scan_all_with_failures(paths).0
}

// Issue #8: GC needs failures from the same traversal as entries before deleting paid summaries.
pub(crate) fn scan_all_with_failures(paths: &Paths) -> (Vec<ArchiveEntry>, Vec<ScanFailure>) {
    let mut failures = Vec::new();
    let mut all = Vec::new();
    for (root, provider, archived) in [
        (&paths.claude_archive, Provider::Claude, false),
        (&paths.codex_archive, Provider::Codex, false),
        (&paths.codex_archived, Provider::Codex, true),
    ] {
        let t = std::time::Instant::now();
        let before = failures.len();
        let found = scan_root(root, provider, archived, &mut failures);
        tracing::debug!(
            root = %root.display(),
            files = found.len(),
            failures = failures.len() - before,
            ms = t.elapsed().as_millis() as u64,
            "scanned archive root"
        );
        for f in &failures[before..] {
            tracing::debug!(path = %f.path.display(), kind = ?f.kind, "could not read under archive root");
        }
        all.extend(found);
    }
    // Issue #8: archived_sessions is optional, but only an accessible active root
    // lets GC distinguish a never-created archive from unavailable Codex storage.
    let active_unavailable = failures.iter().any(|f| f.path == paths.codex_archive);
    failures.retain(|f| {
        active_unavailable
            || f.path != paths.codex_archived
            || f.kind != std::io::ErrorKind::NotFound
    });
    // Codex can retain both formats on resume. Newest still wins (#28); at equal
    // mtimes prefer the writable plain file, then path order, independent of traversal.
    all.sort_by(|a, b| {
        b.mtime
            .cmp(&a.mtime)
            .then(a.session_id.cmp(&b.session_id))
            .then_with(|| is_compressed(&a.file).cmp(&is_compressed(&b.file)))
            .then(a.file.cmp(&b.file))
    });
    let mut seen = std::collections::HashSet::new();
    all.retain(|e| seen.insert((e.provider, e.session_id.clone())));
    (all, failures)
}

// --- lookup ----------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum LookupError {
    #[error("no {} session matches \"{id}\"", provider.map(|b| b.as_str()).unwrap_or("archived"))]
    NotFound {
        id: String,
        provider: Option<Provider>,
    },
    #[error("\"{id}\" matches {} sessions ({}); use a longer prefix", candidates.len(), candidates.join(", "))]
    Ambiguous { id: String, candidates: Vec<String> },
}

/// Exact id wins. Short of that an unambiguous prefix resolves, because full ids are
/// unwieldy to type or say aloud and callers routinely shorten them.
pub fn find_session(
    paths: &Paths,
    id: &str,
    provider: Option<Provider>,
) -> Result<ArchiveEntry, LookupError> {
    let candidates: Vec<ArchiveEntry> = scan_all(paths)
        .into_iter()
        .filter(|e| provider.is_none_or(|b| e.provider == b))
        .collect();
    if let Some(exact) = candidates.iter().find(|e| e.session_id == id) {
        return Ok(exact.clone());
    }
    let mut prefixed: Vec<ArchiveEntry> = if id.is_empty() {
        Vec::new()
    } else {
        candidates
            .into_iter()
            .filter(|e| e.session_id.starts_with(id))
            .collect()
    };
    match prefixed.len() {
        0 => Err(LookupError::NotFound {
            id: id.to_string(),
            provider,
        }),
        1 => Ok(prefixed.remove(0)),
        _ => Err(LookupError::Ambiguous {
            id: id.to_string(),
            candidates: prefixed
                .iter()
                .map(|e| format!("{}/{}", e.provider, e.session_id))
                .collect(),
        }),
    }
}

// --- metadata --------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMeta {
    /// The directory the session started in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Where the session lives now. Claude records `relocated` when a session moves to a
    /// different project directory, and a resume has to run from there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relocated_cwd: Option<String>,
    /// The first real user message, collapsed and cut to 120 characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// What a person would call it: what they typed, else what the provider generated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_source: Option<TitleSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Artifacts the session published, as the provider recorded them.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub artifacts: Vec<String>,
    /// Pull requests the session opened, as the provider recorded them.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub pull_requests: Vec<String>,
    /// Codex: which client wrote the session (`codex_exec`, `Codex Desktop`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub originator: Option<String>,
    /// How the session was started, in the provider's own terms. Claude: `entrypoint`
    /// (`cli`, `claude-desktop`, `sdk-cli` for `claude -p`). Codex: `source` (`cli`,
    /// `vscode`, `exec`), or for a subagent its kind (`guardian`, `thread_spawn`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Started by another session rather than by a person. On 2026-10-07, 1,694 of 3,705
    /// local Codex sessions were `guardian` approval reviewers, which is why lists hide
    /// subagents unless asked.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub subagent: bool,
    /// The session that started this one, when the provider records it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TitleSource {
    Custom,
    Generated,
}

impl SessionMeta {
    /// The directory a resume should run from.
    pub fn project(&self) -> Option<&str> {
        self.relocated_cwd.as_deref().or(self.cwd.as_deref())
    }
}

/// Reads cwd and snippets from head windows, and rewritten metadata from the tail.
/// Issue #36: an unindexed file gets a title scan only within the configured byte budget.
pub fn read_meta(e: &ArchiveEntry, limits: &Limits) -> SessionMeta {
    read_meta_inner(e, limits, true)
}

pub(crate) fn read_meta_windows(e: &ArchiveEntry, limits: &Limits) -> SessionMeta {
    read_meta_inner(e, limits, false)
}

fn read_meta_inner(e: &ArchiveEntry, limits: &Limits, scan_titles: bool) -> SessionMeta {
    let mut meta = SessionMeta::default();
    let mut covered = 0u64;
    for &step in &limits.head_steps {
        if !is_compressed(&e.file) && covered >= e.size_bytes {
            break;
        }
        let (head, complete) = match read_range(&e.file, 0, step) {
            Ok(head) => head,
            Err(error) => {
                tracing::warn!(path = %e.file.display(), %error, "could not read archive metadata");
                break;
            }
        };
        meta = match e.provider {
            Provider::Claude => parse_claude_head(&head),
            Provider::Codex => parse_codex_head(&head, e.finished(), complete),
        };
        covered = step as u64;
        if complete || (meta.cwd.is_some() && meta.snippet.is_some()) {
            break;
        }
    }
    if e.provider == Provider::Claude && covered < e.size_bytes {
        let tail_from = e.size_bytes.saturating_sub(TAIL_BYTES as u64).max(covered);
        if let Ok((tail, _)) = read_range(&e.file, tail_from, TAIL_BYTES) {
            apply_claude_latest(&mut meta, lines_after_first(&tail));
        }
        if scan_titles && e.size_bytes <= limits.title_scan_max_bytes as u64 {
            let mut custom_title = None;
            if scan_custom_titles(&e.file, 0, e.size_bytes, &mut custom_title).is_ok() {
                apply_custom_title(&mut meta, custom_title);
            }
        }
    }
    meta
}

const TAIL_BYTES: usize = 256 * 1024;

pub(crate) fn apply_custom_title(meta: &mut SessionMeta, title: Option<String>) {
    if let Some(title) = title {
        meta.title = Some(title);
        meta.title_source = Some(TitleSource::Custom);
    }
}

pub(crate) fn custom_title(record: &Value) -> Option<&str> {
    (str_at(record, "type") == Some("custom-title"))
        .then(|| str_at(record, "customTitle"))
        .flatten()
}

pub(crate) fn scan_custom_titles(
    path: &Path,
    from: u64,
    until: u64,
    title: &mut Option<String>,
) -> std::io::Result<()> {
    // Issue #36: only candidate records need JSON parsing. Large byte chunks avoid
    // validating every transcript line as UTF-8, including on a metadata-only upgrade.
    let mut reader = open_archive(path, from)?.take(until.saturating_sub(from));
    let mut chunk = [0u8; 64 * 1024];
    let mut pending = Vec::new();
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&chunk[..n]);
        if let Some(end) = memchr::memrchr(b'\n', &chunk[..n]) {
            let end = pending.len() - n + end;
            let complete = &pending[..=end];
            let mut parsed_until = 0;
            for at in memchr::memmem::find_iter(complete, b"custom-title") {
                if at < parsed_until {
                    continue;
                }
                let start = memchr::memrchr(b'\n', &complete[..at]).map_or(0, |i| i + 1);
                let stop = at + memchr::memchr(b'\n', &complete[at..]).unwrap();
                parsed_until = stop + 1;
                if let Ok(record) = serde_json::from_slice::<Value>(&complete[start..stop])
                    && let Some(t) = custom_title(&record)
                {
                    *title = Some(t.to_owned());
                }
            }
            pending.drain(..=end);
        }
    }
    // Issue #36: metadata windows accept complete final JSON without a newline too.
    if memchr::memmem::find(&pending, b"custom-title").is_some()
        && let Ok(record) = serde_json::from_slice::<Value>(&pending)
        && let Some(t) = custom_title(&record)
    {
        *title = Some(t.to_owned());
    }
    Ok(())
}

fn read_range(path: &Path, from: u64, len: usize) -> std::io::Result<(String, bool)> {
    let mut reader = open_archive(path, from)?;
    let mut buf = Vec::with_capacity(len.min(1 << 20));
    reader.by_ref().take(len as u64).read_to_end(&mut buf)?;
    // Compressed disk size cannot tell whether the head contains the complete rollout,
    // which determines whether an impossible fork boundary may use its fallback (#35).
    let complete = reader.fill_buf()?.is_empty();
    Ok((String::from_utf8_lossy(&buf).into_owned(), complete))
}

/// A tail read starts mid-line; the first fragment is never a whole record.
fn lines_after_first(s: &str) -> &str {
    s.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
}

fn records(text: &str) -> impl Iterator<Item = Value> + '_ {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Text from either a bare string or an array of content blocks.
pub(crate) fn block_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// A snippet a person would recognise: whitespace collapsed, at most 120 characters, and
/// never injected context, which providers wrap in angle-bracket tags.
fn snippet_of(text: &str) -> Option<String> {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // A Claude Desktop scheduled task wraps its whole prompt in a tag; the task's name is
    // what a person would recognise it by.
    if let Some(rest) = clean.strip_prefix("<scheduled-task name=\"") {
        return rest
            .split_once('"')
            .map(|(name, _)| format!("scheduled task: {name}"));
    }
    if clean.is_empty() || clean.starts_with('<') {
        return None;
    }
    Some(clean.chars().take(120).collect())
}

fn parse_claude_head(text: &str) -> SessionMeta {
    let mut meta = SessionMeta::default();
    for r in records(text) {
        if meta.cwd.is_none() {
            meta.cwd = str_at(&r, "cwd").map(str::to_string);
        }
        if meta.origin.is_none() {
            meta.origin = str_at(&r, "entrypoint").map(str::to_string);
        }
        // Issue #38: generated context cannot supply the first prompt the user typed.
        if meta.snippet.is_none()
            && str_at(&r, "type") == Some("user")
            && r.get("isMeta").and_then(Value::as_bool) != Some(true)
            && r.get("isCompactSummary").and_then(Value::as_bool) != Some(true)
        {
            meta.snippet = typed_snippet(r.pointer("/message/content").unwrap_or(&Value::Null));
        }
        apply_claude_latest_record(&mut meta, &r);
    }
    meta
}

fn apply_claude_latest(meta: &mut SessionMeta, text: &str) {
    for r in records(text) {
        apply_claude_latest_record(meta, &r);
    }
}

/// Records Claude rewrites as the session goes. Later is current, so last one wins.
fn apply_claude_latest_record(meta: &mut SessionMeta, r: &Value) {
    match str_at(r, "type") {
        Some("custom-title") => {
            if let Some(t) = str_at(r, "customTitle") {
                meta.title = Some(t.to_string());
                meta.title_source = Some(TitleSource::Custom);
            }
        }
        Some("ai-title") => {
            if let Some(t) = str_at(r, "aiTitle") {
                // A title the user typed outranks one the provider generated.
                if meta.title_source != Some(TitleSource::Custom) {
                    meta.title = Some(t.to_string());
                    meta.title_source = Some(TitleSource::Generated);
                }
            }
        }
        Some("agent-name") => {
            if let Some(n) = str_at(r, "agentName") {
                meta.agent_name = Some(n.to_string());
            }
        }
        Some("relocated") => {
            if let Some(c) = str_at(r, "relocatedCwd") {
                meta.relocated_cwd = Some(c.to_string());
            }
        }
        Some("frame-link") => push_unique(&mut meta.artifacts, str_at(r, "frameUrl")),
        Some("pr-link") => push_unique(&mut meta.pull_requests, str_at(r, "prUrl")),
        _ => {}
    }
}

fn push_unique(list: &mut Vec<String>, item: Option<&str>) {
    if let Some(s) = item
        && !list.iter().any(|x| x == s)
    {
        list.push(s.to_string());
    }
}

/// Codex has written the first user message three ways. Current versions emit an
/// `item_completed` event carrying a `UserMessage` item; older versions emit
/// `event_msg/user_message`; both also write the model-facing `response_item/message`,
/// which includes injected context. The first two are what the user typed, so they win.
fn parse_codex_head(text: &str, finished: bool, complete: bool) -> SessionMeta {
    let mut meta = SessionMeta::default();
    let (mut from_item, mut from_event, mut from_response) = (None, None, None);
    let mut seen_meta = false;
    let boundary = crate::codex::fork_boundary(&mut std::io::Cursor::new(text), finished, complete)
        .expect("reading an in-memory head");
    let inherited_end = boundary.end.unwrap_or(boundary.scanned);
    let mut fork_snippet = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len() as u64;
        if line_start > 0 && boundary.end.is_none() && line_start >= boundary.scanned {
            break;
        }
        if line_start > 0 && line_start < inherited_end {
            continue;
        }
        let Ok(r) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let p = r.get("payload").unwrap_or(&Value::Null);
        match (str_at(&r, "type"), str_at(p, "type")) {
            // Only the first session_meta describes this file. In 510 of 3,746 local files
            // further ones followed, always for other sessions (a spawned child, a fork's
            // parent); reading them let a later record overwrite the origin, so the origin
            // depended on how far the head read reached, and a parent that embedded a
            // child's record was marked as a subagent.
            (Some("session_meta"), _) if !seen_meta => {
                seen_meta = true;
                meta.cwd = str_at(p, "cwd").map(str::to_string);
                meta.originator = str_at(p, "originator").map(str::to_string);
                apply_codex_source(&mut meta, p);
            }
            // Issue #35: a fork's initial assignment is an inter-agent message,
            // not a user turn. It still describes the child's work in session lists.
            (Some("response_item"), Some("agent_message"))
                if inherited_end > 0 && fork_snippet.is_none() =>
            {
                fork_snippet = typed_snippet(p.get("content").unwrap_or(&Value::Null));
            }
            (_, Some("item_completed")) if from_item.is_none() => {
                let item = p.get("item").unwrap_or(&Value::Null);
                if str_at(item, "type") == Some("UserMessage") {
                    from_item = typed_snippet(item.get("content").unwrap_or(&Value::Null));
                }
            }
            (_, Some("user_message")) if from_event.is_none() => {
                from_event = str_at(p, "message").and_then(snippet_of);
            }
            (Some("response_item"), Some("message"))
                if from_response.is_none() && str_at(p, "role") == Some("user") =>
            {
                from_response = typed_snippet(p.get("content").unwrap_or(&Value::Null));
            }
            // The oldest files have no envelope: the record is its own payload.
            (Some("message"), None)
                if from_response.is_none() && str_at(&r, "role") == Some("user") =>
            {
                from_response = typed_snippet(r.get("content").unwrap_or(&Value::Null));
            }
            _ => {}
        }
    }
    meta.snippet = fork_snippet.or(from_item).or(from_event).or(from_response);
    meta
}

/// `source` is a string for a top-level session and an object for a subagent:
/// `{"subagent": {"other": "guardian"}}` for an approval reviewer, or
/// `{"subagent": {"thread_spawn": {"agent_nickname": ..., "agent_path": ...}}}` for a
/// spawned agent.
fn apply_codex_source(meta: &mut SessionMeta, p: &Value) {
    match p.get("source") {
        Some(Value::String(s)) => meta.origin = Some(s.clone()),
        Some(Value::Object(o)) => {
            if let Some(sub) = o.get("subagent") {
                meta.subagent = true;
                meta.origin = Some(match sub {
                    Value::Object(kind) => match kind.iter().next() {
                        Some((k, Value::String(v))) if k == "other" => v.clone(),
                        Some((k, _)) => k.clone(),
                        None => "subagent".to_string(),
                    },
                    Value::String(v) => v.clone(),
                    _ => "subagent".to_string(),
                });
                if let Some(nick) = sub
                    .pointer("/thread_spawn/agent_nickname")
                    .and_then(Value::as_str)
                {
                    meta.agent_name = Some(nick.to_string());
                }
            }
        }
        _ => {}
    }
    if let Some(parent) = str_at(p, "parent_thread_id") {
        meta.parent_session_id = Some(parent.to_string());
        meta.subagent = true;
    }
}

/// The first content block a person typed. Both providers put injected context in its own
/// block ahead of what the user wrote: Claude a `<system-reminder>`, Codex Desktop the
/// project's AGENTS.md. Joining the blocks first hid the typed text in 50 top-level local
/// sessions on 2026-10-07. Tool results are not typed text, so only text blocks count.
fn typed_snippet(content: &Value) -> Option<String> {
    match content {
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) != Some("tool_result"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .filter(|t| !t.trim_start().starts_with("# AGENTS.md instructions"))
            .find_map(snippet_of),
        other => snippet_of(&block_text(other)),
    }
}

// --- resuming --------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Resumability {
    /// Small enough for the provider to accept a resumed turn.
    pub resumable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The transcript's size as the provider sees it on resume. A compressed rollout is
/// several times smaller on disk: one measured at 749 KB held 3.0 MB of JSONL, under the
/// resume ceiling by disk size and well over it by content. Codex records the decompressed
/// size in the frame header, so this reads at most 18 bytes and never decodes the file;
/// a header without one falls back to the disk size.
pub fn transcript_bytes(e: &ArchiveEntry) -> u64 {
    if !is_compressed(&e.file) {
        return e.size_bytes;
    }
    let mut header = [0u8; 18];
    let n = File::open(&e.file)
        .and_then(|mut f| f.read(&mut header))
        .unwrap_or(0);
    match zstd::zstd_safe::get_frame_content_size(&header[..n]) {
        Ok(Some(size)) => size,
        _ => e.size_bytes,
    }
}

pub fn resumability(e: &ArchiveEntry, limits: &Limits) -> Resumability {
    let size = transcript_bytes(e);
    let resumable = size < limits.resume_limit_bytes;
    Resumability {
        resumable,
        reason: (!resumable).then(|| {
            format!(
                "transcript is {}KB, over the ~{}KB practical resume ceiling; to carry the work on, read it with sessions action=text and start a new session from what matters",
                size / 1024,
                limits.resume_limit_bytes / 1024
            )
        }),
    }
}

/// The command a person would paste to pick the session up in a terminal.
pub fn open_with(provider: Provider, project: Option<&str>, id: &str) -> String {
    let cmd = match provider {
        Provider::Claude => format!("claude --resume {id}"),
        Provider::Codex => format!("codex resume {id}"),
    };
    match project {
        Some(p) => format!("cd {} && {cmd}", shell_quote(p)),
        None => cmd,
    }
}

/// Single-quoted, so a path containing `$`, backticks, or spaces pastes safely.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_codex_id_is_the_trailing_uuid_of_the_rollout_name() {
        assert_eq!(
            trailing_uuid("rollout-2026-10-07T12-16-06-01a117cb-5e0a-7c31-a391-fa9b66aa0b1c.jsonl"),
            Some("01a117cb-5e0a-7c31-a391-fa9b66aa0b1c")
        );
        assert_eq!(trailing_uuid("notes.jsonl"), None);
        assert_eq!(
            trailing_uuid("rollout-zzzzzzzz-5e0a-7c31-a391-fa9b66aa0b1c.jsonl"),
            None
        );
    }

    #[test]
    fn open_with_quotes_a_project_path_that_a_shell_would_split() {
        assert_eq!(
            open_with(
                Provider::Claude,
                Some("/Users/me/Application Support/it's"),
                "abc"
            ),
            r#"cd '/Users/me/Application Support/it'\''s' && claude --resume abc"#
        );
    }

    #[test]
    fn injected_context_is_never_a_snippet() {
        assert_eq!(
            snippet_of("<environment_context>cwd</environment_context>"),
            None
        );
        assert_eq!(
            snippet_of("  fix\n\nthe   build "),
            Some("fix the build".to_string())
        );
        assert_eq!(
            snippet_of(r#"<scheduled-task name="daily-scan" file="/x">do it</scheduled-task>"#),
            Some("scheduled task: daily-scan".to_string())
        );
    }

    #[test]
    fn a_typed_title_outranks_a_generated_one_whichever_comes_last() {
        let mut m = SessionMeta::default();
        apply_claude_latest(
            &mut m,
            r#"{"type":"custom-title","customTitle":"mine"}
{"type":"ai-title","aiTitle":"generated"}"#,
        );
        assert_eq!(m.title.as_deref(), Some("mine"));
        assert_eq!(m.title_source, Some(TitleSource::Custom));
    }
}

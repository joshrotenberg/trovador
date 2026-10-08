//! The provider archives: scanning, addressing, metadata, resumability.
//!
//! Both archives are read only. Nothing here writes under `~/.claude` or `~/.codex`.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::limits::Limits;
use crate::paths::Paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Claude,
    Codex,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Claude => "claude",
            Backend::Codex => "codex",
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Backend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "claude" => Ok(Backend::Claude),
            "codex" => Ok(Backend::Codex),
            other => Err(format!("unknown backend \"{other}\"; have: claude, codex")),
        }
    }
}

/// One session file, as found on disk, before anything inside it is read.
#[derive(Clone, Debug)]
pub struct ArchiveEntry {
    pub backend: Backend,
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
}

// --- scanning --------------------------------------------------------------

/// Claude keeps `<root>/<project>/<uuid>.jsonl`. Subagent transcripts live one level
/// deeper under `<session>/subagents/` and are deliberately not listed as sessions: they
/// cannot be resumed on their own.
pub fn scan_claude(root: &Path) -> Vec<ArchiveEntry> {
    let mut out = Vec::new();
    let Ok(projects) = fs::read_dir(root) else {
        return out;
    };
    for project in projects.flatten() {
        let Ok(files) = fs::read_dir(project.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(id) = name.strip_suffix(".jsonl") else {
                continue;
            };
            if let Some(e) = entry(Backend::Claude, id, &path, false) {
                out.push(e);
            }
        }
    }
    out
}

/// Codex nests by date, `<root>/YYYY/MM/DD/rollout-<timestamp>-<uuid>.jsonl`, and moves an
/// archived thread to a flat `archived_sessions` directory under the same name. The id is
/// the trailing UUID, which is what `codex resume` accepts.
pub fn scan_codex(root: &Path, archived: bool) -> Vec<ArchiveEntry> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(items) = fs::read_dir(&dir) else {
            continue;
        };
        for item in items.flatten() {
            let path = item.path();
            let Ok(ft) = item.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Some(id) = trailing_uuid(name)
                && let Some(e) = entry(Backend::Codex, id, &path, archived)
            {
                out.push(e);
            }
        }
    }
    out
}

fn trailing_uuid(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".jsonl")?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    let shape_ok = id.char_indices().all(|(i, c)| match i {
        8 | 13 | 18 | 23 => c == '-',
        _ => c.is_ascii_hexdigit(),
    });
    shape_ok.then_some(id)
}

fn entry(backend: Backend, id: &str, path: &Path, archived: bool) -> Option<ArchiveEntry> {
    let md = fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    Some(ArchiveEntry {
        backend,
        session_id: id.to_string(),
        file: path.to_path_buf(),
        size_bytes: md.len(),
        mtime: md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        archived,
    })
}

/// Every session in both archives, newest first.
pub fn scan_all(paths: &Paths) -> Vec<ArchiveEntry> {
    let mut all = scan_claude(&paths.claude_archive);
    all.extend(scan_codex(&paths.codex_archive, false));
    all.extend(scan_codex(&paths.codex_archived, true));
    all.sort_by(|a, b| b.mtime.cmp(&a.mtime).then(a.session_id.cmp(&b.session_id)));
    all
}

// --- lookup ----------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum LookupError {
    #[error("no {} session matches \"{id}\"", backend.map(|b| b.as_str()).unwrap_or("archived"))]
    NotFound {
        id: String,
        backend: Option<Backend>,
    },
    #[error("\"{id}\" matches {} sessions ({}); use a longer prefix", candidates.len(), candidates.join(", "))]
    Ambiguous { id: String, candidates: Vec<String> },
}

/// Exact id wins. Short of that an unambiguous prefix resolves, because full ids are
/// unwieldy to type or say aloud and callers routinely shorten them.
pub fn find_session(
    paths: &Paths,
    id: &str,
    backend: Option<Backend>,
) -> Result<ArchiveEntry, LookupError> {
    let candidates: Vec<ArchiveEntry> = scan_all(paths)
        .into_iter()
        .filter(|e| backend.is_none_or(|b| e.backend == b))
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
            backend,
        }),
        1 => Ok(prefixed.remove(0)),
        _ => Err(LookupError::Ambiguous {
            id: id.to_string(),
            candidates: prefixed
                .iter()
                .map(|e| format!("{}/{}", e.backend, e.session_id))
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

/// Reads as little of the file as answers the question. Most sessions resolve from the
/// first 64 KB; the wider steps only run while cwd or the first user message is missing.
/// Claude's titles and relocation are rewritten as the session goes, so the latest value
/// is near the end and comes from a tail read rather than from reading everything.
pub fn read_meta(e: &ArchiveEntry, limits: &Limits) -> SessionMeta {
    let mut meta = SessionMeta::default();
    // How many bytes from the start the last head read covered.
    let mut covered = 0u64;
    for &step in &limits.head_steps {
        if covered >= e.size_bytes {
            break;
        }
        let Ok(head) = read_range(&e.file, 0, step) else {
            break;
        };
        meta = match e.backend {
            Backend::Claude => parse_claude_head(&head),
            Backend::Codex => parse_codex_head(&head),
        };
        covered = step as u64;
        if meta.cwd.is_some() && meta.snippet.is_some() {
            break;
        }
    }
    if e.backend == Backend::Claude && covered < e.size_bytes {
        let tail_from = e.size_bytes.saturating_sub(TAIL_BYTES as u64).max(covered);
        if let Ok(tail) = read_range(&e.file, tail_from, TAIL_BYTES) {
            // Starting exactly where the head stopped is a line boundary only by luck.
            apply_claude_latest(&mut meta, lines_after_first(&tail));
        }
    }
    meta
}

const TAIL_BYTES: usize = 256 * 1024;

fn read_range(path: &Path, from: u64, len: usize) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity(len.min(1 << 20));
    f.take(len as u64).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
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
        if meta.snippet.is_none()
            && str_at(&r, "type") == Some("user")
            && r.get("isMeta").and_then(Value::as_bool) != Some(true)
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
fn parse_codex_head(text: &str) -> SessionMeta {
    let mut meta = SessionMeta::default();
    let (mut from_item, mut from_event, mut from_response) = (None, None, None);
    for r in records(text) {
        let p = r.get("payload").unwrap_or(&Value::Null);
        match (str_at(&r, "type"), str_at(p, "type")) {
            (Some("session_meta"), _) => {
                if meta.cwd.is_none() {
                    meta.cwd = str_at(p, "cwd").map(str::to_string);
                }
                if meta.originator.is_none() {
                    meta.originator = str_at(p, "originator").map(str::to_string);
                }
                apply_codex_source(&mut meta, p);
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
    meta.snippet = from_item.or(from_event).or(from_response);
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

pub fn resumability(e: &ArchiveEntry, limits: &Limits) -> Resumability {
    let resumable = e.size_bytes < limits.resume_limit_bytes;
    Resumability {
        resumable,
        reason: (!resumable).then(|| {
            format!(
                "transcript is {}KB, over the ~{}KB practical resume ceiling; to carry the work on, ask with about set to this session and the same backend, which starts a new session seeded with its summary",
                e.size_bytes / 1024,
                limits.resume_limit_bytes / 1024
            )
        }),
    }
}

/// The command a person would paste to pick the session up in a terminal.
pub fn open_with(backend: Backend, project: Option<&str>, id: &str) -> String {
    let cmd = match backend {
        Backend::Claude => format!("claude --resume {id}"),
        Backend::Codex => format!("codex resume {id}"),
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
                Backend::Claude,
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

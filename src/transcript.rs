//! Transcript projection: raw provider JSONL into bounded, allowlisted records.
//!
//! Anything not recognised becomes an `omitted` record with a category rather than being
//! dropped, so a reader can tell "nothing happened" from "something happened that this
//! projection does not show". Consecutive omitted records of one category collapse into one
//! with a count: a Claude session carries hundreds of `total_tokens_reminder` attachments,
//! and one record each would fill a page with noise.
//!
//! Files are streamed line by line. Codex sessions over 370 MB exist, so nothing here reads
//! a whole file into memory, and a page stops reading once it is full.

use std::fs::File;
use std::io::{BufRead, BufReader};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::archive::{ArchiveEntry, Backend, block_text};
use crate::limits::Limits;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    Attachment,
    System,
    Omitted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Record {
    /// Position in the projection, stable for a given file and projection version.
    pub index: usize,
    #[serde(rename = "type")]
    pub kind: RecordKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Tool name, attachment file name, or omitted category.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// How many consecutive source records an `omitted` record stands for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}

/// A record before it has a position.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Draft {
    pub kind: RecordKind,
    pub text: Option<String>,
    pub name: Option<String>,
}

impl Draft {
    fn new(kind: RecordKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: Some(text.into()),
            name: None,
        }
    }
    fn named(kind: RecordKind, name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: Some(text.into()),
            name: Some(name.into()),
        }
    }
    fn omitted(category: impl Into<String>) -> Self {
        Self {
            kind: RecordKind::Omitted,
            text: None,
            name: Some(category.into()),
        }
    }
}

fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push_str(" ...[truncated]");
        out
    }
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

// --- claude ----------------------------------------------------------------

fn project_claude(r: &Value, limits: &Limits, out: &mut Vec<Draft>) {
    let (rc, tc) = (limits.record_chars, limits.tool_chars);
    match str_at(r, "type") {
        Some("user") => match r.pointer("/message/content") {
            Some(Value::String(s)) => out.push(Draft::new(RecordKind::User, cap(s, rc))),
            Some(Value::Array(blocks)) => {
                for b in blocks {
                    match str_at(b, "type") {
                        Some("text") => {
                            if let Some(t) = str_at(b, "text").filter(|t| !t.trim().is_empty()) {
                                out.push(Draft::new(RecordKind::User, cap(t, rc)));
                            }
                        }
                        Some("tool_result") => out.push(Draft::new(
                            RecordKind::ToolResult,
                            cap(&block_text(b.get("content").unwrap_or(&Value::Null)), tc),
                        )),
                        _ => {}
                    }
                }
            }
            _ => {}
        },
        Some("assistant") => {
            for b in r
                .pointer("/message/content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                match str_at(b, "type") {
                    Some("text") => {
                        if let Some(t) = str_at(b, "text").filter(|t| !t.trim().is_empty()) {
                            out.push(Draft::new(RecordKind::Assistant, cap(t, rc)));
                        }
                    }
                    Some("tool_use") => out.push(Draft::named(
                        RecordKind::ToolCall,
                        str_at(b, "name").unwrap_or("tool"),
                        cap(&b.get("input").unwrap_or(&Value::Null).to_string(), tc),
                    )),
                    _ => {}
                }
            }
        }
        // Most attachment subtypes are UI bookkeeping. Two carry content that is otherwise
        // unrecoverable: a file the user pasted or referenced, and a subagent's final
        // structured result.
        Some("attachment") => {
            let a = r.get("attachment").unwrap_or(&Value::Null);
            match str_at(a, "type") {
                Some("file") => match a.pointer("/content/file/content").and_then(Value::as_str) {
                    Some(content) => out.push(Draft::named(
                        RecordKind::Attachment,
                        str_at(a, "filename").unwrap_or("file"),
                        cap(content, rc),
                    )),
                    None => out.push(Draft::omitted("attachment")),
                },
                Some("structured_output") if a.get("data").is_some() => out.push(Draft::named(
                    RecordKind::Attachment,
                    "structured_output",
                    cap(&a["data"].to_string(), rc),
                )),
                _ => out.push(Draft::omitted("attachment")),
            }
        }
        // An away summary is a recap sent to the user; other system subtypes are noise.
        Some("system") => match (str_at(r, "subtype"), str_at(r, "content")) {
            (Some("away_summary"), Some(c)) => out.push(Draft::new(RecordKind::System, cap(c, rc))),
            _ => out.push(Draft::omitted("system")),
        },
        Some(other) => out.push(Draft::omitted(other)),
        None => {}
    }
}

// --- codex -----------------------------------------------------------------

/// Which of Codex's parallel streams carries the conversation.
///
/// Codex writes each turn more than once: a model-facing `response_item` (which includes
/// injected context) and a UI-facing event. Projecting both duplicates every message. The
/// UI-facing stream is what the user actually saw, so it wins whenever it exists:
///
/// - `Items`: current versions, `event_msg/item_completed` with typed items.
/// - `Events`: older versions, `event_msg/user_message` and `event_msg/agent_message`.
/// - `Responses`: the oldest files, where `response_item` (or a bare top-level record) is
///   all there is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CodexMode {
    Responses,
    Events,
    Items,
}

/// The mode a single record proves the file is in, if any.
pub(crate) fn codex_mode_signal(r: &Value) -> Option<CodexMode> {
    if str_at(r, "type") != Some("event_msg") {
        return None;
    }
    match r.pointer("/payload/type").and_then(Value::as_str) {
        Some("item_completed") => Some(CodexMode::Items),
        Some("user_message" | "agent_message") => Some(CodexMode::Events),
        _ => None,
    }
}

pub(crate) fn project_codex(r: &Value, mode: CodexMode, limits: &Limits, out: &mut Vec<Draft>) {
    let (rc, tc) = (limits.record_chars, limits.tool_chars);
    let outer = str_at(r, "type");
    // The oldest files have no envelope: the record is its own payload.
    let p = match r.get("payload") {
        Some(p) => p,
        None if matches!(
            outer,
            Some("message" | "function_call" | "function_call_output")
        ) =>
        {
            r
        }
        None => return,
    };
    let inner = str_at(p, "type");
    match (outer, inner) {
        (_, Some("item_completed")) => {
            if mode == CodexMode::Items {
                project_codex_item(p.get("item").unwrap_or(&Value::Null), limits, out);
            }
        }
        (Some("event_msg"), Some("user_message")) => {
            if mode == CodexMode::Events
                && let Some(m) = str_at(p, "message")
            {
                out.push(Draft::new(RecordKind::User, cap(m, rc)));
            }
        }
        (Some("event_msg"), Some("agent_message")) => {
            if mode == CodexMode::Events
                && let Some(m) = str_at(p, "message")
            {
                out.push(Draft::new(RecordKind::Assistant, cap(m, rc)));
            }
        }
        (_, Some("message")) => {
            if mode != CodexMode::Responses {
                return;
            }
            let text = block_text(p.get("content").unwrap_or(&Value::Null));
            if text.trim().is_empty() {
                return;
            }
            match str_at(p, "role") {
                Some("user") => out.push(Draft::new(RecordKind::User, cap(&text, rc))),
                Some("assistant") => out.push(Draft::new(RecordKind::Assistant, cap(&text, rc))),
                Some(role) => out.push(Draft::omitted(role)),
                None => out.push(Draft::omitted("message")),
            }
        }
        // In item mode the same calls arrive as CommandExecution and McpToolCall items.
        (_, Some("function_call" | "custom_tool_call")) => {
            if mode != CodexMode::Items {
                let args = str_at(p, "arguments")
                    .or_else(|| str_at(p, "input"))
                    .unwrap_or("");
                out.push(Draft::named(
                    RecordKind::ToolCall,
                    str_at(p, "name").unwrap_or("tool"),
                    cap(args, tc),
                ));
            }
        }
        (_, Some("function_call_output" | "custom_tool_call_output")) => {
            if mode != CodexMode::Items {
                let output = match p.get("output") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                out.push(Draft::new(RecordKind::ToolResult, cap(&output, tc)));
            }
        }
        // Messages between Codex agents in a multi-agent run.
        (Some("response_item"), Some("agent_message")) => {
            let from = str_at(p, "author").unwrap_or("agent");
            let to = str_at(p, "recipient").unwrap_or("agent");
            out.push(Draft::named(
                RecordKind::System,
                format!("{from} -> {to}"),
                cap(&block_text(p.get("content").unwrap_or(&Value::Null)), rc),
            ));
        }
        (_, Some(kind)) => out.push(Draft::omitted(kind)),
        (Some(kind), None) => out.push(Draft::omitted(kind)),
        (None, None) => {}
    }
}

fn project_codex_item(item: &Value, limits: &Limits, out: &mut Vec<Draft>) {
    let (rc, tc) = (limits.record_chars, limits.tool_chars);
    let content = || block_text(item.get("content").unwrap_or(&Value::Null));
    match str_at(item, "type") {
        Some("UserMessage") => {
            let t = content();
            if !t.trim().is_empty() {
                out.push(Draft::new(RecordKind::User, cap(&t, rc)));
            }
        }
        Some("AgentMessage") => {
            let t = content();
            if !t.trim().is_empty() {
                out.push(Draft::new(RecordKind::Assistant, cap(&t, rc)));
            }
        }
        Some("CommandExecution") => {
            let cmd = item
                .get("parsed_cmd")
                .and_then(Value::as_array)
                .map(|cmds| {
                    cmds.iter()
                        .filter_map(|c| str_at(c, "cmd"))
                        .collect::<Vec<_>>()
                        .join(" && ")
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| block_text(item.get("command").unwrap_or(&Value::Null)));
            out.push(Draft::named(RecordKind::ToolCall, "exec", cap(&cmd, tc)));
            let stdout = str_at(item, "stdout").unwrap_or("");
            let stderr = str_at(item, "stderr").unwrap_or("");
            let result = if stderr.is_empty() {
                stdout.to_string()
            } else {
                format!("{stdout}\n[stderr] {stderr}")
            };
            out.push(Draft::new(RecordKind::ToolResult, cap(&result, tc)));
        }
        Some("McpToolCall") => {
            let name = format!(
                "{}/{}",
                str_at(item, "server").unwrap_or("mcp"),
                str_at(item, "tool").unwrap_or("tool")
            );
            let args = item
                .get("arguments")
                .map(Value::to_string)
                .unwrap_or_default();
            out.push(Draft::named(RecordKind::ToolCall, name, cap(&args, tc)));
            if let Some(err) = item.pointer("/error/message").and_then(Value::as_str) {
                out.push(Draft::new(
                    RecordKind::ToolResult,
                    cap(&format!("error: {err}"), tc),
                ));
            } else if let Some(res) = item.get("result") {
                out.push(Draft::new(
                    RecordKind::ToolResult,
                    cap(&res.to_string(), tc),
                ));
            }
        }
        Some("FileChange") => {
            let changes = item
                .get("changes")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(path, c)| {
                            format!("{} {path}", str_at(c, "type").unwrap_or("change"))
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            out.push(Draft::named(
                RecordKind::ToolCall,
                "file_change",
                cap(&changes, tc),
            ));
        }
        Some("Extension") => {
            let what = str_at(item, "query")
                .map(str::to_string)
                .unwrap_or_else(|| item.get("action").map(Value::to_string).unwrap_or_default());
            out.push(Draft::named(
                RecordKind::ToolCall,
                str_at(item, "kind").unwrap_or("extension"),
                cap(&what, tc),
            ));
        }
        Some("SubAgentActivity") => out.push(Draft::new(
            RecordKind::System,
            format!(
                "subagent {} {}",
                str_at(item, "agent_path").unwrap_or("?"),
                str_at(item, "kind").unwrap_or("activity")
            ),
        )),
        Some("ContextCompaction") => out.push(Draft::new(RecordKind::System, "context compacted")),
        Some(other) => out.push(Draft::omitted(other)),
        None => out.push(Draft::omitted("item")),
    }
}

// --- reading ---------------------------------------------------------------

/// Streams a session's projected records, with omitted runs collapsed and indexes assigned.
pub struct Projection {
    lines: std::io::Lines<BufReader<File>>,
    backend: Backend,
    limits: Limits,
    mode: CodexMode,
    pending: std::collections::VecDeque<Draft>,
    /// The omitted run still being counted, held until a different record arrives.
    run: Option<(String, usize)>,
    next_index: usize,
    done: bool,
}

impl Projection {
    pub fn open(e: &ArchiveEntry, limits: &Limits) -> std::io::Result<Self> {
        let mode = match e.backend {
            Backend::Codex => initial_codex_mode(e, limits),
            Backend::Claude => CodexMode::Responses,
        };
        Ok(Self {
            lines: BufReader::new(File::open(&e.file)?).lines(),
            backend: e.backend,
            limits: limits.clone(),
            mode,
            pending: Default::default(),
            run: None,
            next_index: 0,
            done: false,
        })
    }

    fn fill(&mut self) {
        while self.pending.is_empty() && !self.done {
            let Some(line) = self.lines.next() else {
                self.done = true;
                break;
            };
            // A line that is not valid UTF-8 or not JSON is skipped, as a torn final line
            // in a session still being written is normal.
            let Ok(line) = line else { continue };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(r) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let mut drafts = Vec::new();
            match self.backend {
                Backend::Claude => project_claude(&r, &self.limits, &mut drafts),
                Backend::Codex => {
                    // A stream seen later than the pre-scan reached still takes over from
                    // that point on.
                    if let Some(m) = codex_mode_signal(&r) {
                        self.mode = self.mode.max(m);
                    }
                    project_codex(&r, self.mode, &self.limits, &mut drafts)
                }
            }
            self.pending.extend(drafts);
        }
    }

    fn assign(&mut self, d: Draft, count: Option<usize>) -> Record {
        let index = self.next_index;
        self.next_index += 1;
        Record {
            index,
            kind: d.kind,
            text: d.text,
            name: d.name,
            count,
        }
    }
}

impl Iterator for Projection {
    type Item = Record;

    fn next(&mut self) -> Option<Record> {
        loop {
            self.fill();
            let Some(d) = self.pending.pop_front() else {
                // End of file: flush the run in progress.
                let (cat, n) = self.run.take()?;
                return Some(self.assign(Draft::omitted(cat), Some(n)));
            };
            if d.kind == RecordKind::Omitted {
                let cat = d.name.clone().unwrap_or_default();
                match &mut self.run {
                    Some((c, n)) if *c == cat => *n += 1,
                    Some(_) => {
                        let (prev, n) = self.run.replace((cat, 1)).expect("run is some");
                        return Some(self.assign(Draft::omitted(prev), Some(n)));
                    }
                    None => self.run = Some((cat, 1)),
                }
                continue;
            }
            if let Some((cat, n)) = self.run.take() {
                self.pending.push_front(d);
                return Some(self.assign(Draft::omitted(cat), Some(n)));
            }
            return Some(self.assign(d, None));
        }
    }
}

/// Decides the starting stream from the head of the file, so the first user turn, which
/// Codex writes as a `response_item` just before its first UI event, is not projected twice.
fn initial_codex_mode(e: &ArchiveEntry, limits: &Limits) -> CodexMode {
    let Ok(f) = File::open(&e.file) else {
        return CodexMode::Responses;
    };
    let budget = limits.head_steps.get(1).copied().unwrap_or(512 * 1024) as u64;
    let mut mode = CodexMode::Responses;
    let mut read = 0u64;
    for line in BufReader::new(f).lines() {
        let Ok(line) = line else { break };
        read += line.len() as u64 + 1;
        if let Ok(r) = serde_json::from_str::<Value>(&line)
            && let Some(m) = codex_mode_signal(&r)
        {
            mode = mode.max(m);
            if mode == CodexMode::Items {
                break;
            }
        }
        if read >= budget {
            break;
        }
    }
    mode
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TranscriptPage {
    pub offset: usize,
    pub records: Vec<Record>,
    /// More records exist after this page.
    pub has_more: bool,
    /// The next offset to ask for, when `has_more`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    /// Total record count, known only when the page reached the end of the file. Reading a
    /// 370 MB session to the end just to count it is not worth doing for a single page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    /// Set when the page stopped early because it reached the response size cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated_by: Option<String>,
}

pub fn page(
    e: &ArchiveEntry,
    offset: usize,
    limit: Option<usize>,
    limits: &Limits,
) -> std::io::Result<TranscriptPage> {
    let limit = limit.unwrap_or(limits.page_size).clamp(1, limits.page_size);
    let mut records = Vec::new();
    let mut chars = 0usize;
    let mut seen = 0usize;
    for r in Projection::open(e, limits)? {
        seen += 1;
        if r.index < offset {
            continue;
        }
        let size = r.text.as_deref().map(str::len).unwrap_or(0) + 64;
        let full = records.len() == limit;
        let over = !records.is_empty() && chars + size > limits.response_max_chars;
        if full || over {
            return Ok(TranscriptPage {
                offset,
                records,
                has_more: true,
                next_offset: Some(r.index),
                total: None,
                truncated_by: (over && !full).then(|| {
                    format!(
                        "response_max_chars ({}); continue from next_offset",
                        limits.response_max_chars
                    )
                }),
            });
        }
        chars += size;
        records.push(r);
    }
    Ok(TranscriptPage {
        offset,
        records,
        has_more: false,
        next_offset: None,
        total: Some(seen),
        truncated_by: None,
    })
}

/// Plain text, one record per line, for summaries and search excerpts.
pub fn render(r: &Record) -> String {
    let text = r.text.as_deref().unwrap_or("");
    match r.kind {
        RecordKind::Omitted => format!(
            "[omitted: {}{}]",
            r.name.as_deref().unwrap_or("?"),
            r.count
                .filter(|n| *n > 1)
                .map(|n| format!(" x{n}"))
                .unwrap_or_default()
        ),
        RecordKind::ToolCall => {
            format!("TOOL CALL: {}({text})", r.name.as_deref().unwrap_or("tool"))
        }
        RecordKind::ToolResult => format!("TOOL RESULT: {text}"),
        RecordKind::Attachment => match &r.name {
            Some(n) => format!("ATTACHMENT ({n}): {text}"),
            None => format!("ATTACHMENT: {text}"),
        },
        RecordKind::User => format!("USER: {text}"),
        RecordKind::Assistant => format!("ASSISTANT: {text}"),
        RecordKind::System => match &r.name {
            Some(n) => format!("SYSTEM ({n}): {text}"),
            None => format!("SYSTEM: {text}"),
        },
    }
}

//! Transcript projection: raw provider JSONL into bounded, allowlisted records.
//!
//! Nothing is dropped. A record of a known type with nothing worth reading (bookkeeping,
//! injected context, binary data) becomes an `omitted` record with a category, so a reader
//! can tell "nothing happened" from "something happened that this projection does not
//! show". Consecutive omitted records of one category collapse into one with a count: a
//! Claude session carries hundreds of `total_tokens_reminder` attachments, and one record
//! each would fill a page with noise.
//!
//! A record, content block or item of a type this projection has never seen becomes an
//! `unrecognized` record instead, carrying a bounded copy of its JSON. Providers add types
//! without notice, and text in a new shape was otherwise unsearchable until the projection
//! learned it (trovador#84). The bookkeeping lists below decide which is which.
//!
//! Files are streamed line by line. Codex sessions over 370 MB exist, so nothing here reads
//! a whole file into memory, and a page stops reading once it is full.

use std::io::BufRead;

/// Bumped whenever the projection changes what it emits, which invalidates every index row.
/// A change to the redaction rules in `redact.rs` must bump it too: the revision of a
/// session (`revision.rs`) holds this number and not the rules, so two builds with
/// different rules would otherwise share a revision for `text`.
pub const PROJECTION_VERSION: i64 = 12;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::archive::{ArchiveEntry, Provider, block_text, is_compressed, open_archive};
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
    /// What the model said about its own reasoning: Claude thinking text, Codex reasoning
    /// summaries. Only present when the provider kept readable text.
    Reasoning,
    /// A record, content block or item of a type this projection does not know. `name` is
    /// its type and `text` its JSON, with binary and encrypted values replaced, cut to
    /// `unrecognized_chars`. Searchable, but not interpreted.
    Unrecognized,
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
    /// `v` as an unrecognized record. With `unrecognized_chars` at zero it is an omitted
    /// record of the same category instead, which is how the projection behaved before
    /// unrecognized records existed.
    fn unrecognized(
        category: impl Into<String>,
        v: &Value,
        skip: &[&str],
        limits: &Limits,
    ) -> Self {
        let n = limits.unrecognized_chars;
        if n == 0 {
            return Self::omitted(category);
        }
        Self::named(
            RecordKind::Unrecognized,
            category,
            cap(&opaque_json(v, skip, n), n),
        )
    }
}

/// Keys whose values are binary or encrypted wherever they appear: base64 image and file
/// data (`data`, `bytes`), Codex's encrypted reasoning and compactions
/// (`encrypted_content`, 32,588 chars in one sampled compaction), and Claude's thinking
/// signatures. None of them is searchable text, and one would fill the whole budget.
const OPAQUE_KEYS: &[&str] = &["data", "bytes", "encrypted_content", "signature"];

/// Compact JSON of `v` with values under [`OPAQUE_KEYS`] and `data:` URLs replaced by
/// `"[binary]"`, and the top-level keys in `skip` left out. It stops writing once the output
/// is longer than `max` characters can be (4 bytes each), so a record of many megabytes is
/// not serialized whole to keep a few hundred characters of it; [`cap`] then cuts it to
/// `max` and marks the cut.
fn opaque_json(v: &Value, skip: &[&str], max: usize) -> String {
    let budget = max.saturating_mul(4);
    let mut out = String::new();
    fn string(s: &str, out: &mut String, max: usize) {
        let head: String = s.chars().take(max + 1).collect();
        out.push_str(&serde_json::to_string(&head).unwrap_or_default());
    }
    fn walk(v: &Value, skip: &[&str], out: &mut String, max: usize, budget: usize) {
        match v {
            Value::Object(m) => {
                out.push('{');
                let mut first = true;
                for (k, x) in m {
                    if skip.contains(&k.as_str()) {
                        continue;
                    }
                    if out.len() > budget {
                        return;
                    }
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    string(k, out, max);
                    out.push(':');
                    if OPAQUE_KEYS.contains(&k.as_str()) && !x.is_null() {
                        out.push_str("\"[binary]\"");
                    } else {
                        walk(x, &[], out, max, budget);
                    }
                }
                out.push('}');
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if out.len() > budget {
                        return;
                    }
                    if i > 0 {
                        out.push(',');
                    }
                    walk(x, &[], out, max, budget);
                }
                out.push(']');
            }
            Value::String(s) if s.starts_with("data:") => out.push_str("\"[binary]\""),
            Value::String(s) => string(s, out, max),
            other => out.push_str(&other.to_string()),
        }
    }
    walk(v, skip, &mut out, max, budget);
    out
}

/// Fields every Claude record carries that say where it sits, not what it is. Left out of
/// an unrecognized record's text so its budget goes to the part that is new.
const CLAUDE_ENVELOPE: &[&str] = &[
    "type",
    "uuid",
    "parentUuid",
    "logicalParentUuid",
    "sessionId",
    "session_id",
    "timestamp",
    "cwd",
    "version",
    "gitBranch",
    "userType",
    "isSidechain",
    "entrypoint",
    "slug",
    "requestId",
];

/// Claude record types that carry nothing to read beyond what other records show, from
/// every local Claude file on 2026-11-18 (5,623 files) and Counterpart's list. Titles,
/// cwd moves and PR links feed session metadata; the rest is client state, request
/// bookkeeping and cost accounting. `queue-operation` records a prompt typed during a turn
/// being queued; what the model received is the ordinary user record or the
/// `queued_command` attachment that follows, and a prompt removed from the queue before
/// delivery was never sent (1,355 of 15,331 locally). `last-prompt` is a UI copy, most often
/// a shortened prefix of a user record (trovador#89).
const CLAUDE_BOOKKEEPING: &[&str] = &[
    "agent-name",
    "agent-setting",
    "ai-title",
    "api-request",
    "api-request-blob",
    "api-request-shape",
    "artifact-autoreact-ledger",
    "artifact-comment-monitor",
    "atis-latch",
    "bridge-session",
    "continued-in",
    "cost-state",
    "custom-title",
    "failed",
    "file-history-delta",
    "file-history-snapshot",
    "fork-context-ref",
    "frame-link",
    "last-prompt",
    "launched",
    "mode",
    "permission-mode",
    "pr-link",
    "progress",
    "queue-operation",
    "relocated",
    "result",
    "session-owner",
    "started",
    "summary",
    "tools-used",
    "worktree-state",
];

/// Claude attachment subtypes known to be client bookkeeping or context injected for the
/// model (reminders, CLAUDE.md contents, skill and tool listings): all 50 subtypes in the
/// 5,623 local Claude files on 2026-11-18. `file` and `structured_output` are listed for
/// the records of theirs that have no content to project. A subtype not
/// listed is unrecognized rather than assumed to be more of the same: Claude Code adds
/// them often, and one could carry text a person wrote. `queued_command` is listed for its
/// task notifications; its prompts are projected above.
const CLAUDE_ATTACHMENTS: &[&str] = &[
    "agent_listing_delta",
    "auto_mode",
    "batching_reminder_sent",
    "budget_usd",
    "command_permissions",
    "compact_file_reference",
    "context_sections",
    "credential_org",
    "date",
    "date_change",
    "deferred_tools_delta",
    "deferred_tools_record",
    "directory",
    "dynamic_skill",
    "edited_text_file",
    "environment",
    "file",
    "hook_additional_context",
    "hook_blocking_error",
    "hook_non_blocking_error",
    "hook_success",
    "hook_system_message",
    "inlined_image_paths",
    "instructions",
    "invoked_skills",
    "max_turns_reached",
    "mcp_instructions_delta",
    "model",
    "nested_memory",
    "pdf_reference",
    "plan_mode",
    "prompt_snapshot",
    "queued_command",
    "read_truncation_notice",
    "remote_session_change",
    "sandbox_instructions",
    "session_context",
    "session_cron_carry",
    "silent_turn_reminder",
    "skill_listing",
    "structured_output",
    "task_reminder",
    "task_status",
    "thinking_drop",
    "total_tokens_reminder",
    "ultra_effort_enter",
    "ultra_effort_exit",
    "ultrathink_effort",
    "unknown_command_fallback",
    "workflow_keyword_request",
];

/// Claude content blocks known to hold nothing searchable: binary images and documents,
/// and thinking whose text the provider withheld.
const CLAUDE_OPAQUE_BLOCKS: &[&str] = &["image", "document", "redacted_thinking"];

/// Codex record and payload types that carry nothing to read beyond what other records
/// show, from 791 local rollouts on 2026-11-18 and Counterpart's list. Most are settings,
/// token accounting and task boundaries. The `event_msg` mirrors of tool calls, reasoning
/// and messages repeat what the response or item stream already shows, and are listed so
/// an older rollout does not show every call twice.
const CODEX_BOOKKEEPING: &[&str] = &[
    // Top-level records.
    "session_meta",
    "turn_context",
    "token_usage_record",
    "world_state",
    "inter_agent_communication_metadata",
    // response_item payloads.
    "additional_tools",
    "compaction",
    "configuration_update",
    "ghost_snapshot",
    // event_msg payloads.
    "token_count",
    "task_started",
    "task_complete",
    "turn_started",
    "turn_complete",
    "turn_aborted",
    "thread_settings_applied",
    "context_compacted",
    "agent_reasoning",
    "agent_reasoning_raw_content",
    "agent_reasoning_section_break",
    "exec_command_begin",
    "exec_command_end",
    "mcp_tool_call_begin",
    "mcp_tool_call_end",
    "patch_apply_begin",
    "patch_apply_end",
    "web_search_begin",
    "web_search_end",
    "item_started",
];

/// Fields of a Codex record envelope that say where it sits, not what it is.
const CODEX_ENVELOPE: &[&str] = &["type", "timestamp", "ordinal", "metadata"];

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
        // Issue #38: client-generated context is not a user turn. Keep compact
        // summaries readable even when Claude also marks them as meta.
        Some("user") if r.get("isCompactSummary").and_then(Value::as_bool) == Some(true) => {
            let text = block_text(r.pointer("/message/content").unwrap_or(&Value::Null));
            out.push(if text.trim().is_empty() {
                Draft::omitted("compact_summary")
            } else {
                Draft::named(RecordKind::System, "compact_summary", cap(&text, rc))
            });
        }
        Some("user") if r.get("isMeta").and_then(Value::as_bool) == Some(true) => {
            out.push(Draft::omitted("meta"));
        }
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
                        other => claude_block_fallback(other, b, limits, out),
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
                    // About three quarters of thinking blocks carry only a signature.
                    Some("thinking") => {
                        if let Some(t) = str_at(b, "thinking").filter(|t| !t.trim().is_empty()) {
                            out.push(Draft::new(RecordKind::Reasoning, cap(t, rc)));
                        }
                    }
                    other => claude_block_fallback(other, b, limits, out),
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
                // Issue #89: a prompt typed while a turn ran, delivered into that turn. 3,067
                // of 15,331 local queued prompts reached the model only this way, so showing
                // the attachment as omitted lost what the user said. No case of the same
                // delivery also written as a user record was found; the later user records
                // that contain such a prompt are compact summaries quoting it. Task
                // notifications use the same attachment and stay omitted.
                Some("queued_command") if str_at(a, "commandMode") == Some("prompt") => {
                    match a.get("prompt") {
                        Some(Value::String(s)) if !s.trim().is_empty() => {
                            out.push(Draft::named(RecordKind::User, "queued", cap(s, rc)))
                        }
                        Some(Value::Array(blocks)) => {
                            for b in blocks {
                                match str_at(b, "type") {
                                    Some("text") => {
                                        if let Some(t) =
                                            str_at(b, "text").filter(|t| !t.trim().is_empty())
                                        {
                                            out.push(Draft::named(
                                                RecordKind::User,
                                                "queued",
                                                cap(t, rc),
                                            ));
                                        }
                                    }
                                    other => claude_block_fallback(other, b, limits, out),
                                }
                            }
                        }
                        _ => out.push(Draft::omitted("attachment")),
                    }
                }
                Some(k) if CLAUDE_ATTACHMENTS.contains(&k) => {
                    out.push(Draft::omitted("attachment"))
                }
                // Only the attachment itself: its `rendered` copy is the system reminder the
                // model saw, which repeats it inside injected boilerplate.
                Some(k) => out.push(Draft::unrecognized(
                    format!("attachment:{k}"),
                    a,
                    &["type"],
                    limits,
                )),
                None => out.push(Draft::unrecognized("attachment", a, &[], limits)),
            }
        }
        // An away summary is a recap sent to the user; other system subtypes are noise.
        Some("system") => match (str_at(r, "subtype"), str_at(r, "content")) {
            (Some("away_summary"), Some(c)) => out.push(Draft::new(RecordKind::System, cap(c, rc))),
            _ => out.push(Draft::omitted("system")),
        },
        Some(other) if CLAUDE_BOOKKEEPING.contains(&other) => out.push(Draft::omitted(other)),
        Some(other) => out.push(Draft::unrecognized(other, r, CLAUDE_ENVELOPE, limits)),
        // No Claude file on the development machine has a record without a type.
        None => out.push(Draft::unrecognized("untyped", r, CLAUDE_ENVELOPE, limits)),
    }
}

/// A Claude content block that no arm above projects: a known opaque one is omitted, any
/// other is unrecognized. Before trovador#84 these produced nothing at all, so an assistant
/// turn made only of an unknown block looked like no turn.
fn claude_block_fallback(kind: Option<&str>, b: &Value, limits: &Limits, out: &mut Vec<Draft>) {
    match kind {
        Some(k) if CLAUDE_OPAQUE_BLOCKS.contains(&k) => out.push(Draft::omitted(k)),
        Some(k) => out.push(Draft::unrecognized(k, b, &["type"], limits)),
        None => out.push(Draft::unrecognized("block", b, &[], limits)),
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodexMode {
    #[default]
    Responses,
    Events,
    Items,
}

impl CodexMode {
    pub fn as_i64(self) -> i64 {
        self as i64
    }
    pub fn from_i64(v: i64) -> Self {
        match v {
            2 => CodexMode::Items,
            1 => CodexMode::Events,
            _ => CodexMode::Responses,
        }
    }
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
    // The oldest files have no envelope: the record is its own payload. Bare `reasoning`
    // records were dropped without a trace until unrecognized records surfaced 583 of them
    // in 6 local sessions (trovador#84).
    let p = match r.get("payload") {
        Some(p) => p,
        None if matches!(
            outer,
            Some("message" | "function_call" | "function_call_output" | "reasoning")
        ) =>
        {
            r
        }
        // The oldest rollouts also hold untyped header lines and `record_type: state`
        // lines; neither has a `type`, and neither carries anything a session shows.
        None => {
            if let Some(kind) = outer.filter(|k| !CODEX_BOOKKEEPING.contains(k)) {
                out.push(Draft::unrecognized(kind, r, CODEX_ENVELOPE, limits));
            }
            return;
        }
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
                match user_text(m) {
                    Some(text) => out.push(Draft::new(RecordKind::User, cap(&text, rc))),
                    None => out.push(Draft::omitted("injected_context")),
                }
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
            if mode != CodexMode::Responses
                && !(mode == CodexMode::Items && str_at(p, "role") == Some("user"))
            {
                return;
            }
            let content = p.get("content").unwrap_or(&Value::Null);
            if str_at(p, "role") == Some("user") {
                // Codex sends injected context (environment, AGENTS.md, plugin lists) as
                // user-role messages of their own. Without the pre-scan they reached
                // transcripts as user turns (over 1,200 on the development machine); they
                // are context, so they are counted but not shown.
                let typed = typed_text(content);
                if typed.trim().is_empty() {
                    if !block_text(content).trim().is_empty() {
                        out.push(Draft::omitted("injected_context"));
                    } else {
                        out.push(Draft::omitted("message"));
                    }
                } else {
                    out.push(Draft::new(RecordKind::User, cap(&typed, rc)));
                }
                return;
            }
            let text = block_text(content);
            if text.trim().is_empty() {
                return;
            }
            match str_at(p, "role") {
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
        // Reasoning, as summary headings. The full reasoning is encrypted; about four in
        // five records have no summary at all and project to nothing.
        (Some("response_item" | "reasoning"), Some("reasoning")) => {
            if mode != CodexMode::Items {
                let text = p
                    .get("summary")
                    .and_then(Value::as_array)
                    .map(|s| {
                        s.iter()
                            .filter_map(|x| x.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    out.push(Draft::new(RecordKind::Reasoning, cap(&text, rc)));
                    return;
                }
            }
            out.push(Draft::omitted("reasoning"));
        }
        // The item stream carries the same search as a WebSearch item, with results.
        (Some("response_item"), Some("web_search_call")) => {
            if mode != CodexMode::Items {
                out.push(Draft::named(
                    RecordKind::ToolCall,
                    "web_search",
                    cap(
                        &search_queries(p.get("action").unwrap_or(&Value::Null), p),
                        tc,
                    ),
                ));
            } else {
                out.push(Draft::omitted("web_search_call"));
            }
        }
        // Tool discovery has no item form, so it is projected in every stream mode.
        (Some("response_item"), Some("tool_search_call")) => out.push(Draft::named(
            RecordKind::ToolCall,
            "tool_search",
            cap(
                p.pointer("/arguments/query")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                tc,
            ),
        )),
        (Some("response_item"), Some("tool_search_output")) => {
            let found = p
                .get("tools")
                .and_then(Value::as_array)
                .map(|ts| {
                    ts.iter()
                        .filter_map(|t| t.get("name").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            out.push(Draft::named(
                RecordKind::ToolResult,
                "tool_search",
                cap(&found, tc),
            ));
        }
        // A compaction replaces earlier history with a copy of it; only a written summary
        // is new. None of 3,234 local records had one, but a non-empty one is kept.
        (Some("compacted"), None) => match str_at(p, "message").filter(|m| !m.trim().is_empty()) {
            Some(m) => out.push(Draft::named(RecordKind::System, "compacted", cap(m, rc))),
            None => out.push(Draft::omitted("compacted")),
        },
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
        // `developer` and `system` role messages are recognised above and omitted there; what
        // reaches here is a payload or record type the projection has not been taught.
        (_, Some(kind)) | (Some(kind), None) => {
            if CODEX_BOOKKEEPING.contains(&kind) {
                out.push(Draft::omitted(kind));
            } else {
                out.push(Draft::unrecognized(kind, p, &["type"], limits));
            }
        }
        (None, None) => {}
    }
}

fn project_codex_item(item: &Value, limits: &Limits, out: &mut Vec<Draft>) {
    let (rc, tc) = (limits.record_chars, limits.tool_chars);
    let content = || block_text(item.get("content").unwrap_or(&Value::Null));
    match str_at(item, "type") {
        // The same rule as the response stream, so both copies of a turn project to the
        // same text and the one at a stream switch is recognised as a copy.
        Some("UserMessage") => {
            let content = item.get("content").unwrap_or(&Value::Null);
            let typed = typed_text(content);
            if !typed.trim().is_empty() {
                out.push(Draft::new(RecordKind::User, cap(&typed, rc)));
            } else if !block_text(content).trim().is_empty() {
                out.push(Draft::omitted("injected_context"));
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
            // Codex 0.162.0 stopped writing stdout and stderr and keeps only
            // aggregated_output, so reading stdout alone left every later command's result
            // empty. Earlier versions wrote all three, with aggregated_output equal to
            // stdout in all 6,991 commands sampled (2026-11-03), so preferring it changes
            // nothing for older sessions.
            let result = match str_at(item, "aggregated_output") {
                Some(all) => all.to_string(),
                None => {
                    let stdout = str_at(item, "stdout").unwrap_or("");
                    let stderr = str_at(item, "stderr").unwrap_or("");
                    if stderr.is_empty() {
                        stdout.to_string()
                    } else {
                        format!("{stdout}\n[stderr] {stderr}")
                    }
                }
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
        Some("Reasoning") => {
            let text = item
                .get("summary_text")
                .and_then(Value::as_array)
                .map(|s| {
                    s.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if text.trim().is_empty() {
                out.push(Draft::omitted("Reasoning"));
            } else {
                out.push(Draft::new(RecordKind::Reasoning, cap(&text, rc)));
            }
        }
        Some("WebSearch") => {
            out.push(Draft::named(
                RecordKind::ToolCall,
                "web_search",
                cap(
                    &search_queries(item.get("action").unwrap_or(&Value::Null), item),
                    tc,
                ),
            ));
            let results = item
                .get("results")
                .and_then(Value::as_array)
                .map(|rs| {
                    rs.iter()
                        .map(|r| {
                            format!(
                                "{} {}",
                                str_at(r, "title").unwrap_or(""),
                                str_at(r, "url").unwrap_or("")
                            )
                            .trim()
                            .to_string()
                        })
                        .filter(|l| !l.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if !results.is_empty() {
                out.push(Draft::named(
                    RecordKind::ToolResult,
                    "web_search",
                    cap(&results, tc),
                ));
            }
        }
        Some("FunctionCallOutput") => {
            let name = match (str_at(item, "namespace"), str_at(item, "name")) {
                (Some(ns), Some(n)) => format!("{ns}/{n}"),
                (None, Some(n)) => n.to_string(),
                _ => "function".to_string(),
            };
            let output = match item.get("output") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => String::new(),
            };
            out.push(Draft::named(RecordKind::ToolResult, name, cap(&output, tc)));
        }
        Some("ImageView") => out.push(Draft::named(
            RecordKind::ToolCall,
            "view_image",
            cap(str_at(item, "path").unwrap_or(""), tc),
        )),
        // A call to a tool the Codex app registers at run time (`codex_app/read_thread`,
        // `codex_app/open_in_codex`), with its result in `content_items`. All 9 local
        // instances were unrecognized records when that kind arrived (trovador#84).
        Some("DynamicToolCall") => {
            let name = match (str_at(item, "namespace"), str_at(item, "tool")) {
                (Some(ns), Some(t)) => format!("{ns}/{t}"),
                (None, Some(t)) => t.to_string(),
                _ => "dynamic_tool".to_string(),
            };
            let args = item
                .get("arguments")
                .map(Value::to_string)
                .unwrap_or_default();
            out.push(Draft::named(
                RecordKind::ToolCall,
                name.clone(),
                cap(&args, tc),
            ));
            let result = block_text(item.get("content_items").unwrap_or(&Value::Null));
            if !result.is_empty() {
                out.push(Draft::named(RecordKind::ToolResult, name, cap(&result, tc)));
            }
        }
        // Every one seen was the parent waiting on its subagents with none named; one that
        // names agents says who it was waiting for.
        Some("CollabAgentToolCall") => {
            let agents: Vec<String> = item
                .get("receiver_agents")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| match x {
                            Value::String(s) => s.clone(),
                            other => str_at(other, "agent_path")
                                .or_else(|| str_at(other, "name"))
                                .map(str::to_string)
                                .unwrap_or_else(|| other.to_string()),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if agents.is_empty() {
                out.push(Draft::omitted("CollabAgentToolCall"));
            } else {
                out.push(Draft::named(
                    RecordKind::System,
                    str_at(item, "tool").unwrap_or("collab"),
                    cap(&agents.join(", "), tc),
                ));
            }
        }
        Some(other) => out.push(Draft::unrecognized(other, item, &["type", "id"], limits)),
        None => out.push(Draft::unrecognized("item", item, &[], limits)),
    }
}

/// A Codex `compacted` record, read without building it into a JSON tree.
///
/// Each one carries a copy of the history it replaced: 562 such lines made up 3.3 GB of the
/// 10 GB local Codex archive, up to 15.7 MB each. Only `payload.message` is projected, and
/// it was empty in all of them, but parsing them whole set the peak memory of a full index
/// build (637 MB for Codex alone, against 77 MB for Claude). The type sits in the first
/// few hundred bytes, so the rest is skipped without allocating it.
fn compacted_without_parsing(line: &[u8], limits: &Limits) -> Option<Draft> {
    let head = &line[..line.len().min(512)];
    if !head.windows(18).any(|w| w == br#""type":"compacted""#) {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Compacted {
        #[serde(rename = "type")]
        kind: String,
        #[serde(default)]
        payload: Option<CompactedPayload>,
    }
    #[derive(serde::Deserialize)]
    struct CompactedPayload {
        #[serde(default)]
        message: Option<String>,
    }
    let c: Compacted = serde_json::from_slice(line).ok()?;
    if c.kind != "compacted" {
        return None;
    }
    Some(
        match c
            .payload
            .and_then(|p| p.message)
            .filter(|m| !m.trim().is_empty())
        {
            Some(m) => Draft::named(
                RecordKind::System,
                "compacted",
                cap(&m, limits.record_chars),
            ),
            None => Draft::omitted("compacted"),
        },
    )
}

/// Whether a block of a user-role message is context a client injected rather than text a
/// person typed: a leading tag such as `<environment_context>`, or the project's AGENTS.md.
fn is_injected(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<') || t.starts_with("# AGENTS.md instructions")
}

fn user_text(text: &str) -> Option<String> {
    // Issue #37: question replies carry user decisions inside a client wrapper. Only
    // decode this exact envelope so other injected context remains omitted.
    if let Some(payload) = text
        .trim()
        .strip_prefix("<send_user_message_question_reply>")
    {
        let payload = payload.strip_suffix("</send_user_message_question_reply>")?;
        let replies: Vec<Value> = serde_json::from_str(payload).ok()?;
        let answers: Vec<String> = replies
            .iter()
            .filter_map(|reply| {
                let answer = reply.get("answer")?.as_str()?;
                Some(match reply.get("question").and_then(Value::as_str) {
                    Some(question) => format!("{question} → {answer}"),
                    None => answer.to_owned(),
                })
            })
            .collect();
        return (!answers.is_empty()).then(|| answers.join("\n"));
    }
    (!is_injected(text)).then(|| text.to_owned())
}

/// The blocks of a message a person typed, joined.
fn typed_text(content: &Value) -> String {
    match content {
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .filter(|t| !t.trim().is_empty())
            .filter_map(user_text)
            .collect::<Vec<_>>()
            .concat(),
        Value::String(s) => user_text(s).unwrap_or_default(),
        _ => String::new(),
    }
}

/// A web search's queries: the list when there is one, else the single query.
fn search_queries(action: &Value, outer: &Value) -> String {
    action
        .get("queries")
        .and_then(Value::as_array)
        .map(|q| {
            q.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.is_empty())
        .or_else(|| str_at(action, "query").map(str::to_string))
        .or_else(|| str_at(outer, "query").map(str::to_string))
        .unwrap_or_default()
}

// --- reading ---------------------------------------------------------------

/// Where a projection stopped, so the index can continue an append-only file from there
/// and assign exactly the record indexes a full read would.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectionState {
    /// Decompressed bytes consumed, always at a line boundary.
    pub offset: u64,
    /// Issue #35: a known byte boundary survives index resumes without rescanning history.
    pub codex_inherited_end: Option<u64>,
    pub next_index: usize,
    /// Issue #36: retain typed titles across resumes without rereading the old transcript.
    pub custom_title: Option<String>,
    pub mode: CodexMode,
    /// An omitted run still open at `offset`. A full read would merge records appended
    /// after it into the same run, so it is carried rather than flushed.
    pub run: Option<(String, usize)>,
    /// The last turn eligible for a different-stream copy, with a hash of concatenated source
    /// text for users and projected text for assistants. Persisted across index resumes.
    pub last_turn: Option<(RecordKind, u64)>,
    /// Clients may add injected blocks to just one copy, so retain its filtered hash too.
    pub last_user_filtered_hash: Option<u64>,
    /// Each stream can contribute one copy, including across an index resume.
    pub last_user_streams: u8,
}

/// FNV-1a, 64-bit. Stable across Rust versions and platforms, unlike the std hasher,
/// because the value is stored in the index and compared on a later run.
pub fn text_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn codex_user_hash(r: &Value, filtered: bool) -> Option<u64> {
    let p = r.get("payload").unwrap_or(r);
    let content = match str_at(p, "type") {
        Some("message") if str_at(p, "role") == Some("user") => p.get("content")?,
        Some("item_completed")
            if p.pointer("/item/type").and_then(Value::as_str) == Some("UserMessage") =>
        {
            p.pointer("/item/content")?
        }
        Some("user_message") => {
            let text = str_at(p, "message")?;
            return Some(text_hash(&if filtered {
                user_text(text).unwrap_or_default()
            } else {
                text.to_owned()
            }));
        }
        _ => return None,
    };
    let text = match content {
        Value::String(s) => {
            if filtered {
                user_text(s).unwrap_or_default()
            } else {
                s.clone()
            }
        }
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| str_at(b, "text"))
            .filter_map(|text| {
                if filtered {
                    user_text(text)
                } else {
                    Some(text.to_owned())
                }
            })
            .collect::<String>(),
        _ => return None,
    };
    Some(text_hash(&text))
}

fn is_turn(k: RecordKind) -> bool {
    matches!(k, RecordKind::User | RecordKind::Assistant)
}

/// Streams a session's projected records, with omitted runs collapsed and indexes assigned.
pub struct Projection {
    reader: Box<dyn BufRead + Send>,
    provider: Provider,
    inherited_end: u64,
    scan_limit: Option<u64>,
    limits: Limits,
    state: ProjectionState,
    pending: std::collections::VecDeque<Draft>,
    /// Indexing reads only whole lines and leaves an open run open at the end; a transcript
    /// page reads whatever is there and closes the run.
    for_index: bool,
    done: bool,
    read_error: Option<std::io::Error>,
    line: Vec<u8>,
}

impl Projection {
    /// A full read from the start to the end of the file as it is now. It has no revision
    /// to stay consistent with; pages and text use [`Projection::open_snapshot`]. It stays
    /// for callers that compare a projection with other reads of the whole file, such as
    /// the live-archive tests.
    pub fn open(e: &ArchiveEntry, limits: &Limits) -> std::io::Result<Self> {
        Self::start(e, limits, None, false, None)
    }

    /// A full read of a plain file as it was when `e` was scanned: both the fork-boundary
    /// scan and the projection stop at `e.size_bytes`, so bytes appended after the scan
    /// reach neither a page nor the boundary that decides which earlier records are shown
    /// as `forked_history`. Either would otherwise put a newer state under the revision of
    /// the scan (trovador#83).
    ///
    /// A compressed rollout is read whole: its size is the compressed size and cannot
    /// bound the decoded stream. The revision of such a file names its size and mtime, and
    /// a replacement with the same size and mtime would go unnoticed, as the README says.
    pub fn open_snapshot(e: &ArchiveEntry, limits: &Limits) -> std::io::Result<Self> {
        let bound = (!is_compressed(&e.file)).then_some(e.size_bytes);
        Self::start(e, limits, None, false, bound)
    }

    /// A read for the index: from `resume` when given, whole lines only, with the final
    /// state available from [`Projection::state`] once the iterator is exhausted.
    pub fn for_index(
        e: &ArchiveEntry,
        limits: &Limits,
        resume: Option<ProjectionState>,
    ) -> std::io::Result<Self> {
        Self::start(e, limits, resume, true, None)
    }

    fn start(
        e: &ArchiveEntry,
        limits: &Limits,
        resume: Option<ProjectionState>,
        for_index: bool,
        bound: Option<u64>,
    ) -> std::io::Result<Self> {
        let bounded = |reader: Box<dyn BufRead + Send>| -> Box<dyn BufRead + Send> {
            match bound {
                Some(n) => Box::new(std::io::Read::take(reader, n)),
                None => reader,
            }
        };
        // Every file starts in the oldest stream and moves forward only as newer
        // stream records appear, in order. A pre-scan that started in the newest stream
        // it could find dropped the older-format turns of a session continued by a newer
        // client (#31); the copy written at each switch is dropped by comparing it with
        // the last emitted turn instead.
        let mut state = resume.unwrap_or_default();
        let mut scan_limit = None;
        let inherited_end = if e.provider == Provider::Codex {
            if let Some(end) = state.codex_inherited_end {
                end
            } else {
                let mut reader = bounded(open_archive(&e.file, 0)?);
                let boundary = crate::codex::fork_boundary(&mut reader, e.finished(), true)?;
                // Issue #35: terminal events before the start ordinal belong to
                // copied history. Display may fall back, but indexing must wait
                // for the real boundary and never persist the parent's completion.
                state.codex_inherited_end = boundary.end.filter(|_| !boundary.display_fallback);
                let end = boundary
                    .end
                    .filter(|_| !for_index || !boundary.display_fallback);
                if end.is_none() {
                    scan_limit = Some(boundary.scanned);
                }
                end.unwrap_or(boundary.scanned)
            }
        } else {
            0
        };
        Ok(Self {
            reader: bounded(open_archive(&e.file, state.offset)?),
            provider: e.provider,
            inherited_end,
            scan_limit,
            limits: limits.clone(),
            state,
            pending: Default::default(),
            for_index,
            done: false,
            read_error: None,
            line: Vec::new(),
        })
    }

    /// Where the read stopped. Meaningful once the iterator has returned `None`.
    pub fn state(&self) -> &ProjectionState {
        &self.state
    }

    pub(crate) fn take_read_error(&mut self) -> Option<std::io::Error> {
        self.read_error.take()
    }

    fn fill(&mut self) {
        while self.pending.is_empty() && !self.done {
            // Issue #35: bytes appended after an incomplete boundary scan belong to
            // the next pass, which can determine whether they are child-owned.
            if self
                .scan_limit
                .is_some_and(|limit| self.state.offset >= limit)
            {
                self.done = true;
                break;
            }
            self.line.clear();
            let n = match self.reader.read_until(b'\n', &mut self.line) {
                Ok(0) => {
                    self.done = true;
                    break;
                }
                Err(error) => {
                    // Decoding may fail after a valid Codex header. A partial projection
                    // must not look like a successful transcript or index checkpoint.
                    if self.provider == Provider::Codex {
                        self.read_error = Some(error);
                    }
                    self.done = true;
                    break;
                }
                Ok(n) => n,
            };
            // A torn final line is normal in a session still being written. The index
            // leaves it for the next pass; a transcript page tries it and skips it.
            if !self.line.ends_with(b"\n") && self.for_index {
                self.done = true;
                break;
            }
            let line_start = self.state.offset;
            self.state.offset += n as u64;
            if line_start > 0 && line_start < self.inherited_end {
                self.pending.push_back(Draft::omitted("forked_history"));
                continue;
            }
            if self.line.iter().all(|b| b.is_ascii_whitespace()) {
                continue;
            }
            if self.provider == Provider::Codex
                && let Some(d) = compacted_without_parsing(&self.line, &self.limits)
            {
                self.pending.push_back(d);
                continue;
            }
            // Parse the bytes directly; copy through a lossy conversion only for the rare
            // line that is not valid UTF-8.
            let parsed = serde_json::from_slice::<Value>(&self.line)
                .or_else(|_| serde_json::from_str::<Value>(&String::from_utf8_lossy(&self.line)));
            let Ok(r) = parsed else {
                continue;
            };
            if self.provider == Provider::Claude
                && let Some(title) = crate::archive::custom_title(&r)
            {
                self.state.custom_title = Some(title.to_owned());
            }
            let mut drafts = Vec::new();
            match self.provider {
                Provider::Claude => project_claude(&r, &self.limits, &mut drafts),
                Provider::Codex => {
                    // A stream seen later than the pre-scan reached still takes over from
                    // that point on.
                    // A new task cannot be a copy of the preceding task's identical user.
                    if matches!(
                        r.pointer("/payload/type").and_then(Value::as_str),
                        Some("task_started" | "task_complete")
                    ) && self
                        .state
                        .last_turn
                        .is_some_and(|(kind, _)| kind == RecordKind::User)
                    {
                        self.state.last_turn = None;
                        self.state.last_user_filtered_hash = None;
                        self.state.last_user_streams = 0;
                    }
                    let before = self.state.mode;
                    if let Some(m) = codex_mode_signal(&r) {
                        self.state.mode = self.state.mode.max(m);
                    }
                    project_codex(&r, self.state.mode, &self.limits, &mut drafts);
                    // Issues #19 and #34: copies may span all three streams. Remember
                    // which streams matched instead of consuming eligibility at the first
                    // switch. A repeated stream starts a new turn, so consecutive identical
                    // user messages survive even without a task boundary.
                    // Exact concatenation preserves the real flattened item copy. The
                    // filtered hash also allows injected blocks added to only one copy;
                    // neither hash ignores whitespace or compares a truncated prefix.
                    let source_hash = codex_user_hash(&r, false);
                    let filtered_hash = codex_user_hash(&r, true);
                    let stream = if r.pointer("/payload/item/type").and_then(Value::as_str)
                        == Some("UserMessage")
                    {
                        4
                    } else if r.pointer("/payload/type").and_then(Value::as_str)
                        == Some("user_message")
                    {
                        2
                    } else if source_hash.is_some() {
                        1
                    } else {
                        0
                    };
                    let user_copy = stream != 0
                        && self.state.last_user_streams & stream == 0
                        && self.state.last_turn.is_some_and(|(kind, hash)| {
                            kind == RecordKind::User
                                && (source_hash == Some(hash)
                                    || (filtered_hash.is_some()
                                        && filtered_hash == self.state.last_user_filtered_hash))
                        });
                    if user_copy {
                        drafts.retain(|d| d.kind != RecordKind::User);
                        self.state.last_user_streams |= stream;
                    } else if self.state.mode > before
                        && let Some(last) = self.state.last_turn
                        && let Some(i) = drafts.iter().position(|d| {
                            d.kind == RecordKind::Assistant
                                && (d.kind, text_hash(d.text.as_deref().unwrap_or(""))) == last
                        })
                    {
                        drafts.remove(i);
                    } else if let Some(d) = drafts.iter().rev().find(|d| is_turn(d.kind)) {
                        self.state.last_user_filtered_hash = filtered_hash;
                        self.state.last_user_streams = stream;
                        self.state.last_turn = Some((
                            d.kind,
                            source_hash
                                .unwrap_or_else(|| text_hash(d.text.as_deref().unwrap_or(""))),
                        ));
                    }
                }
            }
            self.pending.extend(drafts);
        }
    }

    fn assign(&mut self, d: Draft, count: Option<usize>) -> Record {
        let index = self.state.next_index;
        self.state.next_index += 1;
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
                if self.for_index {
                    return None;
                }
                // End of file: close the run in progress.
                let (cat, n) = self.state.run.take()?;
                return Some(self.assign(Draft::omitted(cat), Some(n)));
            };
            if d.kind == RecordKind::Omitted {
                let cat = d.name.clone().unwrap_or_default();
                match &mut self.state.run {
                    Some((c, n)) if *c == cat => *n += 1,
                    Some(_) => {
                        let (prev, n) = self.state.run.replace((cat, 1)).expect("run is some");
                        return Some(self.assign(Draft::omitted(prev), Some(n)));
                    }
                    None => self.state.run = Some((cat, 1)),
                }
                continue;
            }
            if let Some((cat, n)) = self.state.run.take() {
                self.pending.push_front(d);
                return Some(self.assign(Draft::omitted(cat), Some(n)));
            }
            return Some(self.assign(d, None));
        }
    }
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
    let mut projection = Projection::open_snapshot(e, limits)?;
    for r in projection.by_ref() {
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
    if let Some(error) = projection.take_read_error() {
        return Err(error);
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
        RecordKind::Reasoning => format!("REASONING: {text}"),
        RecordKind::Unrecognized => {
            format!(
                "UNRECOGNIZED ({}): {text}",
                r.name.as_deref().unwrap_or("?")
            )
        }
    }
}

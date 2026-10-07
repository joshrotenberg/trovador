//! The MCP door. Each tool and resource adapts its input, calls the same library function
//! the CLI calls, and returns the result unchanged.

use std::collections::HashMap;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tower_mcp::protocol::{ReadResourceResult, ResourceContent};
use tower_mcp::{CallToolResult, McpRouter, ResourceTemplateBuilder, ToolBuilder};

use crate::archive::Backend;
use crate::index::SearchOpts;
use crate::run::{self, AskOpts};
use crate::sessions::{self, Ctx, ListOpts};

const INSTRUCTIONS: &str = "\
Local Claude Code and Codex sessions as one surface. Every session either CLI has run is \
addressable by id or unambiguous id prefix.

sessions(action=list) finds sessions, newest first, filterable by backend and project path. \
sessions(action=show) gives one session's metadata, resumability, and the command to resume \
it in a terminal. sessions(action=transcript) pages through a projected transcript; follow \
next_offset while has_more is true. The session:// resources return the same data.

search finds text across every session: words and \"quoted phrases\" by default, fts=true \
for FTS5 syntax, regex=true for a regular expression. Each hit gives match indexes to pass \
as the transcript offset. If exhaustive is false, finding nothing proves nothing; the note \
says what was not covered.

summarize returns a summary of a session, from the cache when the session has not \
changed; a miss costs tokens. session://{backend}/{id}/summary returns only a cached one.

ask runs a turn and costs real model tokens. Without session it starts a new session; with \
session it continues that one, if consent allows. Sessions trovador starts may always be \
continued. Keep the session_id it returns: it is the handle to that conversation, and a run \
that timed out still returns it. Consent is changed only by a person at the CLI, never \
through this server.

Reading is free: nothing here runs a model.";

#[derive(Debug, Deserialize, JsonSchema)]
struct SummarizeInput {
    /// Session id or unambiguous prefix.
    session_id: String,
    backend: Option<Backend>,
    /// Make a new summary even if a current one is cached.
    #[serde(default)]
    force: bool,
    /// Model for the summary; haiku by default.
    model: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SessionsAction {
    List,
    Show,
    Transcript,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SessionsInput {
    /// list: find sessions. show: one session's metadata. transcript: one page of records.
    action: SessionsAction,
    /// For show and transcript: a session id or an unambiguous prefix.
    session_id: Option<String>,
    /// Restrict to one backend.
    backend: Option<Backend>,
    /// For list: only sessions whose project path contains this text.
    project: Option<String>,
    /// For list: only sessions with this origin (cli, claude-desktop, sdk-cli, exec, vscode).
    origin: Option<String>,
    /// For list: include sessions started by another session (hidden by default).
    #[serde(default)]
    include_subagents: bool,
    /// For list: sessions to return. For transcript: records per page.
    limit: Option<usize>,
    /// For transcript: the record index to start from.
    offset: Option<usize>,
}

fn json_result(value: &impl Serialize) -> tower_mcp::Result<CallToolResult> {
    CallToolResult::from_serialize(value)
}

fn sessions_call(ctx: &Ctx, input: SessionsInput) -> tower_mcp::Result<CallToolResult> {
    let need_id = || {
        input
            .session_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "session_id is required for this action".to_string())
    };
    let outcome = match input.action {
        SessionsAction::List => {
            let opts = ListOpts {
                backend: input.backend,
                project: input.project.clone(),
                limit: input.limit,
                origin: input.origin.clone(),
                include_subagents: input.include_subagents,
            };
            return json_result(&sessions::list(ctx, &opts));
        }
        SessionsAction::Show => need_id().and_then(|id| {
            sessions::show(ctx, id, input.backend)
                .map(|d| json_result(&d))
                .map_err(|e| e.to_string())
        }),
        SessionsAction::Transcript => need_id().and_then(|id| {
            sessions::transcript(
                ctx,
                id,
                input.backend,
                input.offset.unwrap_or(0),
                input.limit,
            )
            .map(|t| json_result(&t))
            .map_err(|e| e.to_string())
        }),
    };
    // A lookup miss is the caller's to fix, so it comes back as a tool error they can read
    // rather than a protocol error.
    outcome.unwrap_or_else(|msg| Ok(CallToolResult::error(msg)))
}

fn resource_json(uri: String, value: &impl Serialize) -> tower_mcp::Result<ReadResourceResult> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| tower_mcp::Error::internal(e.to_string()))?;
    Ok(ReadResourceResult {
        contents: vec![ResourceContent {
            uri,
            mime_type: Some("application/json".to_string()),
            text: Some(text),
            blob: None,
            meta: None,
        }],
        meta: None,
        ..Default::default()
    })
}

fn template_args(vars: &HashMap<String, String>) -> tower_mcp::Result<(Option<Backend>, String)> {
    let backend = vars
        .get("backend")
        .map(|b| b.parse::<Backend>())
        .transpose()
        .map_err(tower_mcp::Error::invalid_params)?;
    let id = vars.get("id").cloned().unwrap_or_default();
    Ok((backend, id))
}

/// Runs blocking archive I/O off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> tower_mcp::Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| tower_mcp::Error::internal(e.to_string()))
}

pub fn router(ctx: Ctx) -> McpRouter {
    let ctx = Arc::new(ctx);

    let c = ctx.clone();
    let sessions_tool = ToolBuilder::new("sessions")
        .title("Sessions")
        .description(
            "List, inspect, and read local Claude Code and Codex sessions. action=list finds \
             sessions newest first; action=show gives one session's metadata and resume \
             command; action=transcript pages through its records (follow next_offset). \
             Reads only; costs no model tokens.",
        )
        .read_only_safe()
        .handler(move |input: SessionsInput| {
            let ctx = c.clone();
            async move { blocking(move || sessions_call(&ctx, input)).await? }
        })
        .build();

    let c = ctx.clone();
    let search_tool = ToolBuilder::new("search")
        .title("Search sessions")
        .description(
            "Full-text search across every local Claude Code and Codex session. query is \
             words and \"quoted phrases\", all required; fts=true takes FTS5 syntax; \
             regex=true takes a regular expression. Returns sessions with match counts and \
             excerpts; each match has the record index to open the transcript at. Brings \
             the index up to date first, within a time budget; exhaustive=false means the \
             index was not fully current and an empty result is not proof of absence. \
             Costs no model tokens.",
        )
        .read_only_safe()
        .handler(move |input: SearchOpts| {
            let ctx = c.clone();
            async move {
                blocking(move || match sessions::search(&ctx, &input) {
                    Ok(r) => json_result(&r),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                })
                .await?
            }
        })
        .build();

    let c = ctx.clone();
    let ask_tool = ToolBuilder::new("ask")
        .title("Ask a session")
        .description(
            "Run one turn on claude or codex. Costs real model tokens. Without session, \
             starts a new session (claude unless backend says otherwise). With session (an \
             id or unambiguous prefix), continues it, if consent allows; sessions trovador \
             started are always allowed, others need a person to approve them at the CLI. \
             fork=true resumes a claude session's context into a new one. Returns the reply, \
             the session_id to continue with, and whether it can be continued. A run that \
             times out still returns its session_id. about=<session> consults instead: it \
             summarizes that session and asks a new session on the other backend, which \
             works for sessions too large to resume.",
        )
        .handler(move |input: AskOpts| {
            let ctx = c.clone();
            async move {
                match run::ask(&ctx, &input).await {
                    Ok(r) => json_result(&r),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build();

    let c = ctx.clone();
    let summarize_tool = ToolBuilder::new("summarize")
        .title("Summarize a session")
        .description(
            "Summarize one session: goal, what happened, key decisions, current state, open \
             items. Served from the cache when the session has not changed since it was \
             summarized (cached=true, no cost); otherwise costs model tokens (haiku by \
             default). force=true makes a new one.",
        )
        .handler(move |input: SummarizeInput| {
            let ctx = c.clone();
            async move {
                match crate::summary::summarize(
                    &ctx,
                    &input.session_id,
                    input.backend,
                    input.force,
                    input.model.as_deref(),
                )
                .await
                {
                    Ok(s) => json_result(&s),
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                }
            }
        })
        .build();

    let c = ctx.clone();
    let summary_resource = ResourceTemplateBuilder::new("session://{backend}/{id}/summary")
        .name("Session summary")
        .description("A session's cached summary, if one is current. Never generates one.")
        .mime_type("application/json")
        .handler(move |uri: String, vars: HashMap<String, String>| {
            let ctx = c.clone();
            async move {
                let (backend, id) = template_args(&vars)?;
                let found = blocking(move || {
                    crate::archive::find_session(&ctx.paths, &id, backend)
                        .map(|e| crate::summary::cached(&ctx, &e))
                })
                .await?;
                match found {
                    Ok(Some(s)) => resource_json(uri, &s),
                    Ok(None) => Err(tower_mcp::Error::invalid_params(
                        "no current summary; the summarize tool makes one",
                    )),
                    Err(e) => Err(tower_mcp::Error::invalid_params(e.to_string())),
                }
            }
        });

    let c = ctx.clone();
    let session_resource = ResourceTemplateBuilder::new("session://{backend}/{id}")
        .name("Session")
        .description("One session's metadata, resumability, and resume command")
        .mime_type("application/json")
        .handler(move |uri: String, vars: HashMap<String, String>| {
            let ctx = c.clone();
            async move {
                let (backend, id) = template_args(&vars)?;
                let detail = blocking(move || sessions::show(&ctx, &id, backend)).await?;
                match detail {
                    Ok(d) => resource_json(uri, &d),
                    Err(e) => Err(tower_mcp::Error::invalid_params(e.to_string())),
                }
            }
        });

    let c = ctx.clone();
    let transcript_resource = ResourceTemplateBuilder::new("session://{backend}/{id}/transcript")
        .name("Session transcript")
        .description("The first page of a session's projected transcript")
        .mime_type("application/json")
        .handler(move |uri: String, vars: HashMap<String, String>| {
            let ctx = c.clone();
            async move {
                let (backend, id) = template_args(&vars)?;
                let page =
                    blocking(move || sessions::transcript(&ctx, &id, backend, 0, None)).await?;
                match page {
                    Ok(p) => resource_json(uri, &p),
                    Err(e) => Err(tower_mcp::Error::invalid_params(e.to_string())),
                }
            }
        });

    McpRouter::new()
        .server_info("trovador", env!("CARGO_PKG_VERSION"))
        .instructions(INSTRUCTIONS)
        .tool(sessions_tool)
        .tool(search_tool)
        .tool(ask_tool)
        .tool(summarize_tool)
        .resource_template(session_resource)
        .resource_template(transcript_resource)
        .resource_template(summary_resource)
}

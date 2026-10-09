use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tower_mcp::{CallToolResult, McpRouter, Resource, ResourceBuilder, ToolBuilder};

use crate::manager::{CancelTask, ContinueTask, Manager, SpawnTask, WaitTasks};
use crate::{backend::CliRunner, diagnostics::check_environment, types::Backend};

/// The canonical delegation skill is available with or without plugin packaging.
pub const DELEGATION_SKILL_URI: &str = "skill://tosi/delegate/SKILL.md";

const DELEGATION_SKILL: &str = include_str!("../resources/skills/delegate/SKILL.md");

const SERVER_INSTRUCTIONS: &str = include_str!("../resources/mcp/instructions.md");

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CheckEnvironment {
    /// Check one provider; omitted checks both. This makes no model request.
    backend: Option<Backend>,
}

pub fn router(manager: Manager, runner: Arc<CliRunner>) -> McpRouter {
    let check = ToolBuilder::new("check_environment")
        .description(include_str!("../resources/mcp/tools/check_environment.md").trim())
        .read_only()
        .handler(move |input: CheckEnvironment| {
            let runner = runner.clone();
            async move {
                Ok(reply(Ok::<_, String>(
                    check_environment(&runner, input.backend).await,
                )))
            }
        })
        .build();
    let spawn_manager = manager.clone();
    let spawn = ToolBuilder::new("spawn_task")
        .description(include_str!("../resources/mcp/tools/spawn_task.md").trim())
        .handler(move |input: SpawnTask| {
            let manager = spawn_manager.clone();
            async move { Ok(reply(manager.spawn_task(input).await)) }
        })
        .build();

    let wait_manager = manager.clone();
    let wait = ToolBuilder::new("wait_tasks")
        .description(include_str!("../resources/mcp/tools/wait_tasks.md").trim())
        .read_only()
        .handler(move |input: WaitTasks| {
            let manager = wait_manager.clone();
            async move { Ok(reply(manager.wait_tasks(input).await)) }
        })
        .build();

    let continue_manager = manager.clone();
    let continue_task = ToolBuilder::new("continue_task")
        .description(include_str!("../resources/mcp/tools/continue_task.md").trim())
        .handler(move |input: ContinueTask| {
            let manager = continue_manager.clone();
            async move { Ok(reply(manager.continue_task(input).await)) }
        })
        .build();

    let cancel = ToolBuilder::new("cancel_task")
        .description(include_str!("../resources/mcp/tools/cancel_task.md").trim())
        .handler(move |input: CancelTask| {
            let manager = manager.clone();
            async move { Ok(reply(manager.cancel_task(input).await)) }
        })
        .build();

    McpRouter::new()
        .server_info("tosi", env!("CARGO_PKG_VERSION"))
        .instructions(SERVER_INSTRUCTIONS.trim())
        .resources(skill_resources())
        .tool(spawn)
        .tool(wait)
        .tool(continue_task)
        .tool(cancel)
        .tool(check)
}

fn skill_resources() -> [Resource; 5] {
    [
        ResourceBuilder::new(DELEGATION_SKILL_URI)
            .name("delegate")
            .description("General Tosi guidance and links to detailed patterns. Read first. The same canonical SKILL.md is included in the plugin package.")
            .mime_type("text/markdown")
            .text(DELEGATION_SKILL),
        ResourceBuilder::new("skill://tosi/delegate/references/parallel-tasks.md")
            .name("parallel-tasks")
            .description("Read when delegating independent parallel assignments: bounded briefs, edit ownership, capacity, and collecting outcomes. Also included as a reference in the plugin package.")
            .mime_type("text/markdown")
            .text(include_str!("../resources/skills/delegate/references/parallel-tasks.md")),
        ResourceBuilder::new("skill://tosi/delegate/references/implementation-review.md")
            .name("implementation-review")
            .description("Read when assigning implementation and independent review: provider access limits, repository instructions, diffs, and validation. Also included as a reference in the plugin package.")
            .mime_type("text/markdown")
            .text(include_str!("../resources/skills/delegate/references/implementation-review.md")),
        ResourceBuilder::new("skill://tosi/delegate/references/follow-up.md")
            .name("follow-up")
            .description("Read when continuing or stopping work: latest-turn continuity, wait timeouts, settled cancellation, truncated results, and server lifetime. Also included as a reference in the plugin package.")
            .mime_type("text/markdown")
            .text(include_str!("../resources/skills/delegate/references/follow-up.md")),
        ResourceBuilder::new("skill://tosi/delegate/references/model-effort.md")
            .name("model-effort")
            .description("Read when selecting backend, model, or effort: user preferences, provider support, omitted settings, continuation, and requested versus reported selection. Also included as a reference in the plugin package.")
            .mime_type("text/markdown")
            .text(include_str!("../resources/skills/delegate/references/model-effort.md")),
    ]
}

fn reply<T: Serialize>(result: Result<T, String>) -> CallToolResult {
    match result {
        Ok(value) => match serde_json::to_value(value) {
            Ok(value) => CallToolResult::json(value),
            Err(error) => CallToolResult::error(format!("cannot serialize result: {error}")),
        },
        Err(error) => CallToolResult::error(error),
    }
}

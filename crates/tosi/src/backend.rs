//! Provider adapters for one subprocess per task or follow-up turn.
//!
//! Claude's access policy is an explicit built-in tool list, not an OS sandbox.
//! Read-only workers can inspect files; writable workers also get editing tools.
//! Neither gets Bash, native subagents, skills, or inherited MCP connections.
//! Codex uses its native read-only or workspace-write sandbox.
//! Its user configuration and plugins are disabled; project or managed MCP
//! configuration can still apply. No empty-table override can erase that merge.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use claude_wrapper::{Claude, PermissionMode, QueryCommand};
use codex_wrapper::{ApprovalPolicy, Codex, ExecCommand, ExecResumeCommand, SandboxMode};
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::{
    config, selection,
    types::{AccessMode, Backend, Effort, RunOutput, RunSpec, Runner},
};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_millis(250);

/// Runs installed provider CLIs using their existing authentication.
///
/// `TOSI_CLAUDE_BIN` and `TOSI_CODEX_BIN` override executable
/// discovery, with legacy TASK_RUNNERS_* fallbacks when absent. They are read
/// at construction, not from task input.
#[derive(Clone)]
pub struct CliRunner {
    claude_binary: Option<PathBuf>,
    codex_binary: Option<PathBuf>,
    observer: Option<std::sync::Arc<dyn Fn(u32) + Send + Sync>>,
}

impl Default for CliRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl CliRunner {
    /// Use executable overrides from the environment, falling back to PATH.
    pub fn new() -> Self {
        Self {
            claude_binary: config::var_os("CLAUDE_BIN").map(PathBuf::from),
            codex_binary: config::var_os("CODEX_BIN").map(PathBuf::from),
            observer: None,
        }
    }

    /// Override one executable explicitly, useful for embedding and fake CLIs.
    pub fn with_binary(mut self, backend: Backend, path: impl Into<PathBuf>) -> Self {
        match backend {
            Backend::Claude => self.claude_binary = Some(path.into()),
            Backend::Codex => self.codex_binary = Some(path.into()),
        }
        self
    }

    /// Resolve the same executable used for workers without launching it or
    /// inspecting any credentials. Explicit overrides take precedence over PATH.
    pub fn resolved_binary(&self, backend: Backend) -> Result<PathBuf, String> {
        match backend {
            Backend::Claude => {
                let mut builder = Claude::builder();
                if let Some(binary) = &self.claude_binary {
                    builder = builder.binary(binary);
                }
                builder
                    .build()
                    .map(|client| client.binary().to_path_buf())
                    .map_err(claude_build_error)
            }
            Backend::Codex => {
                let mut builder = Codex::builder();
                if let Some(binary) = &self.codex_binary {
                    builder = builder.binary(binary);
                }
                builder
                    .build()
                    .map(|client| client.binary().to_path_buf())
                    .map_err(codex_build_error)
            }
        }
    }

    async fn claude(&self, spec: RunSpec, cancel: CancellationToken) -> Result<RunOutput, String> {
        let observer = self.observer.clone();
        let client = Claude::builder()
            .binary(self.resolved_binary(Backend::Claude)?)
            .working_dir(&spec.cwd)
            .env("TOSI_CHILD", "1")
            .env("TASK_RUNNERS_CHILD", "1")
            .timeout_secs(spec.timeout_secs)
            .kill_grace(TERMINATION_GRACE)
            .on_spawn(std::sync::Arc::new(move |info| {
                if let Some(observer) = &observer {
                    observer(info.pid);
                }
            }))
            .output_limit(OUTPUT_LIMIT)
            .build()
            .map_err(claude_build_error)?;
        let (permission, tools): (_, &[&str]) = match spec.access {
            AccessMode::ReadOnly => (PermissionMode::DontAsk, &["Read", "Glob", "Grep"]),
            AccessMode::WorkspaceWrite => (
                PermissionMode::AcceptEdits,
                &["Read", "Glob", "Grep", "Edit", "Write", "NotebookEdit"],
            ),
        };
        let mut command = QueryCommand::new(&spec.task)
            .prompt_via_stdin(true)
            .permission_mode(permission)
            .tools(tools.iter().copied())
            .allowed_tools(tools.iter().copied())
            .max_turns(30)
            .safe_mode()
            .strict_mcp_config()
            .mcp_config(r#"{"mcpServers":{}}"#)
            .disable_slash_commands();
        if let Some(model) = &spec.model {
            command = command.model(model);
        }
        if let Some(effort) = spec.effort {
            command = command.effort(claude_effort(effort)?);
        }
        if let Some(session_id) = &spec.session_id {
            command = command.resume(session_id);
        }
        if let Some(schema) = &spec.output_schema {
            command = command.json_schema(schema.to_string());
        }
        // Do not select away from this future: the wrapper must finish cleanup
        // before the manager can report that cancellation or timeout has settled.
        let mut result = command
            .execute_json_cancellable(&client, cancel.cancelled())
            .await
            .map_err(|error| claude_run_error(error, client.binary()))?;
        let subtype = result.extra.get("subtype").and_then(|value| value.as_str());
        if result.is_error || subtype.is_some_and(|value| value != "success") {
            return Err(format!(
                "Claude task failed ({}): {}",
                subtype.unwrap_or("error"),
                result.result
            ));
        }
        if spec.output_schema.is_none() {
            require_text("Claude", &result.result)?;
        }
        Ok(RunOutput {
            text: result.result,
            structured_result: result.extra.remove("structured_output"),
            session_id: nonempty(Some(result.session_id)).or(spec.session_id),
            usage: json!({
                "tokens": result.usage,
                "cost_usd": result.cost_usd,
                "num_turns": result.num_turns,
            }),
        })
    }

    async fn codex(&self, spec: RunSpec, cancel: CancellationToken) -> Result<RunOutput, String> {
        // Keep schema and final-output files alive through awaited child cleanup.
        // Each turn gets fresh paths, including resumed conversations.
        let artifacts = tempfile::Builder::new()
            .prefix("tosi-codex-")
            .tempdir()
            .map_err(|error| format!("cannot create Codex output directory: {error}"))?;
        let final_path = artifacts.path().join("final-message.txt");
        let final_arg = cli_path(&final_path)?;
        let schema_arg = if let Some(schema) = &spec.output_schema {
            let path = artifacts.path().join("schema.json");
            tokio::fs::write(&path, schema.to_string())
                .await
                .map_err(|error| format!("cannot write Codex output schema: {error}"))?;
            Some(cli_path(&path)?)
        } else {
            None
        };
        let observer = self.observer.clone();
        let client = Codex::builder()
            .binary(self.resolved_binary(Backend::Codex)?)
            .working_dir(&spec.cwd)
            .env("TOSI_CHILD", "1")
            .env("TASK_RUNNERS_CHILD", "1")
            .timeout_secs(spec.timeout_secs)
            .termination_grace(TERMINATION_GRACE)
            .on_spawn(std::sync::Arc::new(move |info| {
                if let Some(observer) = &observer {
                    observer(info.pid);
                }
            }))
            .output_limit(OUTPUT_LIMIT)
            .build()
            .map_err(codex_build_error)?;
        let result = if let Some(session_id) = &spec.session_id {
            let mut command = ExecResumeCommand::from_stdin(&spec.task)
                .session_id(session_id)
                .skip_git_repo_check()
                .ignore_user_config()
                .disable("plugins")
                .approval_policy(ApprovalPolicy::Never)
                .config(sandbox_config(spec.access))
                .output_last_message(&final_arg);
            if let Some(model) = &spec.model {
                command = command.model(model);
            }
            if let Some(effort) = spec.effort {
                command = command.config(format!("model_reasoning_effort=\"{}\"", effort.as_str()));
            }
            if let Some(path) = &schema_arg {
                command = command.output_schema(path);
            }
            command
                .execute_json_cancellable(&client, cancel.cancelled())
                .await
        } else {
            let mut command = ExecCommand::from_stdin(&spec.task)
                .skip_git_repo_check()
                .ignore_user_config()
                .disable("plugins")
                .approval_policy(ApprovalPolicy::Never)
                .sandbox(match spec.access {
                    AccessMode::ReadOnly => SandboxMode::ReadOnly,
                    AccessMode::WorkspaceWrite => SandboxMode::WorkspaceWrite,
                })
                .output_last_message(&final_arg);
            if let Some(model) = &spec.model {
                command = command.model(model);
            }
            if let Some(effort) = spec.effort {
                command = command.config(format!("model_reasoning_effort=\"{}\"", effort.as_str()));
            }
            if let Some(path) = &schema_arg {
                command = command.output_schema(path);
            }
            command
                .execute_json_cancellable(&client, cancel.cancelled())
                .await
        }
        .map_err(|error| codex_run_error(error, client.binary()))?;

        if let Some(failure) = result.events.iter().find(|event| event.is_turn_failed()) {
            return Err(format!(
                "Codex task failed: {}",
                failure.turn_failure_message().unwrap_or("turn.failed")
            ));
        }
        if !result.events.iter().any(|event| event.is_turn_completed()) {
            return Err("Codex exited without a successful turn.completed event".into());
        }
        // QueryResult.result concatenates agent messages, including narration.
        // Only the CLI's final-message file defines this task's answer.
        let text = read_codex_final_message(&final_path).await?;
        require_text("Codex", &text)?;
        let structured_result = if spec.output_schema.is_some() {
            Some(serde_json::from_str(&text).map_err(|error| {
                format!("Codex final message is not valid structured JSON: {error}")
            })?)
        } else {
            None
        };
        Ok(RunOutput {
            text,
            structured_result,
            session_id: nonempty(result.thread_id)
                .or_else(|| nonempty(result.session_id))
                .or(spec.session_id),
            usage: json!({"tokens": result.usage}),
        })
    }
}

fn claude_effort(effort: Effort) -> Result<claude_wrapper::Effort, String> {
    match effort {
        Effort::Low => Ok(claude_wrapper::Effort::Low),
        Effort::Medium => Ok(claude_wrapper::Effort::Medium),
        Effort::High => Ok(claude_wrapper::Effort::High),
        Effort::Xhigh => Ok(claude_wrapper::Effort::Xhigh),
        Effort::Max => Ok(claude_wrapper::Effort::Max),
        Effort::None | Effort::Minimal | Effort::Ultra => Err(format!(
            "Claude does not support effort {}",
            effort.as_str()
        )),
    }
}

fn claude_build_error(error: claude_wrapper::Error) -> String {
    if matches!(error, claude_wrapper::Error::NotFound) {
        missing_binary_error("Claude", "TOSI_CLAUDE_BIN")
    } else {
        error.to_string()
    }
}

fn codex_build_error(error: codex_wrapper::Error) -> String {
    if matches!(error, codex_wrapper::Error::NotFound) {
        missing_binary_error("Codex", "TOSI_CODEX_BIN")
    } else {
        error.to_string()
    }
}

fn missing_binary_error(provider: &str, override_name: &str) -> String {
    format!(
        "{provider} executable was not found on the worker host's PATH. Set \
         {override_name} to its executable path or check the host PATH, then run \
         check_environment or `tosi doctor`."
    )
}

fn claude_run_error(error: claude_wrapper::Error, binary: &Path) -> String {
    match error.auth_kind() {
        Some(claude_wrapper::auth::AuthErrorKind::RateLimit) => format!(
            "Claude reported a rate or usage limit in this worker context \
             (rate_limit; executable: {}). Check the provider's usage or billing \
             limits; run check_environment or `tosi doctor` to inspect \
             this launch context.",
            binary.display()
        ),
        Some(kind) => auth_context_error("Claude", binary, kind.as_str()),
        None => error.to_string(),
    }
}

fn codex_run_error(error: codex_wrapper::Error, binary: &Path) -> String {
    if error.failure_kind() == Some(codex_wrapper::FailureKind::Auth) {
        auth_context_error("Codex", binary, "auth")
    } else {
        error.to_string()
    }
}

fn auth_context_error(provider: &str, binary: &Path, kind: &str) -> String {
    format!(
        "{provider} authentication is unavailable in this worker launch context \
         ({kind}; executable: {}). Run check_environment or `tosi doctor` \
         and compare with your normal terminal or host environment. An inherited \
         sandbox may hide the macOS Keychain; this does not establish that you \
         are logged out.",
        binary.display()
    )
}

fn cli_path(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "Codex temporary file path is not UTF-8".into())
}

async fn read_codex_final_message(path: &Path) -> Result<String, String> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| format!("cannot read Codex final-message file: {error}"))?;
    if !metadata.is_file() {
        return Err("Codex final-message output must be a regular file".into());
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("cannot open Codex final-message file: {error}"))?;
    let mut bytes = Vec::new();
    file.take((OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("cannot read Codex final-message file: {error}"))?;
    if bytes.len() > OUTPUT_LIMIT {
        return Err(format!(
            "Codex final-message file exceeds {OUTPUT_LIMIT} bytes"
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| format!("Codex final-message file is not UTF-8: {error}"))
}

#[async_trait]
impl Runner for CliRunner {
    async fn run_observed(
        &self,
        spec: RunSpec,
        cancel: CancellationToken,
        observer: std::sync::Arc<dyn Fn(u32) + Send + Sync>,
    ) -> Result<RunOutput, String> {
        let mut runner = self.clone();
        runner.observer = Some(observer);
        runner.run(spec, cancel).await
    }

    async fn run(&self, spec: RunSpec, cancel: CancellationToken) -> Result<RunOutput, String> {
        if cancel.is_cancelled() {
            return Err("task cancelled before process start".into());
        }
        if spec.task.trim().is_empty() || spec.timeout_secs == 0 {
            return Err("task must be nonempty and timeout_secs must be positive".into());
        }
        if spec
            .model
            .as_ref()
            .is_some_and(|model| model.trim().is_empty())
            || spec
                .session_id
                .as_ref()
                .is_some_and(|id| id.trim().is_empty())
        {
            return Err("model and session_id must be nonempty when supplied".into());
        }
        if !spec.cwd.is_absolute() || !spec.cwd.is_dir() {
            return Err("cwd must be an existing absolute directory".into());
        }
        selection::validate(spec.backend, spec.model.as_deref(), spec.effort)?;
        match spec.backend {
            Backend::Claude => self.claude(spec, cancel).await,
            Backend::Codex => self.codex(spec, cancel).await,
        }
    }
}

fn sandbox_config(access: AccessMode) -> &'static str {
    match access {
        AccessMode::ReadOnly => r#"sandbox_mode="read-only""#,
        AccessMode::WorkspaceWrite => r#"sandbox_mode="workspace-write""#,
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn require_text(provider: &str, text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        Err(format!(
            "{provider} exited without a nonempty assistant result"
        ))
    } else {
        Ok(())
    }
}

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::types::{
    AccessMode, Backend, Effort, ReportedSettings, RequestedSettings, RunOutput, RunSpec, Runner,
    SelectedSettings, Selection,
};

const MAX_TASKS: usize = 1024;
const MAX_TEXT_CHARS: usize = 64_000;
const MAX_PROMPT_BYTES: usize = 128_000;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpawnTask {
    /// A self-contained assignment, relevant context, constraints, and desired output.
    pub task: String,
    /// Absolute path to the worker's existing working directory. Use separate worktrees for parallel edits.
    pub cwd: String,
    /// Claude or Codex; omitted uses the server's configured default.
    pub backend: Option<Backend>,
    pub model: Option<String>,
    /// Literal provider effort. Omitted leaves the CLI default; not inherited from this chat.
    /// Claude: low/medium/high/xhigh/max. Codex also accepts none/minimal/ultra, model-dependent.
    pub effort: Option<Effort>,
    /// Optional self-contained JSON Schema with root type object, at most 64000 serialized bytes.
    /// Completed output is validated and returned as structured_result. Follow-ups inherit this schema.
    pub output_schema: Option<serde_json::Value>,
    /// Defaults to read_only. Claude workspace_write enables file edits, not shell commands.
    pub access: Option<AccessMode>,
    /// Maximum duration of this turn, from 1 to 3600 seconds; defaults to 600.
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContinueTask {
    /// The latest successfully completed task in this conversation.
    pub task_id: String,
    /// The next bounded assignment. The provider resumes its saved conversation.
    pub message: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitTasks {
    /// Between 1 and 32 task IDs. Remove completed IDs before waiting again.
    pub task_ids: Vec<String>,
    /// Return when any task finishes or this timeout expires. Zero gives an immediate snapshot. Maximum 55000.
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelTask {
    pub task_id: String,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

impl Status {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub conversation_id: String,
    pub parent_task_id: Option<String>,
    pub backend: Backend,
    pub status: Status,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub selection: Selection,
    pub access: AccessMode,
    pub timeout_secs: u64,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub result: Option<String>,
    pub structured_result: Option<serde_json::Value>,
    pub result_truncated: bool,
    pub error: Option<String>,
    pub provider_session_id: Option<String>,
    pub usage: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct WaitResult {
    pub tasks: Vec<TaskSnapshot>,
    pub timed_out: bool,
}

struct TaskEntry {
    snapshot: TaskSnapshot,
    spec: RunSpec,
    cancel: CancellationToken,
}

struct Conversation {
    latest_task_id: String,
    active_task_id: Option<String>,
}

#[derive(Default)]
struct State {
    tasks: HashMap<String, TaskEntry>,
    conversations: HashMap<String, Conversation>,
    closing: bool,
}

/// Where trovador keeps the record of sessions the toolkit started. Each session this
/// server starts is recorded there, so trovador's list and search include it although it is
/// an automated session. Past sessions are read-only history: nothing here continues one.
#[derive(Clone, Debug)]
pub struct Sessions {
    pub paths: std::path::PathBuf,
}

impl Sessions {
    pub fn from_prepared(state: &bottega_core::state::State) -> Self {
        Self {
            paths: state.shared.clone(),
        }
    }
}

fn to_core(backend: Backend) -> bottega_core::Backend {
    match backend {
        Backend::Claude => bottega_core::Backend::Claude,
        Backend::Codex => bottega_core::Backend::Codex,
    }
}

#[derive(Clone)]
pub struct Manager {
    state: Arc<Mutex<State>>,
    changed: watch::Sender<u64>,
    runner: Arc<dyn Runner>,
    default_backend: Backend,
    max_concurrency: usize,
    sessions: Option<Sessions>,
    redact: bool,
}

impl Manager {
    pub fn new(runner: Arc<dyn Runner>, default_backend: Backend, max_concurrency: usize) -> Self {
        let (changed, _) = watch::channel(0);
        Self {
            state: Arc::new(Mutex::new(State::default())),
            changed,
            runner,
            default_backend,
            max_concurrency,
            sessions: None,
            redact: true,
        }
    }

    /// Whether secrets in worker results and errors are replaced with `[redacted:<kind>]`.
    /// On by default. A worker that read a repository can quote its keys and tokens, and the
    /// text goes straight into the calling agent's context. Set only from the server's
    /// environment, never from a tool argument.
    pub fn with_redaction(mut self, redact: bool) -> Self {
        self.redact = redact;
        self
    }

    /// Records the sessions this server starts in trovador's provenance record.
    pub fn with_sessions(mut self, sessions: Sessions) -> Self {
        self.sessions = Some(sessions);
        self
    }

    pub async fn spawn_task(&self, input: SpawnTask) -> Result<TaskSnapshot, String> {
        validate_prompt(&input.task)?;
        let cwd = PathBuf::from(&input.cwd);
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Err("cwd must be an absolute path to an existing directory".into());
        }
        let cwd = cwd
            .canonicalize()
            .map_err(|e| format!("cannot resolve cwd: {e}"))?;
        let timeout_secs = input.timeout_secs.unwrap_or(600);
        if !(1..=3600).contains(&timeout_secs) {
            return Err("timeout_secs must be between 1 and 3600".into());
        }
        if input.model.as_ref().is_some_and(|m| m.trim().is_empty()) {
            return Err("model cannot be empty".into());
        }
        if let Some(schema) = &input.output_schema {
            crate::output::compile_schema(schema)?;
        }
        let backend = input.backend.unwrap_or(self.default_backend);
        crate::selection::validate(backend, input.model.as_deref(), input.effort)?;
        let requested = RequestedSettings {
            backend: input.backend,
            model: input.model.clone(),
            effort: input.effort,
        };
        let spec = RunSpec {
            backend,
            task: input.task,
            cwd,
            model: input.model,
            effort: input.effort,
            output_schema: input.output_schema,
            access: input.access.unwrap_or_default(),
            session_id: None,
            timeout_secs,
        };
        let mut state = self.state.lock().await;
        self.check_capacity(&state)?;
        let conversation_id = Uuid::new_v4().to_string();
        let snapshot = self.insert(&mut state, conversation_id, None, spec, requested);
        Ok(snapshot)
    }

    pub async fn continue_task(&self, input: ContinueTask) -> Result<TaskSnapshot, String> {
        validate_prompt(&input.message)?;
        let mut state = self.state.lock().await;
        self.check_capacity(&state)?;
        let prior = state.tasks.get(&input.task_id).ok_or("unknown task_id")?;
        let conversation_id = prior.snapshot.conversation_id.clone();
        let conversation = state
            .conversations
            .get(&conversation_id)
            .ok_or("conversation missing")?;
        if conversation.active_task_id.is_some() {
            return Err("this conversation already has an active task".into());
        }
        if conversation.latest_task_id != input.task_id {
            return Err(format!(
                "continue the latest task instead: {}",
                conversation.latest_task_id
            ));
        }
        if prior.snapshot.status != Status::Completed {
            return Err("only a successfully completed task can be continued; spawn a new task after failure or cancellation".into());
        }
        let session_id = prior
            .snapshot
            .provider_session_id
            .clone()
            .ok_or("provider returned no resumable session ID")?;
        let mut spec = prior.spec.clone();
        let requested = prior.snapshot.selection.requested.clone();
        spec.task = input.message;
        spec.session_id = Some(session_id);
        let snapshot = self.insert(
            &mut state,
            conversation_id,
            Some(input.task_id),
            spec,
            requested,
        );
        Ok(snapshot)
    }

    fn check_capacity(&self, state: &State) -> Result<(), String> {
        if state.closing {
            return Err("server is shutting down".into());
        }
        if state.tasks.len() >= MAX_TASKS {
            return Err("this experiment retains at most 1024 tasks per server session; start a new server session".into());
        }
        let active = state
            .tasks
            .values()
            .filter(|t| !t.snapshot.status.is_terminal())
            .count();
        if active >= self.max_concurrency {
            return Err(format!(
                "concurrency limit ({}) reached; wait for a task to finish",
                self.max_concurrency
            ));
        }
        Ok(())
    }

    fn insert(
        &self,
        state: &mut State,
        conversation_id: String,
        parent_task_id: Option<String>,
        spec: RunSpec,
        requested: RequestedSettings,
    ) -> TaskSnapshot {
        let task_id = Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let snapshot = TaskSnapshot {
            task_id: task_id.clone(),
            conversation_id: conversation_id.clone(),
            parent_task_id,
            backend: spec.backend,
            status: Status::Running,
            cwd: spec.cwd.clone(),
            model: spec.model.clone(),
            effort: spec.effort,
            selection: Selection {
                requested,
                selected: SelectedSettings {
                    backend: spec.backend,
                    model: spec.model.clone(),
                    effort: spec.effort,
                },
                // Neither adapter exposes reliable per-turn effective model/effort.
                reported: ReportedSettings::default(),
            },
            access: spec.access,
            timeout_secs: spec.timeout_secs,
            started_at_ms: now_ms(),
            finished_at_ms: None,
            result: None,
            structured_result: None,
            result_truncated: false,
            error: None,
            provider_session_id: None,
            usage: None,
        };
        state.tasks.insert(
            task_id.clone(),
            TaskEntry {
                snapshot: snapshot.clone(),
                spec: spec.clone(),
                cancel: cancel.clone(),
            },
        );
        state.conversations.insert(
            conversation_id,
            Conversation {
                latest_task_id: task_id.clone(),
                active_task_id: Some(task_id.clone()),
            },
        );
        let runner = self.runner.clone();
        let manager = self.clone();
        // The monitor owns the execution handle. Dropping an MCP request must not abort provider cleanup.
        tokio::spawn(async move {
            let run = tokio::spawn(async move { runner.run(spec, cancel).await });
            let outcome = match run.await {
                Ok(outcome) => outcome,
                Err(e) => Err(format!("worker panicked or stopped: {e}")),
            };
            manager.finish(&task_id, outcome).await;
        });
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
        snapshot
    }

    async fn finish(&self, task_id: &str, outcome: Result<RunOutput, String>) {
        let mut state = self.state.lock().await;
        let Some(entry) = state.tasks.get_mut(task_id) else {
            return;
        };
        entry.snapshot.finished_at_ms = Some(now_ms());
        if entry.cancel.is_cancelled() {
            entry.snapshot.status = Status::Cancelled;
        } else {
            // Redaction runs before validation, so the structured result a caller receives is the
            // one checked against output_schema (found in review: redacting after validation
            // could complete with a result that no longer matched it). The session id is kept
            // either way, so a turn that fails validation is still recorded as started here.
            let mut session_id = None;
            let outcome = outcome.and_then(|output| {
                session_id = output.session_id.clone();
                let original = output.structured_result.clone();
                let (output, changed) = self.redact_output(output);
                let schema = entry.spec.output_schema.as_ref();
                crate::output::validate_result(schema, output).map_err(|e| {
                    let passed_before = changed
                        && crate::output::validate_result(
                            schema,
                            RunOutput {
                                text: String::new(),
                                structured_result: original,
                                session_id: None,
                                usage: serde_json::Value::Null,
                            },
                        )
                        .is_ok();
                    if passed_before {
                        format!("structured_result held secrets, and with them redacted it no longer matches output_schema: {e}")
                    } else {
                        e
                    }
                })
            });
            // A session this server started is recorded as started by the toolkit, so
            // trovador's list and search include it although it is automated. A follow-up
            // resumes a session already recorded.
            if entry.spec.session_id.is_none()
                && let (Some(sessions), Some(id)) = (&self.sessions, &session_id)
                && let Err(e) = bottega_core::provenance::record_spawned(
                    &sessions.paths,
                    to_core(entry.spec.backend),
                    id,
                )
            {
                tracing::warn!("recording {id} as started by tosi failed: {e}");
            }
            match outcome {
                Ok(output) => {
                    entry.snapshot.status = Status::Completed;
                    // Already redacted whole by redact_output, before this cut, so a cut cannot
                    // split a secret into pieces no rule matches (trovador#43).
                    let (text, truncated) = bounded_text(&output.text);
                    entry.snapshot.result = Some(text);
                    entry.snapshot.structured_result = output.structured_result;
                    entry.snapshot.result_truncated = truncated;
                    entry.snapshot.provider_session_id = output.session_id;
                    entry.snapshot.usage = Some(output.usage);
                }
                Err(error) => {
                    entry.snapshot.status = Status::Failed;
                    entry.snapshot.provider_session_id = session_id;
                    entry.snapshot.error = Some(bounded_text(&self.redacted(&error)).0);
                }
            }
        }
        let conversation_id = entry.snapshot.conversation_id.clone();
        if let Some(conversation) = state.conversations.get_mut(&conversation_id) {
            conversation.active_task_id = None;
        }
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    pub async fn wait_tasks(&self, input: WaitTasks) -> Result<WaitResult, String> {
        if input.task_ids.is_empty() || input.task_ids.len() > 32 {
            return Err("task_ids must contain between 1 and 32 IDs".into());
        }
        let timeout_ms = input.timeout_ms.unwrap_or(30_000);
        if timeout_ms > 55_000 {
            return Err("timeout_ms must be at most 55000".into());
        }
        let mut changed = self.changed.subscribe();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let tasks = self.snapshots(&input.task_ids).await?;
            if tasks.iter().any(|t| t.status.is_terminal()) {
                return Ok(WaitResult {
                    tasks,
                    timed_out: false,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(WaitResult {
                    tasks,
                    timed_out: true,
                });
            }
            if tokio::time::timeout_at(deadline, changed.changed())
                .await
                .is_err()
            {
                let tasks = self.snapshots(&input.task_ids).await?;
                let timed_out = !tasks.iter().any(|t| t.status.is_terminal());
                return Ok(WaitResult { tasks, timed_out });
            }
        }
    }

    async fn snapshots(&self, ids: &[String]) -> Result<Vec<TaskSnapshot>, String> {
        let state = self.state.lock().await;
        ids.iter()
            .map(|id| {
                state
                    .tasks
                    .get(id)
                    .map(|t| t.snapshot.clone())
                    .ok_or_else(|| format!("unknown task_id: {id}"))
            })
            .collect()
    }

    pub async fn cancel_task(&self, input: CancelTask) -> Result<TaskSnapshot, String> {
        {
            let mut state = self.state.lock().await;
            let entry = state
                .tasks
                .get_mut(&input.task_id)
                .ok_or("unknown task_id")?;
            if entry.snapshot.status.is_terminal() {
                return Ok(entry.snapshot.clone());
            }
            entry.snapshot.status = Status::Cancelling;
            entry.cancel.cancel();
            self.changed.send_modify(|v| *v = v.wrapping_add(1));
        }
        let result = self
            .wait_tasks(WaitTasks {
                task_ids: vec![input.task_id],
                timeout_ms: Some(55_000),
            })
            .await?;
        // A slow provider remains `cancelling`; do not claim it has exited before cleanup finishes.
        Ok(result.tasks.into_iter().next().expect("one requested task"))
    }

    pub async fn shutdown(&self) {
        let mut changed = self.changed.subscribe();
        {
            let mut state = self.state.lock().await;
            state.closing = true;
            for entry in state.tasks.values_mut() {
                if !entry.snapshot.status.is_terminal() {
                    entry.snapshot.status = Status::Cancelling;
                    entry.cancel.cancel();
                }
            }
        }
        loop {
            if self
                .state
                .lock()
                .await
                .tasks
                .values()
                .all(|t| t.snapshot.status.is_terminal())
            {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    }
}

fn validate_prompt(prompt: &str) -> Result<(), String> {
    if prompt.trim().is_empty() {
        return Err("task/message cannot be empty".into());
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        return Err("task/message exceeds 128000 bytes".into());
    }
    Ok(())
}

impl Manager {
    /// The output with secrets redacted, and whether structured_result changed.
    fn redact_output(&self, mut output: RunOutput) -> (RunOutput, bool) {
        if !self.redact {
            return (output, false);
        }
        output.text = bottega_core::redact::redact_str(&output.text).into_owned();
        let mut changed = false;
        if let Some(value) = output.structured_result.as_mut() {
            let before = value.clone();
            bottega_core::redact::redact_value(value);
            changed = *value != before;
        }
        (output, changed)
    }

    fn redacted<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        if self.redact {
            bottega_core::redact::redact_str(text)
        } else {
            std::borrow::Cow::Borrowed(text)
        }
    }
}

fn bounded_text(text: &str) -> (String, bool) {
    let mut chars = text.chars();
    let result: String = chars.by_ref().take(MAX_TEXT_CHARS).collect();
    (result, chars.next().is_some())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

//! Running a provider turn: start a session, continue one, or fork one.
//!
//! Every run, from either door, goes through [`ask`]. That is where consent is checked,
//! where spawn provenance is recorded, and where the run log is written, so no caller can
//! skip any of them.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::archive::{self, Backend, LookupError};
use crate::consent::{self, Check};
use crate::sessions::Ctx;

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct AskOpts {
    /// What to send.
    pub prompt: String,
    /// claude or codex. Taken from the archive when continuing; claude for a new session.
    pub backend: Option<Backend>,
    /// Continue this session (id or unambiguous prefix) instead of starting one.
    pub session: Option<String>,
    /// Consult about this session instead: summarize it (cached) and start a new session,
    /// on the other backend unless `backend` says otherwise, seeded with the summary and
    /// the prompt as a question. Works on sessions too large to resume.
    pub about: Option<String>,
    /// With `session`, resume its context into a new session instead (claude only).
    #[serde(default)]
    pub fork: bool,
    /// Directory to run in. Defaults to the session's project when continuing.
    pub cwd: Option<String>,
    /// Further directories the run may use.
    #[serde(default)]
    pub add_dirs: Vec<String>,
    pub model: Option<String>,
    /// Reasoning effort: low, medium, high, xhigh, max.
    pub effort: Option<String>,
    /// Tools the run may use without asking (claude), e.g. "Read", "Bash(git:*)".
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Tools the run may not use (claude).
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    /// claude permission mode: default, acceptEdits, plan, dontAsk, auto.
    pub permission_mode: Option<String>,
    /// Text appended to the system prompt (claude).
    pub append_system_prompt: Option<String>,
    /// Turn limit for one run (claude).
    pub max_turns: Option<u32>,
    /// Seconds before the run is killed. Defaults per backend.
    pub timeout_s: Option<u64>,
    /// Do not load the operator's ambient Claude configuration and MCP servers (claude).
    #[serde(default)]
    pub hermetic: bool,
    /// codex sandbox: read-only (default) or workspace-write.
    pub sandbox: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Continuation {
    /// Small enough for the provider to accept another turn.
    pub resumable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub consent: Check,
    /// The final answer: both of the above allow it.
    pub allowed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AskResult {
    pub backend: Backend,
    /// The session the turn ran in. Present whenever the provider got far enough to name
    /// it, including runs that failed or timed out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// started, continued, or forked.
    pub verb: String,
    /// The provider's final message.
    pub result: String,
    pub is_error: bool,
    pub timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    pub duration_ms: u64,
    /// Options the backend does not support, which were not applied.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub warnings: Vec<String>,
    /// Whether this session can take another turn, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<Continuation>,
    /// Present for a consultation: the session it was about and what its summary cost.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consulted: Option<Consulted>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_with: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Consulted {
    pub backend: Backend,
    pub session_id: String,
    pub summary_cached: bool,
    pub summary_cost_usd: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Summary(#[from] crate::summary::SummaryError),
    #[error(transparent)]
    Lookup(#[from] LookupError),
    #[error(
        "continuation denied: {}. Consent is changed only from the local CLI: {}",
        .0.explanation,
        if .0.next_steps.is_empty() { "trovador consent status".to_string() } else { .0.next_steps.join("; ") }
    )]
    Denied(Box<Check>),
    #[error("cannot continue {backend}/{session_id}: {reason}")]
    NotResumable {
        backend: Backend,
        session_id: String,
        reason: String,
    },
    #[error(
        "spawn depth {depth} has reached TROVADOR_MAX_DEPTH={max}; refusing to start another run"
    )]
    Depth { depth: usize, max: usize },
    #[error("{0}")]
    Invalid(String),
    #[error("could not start {backend}: {message}")]
    Spawn { backend: Backend, message: String },
}

/// What the stream callback learns as the run goes.
#[derive(Default)]
struct Seen {
    session_id: Option<String>,
    result: Option<String>,
    last_text: Option<String>,
    is_error: bool,
    error: Option<String>,
    cost_usd: Option<f64>,
    num_turns: Option<u64>,
    usage: Option<Value>,
    /// Recording provenance failed. The run went ahead, but continuing it would be denied,
    /// so the result says so.
    provenance_error: Option<String>,
}

fn depth() -> usize {
    std::env::var("TROVADOR_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Starts, continues, forks, or consults, and runs one turn.
pub async fn ask(ctx: &Ctx, opts: &AskOpts) -> Result<AskResult, RunError> {
    match opts.about.as_deref().filter(|a| !a.is_empty()) {
        Some(about) => consult(ctx, opts, about).await,
        None => run_turn(ctx, opts).await,
    }
}

async fn consult(ctx: &Ctx, opts: &AskOpts, about: &str) -> Result<AskResult, RunError> {
    if opts.session.is_some() || opts.fork {
        return Err(RunError::Invalid(
            "about starts a new session; it cannot be combined with session or fork".into(),
        ));
    }
    if opts.prompt.trim().is_empty() {
        return Err(RunError::Invalid("prompt is empty".into()));
    }
    let source = archive::find_session(&ctx.paths, about, None)?;
    let summary =
        crate::summary::summarize(ctx, &source.session_id, Some(source.backend), false, None)
            .await?;
    let target = opts.backend.unwrap_or(match source.backend {
        Backend::Claude => Backend::Codex,
        Backend::Codex => Backend::Claude,
    });
    let seeded = format!(
        "You are being consulted about a prior agent session run by {}{}. Summary of that session:\n\n{}\n\n---\nQuestion: {}",
        source.backend,
        summary
            .project
            .as_deref()
            .map(|p| format!(" in project {p}"))
            .unwrap_or_default(),
        summary.summary,
        opts.prompt
    );
    // The source's project, when it still exists, is the natural place to answer from.
    let cwd = opts
        .cwd
        .clone()
        .or_else(|| summary.project.clone().filter(|p| Path::new(p).is_dir()));
    let mut result = run_turn(
        ctx,
        &AskOpts {
            prompt: seeded,
            backend: Some(target),
            about: None,
            cwd,
            ..opts.clone()
        },
    )
    .await?;
    result.consulted = Some(Consulted {
        backend: source.backend,
        session_id: source.session_id,
        summary_cached: summary.cached,
        summary_cost_usd: summary.cost_usd,
    });
    Ok(result)
}

async fn run_turn(ctx: &Ctx, opts: &AskOpts) -> Result<AskResult, RunError> {
    if opts.prompt.trim().is_empty() {
        return Err(RunError::Invalid("prompt is empty".into()));
    }
    // Every option is checked before anything is recorded or started.
    if let Some(e) = &opts.effort {
        parse_effort(e).map_err(RunError::Invalid)?;
    }
    if let Some(p) = &opts.permission_mode {
        parse_permission_mode(p).map_err(RunError::Invalid)?;
    }
    if let Some(s) = &opts.sandbox {
        parse_sandbox(s).map_err(RunError::Invalid)?;
    }
    let depth = depth();
    if depth >= ctx.limits.max_depth {
        return Err(RunError::Depth {
            depth,
            max: ctx.limits.max_depth,
        });
    }

    // Resolve the target once. Consent is keyed by the canonical id, and only that id is
    // given to the provider, so a prefix or alias cannot get around an explicit denial.
    let mut cwd = opts.cwd.clone();
    let (backend, resume) = match opts.session.as_deref().filter(|s| !s.is_empty()) {
        Some(token) => {
            let e = archive::find_session(&ctx.paths, token, opts.backend)?;
            if opts.fork && e.backend == Backend::Codex {
                return Err(RunError::Invalid(
                    "fork is supported for claude sessions only".into(),
                ));
            }
            let r = archive::resumability(&e, &ctx.limits);
            if !r.resumable {
                return Err(RunError::NotResumable {
                    backend: e.backend,
                    session_id: e.session_id,
                    reason: r.reason.unwrap_or_default(),
                });
            }
            if !opts.fork {
                let check = consent::check(&ctx.paths, e.backend, &e.session_id);
                if !check.allowed {
                    return Err(RunError::Denied(Box::new(check)));
                }
            }
            // A resume has to run where the session lives, or the provider cannot find it.
            if cwd.is_none() {
                cwd = archive::read_meta(&e, &ctx.limits)
                    .project()
                    .map(str::to_string);
            }
            (e.backend, Some(e.session_id))
        }
        None if opts.fork => return Err(RunError::Invalid("fork needs a session".into())),
        None => (opts.backend.unwrap_or(Backend::Claude), None),
    };
    if let Some(dir) = &cwd
        && !Path::new(dir).is_dir()
    {
        return Err(RunError::Invalid(format!(
            "cwd {dir} is missing or not a directory"
        )));
    }
    let verb = match (&resume, opts.fork) {
        (None, _) => "started",
        (Some(_), false) => "continued",
        (Some(_), true) => "forked",
    };
    let timeout_s = opts.timeout_s.unwrap_or(match backend {
        Backend::Claude => ctx.limits.claude_timeout_s,
        Backend::Codex => ctx.limits.codex_timeout_s,
    } as u64);

    let seen = Arc::new(Mutex::new(Seen::default()));
    let started = Instant::now();
    log_run(
        ctx,
        json!({"phase": "start", "backend": backend, "verb": verb,
        "session": resume, "cwd": cwd, "depth": depth}),
    );
    let mut warnings = Vec::new();
    let outcome = match backend {
        Backend::Claude => {
            run_claude(
                ctx,
                opts,
                resume.as_deref(),
                cwd.as_deref(),
                timeout_s,
                depth,
                &seen,
            )
            .await
        }
        Backend::Codex => {
            run_codex(
                ctx,
                opts,
                resume.as_deref(),
                cwd.as_deref(),
                timeout_s,
                depth,
                &seen,
                &mut warnings,
            )
            .await
        }
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    let mut seen = std::mem::take(&mut *seen.lock().expect("seen"));

    // A resumed (not forked) run stays in the session it was given, whatever it reports.
    if verb == "continued" {
        seen.session_id = resume.clone();
    }
    let timed_out = matches!(outcome, Err(Failure::Timeout));
    let error = match &outcome {
        Ok(()) => seen.error.clone(),
        Err(Failure::Timeout) => Some(format!(
            "{backend} exceeded its {timeout_s}s timeout and was killed; raise timeout_s to give it longer"
        )),
        Err(Failure::Failed(m)) => Some(seen.error.clone().unwrap_or_else(|| m.clone())),
        Err(Failure::Spawn(m)) => {
            log_run(
                ctx,
                json!({"phase": "end", "backend": backend, "verb": verb,
                "outcome": "spawn_failed", "ms": duration_ms}),
            );
            return Err(RunError::Spawn {
                backend,
                message: m.clone(),
            });
        }
    };
    let error = match (error, seen.provenance_error.take()) {
        (Some(e), Some(p)) => Some(format!("{e}; recording the session as spawned failed: {p}")),
        (None, Some(p)) => Some(format!("recording the session as spawned failed: {p}")),
        (e, None) => e,
    };
    let is_error = seen.is_error || outcome.is_err() || error.is_some();

    let session_id = seen.session_id.clone();
    let (continuation, resource, open_with) = match &session_id {
        Some(id) => {
            let entry = archive::find_session(&ctx.paths, id, Some(backend)).ok();
            // A run killed in its first seconds can end before the provider writes any
            // transcript. The id is real but there is nothing to resume; saying it can be
            // continued sent the next turn into a not-found error (found by tests/live.rs).
            let r = entry
                .as_ref()
                .map(|e| archive::resumability(e, &ctx.limits))
                .unwrap_or_else(|| archive::Resumability {
                    resumable: false,
                    reason: Some(
                        "the provider wrote no transcript for this session before the run ended, so there is nothing to continue; start a new session".into(),
                    ),
                });
            let consent = consent::check(&ctx.paths, backend, id);
            let project = entry
                .as_ref()
                .and_then(|e| {
                    archive::read_meta(e, &ctx.limits)
                        .project()
                        .map(str::to_string)
                })
                .or(cwd.clone());
            (
                Some(Continuation {
                    allowed: r.resumable && consent.allowed,
                    resumable: r.resumable,
                    reason: r.reason,
                    consent,
                }),
                Some(format!("session://{backend}/{id}")),
                Some(archive::open_with(backend, project.as_deref(), id)),
            )
        }
        None => (None, None, None),
    };
    log_run(
        ctx,
        json!({"phase": "end", "backend": backend, "verb": verb,
        "session": session_id, "outcome": if timed_out { "timeout" } else if is_error { "error" } else { "ok" },
        "ms": duration_ms}),
    );

    Ok(AskResult {
        backend,
        session_id,
        verb: verb.into(),
        result: seen.result.or(seen.last_text).unwrap_or_default(),
        is_error,
        timed_out,
        error,
        cost_usd: seen.cost_usd,
        num_turns: seen.num_turns,
        usage: seen.usage,
        duration_ms,
        warnings,
        continuation,
        consulted: None,
        resource,
        open_with,
    })
}

enum Failure {
    Timeout,
    Failed(String),
    Spawn(String),
}

/// Records a session as spawned from inside the stream callback, the moment its id is
/// known. A failure is kept for the result rather than stopping the run.
fn note_spawned(ctx: &Ctx, backend: Backend, id: &str, seen: &mut Seen) {
    if let Err(e) = consent::record_spawned(&ctx.paths, backend, id) {
        seen.provenance_error = Some(e.to_string());
    }
}

async fn run_claude(
    ctx: &Ctx,
    opts: &AskOpts,
    resume: Option<&str>,
    cwd: Option<&str>,
    timeout_s: u64,
    depth: usize,
    seen: &Arc<Mutex<Seen>>,
) -> Result<(), Failure> {
    use claude_wrapper::{Claude, OutputFormat, QueryCommand};

    let mut builder = Claude::builder()
        .timeout(Duration::from_secs(timeout_s))
        .process_group(true)
        .env("TROVADOR_DEPTH", (depth + 1).to_string());
    if let Some(bin) = &ctx.paths.claude_bin {
        builder = builder.binary(bin);
    }
    if let Some(dir) = cwd {
        builder = builder.working_dir(dir);
    }
    let claude = builder.build().map_err(|e| Failure::Spawn(e.to_string()))?;

    let mut cmd = QueryCommand::new(opts.prompt.clone())
        // The prompt goes on stdin: variadic flags such as --allowed-tools swallow a
        // trailing positional prompt.
        .prompt_via_stdin(true)
        .output_format(OutputFormat::StreamJson)
        .verbose(true);
    // A new session's id is chosen here and recorded before the process starts, so even
    // a run killed before it prints anything can be continued (spike#108).
    let new_id = match resume {
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let mut s = seen.lock().expect("seen");
            note_spawned(ctx, Backend::Claude, &id, &mut s);
            s.session_id = Some(id.clone());
            cmd = cmd.session_id(id.clone());
            Some(id)
        }
        Some(id) => {
            cmd = cmd.resume(id);
            if opts.fork {
                cmd = cmd.fork_session();
            }
            None
        }
    };
    if let Some(m) = &opts.model {
        cmd = cmd.model(m);
    }
    if let Some(e) = &opts.effort {
        cmd = cmd.effort(parse_effort(e).map_err(Failure::Spawn)?);
    }
    if let Some(p) = &opts.permission_mode {
        cmd = cmd.permission_mode(parse_permission_mode(p).map_err(Failure::Spawn)?);
    }
    if !opts.allowed_tools.is_empty() {
        cmd = cmd.allowed_tools(opts.allowed_tools.clone());
    }
    if !opts.disallowed_tools.is_empty() {
        cmd = cmd.disallowed_tools(opts.disallowed_tools.clone());
    }
    for d in &opts.add_dirs {
        cmd = cmd.add_dir(d);
    }
    if let Some(n) = opts.max_turns {
        cmd = cmd.max_turns(n);
    }
    if let Some(s) = &opts.append_system_prompt {
        cmd = cmd.append_system_prompt(s);
    }
    if opts.hermetic {
        cmd = cmd.hermetic();
    }

    let forked = opts.fork;
    let handler_seen = seen.clone();
    let result = claude_wrapper::streaming::stream_query(&claude, &cmd, |event| {
        let mut s = handler_seen.lock().expect("seen");
        let d = &event.data;
        match d.get("type").and_then(Value::as_str) {
            Some("system") => {
                if let Some(id) = d.get("session_id").and_then(Value::as_str) {
                    // A fork's new id is first known here.
                    if forked && s.session_id.as_deref() != Some(id) {
                        note_spawned(ctx, Backend::Claude, id, &mut s);
                    }
                    if new_id.is_none() {
                        s.session_id = Some(id.to_string());
                    }
                }
            }
            Some("assistant") => {
                let text =
                    archive::block_text(d.pointer("/message/content").unwrap_or(&Value::Null));
                if !text.trim().is_empty() {
                    s.last_text = Some(text);
                }
            }
            Some("result") => {
                s.result = d.get("result").and_then(Value::as_str).map(str::to_string);
                s.is_error = d.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                if s.is_error {
                    s.error = Some(s.result.clone().filter(|r| !r.is_empty()).unwrap_or_else(
                        || {
                            d.get("subtype")
                                .and_then(Value::as_str)
                                .unwrap_or("error")
                                .to_string()
                        },
                    ));
                }
                s.cost_usd = d.get("total_cost_usd").and_then(Value::as_f64);
                s.num_turns = d.get("num_turns").and_then(Value::as_u64);
                s.usage = d.get("usage").cloned();
                if new_id.is_none()
                    && let Some(id) = d.get("session_id").and_then(Value::as_str)
                {
                    s.session_id = Some(id.to_string());
                }
            }
            _ => {}
        }
    })
    .await;
    match result {
        Ok(_) => Ok(()),
        Err(claude_wrapper::Error::Timeout { .. }) => Err(Failure::Timeout),
        Err(claude_wrapper::Error::NotFound) => {
            Err(Failure::Spawn("claude binary not found".into()))
        }
        Err(e) => {
            let msg = e.to_string();
            if seen.lock().expect("seen").session_id.is_some() || new_id.is_some() {
                Err(Failure::Failed(msg))
            } else {
                Err(Failure::Spawn(msg))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_codex(
    ctx: &Ctx,
    opts: &AskOpts,
    resume: Option<&str>,
    cwd: Option<&str>,
    timeout_s: u64,
    depth: usize,
    seen: &Arc<Mutex<Seen>>,
    warnings: &mut Vec<String>,
) -> Result<(), Failure> {
    use codex_wrapper::{Codex, ExecCommand, ExecResumeCommand};

    // Options real callers pass to every backend in a fan-out. Codex cannot honour them,
    // and refusing the run would break the fan-out, so they are reported instead.
    let unsupported = [
        (!opts.allowed_tools.is_empty(), "allowed_tools"),
        (!opts.disallowed_tools.is_empty(), "disallowed_tools"),
        (opts.permission_mode.is_some(), "permission_mode"),
        (opts.max_turns.is_some(), "max_turns"),
        (opts.append_system_prompt.is_some(), "append_system_prompt"),
        (opts.hermetic, "hermetic"),
        (
            resume.is_some() && !opts.add_dirs.is_empty(),
            "add_dirs (codex exec resume has no --add-dir)",
        ),
    ];
    for (set, name) in unsupported {
        if set {
            warnings.push(format!(
                "{name} is not supported by codex and was not applied"
            ));
        }
    }
    let sandbox =
        parse_sandbox(opts.sandbox.as_deref().unwrap_or("read-only")).map_err(Failure::Spawn)?;
    let sandbox_name = opts.sandbox.clone().unwrap_or_else(|| "read-only".into());

    let mut builder = Codex::builder()
        .timeout(Duration::from_secs(timeout_s))
        .process_group(true)
        .env("TROVADOR_DEPTH", (depth + 1).to_string());
    if let Some(bin) = &ctx.paths.codex_bin {
        builder = builder.binary(bin);
    }
    if let Some(dir) = cwd {
        builder = builder.working_dir(dir);
    }
    let codex = builder.build().map_err(|e| Failure::Spawn(e.to_string()))?;
    let effort = opts
        .effort
        .as_ref()
        .map(|e| format!("model_reasoning_effort=\"{e}\""));

    let handler_seen = seen.clone();
    let is_new = resume.is_none();
    let handler = move |event: codex_wrapper::JsonLineEvent| {
        let mut s = handler_seen.lock().expect("seen");
        let x = &event.extra;
        match event.event_type.as_str() {
            // The id Codex minted for a new session, recorded the moment it appears.
            "thread.started" => {
                if let Some(id) = x.get("thread_id").and_then(Value::as_str) {
                    if is_new {
                        note_spawned(ctx, Backend::Codex, id, &mut s);
                    }
                    s.session_id = Some(id.to_string());
                }
            }
            "item.completed" => {
                let item = x.get("item").unwrap_or(&Value::Null);
                if item.get("type").and_then(Value::as_str) == Some("agent_message")
                    && let Some(t) = item.get("text").and_then(Value::as_str)
                {
                    s.result = Some(t.to_string());
                }
            }
            "turn.completed" => s.usage = x.get("usage").cloned(),
            "turn.failed" | "error" => {
                s.is_error = true;
                s.error = Some(
                    x.get("error")
                        .and_then(|e| e.get("message"))
                        .or_else(|| x.get("message"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| event.event_type.clone()),
                );
            }
            _ => {}
        }
    };

    let result = match resume {
        None => {
            let mut cmd = ExecCommand::from_stdin(opts.prompt.clone())
                .json()
                .skip_git_repo_check()
                .sandbox(sandbox);
            if let Some(m) = &opts.model {
                cmd = cmd.model(m);
            }
            if let Some(e) = &effort {
                cmd = cmd.config(e);
            }
            for d in &opts.add_dirs {
                cmd = cmd.add_dir(d);
            }
            cmd.stream(&codex, handler).await
        }
        Some(id) => {
            let mut cmd = ExecResumeCommand::from_stdin(opts.prompt.clone())
                .session_id(id)
                .json()
                .skip_git_repo_check()
                // exec resume has no --sandbox flag; the config key sets the same thing.
                .config(format!("sandbox_mode=\"{sandbox_name}\""));
            if let Some(m) = &opts.model {
                cmd = cmd.model(m);
            }
            if let Some(e) = &effort {
                cmd = cmd.config(e);
            }
            cmd.stream(&codex, handler).await
        }
    };
    match result {
        Ok(()) => Ok(()),
        Err(codex_wrapper::Error::Timeout { .. }) => Err(Failure::Timeout),
        Err(codex_wrapper::Error::NotFound) => Err(Failure::Spawn("codex binary not found".into())),
        Err(e) => {
            let msg = e.to_string();
            if seen.lock().expect("seen").session_id.is_some() {
                Err(Failure::Failed(msg))
            } else {
                Err(Failure::Spawn(msg))
            }
        }
    }
}

fn parse_sandbox(s: &str) -> Result<codex_wrapper::SandboxMode, String> {
    use codex_wrapper::SandboxMode;
    match s {
        "read-only" => Ok(SandboxMode::ReadOnly),
        "workspace-write" => Ok(SandboxMode::WorkspaceWrite),
        other => Err(format!(
            "sandbox \"{other}\" is not available through trovador; have: read-only, workspace-write"
        )),
    }
}

fn parse_effort(s: &str) -> Result<claude_wrapper::Effort, String> {
    use claude_wrapper::Effort;
    Ok(match s {
        "low" => Effort::Low,
        "medium" => Effort::Medium,
        "high" => Effort::High,
        "xhigh" => Effort::Xhigh,
        "max" => Effort::Max,
        other => {
            return Err(format!(
                "unknown effort \"{other}\"; have: low, medium, high, xhigh, max"
            ));
        }
    })
}

fn parse_permission_mode(s: &str) -> Result<claude_wrapper::PermissionMode, String> {
    use claude_wrapper::PermissionMode;
    Ok(match s {
        "default" => PermissionMode::Default,
        "acceptEdits" => PermissionMode::AcceptEdits,
        "plan" => PermissionMode::Plan,
        "dontAsk" => PermissionMode::DontAsk,
        "auto" => PermissionMode::Auto,
        // A run started over MCP that skips every permission check would let any client
        // of this server act with the operator's full authority.
        "bypassPermissions" => {
            return Err("bypassPermissions is not available through trovador".into());
        }
        other => {
            return Err(format!(
                "unknown permission_mode \"{other}\"; have: default, acceptEdits, plan, dontAsk, auto"
            ));
        }
    })
}

/// One Claude call whose session is not kept: no transcript is written, so it never shows
/// up in the archive or as something to continue. Used for summaries.
pub(crate) async fn oneshot(
    ctx: &Ctx,
    prompt: &str,
    model: &str,
    timeout: Duration,
) -> Result<(String, f64), String> {
    use claude_wrapper::{Claude, QueryCommand};

    let depth = depth();
    if depth >= ctx.limits.max_depth {
        return Err(format!(
            "spawn depth {depth} has reached TROVADOR_MAX_DEPTH={}",
            ctx.limits.max_depth
        ));
    }
    let mut builder = Claude::builder()
        .timeout(timeout)
        .process_group(true)
        .env("TROVADOR_DEPTH", (depth + 1).to_string());
    if let Some(bin) = &ctx.paths.claude_bin {
        builder = builder.binary(bin);
    }
    let claude = builder.build().map_err(|e| e.to_string())?;
    let cmd = QueryCommand::new(prompt.to_string())
        .prompt_via_stdin(true)
        .model(model)
        .no_session_persistence()
        .hermetic()
        // Reading the transcript in the prompt is the whole job; no tools are needed.
        .max_turns(1);
    let started = Instant::now();
    let result = cmd.execute_json(&claude).await;
    let outcome = match &result {
        Ok(r) if !r.is_error => "ok",
        Ok(_) => "error",
        Err(claude_wrapper::Error::Timeout { .. }) => "timeout",
        Err(_) => "failed",
    };
    log_run(
        ctx,
        json!({"phase": "end", "backend": "claude", "verb": "summarize",
        "outcome": outcome, "ms": started.elapsed().as_millis() as u64}),
    );
    match result {
        Ok(r) if r.is_error => Err(r.result),
        Ok(r) => Ok((r.result, r.cost_usd.unwrap_or(0.0))),
        Err(e) => Err(e.to_string()),
    }
}

/// One JSONL line per run start and end. Losing a line never affects the run.
fn log_run(ctx: &Ctx, mut line: Value) {
    if let Some(obj) = line.as_object_mut() {
        obj.insert(
            "at".into(),
            humantime::format_rfc3339_millis(std::time::SystemTime::now())
                .to_string()
                .into(),
        );
        obj.insert("pid".into(), std::process::id().into());
    }
    let _ = std::fs::create_dir_all(&ctx.paths.home);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ctx.paths.home.join("runs.jsonl"))
    {
        let _ = writeln!(f, "{line}");
    }
}

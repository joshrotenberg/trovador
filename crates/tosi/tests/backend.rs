#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use tosi::{
    backend::CliRunner,
    types::{AccessMode, Backend, Effort, RunSpec, Runner},
};

const CLAUDE_SUCCESS: &str = r#"{"type":"result","subtype":"success","result":"done","session_id":"claude-session","is_error":false,"total_cost_usd":0.01,"usage":{"input_tokens":5,"output_tokens":3}}"#;
const CODEX_SUCCESS: &str = concat!(
    "{\"type\":\"thread.started\",\"thread_id\":\"codex-thread\"}\n",
    "{\"type\":\"item.completed\",\"item\":{\"id\":\"item_0\",\"type\":\"agent_message\",\"text\":\"done\"}}\n",
    "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":5,\"cached_input_tokens\":0,\"output_tokens\":3}}\n"
);

struct FakeCli {
    dir: TempDir,
    runner: CliRunner,
}

impl FakeCli {
    fn new(backend: Backend, output: &str, hang: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("fake-cli");
        let script = format!(
            r#"#!/bin/sh
fixture=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf '%s\n' "$@" > "$fixture/args"
printf '%s\n' "$TOSI_CHILD" "$TASK_RUNNERS_CHILD" > "$fixture/child-guards"
previous=""
final_output=""
for argument in "$@"; do
    case "$previous" in
        --output-last-message) final_output="$argument" ;;
        --output-schema) cp "$argument" "$fixture/schema" ;;
    esac
    previous="$argument"
done
pwd > "$fixture/cwd"
cat > "$fixture/prompt"
printf '%s' "$$" > "$fixture/pid"
{}
"#,
            if hang {
                "exec /bin/sleep 60"
            } else {
                "if [ -n \"$final_output\" ] && [ -f \"$fixture/final\" ]; then cp \"$fixture/final\" \"$final_output\"; fi\ncat \"$fixture/response\"\nif [ -f \"$fixture/stderr\" ]; then cat \"$fixture/stderr\" >&2; fi\nif [ -f \"$fixture/exit-code\" ]; then exit \"$(cat \"$fixture/exit-code\")\"; fi"
            }
        );
        fs::write(&binary, script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(dir.path().join("response"), output).unwrap();
        if backend == Backend::Codex {
            // Model the provider's separate last-message file, not a concatenation
            // of intermediate assistant messages from its event stream.
            let last_message = output
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter(|event| event["type"] == "item.completed")
                .filter(|event| event["item"]["type"] == "agent_message")
                .filter_map(|event| event["item"]["text"].as_str().map(str::to_owned))
                .next_back();
            if let Some(text) = last_message {
                fs::write(dir.path().join("final"), text).unwrap();
            }
        }
        let runner = CliRunner::new().with_binary(backend, binary);
        Self { dir, runner }
    }

    fn with_failure(self, stderr: &str) -> Self {
        fs::write(self.dir.path().join("stderr"), stderr).unwrap();
        fs::write(self.dir.path().join("exit-code"), "1").unwrap();
        self
    }

    fn spec(&self, backend: Backend) -> RunSpec {
        RunSpec {
            backend,
            task: "Inspect the repo; literal $(echo do-not-run)".into(),
            cwd: self.dir.path().canonicalize().unwrap(),
            model: Some("test-model".into()),
            effort: None,
            session_id: None,
            fork: false,
            output_schema: None,
            timeout_secs: 10,
            access: AccessMode::ReadOnly,
        }
    }

    fn args(&self) -> Vec<String> {
        fs::read_to_string(self.dir.path().join("args"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"answer": {"type": "integer"}},
        "required": ["answer"],
        "additionalProperties": false
    })
}

fn has_pair(args: &[String], first: &str, second: &str) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == first && pair[1] == second)
}

#[tokio::test]
async fn both_worker_backends_propagate_new_and_legacy_child_guards() {
    for (backend, response) in [
        (Backend::Claude, CLAUDE_SUCCESS),
        (Backend::Codex, CODEX_SUCCESS),
    ] {
        let fake = FakeCli::new(backend, response, false);
        fake.runner
            .run(fake.spec(backend), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(fake.dir.path().join("child-guards")).unwrap(),
            "1\n1\n"
        );
    }
}

#[test]
fn binary_resolution_honors_overrides_without_launching_a_process() {
    for backend in [Backend::Claude, Backend::Codex] {
        let fake = FakeCli::new(backend, "", false);
        assert_eq!(
            fake.runner.resolved_binary(backend).unwrap(),
            fake.dir.path().join("fake-cli")
        );
        assert!(!fake.dir.path().join("args").exists());
        assert!(!fake.dir.path().join("pid").exists());
    }
}

#[tokio::test]
async fn typed_auth_failures_explain_the_worker_context_without_raw_provider_output() {
    let canary = "AUTH_SECRET_MUST_NOT_LEAK";
    for (backend, stderr, expected_kind) in [
        (
            Backend::Claude,
            format!("Not logged in. Please run /login. secret={canary}"),
            "not_authenticated",
        ),
        (Backend::Claude, String::new(), "not_authenticated"),
        (
            Backend::Codex,
            format!("401 Unauthorized: request data {canary}"),
            "auth",
        ),
    ] {
        let response = json!({
            "type": "result", "is_error": true,
            "result": "Not logged in. Please run /login.",
            "secret": canary,
            "payload": "large raw provider payload".repeat(200),
        })
        .to_string();
        let fake = FakeCli::new(backend, &response, false).with_failure(&stderr);
        let error = fake
            .runner
            .run(fake.spec(backend), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.contains("worker launch context"), "{error}");
        assert!(error.contains(expected_kind), "{error}");
        assert!(
            error.contains(fake.dir.path().join("fake-cli").to_str().unwrap()),
            "{error}"
        );
        assert!(error.contains("check_environment"), "{error}");
        assert!(error.contains("tosi doctor"), "{error}");
        assert!(error.contains("inherited sandbox"), "{error}");
        assert!(error.contains("normal terminal or host"), "{error}");
        assert!(!error.contains(canary), "{error}");
        assert!(!error.contains("raw provider payload"), "{error}");
        assert!(!error.contains("do-not-run"), "{error}");
        assert!(error.len() < 1024, "{error}");
    }
}

#[tokio::test]
async fn claude_rate_limits_do_not_suggest_missing_keychain_access() {
    let fake = FakeCli::new(Backend::Claude, "", false)
        .with_failure("Rate limit exceeded: AUTH_SECRET_MUST_NOT_LEAK");
    let error = fake
        .runner
        .run(fake.spec(Backend::Claude), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.contains("rate_limit"), "{error}");
    assert!(error.contains("usage or billing"), "{error}");
    assert!(!error.contains("Keychain"), "{error}");
    assert!(!error.contains("AUTH_SECRET_MUST_NOT_LEAK"), "{error}");
}

#[tokio::test]
async fn unrelated_provider_errors_are_not_reclassified_as_authentication() {
    for (backend, stderr) in [
        (Backend::Claude, "Unknown option --fictional-flag"),
        (Backend::Codex, "Error loading config.toml: malformed TOML"),
    ] {
        let fake = FakeCli::new(backend, "", false).with_failure(stderr);
        let error = fake
            .runner
            .run(fake.spec(backend), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.contains(stderr), "{error}");
        assert!(!error.contains("authentication is unavailable"), "{error}");
    }
}

#[tokio::test]
async fn explicit_effort_is_passed_unchanged_on_fresh_and_resumed_workers() {
    let claude_efforts = [
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::Xhigh,
        Effort::Max,
    ];
    let codex_efforts = [
        Effort::None,
        Effort::Minimal,
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::Xhigh,
        Effort::Max,
        Effort::Ultra,
    ];
    for (backend, response, efforts) in [
        (Backend::Claude, CLAUDE_SUCCESS, claude_efforts.as_slice()),
        (Backend::Codex, CODEX_SUCCESS, codex_efforts.as_slice()),
    ] {
        for &effort in efforts {
            for resume in [false, true] {
                let fake = FakeCli::new(backend, response, false);
                let mut spec = fake.spec(backend);
                spec.effort = Some(effort);
                spec.session_id = resume.then(|| "prior-session".into());
                let output = fake
                    .runner
                    .run(spec, CancellationToken::new())
                    .await
                    .unwrap();
                assert_eq!(output.text, "done");
                let args = fake.args();
                assert!(has_pair(&args, "--model", "test-model"));
                assert_eq!(args.iter().any(|arg| arg == "prior-session"), resume);
                match backend {
                    Backend::Claude => {
                        assert!(has_pair(&args, "--effort", effort.as_str()));
                        assert_eq!(args.iter().filter(|arg| *arg == "--effort").count(), 1);
                        assert_eq!(args.iter().any(|arg| arg == "--resume"), resume);
                    }
                    Backend::Codex => {
                        assert!(has_pair(
                            &args,
                            "-c",
                            &format!("model_reasoning_effort=\"{}\"", effort.as_str())
                        ));
                        assert_eq!(
                            args.iter()
                                .filter(|arg| arg.starts_with("model_reasoning_effort="))
                                .count(),
                            1
                        );
                        assert_eq!(args.iter().any(|arg| arg == "resume"), resume);
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn omitted_model_and_effort_add_no_cli_overrides_on_start_or_resume() {
    for (backend, response) in [
        (Backend::Claude, CLAUDE_SUCCESS),
        (Backend::Codex, CODEX_SUCCESS),
    ] {
        for resume in [false, true] {
            let fake = FakeCli::new(backend, response, false);
            let mut spec = fake.spec(backend);
            spec.model = None;
            spec.effort = None;
            spec.session_id = resume.then(|| "prior-session".into());
            fake.runner
                .run(spec, CancellationToken::new())
                .await
                .unwrap();
            let args = fake.args();
            assert!(!args.iter().any(|arg| arg == "--model" || arg == "--effort"));
            assert!(
                !args
                    .iter()
                    .any(|arg| arg.starts_with("model_reasoning_effort="))
            );
        }
    }
}

#[tokio::test]
async fn unsupported_claude_effort_is_rejected_before_start_or_resume_launches() {
    for effort in [Effort::None, Effort::Minimal, Effort::Ultra] {
        for resume in [false, true] {
            let fake = FakeCli::new(Backend::Claude, CLAUDE_SUCCESS, false);
            let mut spec = fake.spec(Backend::Claude);
            spec.effort = Some(effort);
            spec.session_id = resume.then(|| "prior-session".into());
            let error = fake
                .runner
                .run(spec, CancellationToken::new())
                .await
                .unwrap_err();
            assert!(error.contains(effort.as_str()), "{error}");
            assert!(!fake.dir.path().join("args").exists());
            assert!(!fake.dir.path().join("pid").exists());
        }
    }
}

#[tokio::test]
async fn known_model_effort_mismatches_are_rejected_before_worker_launch() {
    for (backend, model, effort, response) in [
        (
            Backend::Claude,
            "claude-opus-4-6",
            Effort::Xhigh,
            CLAUDE_SUCCESS,
        ),
        (Backend::Codex, "gpt-5.4", Effort::Max, CODEX_SUCCESS),
    ] {
        for resume in [false, true] {
            let fake = FakeCli::new(backend, response, false);
            let mut spec = fake.spec(backend);
            spec.model = Some(model.into());
            spec.effort = Some(effort);
            spec.session_id = resume.then(|| "prior-session".into());
            let error = fake
                .runner
                .run(spec, CancellationToken::new())
                .await
                .unwrap_err();
            assert!(error.contains(model), "{error}");
            assert!(error.contains(effort.as_str()), "{error}");
            assert!(!fake.dir.path().join("args").exists());
            assert!(!fake.dir.path().join("pid").exists());
        }
    }
}

#[tokio::test]
async fn claude_uses_explicit_tools_and_resumes_without_inherited_customizations() {
    for access in [AccessMode::ReadOnly, AccessMode::WorkspaceWrite] {
        let fake = FakeCli::new(Backend::Claude, CLAUDE_SUCCESS, false);
        let mut spec = fake.spec(Backend::Claude);
        spec.access = access;
        spec.session_id = Some("prior-session".into());
        let expected_prompt = spec.task.clone();
        let output = fake
            .runner
            .run(spec, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(output.text, "done");
        assert_eq!(output.session_id.as_deref(), Some("claude-session"));
        assert_eq!(output.usage["tokens"]["input_tokens"], 5);
        assert_eq!(output.usage["cost_usd"], 0.01);
        let args = fake.args();
        assert!(args.contains(&"--safe-mode".into()));
        assert!(args.contains(&"--strict-mcp-config".into()));
        assert!(args.contains(&"--disable-slash-commands".into()));
        assert!(has_pair(&args, "--mcp-config", r#"{"mcpServers":{}}"#));
        assert!(has_pair(&args, "--resume", "prior-session"));
        assert!(!args.contains(&"--fork-session".into()));
        assert!(has_pair(&args, "--model", "test-model"));
        assert!(has_pair(
            &args,
            "--permission-mode",
            if access == AccessMode::ReadOnly {
                "dontAsk"
            } else {
                "acceptEdits"
            }
        ));
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains("Bash") || arg.contains("bypass"))
        );
        assert_eq!(
            fs::read_to_string(fake.dir.path().join("prompt")).unwrap(),
            expected_prompt
        );
        assert!(!args.contains(&expected_prompt));
    }
}

#[tokio::test]
async fn a_claude_fork_resumes_the_session_into_a_new_one() {
    let fake = FakeCli::new(Backend::Claude, CLAUDE_SUCCESS, false);
    let mut spec = fake.spec(Backend::Claude);
    spec.session_id = Some("prior-session".into());
    spec.fork = true;
    fake.runner
        .run(spec, CancellationToken::new())
        .await
        .unwrap();
    let args = fake.args();
    assert!(has_pair(&args, "--resume", "prior-session"));
    assert!(args.contains(&"--fork-session".into()));
}

#[tokio::test]
async fn a_fork_with_no_reported_session_id_does_not_take_the_original() {
    let fake = FakeCli::new(
        Backend::Claude,
        &CLAUDE_SUCCESS.replace("claude-session", ""),
        false,
    );
    let mut spec = fake.spec(Backend::Claude);
    spec.session_id = Some("prior-session".into());
    spec.fork = true;
    let output = fake
        .runner
        .run(spec.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.session_id, None);
    // A plain resume still reports the session it resumed.
    spec.fork = false;
    let output = fake
        .runner
        .run(spec, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.session_id.as_deref(), Some("prior-session"));
}

#[tokio::test]
async fn codex_preserves_sandbox_policy_on_start_and_resume() {
    for access in [AccessMode::ReadOnly, AccessMode::WorkspaceWrite] {
        for resume in [false, true] {
            let fake = FakeCli::new(Backend::Codex, CODEX_SUCCESS, false);
            let mut spec = fake.spec(Backend::Codex);
            spec.access = access;
            spec.session_id = resume.then(|| "prior-thread".into());
            let expected_cwd = spec.cwd.clone();
            let expected_prompt = spec.task.clone();
            let output = fake
                .runner
                .run(spec, CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(output.text, "done");
            assert_eq!(output.session_id.as_deref(), Some("codex-thread"));
            assert_eq!(output.usage["tokens"]["output_tokens"], 3);
            let args = fake.args();
            assert!(has_pair(&args, "--disable", "plugins"));
            assert!(args.contains(&"--skip-git-repo-check".into()));
            assert!(args.contains(&"--ignore-user-config".into()));
            assert!(has_pair(&args, "-c", r#"approval_policy="never""#));
            assert!(has_pair(&args, "--model", "test-model"));
            let final_path = &args[args
                .iter()
                .position(|arg| arg == "--output-last-message")
                .unwrap()
                + 1];
            assert!(
                !Path::new(final_path).exists(),
                "per-run final output must be cleaned up"
            );
            let mode = if access == AccessMode::ReadOnly {
                "read-only"
            } else {
                "workspace-write"
            };
            if resume {
                assert!(args.contains(&"resume".into()));
                assert!(args.contains(&"prior-thread".into()));
                assert!(has_pair(&args, "-c", &format!("sandbox_mode=\"{mode}\"")));
            } else {
                assert!(has_pair(&args, "--sandbox", mode));
            }
            assert!(!args.iter().any(|arg| arg.contains("bypass")));
            assert_eq!(
                fs::read_to_string(fake.dir.path().join("cwd"))
                    .unwrap()
                    .trim(),
                expected_cwd.to_str().unwrap()
            );
            assert_eq!(
                fs::read_to_string(fake.dir.path().join("prompt")).unwrap(),
                expected_prompt
            );
            assert!(!args.contains(&expected_prompt));
        }
    }
}

#[tokio::test]
async fn claude_accepts_structured_only_output_and_passes_schema_on_start_and_resume() {
    let response = r#"{"type":"result","subtype":"success","result":"","session_id":"structured-session","is_error":false,"structured_output":{"answer":42}}"#;
    for resume in [false, true] {
        let fake = FakeCli::new(Backend::Claude, response, false);
        let mut spec = fake.spec(Backend::Claude);
        spec.session_id = resume.then(|| "prior-session".into());
        spec.output_schema = Some(output_schema());
        let result = fake
            .runner
            .run(spec, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.text, "");
        assert_eq!(result.structured_result, Some(json!({"answer": 42})));
        let args = fake.args();
        let schema_arg = &args[args.iter().position(|arg| arg == "--json-schema").unwrap() + 1];
        assert_eq!(
            serde_json::from_str::<Value>(schema_arg).unwrap(),
            output_schema()
        );
        assert_eq!(args.iter().any(|arg| arg == "--resume"), resume);
    }
}

#[tokio::test]
async fn codex_uses_schema_and_final_output_files_on_start_and_resume() {
    for resume in [false, true] {
        let fake = FakeCli::new(Backend::Codex, CODEX_SUCCESS, false);
        fs::write(fake.dir.path().join("final"), r#"{"answer":42}"#).unwrap();
        let mut spec = fake.spec(Backend::Codex);
        spec.session_id = resume.then(|| "prior-thread".into());
        spec.output_schema = Some(output_schema());
        let result = fake
            .runner
            .run(spec, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.text, r#"{"answer":42}"#);
        assert_eq!(result.structured_result, Some(json!({"answer": 42})));
        let args = fake.args();
        assert_eq!(args.iter().any(|arg| arg == "resume"), resume);
        assert_eq!(
            serde_json::from_str::<Value>(
                &fs::read_to_string(fake.dir.path().join("schema")).unwrap()
            )
            .unwrap(),
            output_schema()
        );
        for flag in ["--output-schema", "--output-last-message"] {
            let path = &args[args.iter().position(|arg| arg == flag).unwrap() + 1];
            assert!(
                !Path::new(path).exists(),
                "per-run {flag} file must be cleaned up"
            );
        }
    }
}

#[tokio::test]
async fn codex_returns_only_final_text_and_never_falls_back_to_event_narration() {
    let events = [
        json!({"type":"thread.started","thread_id":"codex-thread"}),
        json!({"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"I will inspect the repository."}}),
        json!({"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"The final answer."}}),
        json!({"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":3}}),
    ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    let fake = FakeCli::new(Backend::Codex, &events, false);
    let result = fake
        .runner
        .run(fake.spec(Backend::Codex), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.text, "The final answer.");
    assert_eq!(result.structured_result, None);

    fs::remove_file(fake.dir.path().join("final")).unwrap();
    let error = fake
        .runner
        .run(fake.spec(Backend::Codex), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.contains("final-message file"), "{error}");
}

#[tokio::test]
async fn codex_rejects_malformed_structured_output_and_oversized_final_files() {
    let fake = FakeCli::new(Backend::Codex, CODEX_SUCCESS, false);
    fs::write(fake.dir.path().join("final"), "not JSON").unwrap();
    let mut spec = fake.spec(Backend::Codex);
    spec.output_schema = Some(output_schema());
    let error = fake
        .runner
        .run(spec, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.contains("not valid structured JSON"), "{error}");

    fs::write(
        fake.dir.path().join("final"),
        vec![b'a'; 4 * 1024 * 1024 + 1],
    )
    .unwrap();
    let error = fake
        .runner
        .run(fake.spec(Backend::Codex), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.contains("exceeds 4194304 bytes"), "{error}");
}

#[tokio::test]
async fn zero_exit_provider_failure_or_missing_terminal_result_is_an_error() {
    let failures = [
        (Backend::Claude, r#"{"is_error":true,"result":"denied"}"#),
        (
            Backend::Claude,
            r#"{"subtype":"error_max_turns","result":"partial"}"#,
        ),
        (Backend::Claude, r#"{"is_error":false,"result":"  "}"#),
        (
            Backend::Codex,
            "{\"type\":\"turn.failed\",\"error\":{\"message\":\"denied\"}}\n",
        ),
        (
            Backend::Codex,
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"partial\"}}\n",
        ),
        (Backend::Codex, "{\"type\":\"turn.completed\"}\n"),
    ];
    for (backend, response) in failures {
        let fake = FakeCli::new(backend, response, false);
        assert!(
            fake.runner
                .run(fake.spec(backend), CancellationToken::new())
                .await
                .is_err(),
            "accepted failure: {response}"
        );
    }
}

async fn wait_for_pid(path: &Path) -> String {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(pid) = fs::read_to_string(path)
                && !pid.is_empty()
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

fn assert_reaped(pid: &str) {
    let result = Command::new("/bin/kill")
        .args(["-0", pid])
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "child {pid} is still alive after settled stop"
    );
}

#[tokio::test]
async fn cancellation_waits_for_provider_process_cleanup() {
    for backend in [Backend::Claude, Backend::Codex] {
        let fake = FakeCli::new(backend, "", true);
        let cancel = CancellationToken::new();
        let runner = fake.runner.clone();
        let spec = fake.spec(backend);
        let worker_cancel = cancel.clone();
        let run = tokio::spawn(async move { runner.run(spec, worker_cancel).await });
        let pid = wait_for_pid(&fake.dir.path().join("pid")).await;
        cancel.cancel();
        let error = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        assert_reaped(&pid);
    }
}

#[tokio::test]
async fn timeout_waits_for_provider_process_cleanup() {
    for backend in [Backend::Claude, Backend::Codex] {
        let fake = FakeCli::new(backend, "", true);
        let mut spec = fake.spec(backend);
        spec.timeout_secs = 1;
        let error = fake
            .runner
            .run(spec, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        let pid = fs::read_to_string(fake.dir.path().join("pid")).unwrap();
        assert_reaped(&pid);
    }
}

#[tokio::test]
async fn precancelled_task_does_not_start_a_process() {
    let fake = FakeCli::new(Backend::Claude, CLAUDE_SUCCESS, false);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        fake.runner
            .run(fake.spec(Backend::Claude), cancel)
            .await
            .is_err()
    );
    assert!(!fake.dir.path().join("pid").exists());
}

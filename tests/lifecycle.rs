use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tosi::{
    manager::{CancelTask, ContinueTask, Manager, SpawnTask, Status, WaitTasks},
    types::{Backend, Effort, RunOutput, RunSpec, Runner},
};

#[derive(Default)]
struct FakeRunner {
    calls: Mutex<Vec<RunSpec>>,
}

#[async_trait]
impl Runner for FakeRunner {
    async fn run(&self, spec: RunSpec, cancel: CancellationToken) -> Result<RunOutput, String> {
        self.calls.lock().await.push(spec.clone());
        if spec.task == "block" {
            cancel.cancelled().await;
            // Exercise the gap between cancellation request and actual provider cleanup.
            tokio::time::sleep(Duration::from_millis(30)).await;
            return Err("cancelled".into());
        }
        if spec.task == "fail" {
            return Err("provider rejected request".into());
        }
        if spec.task == "panic" {
            panic!("worker fault");
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
        Ok(RunOutput {
            text: spec.task,
            structured_result: None,
            session_id: Some(spec.session_id.unwrap_or_else(|| "provider-session".into())),
            usage: serde_json::json!({}),
        })
    }
}

fn request(task: &str) -> SpawnTask {
    SpawnTask {
        task: task.into(),
        cwd: Some(
            std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        ),
        session_id: None,
        fork: None,
        backend: None,
        model: None,
        effort: None,
        access: None,
        timeout_secs: None,
        output_schema: None,
    }
}

async fn wait(manager: &Manager, id: &str) -> tosi::manager::TaskSnapshot {
    manager
        .wait_tasks(WaitTasks {
            task_ids: vec![id.into()],
            timeout_ms: Some(1000),
        })
        .await
        .unwrap()
        .tasks
        .remove(0)
}

#[tokio::test]
async fn selection_distinguishes_requested_defaults_and_keeps_explicit_settings_on_resume() {
    for backend in [Backend::Claude, Backend::Codex] {
        let runner = Arc::new(FakeRunner::default());
        let manager = Manager::new(runner.clone(), backend, 1);
        let omitted = manager.spawn_task(request("omitted")).await.unwrap();
        assert_eq!(omitted.selection.requested.backend, None);
        assert_eq!(omitted.selection.selected.backend, backend);
        assert_eq!(omitted.effort, None);
        assert_eq!(omitted.selection.selected.effort, None);
        assert_eq!(omitted.selection.reported.model, None);
        assert_eq!(omitted.selection.reported.effort, None);
        let omitted = wait(&manager, &omitted.task_id).await;
        assert_eq!(omitted.selection.requested.model, None);
        assert_eq!(omitted.selection.selected.model, None);
        let mut input = request("explicit");
        input.backend = Some(backend);
        input.model = Some("test-model".into());
        input.effort = Some(Effort::High);
        let first = manager.spawn_task(input).await.unwrap();
        assert_eq!(first.selection.requested.backend, Some(backend));
        assert_eq!(
            first.selection.requested.model.as_deref(),
            Some("test-model")
        );
        assert_eq!(first.selection.requested.effort, Some(Effort::High));
        let first = wait(&manager, &first.task_id).await;
        let next = manager
            .continue_task(ContinueTask {
                task_id: first.task_id.clone(),
                message: "follow-up".into(),
            })
            .await
            .unwrap();
        let next = wait(&manager, &next.task_id).await;
        assert_eq!(first.selection, next.selection);
        assert_eq!(next.effort, Some(Effort::High));
        assert_eq!(next.model.as_deref(), Some("test-model"));
        let calls = runner.calls.lock().await;
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[2].model, calls[1].model);
        assert_eq!(calls[2].effort, calls[1].effort);
        assert_eq!(calls[2].backend, calls[1].backend);
    }
}

#[tokio::test]
async fn invalid_effort_is_rejected_before_admission_or_runner_invocation() {
    let runner = Arc::new(FakeRunner::default());
    let manager = Manager::new(runner.clone(), Backend::Claude, 1);
    for (backend, model, effort) in [
        (Backend::Claude, "opus", Effort::Minimal),
        (Backend::Claude, "claude-opus-4-6", Effort::Xhigh),
        (Backend::Codex, "gpt-5.4", Effort::Max),
    ] {
        let mut input = request("should not run");
        input.backend = Some(backend);
        input.model = Some(model.into());
        input.effort = Some(effort);
        assert!(
            manager
                .spawn_task(input)
                .await
                .unwrap_err()
                .contains("unsupported")
        );
    }
    assert!(runner.calls.lock().await.is_empty());
    let valid = manager
        .spawn_task(request("still have capacity"))
        .await
        .unwrap();
    assert_eq!(
        wait(&manager, &valid.task_id).await.status,
        Status::Completed
    );
}

#[test]
fn effort_json_distinguishes_null_from_none_and_rejects_unknown_values() {
    for (value, expected) in [
        (Value::Null, None),
        (json!("none"), Some(Effort::None)),
        (json!("xhigh"), Some(Effort::Xhigh)),
    ] {
        let input: SpawnTask =
            serde_json::from_value(json!({"task":"a", "cwd":"/tmp", "effort":value})).unwrap();
        assert_eq!(input.effort, expected);
    }
    assert!(
        serde_json::from_value::<SpawnTask>(json!({"task":"a", "cwd":"/tmp", "effort":"typo"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<ContinueTask>(
            json!({"task_id":"x", "message":"a", "effort":"high"})
        )
        .is_err()
    );
}

#[tokio::test]
async fn parallel_runs_return_first_completion_without_cancelling_others() {
    let manager = Manager::new(Arc::new(FakeRunner::default()), Backend::Claude, 2);
    let slow = manager.spawn_task(request("block")).await.unwrap();
    let fast = manager.spawn_task(request("done")).await.unwrap();
    assert!(manager.spawn_task(request("over capacity")).await.is_err());
    let result = manager
        .wait_tasks(WaitTasks {
            task_ids: vec![slow.task_id.clone(), fast.task_id],
            timeout_ms: Some(1000),
        })
        .await
        .unwrap();
    assert!(!result.timed_out);
    assert_eq!(result.tasks[0].status, Status::Running);
    assert_eq!(result.tasks[1].status, Status::Completed);
    let snapshot = manager
        .wait_tasks(WaitTasks {
            task_ids: vec![slow.task_id.clone()],
            timeout_ms: Some(0),
        })
        .await
        .unwrap();
    assert!(snapshot.timed_out);
    assert_eq!(snapshot.tasks[0].status, Status::Running);
    manager.shutdown().await;
    assert_eq!(
        wait(&manager, &slow.task_id).await.status,
        Status::Cancelled
    );
}

#[tokio::test]
async fn follow_up_resumes_same_provider_but_keeps_separate_result_and_rejects_stale_parent() {
    let runner = Arc::new(FakeRunner::default());
    let manager = Manager::new(runner.clone(), Backend::Codex, 4);
    let first = manager.spawn_task(request("original")).await.unwrap();
    let first = wait(&manager, &first.task_id).await;
    let next = manager
        .continue_task(ContinueTask {
            task_id: first.task_id.clone(),
            message: "follow-up".into(),
        })
        .await
        .unwrap();
    assert_ne!(first.task_id, next.task_id);
    assert_eq!(first.conversation_id, next.conversation_id);
    assert_eq!(next.parent_task_id.as_deref(), Some(first.task_id.as_str()));
    assert!(
        manager
            .continue_task(ContinueTask {
                task_id: first.task_id.clone(),
                message: "overlapping".into()
            })
            .await
            .is_err()
    );
    assert_eq!(
        wait(&manager, &next.task_id).await.result.as_deref(),
        Some("follow-up")
    );
    assert_eq!(
        wait(&manager, &first.task_id).await.result.as_deref(),
        Some("original")
    );
    assert!(
        manager
            .continue_task(ContinueTask {
                task_id: first.task_id,
                message: "stale".into()
            })
            .await
            .unwrap_err()
            .contains("latest")
    );
    let calls = runner.calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].session_id.as_deref(), Some("provider-session"));
    assert_eq!(calls[1].backend, Backend::Codex);
    assert_eq!(calls[0].cwd, calls[1].cwd);
}

#[tokio::test]
async fn cancellation_waits_for_cleanup_and_is_idempotent() {
    let manager = Manager::new(Arc::new(FakeRunner::default()), Backend::Claude, 4);
    let task = manager.spawn_task(request("block")).await.unwrap();
    let before = tokio::time::Instant::now();
    let result = manager
        .cancel_task(CancelTask {
            task_id: task.task_id.clone(),
        })
        .await
        .unwrap();
    assert!(before.elapsed() >= Duration::from_millis(30));
    assert_eq!(result.status, Status::Cancelled);
    assert!(result.finished_at_ms.is_some());
    let again = manager
        .cancel_task(CancelTask {
            task_id: task.task_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(again.finished_at_ms, result.finished_at_ms);
    assert!(
        manager
            .continue_task(ContinueTask {
                task_id: task.task_id,
                message: "resume cancelled".into()
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn failed_and_panicked_workers_become_terminal_and_release_capacity() {
    let manager = Manager::new(Arc::new(FakeRunner::default()), Backend::Claude, 1);
    for prompt in ["fail", "panic"] {
        let task = manager.spawn_task(request(prompt)).await.unwrap();
        let result = wait(&manager, &task.task_id).await;
        assert_eq!(result.status, Status::Failed);
        assert!(result.error.is_some());
        assert!(
            manager
                .continue_task(ContinueTask {
                    task_id: task.task_id,
                    message: "again".into()
                })
                .await
                .is_err()
        );
    }
    let task = manager.spawn_task(request("recovered")).await.unwrap();
    assert_eq!(
        wait(&manager, &task.task_id).await.status,
        Status::Completed
    );
}

#[tokio::test]
async fn bad_input_does_not_start_work_and_shutdown_rejects_new_tasks() {
    let runner = Arc::new(FakeRunner::default());
    let manager = Manager::new(runner.clone(), Backend::Claude, 4);
    assert!(manager.spawn_task(request(" ")).await.is_err());
    let mut bad = request("test");
    bad.cwd = Some(".".into());
    assert!(manager.spawn_task(bad).await.is_err());
    let mut bad = request("test");
    bad.timeout_secs = Some(3601);
    assert!(manager.spawn_task(bad).await.is_err());
    assert!(
        manager
            .wait_tasks(WaitTasks {
                task_ids: vec!["missing".into()],
                timeout_ms: Some(0)
            })
            .await
            .is_err()
    );
    assert!(runner.calls.lock().await.is_empty());
    manager.shutdown().await;
    assert!(manager.spawn_task(request("closed")).await.is_err());
}

struct StructuredRunner {
    calls: Mutex<Vec<RunSpec>>,
    result: Option<Value>,
}

impl StructuredRunner {
    fn new(result: Option<Value>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            result,
        }
    }
}

#[async_trait]
impl Runner for StructuredRunner {
    async fn run(&self, spec: RunSpec, _cancel: CancellationToken) -> Result<RunOutput, String> {
        self.calls.lock().await.push(spec.clone());
        Ok(RunOutput {
            text: "provider produced structured data".into(),
            structured_result: self.result.clone(),
            session_id: Some(
                spec.session_id
                    .unwrap_or_else(|| "structured-session".into()),
            ),
            usage: json!({}),
        })
    }
}

fn answer_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
        "additionalProperties": false
    })
}

#[tokio::test]
async fn structured_results_are_preserved_and_follow_up_inherits_the_schema() {
    let answer = json!({"answer": "remembered"});
    let runner = Arc::new(StructuredRunner::new(Some(answer.clone())));
    let manager = Manager::new(runner.clone(), Backend::Codex, 1);
    let schema = answer_schema();
    let mut input = request("first structured turn");
    input.output_schema = Some(schema.clone());
    let first = manager.spawn_task(input).await.unwrap();
    let first = wait(&manager, &first.task_id).await;
    assert_eq!(first.status, Status::Completed);
    assert_eq!(first.structured_result, Some(answer.clone()));
    assert!(!first.result_truncated);

    let next = manager
        .continue_task(ContinueTask {
            task_id: first.task_id.clone(),
            message: "continue with the same format".into(),
        })
        .await
        .unwrap();
    let next = wait(&manager, &next.task_id).await;
    assert_eq!(next.status, Status::Completed);
    assert_eq!(next.structured_result, Some(answer.clone()));
    assert!(!next.result_truncated);
    assert_ne!(first.task_id, next.task_id);
    assert_eq!(first.conversation_id, next.conversation_id);
    assert_eq!(
        wait(&manager, &first.task_id).await.structured_result,
        Some(answer)
    );
    let calls = runner.calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].output_schema, Some(schema.clone()));
    assert_eq!(calls[1].output_schema, Some(schema));
    assert_eq!(calls[1].session_id.as_deref(), Some("structured-session"));
}

#[tokio::test]
async fn invalid_or_oversized_schema_is_rejected_before_starting_a_runner() {
    let runner = Arc::new(StructuredRunner::new(Some(json!({"answer": "ok"}))));
    let manager = Manager::new(runner.clone(), Backend::Claude, 1);
    let invalid_schemas = [
        json!(true),
        json!({"type": "array", "items": {"type": "string"}}),
        json!({"properties": {"answer": {"type": "string"}}}),
        json!({"type": "object", "properties": {"answer": {"type": 17}}}),
        json!({"type": "object", "description": "x".repeat(64_001)}),
    ];
    for schema in invalid_schemas {
        let mut input = request("must not start");
        input.output_schema = Some(schema);
        assert!(manager.spawn_task(input).await.is_err());
    }
    assert!(runner.calls.lock().await.is_empty());

    // Invalid admissions must not consume the only active-task slot.
    let mut valid = request("valid admission");
    valid.output_schema = Some(answer_schema());
    let task = manager.spawn_task(valid).await.unwrap();
    assert_eq!(
        wait(&manager, &task.task_id).await.status,
        Status::Completed
    );
    assert_eq!(runner.calls.lock().await.len(), 1);
}

#[tokio::test]
async fn missing_mismatched_or_oversized_structured_results_fail_without_truncation() {
    for output in [
        None,
        Some(json!({"answer": 42})),
        Some(json!({"answer": "x".repeat(64_001)})),
    ] {
        let runner = Arc::new(StructuredRunner::new(output));
        let manager = Manager::new(runner, Backend::Claude, 1);
        let mut input = request("invalid structured result");
        input.output_schema = Some(answer_schema());
        let task = manager.spawn_task(input).await.unwrap();
        let result = wait(&manager, &task.task_id).await;
        assert_eq!(result.status, Status::Failed);
        assert!(result.error.is_some());
        assert_eq!(result.structured_result, None);
        assert!(!result.result_truncated);
        assert!(
            manager
                .continue_task(ContinueTask {
                    task_id: task.task_id,
                    message: "cannot continue a failed result".into(),
                })
                .await
                .is_err()
        );
    }
}

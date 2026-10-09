use super::*;
use crate::registry::{ListTasks, Registry};
use async_trait::async_trait;
use tokio::sync::Semaphore;

const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890";
struct FakeRunner {
    release: Semaphore,
    calls: Mutex<Vec<RunSpec>>,
}
#[async_trait]
impl Runner for FakeRunner {
    async fn run(&self, spec: RunSpec, cancel: CancellationToken) -> Result<RunOutput, String> {
        self.calls.lock().await.push(spec.clone());
        if spec.task == "gated" {
            tokio::select! {
                permit = self.release.acquire() => permit.unwrap().forget(),
                _ = cancel.cancelled() => return Err("cancelled".into()),
            }
        }
        if let Some(error) = spec.task.strip_prefix("error:") {
            return Err(error.into());
        }
        Ok(RunOutput {
            text: spec.task,
            structured_result: None,
            session_id: Some(spec.session_id.unwrap_or_else(|| "saved-session".into())),
            usage: serde_json::json!({}),
        })
    }
}
fn fixture(
    cap: usize,
) -> (
    tempfile::TempDir,
    Manager,
    Arc<FakeRunner>,
    bottega_core::state::State,
) {
    let root = tempfile::tempdir().unwrap();
    let layout = bottega_core::state::prepare(bottega_core::state::resolve(
        Some(root.path().join("state").into_os_string()),
        None,
        root.path(),
    ))
    .unwrap();
    assert!(layout.tosi.starts_with(root.path()));
    let runner = Arc::new(FakeRunner {
        release: Semaphore::new(0),
        calls: Mutex::new(Vec::new()),
    });
    let manager = Manager::new(runner.clone(), Backend::Claude, 4)
        .with_registry(Registry::open_with_retention(&layout, 500, 30).unwrap())
        .with_memory_cap(cap)
        .with_redaction(false);
    (root, manager, runner, layout)
}
fn request(root: &std::path::Path, task: &str) -> SpawnTask {
    SpawnTask {
        task: task.into(),
        cwd: root.canonicalize().unwrap().to_string_lossy().into_owned(),
        backend: None,
        model: None,
        effort: None,
        output_schema: None,
        access: None,
        timeout_secs: None,
    }
}
async fn wait(manager: &Manager, id: &str) -> TaskSnapshot {
    let result = manager
        .wait_tasks(WaitTasks {
            task_ids: vec![id.into()],
            timeout_ms: Some(1000),
        })
        .await
        .unwrap();
    assert!(result.errors.is_empty());
    assert!(result.tasks[0].status.is_terminal());
    result.tasks.into_iter().next().unwrap()
}
fn block_save(layout: &bottega_core::state::State, id: &str) -> PathBuf {
    let path = layout
        .tosi
        .join("tasks")
        .join(format!("{id}.json.{}.tmp", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    path
}
#[tokio::test]
async fn memory_cap_evicts_persisted_terminal_tasks_and_resumes_evicted_owner_tasks() {
    let (root, manager, runner, _) = fixture(1);
    let first = manager
        .spawn_task(request(root.path(), SECRET))
        .await
        .unwrap();
    assert_eq!(
        wait(&manager, &first.task_id).await.result.as_deref(),
        Some(SECRET)
    );
    {
        let state = manager.state.lock().await;
        assert_eq!(state.tasks.len(), 1);
        assert!(!state.tasks[&first.task_id].dirty);
        assert!(state.tasks[&first.task_id].disk_result.is_none());
    }
    let second = manager
        .spawn_task(request(root.path(), "second"))
        .await
        .unwrap();
    wait(&manager, &second.task_id).await;
    {
        let state = manager.state.lock().await;
        assert_eq!(state.tasks.len(), 1);
        assert!(!state.tasks.contains_key(&first.task_id));
        assert!(!state.conversations.contains_key(&first.conversation_id));
    }
    let evicted = wait(&manager, &first.task_id).await;
    assert!(evicted.result.unwrap().contains("[redacted:"));
    let cancelled = manager
        .cancel_task(CancelTask {
            task_id: first.task_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(cancelled.status, Status::Completed);
    assert!(!cancelled.result.unwrap().contains(SECRET));
    let listed = manager.list_tasks(ListTasks::default()).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed
            .iter()
            .map(|t| &t.task_id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2
    );
    let continued = manager
        .continue_task(ContinueTask {
            task_id: first.task_id.clone(),
            message: "follow up".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        continued.parent_task_id.as_deref(),
        Some(first.task_id.as_str())
    );
    wait(&manager, &continued.task_id).await;
    assert_eq!(
        runner
            .calls
            .lock()
            .await
            .last()
            .unwrap()
            .session_id
            .as_deref(),
        Some("saved-session")
    );
    assert_eq!(manager.state.lock().await.tasks.len(), 1);
    assert!(
        manager
            .continue_task(ContinueTask {
                task_id: first.task_id.clone(),
                message: "stale parent".into()
            })
            .await
            .is_err()
    );
    let state = manager.state.lock().await;
    assert!(state.tasks.len() <= 1);
    assert!(!state.tasks.contains_key(&first.task_id));
    assert!(state.tasks.values().all(|e| e.owned));
    drop(state);
    manager.shutdown().await;
}
#[tokio::test]
async fn unsaved_terminal_entries_are_protected_and_list_retries_every_dirty_entry() {
    let (root, manager, runner, layout) = fixture(2);
    let first = manager
        .spawn_task(request(root.path(), "gated"))
        .await
        .unwrap();
    let second = manager
        .spawn_task(request(root.path(), "gated"))
        .await
        .unwrap();
    let blocked = [
        block_save(&layout, &first.task_id),
        block_save(&layout, &second.task_id),
    ];
    runner.release.add_permits(2);
    wait(&manager, &first.task_id).await;
    wait(&manager, &second.task_id).await;
    assert!(
        manager
            .spawn_task(request(root.path(), "would evict unsaved data"))
            .await
            .unwrap_err()
            .contains("memory task limit")
    );
    assert_eq!(manager.state.lock().await.tasks.len(), 2);
    for id in [&first.task_id, &second.task_id] {
        assert!(manager.state.lock().await.tasks[id].dirty);
        assert!(manager.state.lock().await.tasks[id].disk_result.is_some());
    }
    for path in blocked {
        std::fs::remove_dir(path).unwrap();
    }
    manager
        .list_tasks(ListTasks {
            status: Some(Status::Completed),
            ..Default::default()
        })
        .await
        .unwrap();
    for id in [&first.task_id, &second.task_id] {
        let record: serde_json::Value = serde_json::from_slice(
            &std::fs::read(layout.tosi.join("tasks").join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(record["status"], "completed");
        let state = manager.state.lock().await;
        assert!(!state.tasks[id].dirty);
        assert!(state.tasks[id].disk_result.is_none());
    }
    let next = manager
        .spawn_task(request(root.path(), "now fits"))
        .await
        .unwrap();
    wait(&manager, &next.task_id).await;
    assert_eq!(manager.state.lock().await.tasks.len(), 2);
    manager.shutdown().await;
}
#[tokio::test]
async fn maintenance_retries_failed_terminal_saves_without_task_id_polls() {
    let (root, manager, runner, layout) = fixture(1);
    let task = manager
        .spawn_task(request(root.path(), "gated"))
        .await
        .unwrap();
    let blocked = block_save(&layout, &task.task_id);
    runner.release.add_permits(1);
    wait(&manager, &task.task_id).await;
    assert!(manager.state.lock().await.tasks[&task.task_id].dirty);
    std::fs::remove_dir(blocked).unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !manager.state.lock().await.tasks[&task.task_id].dirty {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            layout
                .tosi
                .join("tasks")
                .join(format!("{}.json", task.task_id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(record["status"], "completed");
    assert!(
        manager.state.lock().await.tasks[&task.task_id]
            .disk_result
            .is_none()
    );
    manager.shutdown().await;
}
#[tokio::test]
async fn opted_out_errors_are_redacted_before_disk_truncation() {
    let (root, manager, _, layout) = fixture(1);
    let prompt = format!("error:{}{SECRET} tail", "x".repeat(64_000 - 20));
    let task = manager
        .spawn_task(request(root.path(), &prompt))
        .await
        .unwrap();
    let result = wait(&manager, &task.task_id).await;
    assert_eq!(result.status, Status::Failed);
    assert!(result.error.as_deref().unwrap().contains("sk-ant-"));
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            layout
                .tosi
                .join("tasks")
                .join(format!("{}.json", task.task_id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(!record["error"].as_str().unwrap().contains("sk-ant-"));
    assert!(record["error"].as_str().unwrap().contains("[redacted:"));
    assert!(
        manager.state.lock().await.tasks[&task.task_id]
            .disk_error
            .is_none()
    );
    let next = manager
        .spawn_task(request(root.path(), "evict failure"))
        .await
        .unwrap();
    wait(&manager, &next.task_id).await;
    assert!(
        manager
            .continue_task(ContinueTask {
                task_id: task.task_id.clone(),
                message: "cannot continue failure".into()
            })
            .await
            .is_err()
    );
    assert_eq!(manager.state.lock().await.tasks.len(), 1);
    assert!(!manager.state.lock().await.tasks.contains_key(&task.task_id));
    manager.shutdown().await;
}

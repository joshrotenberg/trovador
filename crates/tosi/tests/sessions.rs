//! The sessions tosi starts are recorded in trovador's provenance record, so trovador's list
//! and search include them although they are automated sessions.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tosi::{
    manager::{ContinueTask, Manager, Sessions, SpawnTask, Status, WaitTasks},
    types::{Backend, RunOutput, RunSpec, Runner},
};
use trovador::provenance;

const NEW_ID: &str = "bbbbbbbb-0000-4000-8000-000000000004";

/// Reports `NEW_ID` for a new session, and the resumed id for a follow-up.
#[derive(Default)]
struct FakeRunner {
    calls: Mutex<Vec<RunSpec>>,
}

#[async_trait]
impl Runner for FakeRunner {
    async fn run(&self, spec: RunSpec, _cancel: CancellationToken) -> Result<RunOutput, String> {
        self.calls.lock().await.push(spec.clone());
        Ok(RunOutput {
            text: "done".into(),
            structured_result: None,
            session_id: Some(spec.session_id.unwrap_or_else(|| NEW_ID.into())),
            usage: json!({}),
        })
    }
}

fn request(cwd: &std::path::Path) -> SpawnTask {
    SpawnTask {
        task: "next step".into(),
        cwd: cwd.to_string_lossy().into_owned(),
        backend: Some(Backend::Codex),
        model: None,
        effort: None,
        output_schema: None,
        access: None,
        timeout_secs: None,
    }
}

async fn finished(m: &Manager, id: &str) -> tosi::manager::TaskSnapshot {
    for _ in 0..50 {
        let t = m
            .wait_tasks(WaitTasks {
                task_ids: vec![id.into()],
                timeout_ms: Some(1000),
            })
            .await
            .unwrap()
            .tasks
            .remove(0);
        if t.status.is_terminal() {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("task {id} did not finish");
}

#[tokio::test]
async fn a_session_tosi_starts_is_recorded_and_its_follow_ups_continue_it() {
    let dir = tempfile::tempdir().unwrap();
    let paths = trovador::paths::Paths::under(dir.path());
    let runner = Arc::new(FakeRunner::default());
    let m = Manager::new(runner.clone(), Backend::Claude, 4).with_sessions(Sessions {
        paths: paths.clone(),
    });
    let t = m.spawn_task(request(dir.path())).await.unwrap();
    assert_eq!(finished(&m, &t.task_id).await.status, Status::Completed);
    assert!(provenance::is_spawned(
        &paths,
        trovador::Backend::Codex,
        NEW_ID
    ));

    let next = m
        .continue_task(ContinueTask {
            task_id: t.task_id,
            message: "and then".into(),
        })
        .await
        .unwrap();
    assert_eq!(finished(&m, &next.task_id).await.status, Status::Completed);
    assert_eq!(
        runner.calls.lock().await[1].session_id.as_deref(),
        Some(NEW_ID)
    );
    assert_eq!(provenance::spawned_keys(&paths).len(), 1);
}

#[tokio::test]
async fn without_a_provenance_record_tasks_still_run() {
    let dir = tempfile::tempdir().unwrap();
    let m = Manager::new(Arc::new(FakeRunner::default()), Backend::Claude, 4);
    let t = m.spawn_task(request(dir.path())).await.unwrap();
    assert_eq!(finished(&m, &t.task_id).await.status, Status::Completed);
}

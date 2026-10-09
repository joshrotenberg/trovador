//! The sessions tosi starts are recorded in trovador's provenance record, so trovador's list
//! and search include them although they are automated sessions.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use bottega_core::provenance;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tosi::{
    manager::{ContinueTask, Manager, Sessions, SpawnTask, Status, WaitTasks},
    types::{Backend, RunOutput, RunSpec, Runner},
};

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
    let state =
        bottega_core::state::prepare(bottega_core::state::resolve(None, None, dir.path())).unwrap();
    let paths = state.shared;
    let runner = Arc::new(FakeRunner::default());
    let m = Manager::new(runner.clone(), Backend::Claude, 4).with_sessions(Sessions {
        paths: paths.clone(),
    });
    let t = m.spawn_task(request(dir.path())).await.unwrap();
    assert_eq!(finished(&m, &t.task_id).await.status, Status::Completed);
    assert!(provenance::is_spawned(
        &paths,
        bottega_core::Backend::Codex,
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

#[test]
fn both_services_resolve_the_same_provenance_under_the_same_environment() {
    const CHILD: &str = "BOTTEGA_SHARED_PATH_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        let state = bottega_core::state::from_env();
        let paths = trovador::Paths::from_env();
        assert!(!root.join(".bottega").exists());
        let state = bottega_core::state::prepare(state).unwrap();
        let sessions = Sessions::from_prepared(&state);
        assert_eq!(
            provenance::path(&sessions.paths),
            trovador::provenance::path(&paths)
        );
        assert_eq!(
            provenance::path(&sessions.paths),
            root.join(".bottega/shared/spawned.json")
        );
        assert!(root.join(".bottega").exists());
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "both_services_resolve_the_same_provenance_under_the_same_environment",
        ])
        .env(CHILD, temp.path())
        .env("BOTTEGA_TEST_ROOT", temp.path())
        .env("BOTTEGA_HOME", temp.path().join(".bottega"))
        .env("TROVADOR_HOME", temp.path().join("service"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn sessions_use_the_prepared_location_after_a_permanent_migration_failure() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join(".trovador");
    std::fs::create_dir(&old).unwrap();
    std::os::unix::fs::symlink("missing", old.join("unsupported-link")).unwrap();
    let state = bottega_core::state::prepare_with_copy(
        bottega_core::state::resolve(None, None, temp.path()),
        bottega_core::state::copy_tree,
    )
    .unwrap();
    assert_eq!(state.shared, old);
    assert!(state.warning.is_some());
    let sessions = Sessions::from_prepared(&state);
    assert_eq!(
        provenance::path(&sessions.paths),
        provenance::path(&state.shared)
    );
    assert_eq!(sessions.paths, old);
}

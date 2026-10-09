//! Continuing and forking existing sessions, decided by trovador's archive and consent rules.

use std::{path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tosi::{
    manager::{ContinueTask, Manager, Sessions, SpawnTask, Status, WaitTasks},
    types::{Backend, RunOutput, RunSpec, Runner},
};
use trovador::consent::{self, Decision, Level};

const CLAUDE_ID: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const OTHER_ID: &str = "abbbbbbb-0000-4000-8000-000000000002";
const CODEX_ID: &str = "cccccccc-0000-4000-8000-000000000003";
const NEW_ID: &str = "bbbbbbbb-0000-4000-8000-000000000004";

/// Returns `NEW_ID` for a new session or a fork, and the resumed id otherwise. With
/// `echo_fork`, a fork reports the id it resumed, as a provider that ignored the fork would.
#[derive(Default)]
struct FakeRunner {
    calls: Mutex<Vec<RunSpec>>,
    echo_fork: bool,
}

#[async_trait]
impl Runner for FakeRunner {
    async fn run(&self, spec: RunSpec, _cancel: CancellationToken) -> Result<RunOutput, String> {
        self.calls.lock().await.push(spec.clone());
        let id = match (&spec.session_id, spec.fork) {
            (Some(id), false) => id.clone(),
            (Some(id), true) if self.echo_fork => id.clone(),
            _ => NEW_ID.into(),
        };
        Ok(RunOutput {
            text: "done".into(),
            structured_result: None,
            session_id: Some(id),
            usage: json!({}),
        })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    sessions: Sessions,
    project: std::path::PathBuf,
}

fn claude_session(sessions: &Sessions, id: &str, project: &Path) {
    let dir = sessions.paths.claude_archive.join("-project");
    std::fs::create_dir_all(&dir).unwrap();
    let record = json!({"type": "user", "cwd": project, "entrypoint": "cli",
        "message": {"role": "user", "content": "earlier work"}});
    std::fs::write(dir.join(format!("{id}.jsonl")), format!("{record}\n")).unwrap();
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let sessions = Sessions {
        paths: trovador::paths::Paths::under(dir.path()),
        limits: trovador::limits::Limits::default(),
    };
    claude_session(&sessions, CLAUDE_ID, &project);
    claude_session(&sessions, OTHER_ID, &project);
    let codex = sessions.paths.codex_archive.join("2026/10/08");
    std::fs::create_dir_all(&codex).unwrap();
    let meta = json!({"type": "session_meta", "payload": {"cwd": project, "source": "cli"}});
    std::fs::write(
        codex.join(format!("rollout-2026-10-08T00-00-00-{CODEX_ID}.jsonl")),
        format!("{meta}\n"),
    )
    .unwrap();
    Fixture {
        _dir: dir,
        sessions,
        project,
    }
}

fn manager(f: &Fixture) -> (Manager, Arc<FakeRunner>) {
    let runner = Arc::new(FakeRunner::default());
    let m = Manager::new(runner.clone(), Backend::Claude, 4).with_sessions(f.sessions.clone());
    (m, runner)
}

fn request(session_id: Option<&str>, fork: bool) -> SpawnTask {
    SpawnTask {
        task: "next step".into(),
        cwd: None,
        session_id: session_id.map(str::to_string),
        fork: fork.then_some(true),
        backend: None,
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
async fn continuing_a_session_needs_consent_and_runs_in_its_project() {
    let f = fixture();
    let (m, runner) = manager(&f);
    let err = m.spawn_task(request(Some("a"), false)).await.unwrap_err();
    assert!(err.contains("matches 2 sessions"), "{err}");

    let err = m
        .spawn_task(request(Some(CLAUDE_ID), false))
        .await
        .unwrap_err();
    assert!(err.contains("continuation denied"), "{err}");
    assert!(err.contains("trovador consent approve"), "{err}");
    assert!(runner.calls.lock().await.is_empty(), "nothing ran");

    consent::set_level(&f.sessions.paths, Level::Approved).unwrap();
    consent::set_decision(
        &f.sessions.paths,
        trovador::Backend::Claude,
        CLAUDE_ID,
        Some(Decision::Approved),
    )
    .unwrap();
    let t = m
        .spawn_task(request(Some(&CLAUDE_ID[..8]), false))
        .await
        .unwrap();
    assert_eq!(finished(&m, &t.task_id).await.status, Status::Completed);
    let call = runner.calls.lock().await[0].clone();
    assert_eq!(
        call.session_id.as_deref(),
        Some(CLAUDE_ID),
        "the canonical id"
    );
    assert_eq!(call.cwd, f.project, "the session's project, by default");
    assert!(!call.fork);
}

#[tokio::test]
async fn an_explicit_denial_wins_even_at_level_all() {
    let f = fixture();
    let (m, _) = manager(&f);
    consent::set_level(&f.sessions.paths, Level::All).unwrap();
    consent::set_decision(
        &f.sessions.paths,
        trovador::Backend::Claude,
        CLAUDE_ID,
        Some(Decision::Denied),
    )
    .unwrap();
    let err = m
        .spawn_task(request(Some(CLAUDE_ID), false))
        .await
        .unwrap_err();
    assert!(err.contains("explicitly denied"), "{err}");
    m.spawn_task(request(Some(OTHER_ID), false)).await.unwrap();
}

#[tokio::test]
async fn a_fork_needs_no_consent_but_only_works_for_claude() {
    let f = fixture();
    let (m, runner) = manager(&f);
    let t = m.spawn_task(request(Some(CLAUDE_ID), true)).await.unwrap();
    assert_eq!(finished(&m, &t.task_id).await.status, Status::Completed);
    assert!(runner.calls.lock().await[0].fork);

    let err = m
        .spawn_task(request(Some(CODEX_ID), true))
        .await
        .unwrap_err();
    assert!(err.contains("Claude sessions only"), "{err}");
    let err = m.spawn_task(request(None, true)).await.unwrap_err();
    assert!(err.contains("fork needs a session_id"), "{err}");

    // A follow-up adds to the forked session; it does not fork again.
    let next = m
        .continue_task(ContinueTask {
            task_id: t.task_id,
            message: "and then".into(),
        })
        .await
        .unwrap();
    finished(&m, &next.task_id).await;
    let calls = runner.calls.lock().await;
    assert_eq!(calls[1].session_id.as_deref(), Some(NEW_ID));
    assert!(!calls[1].fork);
}

#[tokio::test]
async fn a_session_past_the_resume_ceiling_is_refused() {
    let mut f = fixture();
    f.sessions.limits.resume_limit_bytes = 10;
    let (m, runner) = manager(&f);
    let err = m
        .spawn_task(request(Some(CLAUDE_ID), true))
        .await
        .unwrap_err();
    assert!(err.contains("cannot be continued"), "{err}");
    assert!(runner.calls.lock().await.is_empty());
}

#[tokio::test]
async fn a_session_tosi_starts_is_recorded_and_can_be_continued_later() {
    let f = fixture();
    let (m, _) = manager(&f);
    let mut req = request(None, false);
    req.cwd = Some(f.project.to_string_lossy().into_owned());
    let t = m.spawn_task(req).await.unwrap();
    assert_eq!(finished(&m, &t.task_id).await.status, Status::Completed);
    assert!(consent::check(&f.sessions.paths, trovador::Backend::Claude, NEW_ID).spawned);

    // A later server, after a restart, may continue it by id.
    claude_session(&f.sessions, NEW_ID, &f.project);
    let (later, runner) = manager(&f);
    let t = later
        .spawn_task(request(Some(NEW_ID), false))
        .await
        .unwrap();
    assert_eq!(finished(&later, &t.task_id).await.status, Status::Completed);
    assert_eq!(
        runner.calls.lock().await[0].session_id.as_deref(),
        Some(NEW_ID)
    );
}

#[tokio::test]
async fn without_the_archive_session_id_is_refused_and_new_tasks_still_work() {
    let f = fixture();
    let runner = Arc::new(FakeRunner::default());
    let m = Manager::new(runner, Backend::Claude, 4);
    let err = m
        .spawn_task(request(Some(CLAUDE_ID), false))
        .await
        .unwrap_err();
    assert!(err.contains("trovador session archive"), "{err}");
    let mut req = request(None, false);
    req.cwd = Some(f.project.to_string_lossy().into_owned());
    m.spawn_task(req).await.unwrap();
}

#[tokio::test]
async fn a_fork_that_reports_the_original_id_fails_and_records_nothing() {
    // Review of the fork change: the original session was recorded as started by tosi,
    // which then let it be continued without consent.
    let f = fixture();
    let runner = Arc::new(FakeRunner {
        echo_fork: true,
        ..FakeRunner::default()
    });
    let m = Manager::new(runner, Backend::Claude, 4).with_sessions(f.sessions.clone());
    let t = m.spawn_task(request(Some(CLAUDE_ID), true)).await.unwrap();
    let done = finished(&m, &t.task_id).await;
    assert_eq!(done.status, Status::Failed);
    assert!(
        done.error
            .unwrap()
            .contains("did not report a new session id")
    );
    assert!(done.provider_session_id.is_none());
    assert!(!consent::check(&f.sessions.paths, trovador::Backend::Claude, CLAUDE_ID).spawned);
    let err = m
        .spawn_task(request(Some(CLAUDE_ID), false))
        .await
        .unwrap_err();
    assert!(err.contains("continuation denied"), "{err}");
    assert!(
        m.continue_task(ContinueTask {
            task_id: t.task_id,
            message: "more".into(),
        })
        .await
        .is_err()
    );
}

#[tokio::test]
async fn consent_is_checked_again_on_every_follow_up() {
    // Review of the fork change: a deny after the first turn did not stop continue_task.
    let f = fixture();
    let (m, _) = manager(&f);
    consent::set_level(&f.sessions.paths, Level::Approved).unwrap();
    let approve = |d| {
        consent::set_decision(&f.sessions.paths, trovador::Backend::Claude, CLAUDE_ID, d).unwrap()
    };
    approve(Some(Decision::Approved));
    let t = m.spawn_task(request(Some(CLAUDE_ID), false)).await.unwrap();
    assert_eq!(finished(&m, &t.task_id).await.status, Status::Completed);

    approve(Some(Decision::Denied));
    let err = m
        .continue_task(ContinueTask {
            task_id: t.task_id.clone(),
            message: "more".into(),
        })
        .await
        .unwrap_err();
    assert!(err.contains("explicitly denied"), "{err}");

    // A session tosi started stops too once the level is deny.
    let mut req = request(None, false);
    req.cwd = Some(f.project.to_string_lossy().into_owned());
    let own = m.spawn_task(req).await.unwrap();
    assert_eq!(finished(&m, &own.task_id).await.status, Status::Completed);
    consent::set_level(&f.sessions.paths, Level::Deny).unwrap();
    let err = m
        .continue_task(ContinueTask {
            task_id: own.task_id,
            message: "more".into(),
        })
        .await
        .unwrap_err();
    assert!(err.contains("level is deny"), "{err}");
}

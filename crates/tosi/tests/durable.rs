#![cfg(unix)]
#[path = "support/state.rs"]
mod state;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
};
use tosi::registry::{ListTasks, Registry};

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Lines<BufReader<ChildStdout>>,
    id: u64,
}
impl Server {
    async fn start(root: &Path) -> Self {
        let mut child = state::server_command(env!("CARGO_BIN_EXE_tosi"), root)
            .env_remove("TOSI_CHILD")
            .env_remove("TASK_RUNNERS_CHILD")
            .env("TOSI_DEFAULT_BACKEND", "claude")
            .env("TOSI_CLAUDE_BIN", root.join("provider"))
            .env("TOSI_TASK_RETENTION", "500")
            .env("TOSI_TASK_RETENTION_DAYS", "30")
            .env("TOSI_NO_REDACT", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut server = Self {
            stdin: child.stdin.take(),
            stdout: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
            id: 0,
        };
        server.rpc("initialize", json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"durable-test","version":"1"}})).await;
        server
            .write(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        server
    }
    async fn write(&mut self, v: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(format!("{v}\n").as_bytes()).await.unwrap();
        stdin.flush().await.unwrap();
    }
    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        self.write(json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params}))
            .await;
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let v: Value =
                    serde_json::from_str(&self.stdout.next_line().await.unwrap().unwrap()).unwrap();
                if v["id"] == self.id {
                    assert!(v.get("error").is_none(), "{v}");
                    return v["result"].clone();
                }
            }
        })
        .await
        .unwrap()
    }
    async fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let v = self
            .rpc("tools/call", json!({"name":name,"arguments":args}))
            .await;
        let text = v["content"][0]["text"].as_str().unwrap();
        if v["isError"] == true {
            Err(text.into())
        } else {
            Ok(serde_json::from_str(text).unwrap())
        }
    }
    async fn spawn(&mut self, root: &Path) -> String {
        self.tool(
            "spawn_task",
            json!({"task":"bounded test","cwd":root.canonicalize().unwrap(),"timeout_secs":60}),
        )
        .await
        .unwrap()["task_id"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn stop(mut self) {
        self.stdin.take();
        assert!(
            tokio::time::timeout(Duration::from_secs(8), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}
const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890";
fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("provider"), format!(r#"#!/bin/sh
cat >/dev/null
printf '%s' "$$" > '{}/child-pid'
while [ ! -e '{}/release' ]; do sleep 0.05; done
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"result":"answer {}","session_id":"durable-session","num_turns":1}}'
"#,root.path().display(),root.path().display(),SECRET)).unwrap();
    fs::set_permissions(
        root.path().join("provider"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    root
}
fn record_path(root: &Path, id: &str) -> std::path::PathBuf {
    root.join(".bottega/tosi/tasks").join(format!("{id}.json"))
}
async fn child_pid(root: &Path) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(v) = fs::read_to_string(root.join("child-pid"))
                && !v.is_empty()
            {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn layout(root: &Path) -> bottega_core::state::State {
    let state =
        bottega_core::state::resolve(Some(root.join(".bottega").into_os_string()), None, root);
    assert!(state.tosi.starts_with(root));
    bottega_core::state::prepare(state).unwrap()
}
#[tokio::test]
async fn shared_servers_wait_refuse_nonowners_resume_and_match_cli() {
    let root = fixture();
    let root = root.path();
    let mut owner = Server::start(root).await;
    let id = owner.spawn(root).await;
    child_pid(root).await;
    let mut reader = Server::start(root).await;
    let listed = reader.tool("list_tasks", json!({})).await.unwrap();
    assert_eq!(listed[0]["task_id"], id);
    assert!(listed[0]["result"].is_null());
    for (name, args) in [
        ("cancel_task", json!({"task_id":id})),
        ("continue_task", json!({"task_id":id,"message":"follow up"})),
    ] {
        let error = reader.tool(name, args).await.unwrap_err();
        assert!(
            error.contains(&format!("owner pid {}", owner.child.id().unwrap())),
            "{error}"
        );
        assert!(error.contains("instance"));
    }
    let pending = reader
        .tool("wait_tasks", json!({"task_ids":[id],"timeout_ms":0}))
        .await
        .unwrap();
    assert_eq!(pending["timed_out"], true);
    let release = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        fs::write(root.join("release"), "").unwrap();
    };
    let (done, ()) = tokio::join!(
        reader.tool("wait_tasks", json!({"task_ids":[id],"timeout_ms":5000})),
        release
    );
    let done = done.unwrap();
    assert_eq!(done["tasks"][0]["status"], "completed");
    assert!(
        !done["tasks"][0]["result"]
            .as_str()
            .unwrap()
            .contains(SECRET)
    );
    let own = owner
        .tool("wait_tasks", json!({"task_ids":[id],"timeout_ms":0}))
        .await
        .unwrap();
    assert!(own["tasks"][0]["result"].as_str().unwrap().contains(SECRET));
    let disk = fs::read_to_string(record_path(root, &id)).unwrap();
    assert!(!disk.contains(SECRET));
    assert!(disk.contains("redacted"));
    assert_eq!(
        fs::metadata(record_path(root, &id))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(root.join(".bottega/tosi"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    owner.stop().await;
    let next = reader
        .tool("continue_task", json!({"task_id":id,"message":"follow up"}))
        .await
        .unwrap();
    let result = reader
        .tool(
            "wait_tasks",
            json!({"task_ids":[next["task_id"]],"timeout_ms":5000}),
        )
        .await
        .unwrap();
    assert_eq!(result["tasks"][0]["status"], "completed");
    let compact = reader
        .tool(
            "list_tasks",
            json!({"status":"completed","limit":1,"since":next["started_at_ms"]}),
        )
        .await
        .unwrap();
    assert_eq!(compact.as_array().unwrap().len(), 1);
    assert_eq!(compact[0]["task_id"], next["task_id"]);
    assert!(compact[0]["result"].is_null());
    let mut observer = Server::start(root).await;
    let listed = observer
        .tool(
            "list_tasks",
            json!({"status":"completed","full_result":true}),
        )
        .await
        .unwrap();
    let output = state::server_command(env!("CARGO_BIN_EXE_tosi"), root)
        .args(["tasks", "--status", "completed", "--full-result"])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        listed,
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    );
    observer.stop().await;
    reader.stop().await;
}
#[tokio::test]
async fn killed_server_is_interrupted_and_reports_surviving_child() {
    let root = fixture();
    let root = root.path();
    let mut owner = Server::start(root).await;
    let id = owner.spawn(root).await;
    let pid = child_pid(root).await;
    owner.child.kill().await.unwrap();
    let mut reader = Server::start(root).await;
    let task = reader
        .tool("wait_tasks", json!({"task_ids":[id],"timeout_ms":0}))
        .await
        .unwrap();
    assert_eq!(task["tasks"][0]["status"], "interrupted");
    let error = task["tasks"][0]["error"].as_str().unwrap();
    assert!(error.contains("stopped before the task finished"));
    assert!(
        std::process::Command::new("kill")
            .args(["-0", &pid])
            .output()
            .unwrap()
            .status
            .success(),
        "fake provider must still be alive"
    );
    assert!(error.contains(&format!(
        "pid {pid} was the provider when the server stopped; check whether it is still running"
    )));
    assert!(task["tasks"][0]["result"].is_null());
    let _ = std::process::Command::new("kill")
        .args(["-KILL", &pid])
        .output();
    reader.stop().await;
}
#[tokio::test]
async fn reused_identity_future_versions_and_concurrent_retention() {
    let root = fixture();
    let root = root.path();
    fs::write(root.join("release"), "").unwrap();
    let mut owner = Server::start(root).await;
    let id = owner.spawn(root).await;
    owner
        .tool("wait_tasks", json!({"task_ids":[id],"timeout_ms":5000}))
        .await
        .unwrap();
    let path = record_path(root, &id);
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let store = Registry::open_with_retention(&layout(root), 500, 30).unwrap();
    let mut forged = original.clone();
    forged["status"] = json!("running");
    forged["owner"]["instance_id"] = json!(uuid::Uuid::new_v4().to_string());
    fs::write(&path, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert_eq!(
        store.read(&id).unwrap().snapshot.status,
        tosi::manager::Status::Interrupted
    );
    forged = original.clone();
    forged["status"] = json!("running");
    forged["owner"]["started_at_ms"] = json!(0);
    fs::write(&path, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert_eq!(
        store.read(&id).unwrap().snapshot.status,
        tosi::manager::Status::Interrupted
    );
    forged = original.clone();
    forged["version"] = json!(999);
    fs::write(&path, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert!(
        store
            .read(&id)
            .unwrap_err()
            .contains("unsupported task record version 999")
    );
    let future = state::server_command(env!("CARGO_BIN_EXE_tosi"), root)
        .arg("tasks")
        .output()
        .await
        .unwrap();
    assert!(future.status.success());
    assert!(path.exists());
    assert!(
        !root
            .join(".bottega/tosi/tasks/quarantine")
            .join(format!("{id}.json"))
            .exists()
    );
    assert!(
        serde_json::from_slice::<Value>(&future.stdout)
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .read(&id)
            .unwrap_err()
            .contains("unsupported task record version 999")
    );
    assert!(
        String::from_utf8_lossy(&future.stderr).contains("unsupported task record version 999")
    );
    fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    let active = owner.spawn(root).await;
    owner
        .tool("wait_tasks", json!({"task_ids":[active],"timeout_ms":5000}))
        .await
        .unwrap();
    let mut live: Value =
        serde_json::from_slice(&fs::read(record_path(root, &active)).unwrap()).unwrap();
    live["status"] = json!("running");
    live["started_at_ms"] = json!(1);
    fs::write(
        record_path(root, &active),
        serde_json::to_vec(&live).unwrap(),
    )
    .unwrap();
    let layout = layout(root);
    let a = Registry::open_with_retention(&layout, 1, 30).unwrap();
    let b = Registry::open_with_retention(&layout, 1, 30).unwrap();
    fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    for _ in 0..20 {
        let mut record = original.clone();
        let task_id = uuid::Uuid::new_v4().to_string();
        record["task_id"] = json!(task_id);
        fs::write(
            record_path(root, &task_id),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }
    let mut first = state::server_command(env!("CARGO_BIN_EXE_tosi"), root);
    let mut second = state::server_command(env!("CARGO_BIN_EXE_tosi"), root);
    for command in [&mut first, &mut second] {
        command
            .arg("tasks")
            .env("TOSI_TASK_RETENTION", "1")
            .env("TOSI_TASK_RETENTION_DAYS", "30");
    }
    let (first, second) = tokio::join!(first.output(), second.output());
    for output in [first.unwrap(), second.unwrap()] {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let tasks: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(tasks.as_array().unwrap().len(), 1);
        assert_eq!(tasks[0]["task_id"], active);
    }
    std::thread::scope(|scope| {
        let first = scope.spawn(|| a.prune().unwrap());
        let second = scope.spawn(|| b.prune().unwrap());
        first.join().unwrap();
        second.join().unwrap();
    });
    assert!(record_path(root, &active).exists());
    assert!(!path.exists());
    let tasks = a.list(ListTasks::default()).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_id, active);
    live["status"] = json!("completed");
    live["started_at_ms"] = json!(1);
    live["finished_at_ms"] = json!(1);
    fs::write(
        record_path(root, &active),
        serde_json::to_vec(&live).unwrap(),
    )
    .unwrap();
    a.prune().unwrap();
    assert!(!record_path(root, &active).exists());
    owner.stop().await;
}

#[tokio::test]
async fn unreadable_neighbors_are_quarantined_and_do_not_prevent_cancellation() {
    let root = fixture();
    let root = root.path();
    let prepared = layout(root);
    let empty = uuid::Uuid::new_v4().to_string();
    fs::create_dir_all(prepared.tosi.join("tasks")).unwrap();
    fs::write(record_path(root, &empty), b"").unwrap();
    let mut owner = Server::start(root).await;
    let task = owner.spawn(root).await;
    let pid = child_pid(root).await;
    let mut record: Value =
        serde_json::from_slice(&fs::read(record_path(root, &task)).unwrap()).unwrap();
    let future = uuid::Uuid::new_v4().to_string();
    record["task_id"] = json!(future);
    record["version"] = json!(999);
    fs::write(
        record_path(root, &future),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let mut reader = Server::start(root).await;
    assert!(
        prepared
            .tosi
            .join(format!("tasks/quarantine/{empty}.json"))
            .exists()
    );
    assert!(record_path(root, &future).exists());
    assert!(
        !prepared
            .tosi
            .join(format!("tasks/quarantine/{future}.json"))
            .exists()
    );
    let listed = reader.tool("list_tasks", json!({})).await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["task_id"], task);
    let unknown = uuid::Uuid::new_v4().to_string();
    let mixed = reader
        .tool(
            "wait_tasks",
            json!({"task_ids":[task,unknown,future,empty,"missing"],"timeout_ms":0}),
        )
        .await
        .unwrap();
    assert_eq!(mixed["tasks"][0]["task_id"], task);
    assert_eq!(mixed["errors"].as_array().unwrap().len(), 4);
    assert_eq!(
        mixed["errors"][3]["error"],
        "unknown task_id: missing (pruned or never existed)"
    );
    assert_eq!(
        mixed["errors"][0]["error"],
        format!("unknown task_id: {unknown} (pruned or never existed)")
    );
    assert!(
        mixed["errors"][1]["error"]
            .as_str()
            .unwrap()
            .contains("unsupported task record version 999")
    );
    let corrupt = uuid::Uuid::new_v4().to_string();
    fs::write(record_path(root, &corrupt), b"{").unwrap();
    let cancelled = owner
        .tool("cancel_task", json!({"task_id":task}))
        .await
        .unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert!(
        !std::process::Command::new("kill")
            .args(["-0", &pid])
            .output()
            .unwrap()
            .status
            .success()
    );
    let listed = reader.tool("list_tasks", json!({})).await.unwrap();
    assert_eq!(listed[0]["status"], "cancelled");
    assert!(
        prepared
            .tosi
            .join(format!("tasks/quarantine/{corrupt}.json"))
            .exists()
    );
    reader.stop().await;
    owner.stop().await;
}

#[tokio::test]
async fn failed_terminal_save_remains_in_memory_and_a_poll_retries_it() {
    let root = fixture();
    let root = root.path();
    let mut owner = Server::start(root).await;
    let task = owner.spawn(root).await;
    child_pid(root).await;
    let path = record_path(root, &task);
    let temp = path.with_file_name(format!("{task}.json.{}.tmp", owner.child.id().unwrap()));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            if record["provider_pid"].is_number() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    fs::create_dir(&temp).unwrap();
    fs::write(root.join("release"), "").unwrap();
    let done = owner
        .tool("wait_tasks", json!({"task_ids":[task],"timeout_ms":5000}))
        .await
        .unwrap();
    assert_eq!(done["tasks"][0]["status"], "completed");
    assert!(
        done["tasks"][0]["result"]
            .as_str()
            .unwrap()
            .contains(SECRET)
    );
    let disk: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk["status"], "running");
    fs::remove_dir(temp).unwrap();
    let retried = owner
        .tool("wait_tasks", json!({"task_ids":[task],"timeout_ms":0}))
        .await
        .unwrap();
    assert_eq!(retried["tasks"][0]["status"], "completed");
    let disk: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk["status"], "completed");
    assert!(!disk["result"].as_str().unwrap().contains(SECRET));
    owner.stop().await;
}

#[tokio::test]
async fn opt_out_structured_results_are_validated_in_memory_and_redacted_for_other_servers() {
    let root = fixture();
    let root = root.path();
    let script = fs::read_to_string(root.join("provider")).unwrap();
    fs::write(
        root.join("provider"),
        script.replace(
            "\"num_turns\":1",
            &format!("\"num_turns\":1,\"structured_output\":{{\"answer\":\"{SECRET}\"}}"),
        ),
    )
    .unwrap();
    fs::write(root.join("release"), "").unwrap();
    let mut owner = Server::start(root).await;
    let task = owner.tool("spawn_task",json!({"task":"structured response","cwd":root.canonicalize().unwrap(),"output_schema":{"type":"object","properties":{"answer":{"type":"string","pattern":"^sk-ant-"}},"required":["answer"],"additionalProperties":false}})).await.unwrap()["task_id"].as_str().unwrap().to_owned();
    let own = owner
        .tool("wait_tasks", json!({"task_ids":[task],"timeout_ms":5000}))
        .await
        .unwrap();
    assert_eq!(own["tasks"][0]["status"], "completed");
    assert_eq!(own["tasks"][0]["structured_result"]["answer"], SECRET);
    let mut reader = Server::start(root).await;
    let foreign = reader
        .tool("wait_tasks", json!({"task_ids":[task],"timeout_ms":0}))
        .await
        .unwrap();
    assert_eq!(foreign["tasks"][0]["status"], "completed");
    assert!(
        foreign["tasks"][0]["structured_result"]["answer"]
            .as_str()
            .unwrap()
            .starts_with("[redacted:")
    );
    let listed = owner
        .tool("list_tasks", json!({"full_result":true}))
        .await
        .unwrap();
    assert_eq!(listed[0]["structured_result"]["answer"], SECRET);
    reader.stop().await;
    owner.stop().await;
}

#[tokio::test]
async fn retention_zero_is_rejected_before_a_server_starts() {
    let root = fixture();
    for setting in ["TOSI_TASK_RETENTION", "TOSI_TASK_RETENTION_DAYS"] {
        let output = state::server_command(env!("CARGO_BIN_EXE_tosi"), root.path())
            .arg("tasks")
            .env(setting, "0")
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("must be greater than zero"));
    }
}

#[tokio::test]
async fn unreadable_or_unparseable_owner_identity_uses_the_live_lease() {
    let root = fixture();
    let root = root.path();
    let mut owner = Server::start(root).await;
    let task = owner.spawn(root).await;
    child_pid(root).await;
    let record: Value =
        serde_json::from_slice(&fs::read(record_path(root, &task)).unwrap()).unwrap();
    let instance = record["owner"]["instance_id"].as_str().unwrap();
    let marker = root.join(format!(".bottega/tosi/owners/{instance}.json"));
    let original = fs::read(&marker).unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o000)).unwrap();
    let store = Registry::open_with_retention(&layout(root), 500, 30).unwrap();
    assert_eq!(
        store.list(ListTasks::default()).unwrap()[0].status,
        tosi::manager::Status::Running
    );
    assert!(record_path(root, &task).exists());
    assert!(!root.join(".bottega/tosi/tasks/quarantine").exists());
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&marker, b"{").unwrap();
    assert_eq!(
        store.read(&task).unwrap().snapshot.status,
        tosi::manager::Status::Running
    );
    assert_eq!(
        store.list(ListTasks::default()).unwrap()[0].status,
        tosi::manager::Status::Running
    );
    assert!(marker.exists());
    assert!(!root.join(".bottega/tosi/tasks/quarantine").exists());
    fs::remove_file(&marker).unwrap();
    assert_eq!(
        store.list(ListTasks::default()).unwrap()[0].status,
        tosi::manager::Status::Running
    );
    fs::write(&marker, &original).unwrap();
    let lease = marker.with_extension("");
    fs::set_permissions(&lease, fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        store.list(ListTasks::default()).unwrap()[0].status,
        tosi::manager::Status::Running
    );
    assert!(record_path(root, &task).exists());
    assert!(!root.join(".bottega/tosi/tasks/quarantine").exists());
    fs::set_permissions(&lease, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        owner
            .tool("cancel_task", json!({"task_id":task}))
            .await
            .unwrap()["status"],
        "cancelled"
    );
    owner.stop().await;
}

#[tokio::test]
async fn maintenance_cleans_only_old_temps_and_uses_terminal_finish_time() {
    let root = fixture();
    let root = root.path();
    fs::write(root.join("release"), "").unwrap();
    let mut owner = Server::start(root).await;
    let task = owner.spawn(root).await;
    owner
        .tool("wait_tasks", json!({"task_ids":[task],"timeout_ms":5000}))
        .await
        .unwrap();
    let path = record_path(root, &task);
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["started_at_ms"] = json!(1);
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let prepared = layout(root);
    for directory in ["tasks", "owners"] {
        let dir = prepared.tosi.join(directory);
        let stale = dir.join("stale.tmp");
        let file = fs::File::create(&stale).unwrap();
        file.set_times(
            fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(600)),
        )
        .unwrap();
        fs::write(dir.join("fresh.tmp"), b"fresh").unwrap();
    }
    let store = Registry::open_with_retention(&prepared, 500, 30).unwrap();
    assert!(
        path.exists(),
        "a task finished recently must not expire because it started long ago"
    );
    for directory in ["tasks", "owners"] {
        assert!(!prepared.tosi.join(directory).join("stale.tmp").exists());
        assert!(prepared.tosi.join(directory).join("fresh.tmp").exists());
    }
    record["finished_at_ms"] = json!(1);
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    store.prune().unwrap();
    assert!(!path.exists());
    owner.stop().await;
}

#[tokio::test]
async fn opted_out_owner_keeps_its_copy_while_disk_redacts_before_truncation() {
    let root = fixture();
    let root = root.path();
    let long = format!("{}{SECRET} tail", "x".repeat(64_000 - 20));
    let script = fs::read_to_string(root.join("provider")).unwrap();
    fs::write(
        root.join("provider"),
        script.replace(&format!("answer {SECRET}"), &long),
    )
    .unwrap();
    fs::write(root.join("release"), "").unwrap();
    let mut owner = Server::start(root).await;
    let task = owner.spawn(root).await;
    let done = owner
        .tool("wait_tasks", json!({"task_ids":[task],"timeout_ms":5000}))
        .await
        .unwrap();
    assert_eq!(done["tasks"][0]["status"], "completed");
    assert_eq!(done["tasks"][0]["result_truncated"], true);
    assert!(
        done["tasks"][0]["result"]
            .as_str()
            .unwrap()
            .contains("sk-ant-")
    );
    let disk: Value = serde_json::from_slice(&fs::read(record_path(root, &task)).unwrap()).unwrap();
    assert!(!disk["result"].as_str().unwrap().contains("sk-ant-"));
    assert!(disk["result"].as_str().unwrap().contains("[redacted:"));
    owner.stop().await;
}

#[tokio::test]
async fn future_formats_are_untouched_and_excluded_from_retention() {
    let root = fixture();
    let root = root.path();
    fs::write(root.join("release"), "").unwrap();
    let mut owner = Server::start(root).await;
    let task = owner.spawn(root).await;
    owner
        .tool("wait_tasks", json!({"task_ids":[task],"timeout_ms":5000}))
        .await
        .unwrap();
    let future = uuid::Uuid::new_v4().to_string();
    let future_path = record_path(root, &future);
    let future_bytes = br#"{"version":999,"new_layout":{"live":true}}"#;
    fs::write(&future_path, future_bytes).unwrap();
    let store = Registry::open_with_retention(&layout(root), 1, 30).unwrap();
    for _ in 0..2 {
        store.prune().unwrap();
        let listed = store.list(ListTasks::default()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].task_id, task);
        assert!(
            store
                .read(&future)
                .unwrap_err()
                .contains("unsupported task record version 999")
        );
        assert_eq!(fs::read(&future_path).unwrap(), future_bytes);
    }
    let mut reader = Server::start(root).await;
    let result = reader
        .tool(
            "wait_tasks",
            json!({"task_ids":[task,future],"timeout_ms":0}),
        )
        .await
        .unwrap();
    assert_eq!(result["tasks"][0]["task_id"], task);
    assert!(
        result["errors"][0]["error"]
            .as_str()
            .unwrap()
            .contains("unsupported task record version 999")
    );
    assert!(!root.join(".bottega/tosi/tasks/quarantine").exists());
    reader.stop().await;
    owner.stop().await;
}

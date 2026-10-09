#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    time::Duration,
};

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Lines<BufReader<ChildStdout>>,
    provider_pid: Option<String>,
    dir: TempDir,
}

impl Server {
    async fn start(backend: &str) -> Self {
        Self::start_config(&[
            ("TOSI_DEFAULT_BACKEND", backend),
            ("TOSI_MAX_CONCURRENCY", "1"),
        ])
        .await
    }

    async fn start_config(settings: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-provider");
        fs::write(
            &fake,
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' \"$$\" > \"$TOSI_TEST_PID_FILE\"\nexec /bin/sleep 60\n",
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_tosi"))
            .arg("--stdio")
            .env_remove("TOSI_CHILD")
            .env_remove("TASK_RUNNERS_CHILD")
            .env_remove("TOSI_DEFAULT_BACKEND")
            .env_remove("TASK_RUNNERS_DEFAULT_BACKEND")
            .env_remove("TOSI_MAX_CONCURRENCY")
            .env_remove("TASK_RUNNERS_MAX_CONCURRENCY")
            .envs(settings.iter().copied())
            .env("TOSI_CLAUDE_BIN", &fake)
            .env("TOSI_CODEX_BIN", &fake)
            .env("TOSI_TEST_PID_FILE", dir.path().join("provider-pid"))
            .env("RUST_LOG", "warn")
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
            provider_pid: None,
            dir,
        };
        server
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-03-26", "capabilities": {},
                    "clientInfo": {"name": "shutdown-test", "version": "1"}
                }
            }))
            .await;
        let response = server.response(1).await;
        assert!(response.get("error").is_none(), "{response}");
        server
            .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        server
    }

    async fn send(&mut self, value: Value) {
        let mut line = serde_json::to_vec(&value).unwrap();
        line.push(b'\n');
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(&line).await.unwrap();
        stdin.flush().await.unwrap();
    }

    async fn response(&mut self, id: u64) -> Value {
        tokio::time::timeout(TEST_TIMEOUT, async {
            loop {
                let line = self
                    .stdout
                    .next_line()
                    .await
                    .unwrap()
                    .expect("server exited before returning the response");
                let response: Value = serde_json::from_str(&line).unwrap();
                if response.get("id") == Some(&json!(id)) {
                    return response;
                }
            }
        })
        .await
        .expect("server did not finish the pending request during shutdown")
    }

    async fn start_worker_and_wait(&mut self) -> Value {
        self.send(json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "spawn_task", "arguments": {
                "task": "Wait for cancellation", "cwd": self.dir.path().canonicalize().unwrap(),
                "timeout_secs": 60
            }}
        }))
        .await;
        let response = self.response(2).await;
        let snapshot = tool_value(&response);
        let task_id = snapshot["task_id"].as_str().unwrap();
        self.provider_pid = Some(
            tokio::time::timeout(TEST_TIMEOUT, async {
                loop {
                    if let Ok(pid) = fs::read_to_string(self.dir.path().join("provider-pid"))
                        && !pid.trim().is_empty()
                    {
                        break pid.trim().to_owned();
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("fake provider did not start"),
        );
        self.send(json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "wait_tasks", "arguments": {
                "task_ids": [task_id], "timeout_ms": 55000
            }}
        }))
        .await;
        // Issue #51: a signal stops the transport reading stdin, and only requests already
        // read are drained. Signalling straight after writing the wait could leave it unread
        // in the pipe, so nothing answered it. Requests are read in order and run
        // concurrently, so a ping response proves the wait is in flight.
        self.send(json!({"jsonrpc": "2.0", "id": 90, "method": "ping"}))
            .await;
        let pong = self.response(90).await;
        assert!(pong.get("error").is_none(), "{pong}");
        snapshot
    }

    fn signal(&self, signal: &str) {
        assert!(
            Command::new("/bin/kill")
                .args([signal, &self.child.id().unwrap().to_string()])
                .status()
                .unwrap()
                .success()
        );
    }

    async fn assert_clean_shutdown(&mut self) {
        let response = self.response(3).await;
        assert_eq!(tool_value(&response)["tasks"][0]["status"], "cancelled");
        let status = tokio::time::timeout(TEST_TIMEOUT, self.child.wait())
            .await
            .expect("server did not exit while stdin remained open")
            .unwrap();
        assert!(
            status.success(),
            "server exited without graceful cleanup: {status}"
        );
        let pid = self.provider_pid.as_ref().unwrap();
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", pid])
                .output()
                .unwrap()
                .status
                .success(),
            "provider {pid} was not reaped before the server exited"
        );
        self.provider_pid = None;
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(pid) = &self.provider_pid {
            let _ = Command::new("/bin/kill").args(["-KILL", pid]).output();
        }
    }
}

fn tool_value(response: &Value) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn eof_cancels_workers_before_draining_a_pending_wait() {
    for backend in ["claude", "codex"] {
        let mut server = Server::start(backend).await;
        server.start_worker_and_wait().await;
        drop(server.stdin.take());
        server.assert_clean_shutdown().await;
    }
}

#[tokio::test]
async fn sigterm_cleans_up_workers_and_exits_with_stdin_open() {
    for backend in ["claude", "codex"] {
        let mut server = Server::start(backend).await;
        server.start_worker_and_wait().await;
        server.signal("-TERM");
        server.assert_clean_shutdown().await;
    }
}

#[tokio::test]
async fn sigint_cleans_up_workers_and_exits_with_stdin_open() {
    for backend in ["claude", "codex"] {
        let mut server = Server::start(backend).await;
        server.start_worker_and_wait().await;
        server.signal("-INT");
        server.assert_clean_shutdown().await;
    }
}

#[tokio::test]
async fn legacy_server_defaults_apply_only_when_primary_keys_are_absent() {
    for primary_present in [false, true] {
        let mut settings = vec![
            ("TASK_RUNNERS_DEFAULT_BACKEND", "codex"),
            ("TASK_RUNNERS_MAX_CONCURRENCY", "1"),
        ];
        let backend = if primary_present {
            settings[1].1 = "2";
            settings.extend([
                ("TOSI_DEFAULT_BACKEND", "claude"),
                ("TOSI_MAX_CONCURRENCY", "1"),
            ]);
            "claude"
        } else {
            "codex"
        };
        let mut server = Server::start_config(&settings).await;
        let snapshot = server.start_worker_and_wait().await;
        assert_eq!(snapshot["backend"], backend);
        server.send(json!({
            "jsonrpc":"2.0", "id":4, "method":"tools/call",
            "params":{"name":"spawn_task", "arguments":{
                "task":"Must be rejected at capacity", "cwd":server.dir.path().canonicalize().unwrap()
            }}
        })).await;
        let response = server.response(4).await;
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("concurrency limit (1)")
        );
        drop(server.stdin.take());
        server.assert_clean_shutdown().await;
    }
}

fn clean_server_command() -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_tosi"));
    command
        .arg("--stdio")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    for suffix in ["CHILD", "DEFAULT_BACKEND", "MAX_CONCURRENCY"] {
        command.env_remove(format!("TOSI_{suffix}"));
        command.env_remove(format!("TASK_RUNNERS_{suffix}"));
    }
    command
}

#[tokio::test]
async fn either_child_guard_rejects_startup_even_when_the_other_disagrees() {
    for (primary, legacy, rejected) in [
        (None, Some("1"), true),
        (Some("1"), None, true),
        (Some("0"), Some("1"), true),
        (Some("1"), Some("0"), true),
        (Some(""), Some("1"), true),
        (Some("1"), Some(""), true),
        (Some("0"), Some("0"), false),
    ] {
        let mut command = clean_server_command();
        if let Some(value) = primary {
            command.env("TOSI_CHILD", value);
        }
        if let Some(value) = legacy {
            command.env("TASK_RUNNERS_CHILD", value);
        }
        let output = tokio::time::timeout(TEST_TIMEOUT, command.output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(!output.status.success(), rejected);
        if rejected {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("cannot start inside a delegated worker")
            );
        }
    }
}

#[tokio::test]
async fn empty_or_invalid_primary_settings_do_not_fall_back_to_legacy_values() {
    for (suffix, legacy, expected_reason) in [
        ("DEFAULT_BACKEND", "claude", "must be claude or codex"),
        ("MAX_CONCURRENCY", "1", "must contain a valid number"),
    ] {
        for primary in ["", "invalid"] {
            let output = tokio::time::timeout(
                TEST_TIMEOUT,
                clean_server_command()
                    .env(format!("TOSI_{suffix}"), primary)
                    .env(format!("TASK_RUNNERS_{suffix}"), legacy)
                    .output(),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                !output.status.success(),
                "accepted invalid primary {suffix}"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            let expected_message =
                format!("TOSI_{suffix} (or legacy TASK_RUNNERS_{suffix}) {expected_reason}");
            assert!(stderr.contains(&expected_message), "{stderr}");
        }
    }
}

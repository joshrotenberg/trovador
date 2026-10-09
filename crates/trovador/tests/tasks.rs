//! ask and summarize as MCP tasks, for a client that negotiates them.

mod common;

use std::time::Duration;

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::mcp::{ServeOpts, router_with};
use trovador::runlog;

async fn client(f: &Fixture, opts: ServeOpts) -> (TestClient, Value) {
    let mut c = TestClient::from_router(router_with(f.ctx(), opts));
    let init = c.initialize().await;
    (c, init)
}

async fn start_task(c: &mut TestClient, tool: &str, args: Value) -> Value {
    c.send_request(
        "tools/call",
        Some(json!({"name": tool, "arguments": args, "task": {}})),
    )
    .await
}

async fn finished(c: &mut TestClient, task_id: &str) -> Value {
    for _ in 0..100 {
        let t = c
            .send_request("tasks/get", Some(json!({"taskId": task_id})))
            .await;
        if t["status"] != "working" {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("task {task_id} still working after 5 s");
}

fn tool_json(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn ask_and_summarize_offer_tasks_and_a_read_only_server_offers_none() {
    let f = Fixture::new().with_fakes();
    let (mut c, init) = client(&f, ServeOpts::default()).await;
    assert!(init["capabilities"]["tasks"].is_object(), "{init}");
    for t in c.list_tools().await {
        let support = t.pointer("/execution/taskSupport").cloned();
        match t["name"].as_str().unwrap() {
            "ask" | "summarize" => assert_eq!(support, Some(json!("optional"))),
            _ => assert_eq!(support, None, "{t}"),
        }
    }
    let (_, init) = client(
        &f,
        ServeOpts {
            read_only: true,
            ..ServeOpts::default()
        },
    )
    .await;
    assert!(init["capabilities"].get("tasks").is_none(), "{init}");
}

#[tokio::test]
async fn an_ask_task_returns_at_once_and_ends_with_the_same_result_as_a_plain_call() {
    let f = Fixture::new().with_fakes();
    let limits = f.ctx().limits;
    let (mut c, _) = client(&f, ServeOpts::default()).await;
    let created = start_task(&mut c, "ask", json!({"prompt": "hello"})).await;
    assert_eq!(created["status"], "working");
    // Long enough for the slowest provider timeout, plus an hour to collect the result.
    let longest = limits.claude_timeout_s.max(limits.codex_timeout_s) as u64;
    assert_eq!(created["ttl"], json!((longest + 3600) * 1000));

    let done = finished(&mut c, created["taskId"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed");
    let from_task = tool_json(&done["result"]);
    let plain = c.call_tool_json("ask", json!({"prompt": "hello"})).await;
    for key in ["result", "verb", "backend", "is_error", "timed_out"] {
        assert_eq!(from_task[key], plain[key], "{key}");
    }
    assert_ne!(from_task["run_id"], plain["run_id"]);
}

#[tokio::test]
async fn cancelling_a_task_kills_its_run_and_logs_the_end() {
    for backend in ["claude", "codex"] {
        let f = Fixture::new().with_fakes();
        let ctx = f.ctx();
        let (mut c, _) = client(&f, ServeOpts::default()).await;
        let created =
            start_task(&mut c, "ask", json!({"prompt": "slow", "backend": backend})).await;
        let id = created["taskId"].as_str().unwrap().to_string();
        // Let the provider start before cancelling it.
        tokio::time::sleep(Duration::from_millis(400)).await;
        c.send_request("tasks/cancel", Some(json!({"taskId": id})))
            .await;

        let mut end = None;
        for _ in 0..50 {
            let log = std::fs::read_to_string(runlog::path(&ctx.paths)).unwrap_or_default();
            end = log
                .lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap())
                .find(|r| r["phase"] == "end");
            if end.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let end = end.unwrap_or_else(|| panic!("{backend}: no end record"));
        assert_eq!(end["outcome"], "cancelled", "{backend}");
        let t = c
            .send_request("tasks/get", Some(json!({"taskId": id})))
            .await;
        assert_eq!(t["status"], "cancelled", "{backend}");

        let bin = f.dir.path().join("bin").join(backend);
        let running = std::process::Command::new("pgrep")
            .args(["-f", &bin.display().to_string()])
            .output()
            .unwrap();
        assert!(
            running.stdout.is_empty(),
            "{backend} is still running: {}",
            String::from_utf8_lossy(&running.stdout)
        );
    }
}

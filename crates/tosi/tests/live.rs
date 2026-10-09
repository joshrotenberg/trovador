//! Real provider tests. Explicit opt-in is required before starting any subprocess.
mod support;

use std::{env, path::Path, time::Duration};

use serde_json::{Value, json};
use support::Client;
use tokio::time::Instant;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, String>;

fn enabled(backend: &str) -> TestResult<bool> {
    match tosi::config::var("LIVE_TESTS").as_deref() {
        Err(env::VarError::NotPresent) | Ok("0") => return Ok(false),
        Ok("1") => {}
        _ => return Err("TOSI_LIVE_TESTS must be unset, 0, or 1".into()),
    }
    let backends = match tosi::config::var("LIVE_BACKENDS") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => "claude,codex".into(),
        Err(_) => return Err("TOSI_LIVE_BACKENDS must contain valid text".into()),
    };
    let selected: Vec<_> = backends.split(',').map(str::trim).collect();
    if selected
        .iter()
        .any(|name| !matches!(*name, "claude" | "codex"))
    {
        return Err("TOSI_LIVE_BACKENDS must contain claude and/or codex, comma separated".into());
    }
    Ok(selected.contains(&backend))
}

fn check(condition: bool, message: impl Into<String>) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn string_field(value: &Value, field: &str) -> TestResult<String> {
    value[field]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("missing nonempty {field}: {value}"))
}

async fn completed(client: &mut Client, task: &Value) -> TestResult<Value> {
    let task_id = string_field(task, "task_id")?;
    let deadline = Instant::now() + Duration::from_secs(140);
    loop {
        let waited = client
            .tool(
                "wait_tasks",
                json!({"task_ids": [task_id], "timeout_ms": 55_000}),
            )
            .await?;
        let snapshot = waited["tasks"]
            .as_array()
            .and_then(|tasks| tasks.first())
            .ok_or_else(|| format!("wait returned no task: {waited}"))?;
        check(
            snapshot["task_id"] == task_id,
            "wait returned a different task ID",
        )?;
        match snapshot["status"].as_str() {
            Some("completed") => return Ok(snapshot.clone()),
            Some("running" | "cancelling") if Instant::now() < deadline => {}
            _ => return Err(format!("task did not complete successfully: {snapshot}")),
        }
    }
}

async fn inspect_contract(client: &mut Client) -> TestResult {
    check(
        client.initialization["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("resources/read")),
        "initialize did not advertise instruction resources",
    )?;
    let listed = client.rpc("tools/list", json!({})).await?;
    let spawn = listed["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "spawn_task"))
        .ok_or("spawn_task is missing from tools/list")?;
    check(
        spawn["inputSchema"]["properties"]
            .get("output_schema")
            .is_some(),
        "spawn_task does not expose output_schema",
    )?;
    check(
        spawn["inputSchema"]["properties"].get("effort").is_some(),
        "spawn_task does not expose effort",
    )?;
    let read = client
        .rpc(
            "resources/read",
            json!({"uri": "skill://tosi/delegate/SKILL.md"}),
        )
        .await?;
    check(
        read["contents"][0]["text"] == include_str!("../resources/skills/delegate/SKILL.md"),
        "embedded skill differs from plugin source",
    )
}

async fn structured_follow_up(client: &mut Client, backend: &str, cwd: &Path) -> TestResult {
    inspect_contract(client).await?;
    let token = Uuid::new_v4().to_string();
    let context = cwd.join("context.txt");
    std::fs::write(&context, &token).map_err(|e| e.to_string())?;
    let schema = json!({
        "type": "object",
        "properties": {
            "token": {"type": "string"},
            "phase": {"type": "string", "enum": ["initial", "follow_up"]}
        },
        "required": ["token", "phase"],
        "additionalProperties": false
    });
    let first = client.tool("spawn_task", json!({
        "backend": backend, "cwd": cwd, "access": "read_only", "timeout_secs": 120, "effort": "low",
        "task": "Read context.txt in the current directory. Return the file's complete token and phase initial using the requested output schema. Remember the token for a follow-up. Do not edit files.",
        "output_schema": schema
    })).await?;
    let first = completed(client, &first).await?;
    check(first["effort"] == "low", "lost requested effort")?;
    check(
        first["selection"]["requested"]["backend"] == backend,
        "lost requested backend",
    )?;
    check(
        first["selection"]["requested"]["effort"] == "low",
        "lost requested selection",
    )?;
    check(
        first["selection"]["selected"]["effort"] == "low",
        "lost selected effort",
    )?;
    check(
        first["selection"]["selected"]["model"].is_null(),
        "invented default model",
    )?;
    check(
        first["selection"]["reported"] == json!({"model":null,"effort":null}),
        "invented provider-reported settings",
    )?;
    check(
        first["structured_result"] == json!({"token": token, "phase": "initial"}),
        format!("wrong initial structured result: {first}"),
    )?;
    let session = string_field(&first, "provider_session_id")?;
    std::fs::remove_file(context).map_err(|e| e.to_string())?;
    let next = client.tool("continue_task", json!({
        "task_id": first["task_id"],
        "message": "The context file has been removed. Recall the exact token you read in the previous turn without reading files. Return that token and phase follow_up using the same output schema. Do not edit files."
    })).await?;
    let next = completed(client, &next).await?;
    check(
        next["effort"] == "low" && next["selection"] == first["selection"],
        "continuation changed selection",
    )?;
    check(
        next["structured_result"] == json!({"token": token, "phase": "follow_up"}),
        format!("wrong continued structured result: {next}"),
    )?;
    check(
        next["task_id"] != first["task_id"],
        "follow-up reused task ID",
    )?;
    check(
        next["parent_task_id"] == first["task_id"],
        "follow-up lost parent task",
    )?;
    check(
        next["conversation_id"] == first["conversation_id"],
        "follow-up lost conversation",
    )?;
    check(
        next["provider_session_id"] == session,
        "follow-up lost provider session",
    )?;
    check(
        next["result_truncated"] == false,
        "structured result was truncated",
    )?;
    check(
        next["error"].is_null(),
        "completed structured task has an error",
    )?;
    eprintln!(
        "LIVE {backend}: explicit low effort accepted, schema output and continued provider session verified; effective model/effort unreported"
    );
    Ok(())
}

async fn workspace_write(client: &mut Client, backend: &str, cwd: &Path) -> TestResult {
    let token = Uuid::new_v4().to_string();
    let task = client.tool("spawn_task", json!({
        "backend": backend, "cwd": cwd, "access": "workspace_write", "timeout_secs": 120,
        "task": format!("Create only proof.txt in the current directory with exactly {token} followed by one newline. Use your file editing tool. Then reply briefly confirming completion. Do not change any other file.")
    })).await?;
    let task = completed(client, &task).await?;
    let content = std::fs::read_to_string(cwd.join("proof.txt")).map_err(|e| e.to_string())?;
    check(
        content == format!("{token}\n"),
        format!("unexpected file content: {content:?}"),
    )?;
    check(
        task["structured_result"].is_null(),
        "ordinary task returned structured_result",
    )?;
    string_field(&task, "result")?;
    eprintln!("LIVE {backend}: workspace write and ordinary text result verified");
    Ok(())
}

async fn run_case(backend: &str, structured: bool) -> TestResult {
    if !enabled(backend)? {
        eprintln!("SKIP {backend} live test: set TOSI_LIVE_TESTS=1 and select this backend");
        return Ok(());
    }
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    // Initialization failures clean themselves up. Every subsequent failure
    // goes through finish to allow server cleanup before removing the fixture.
    let mut client = Client::start(env!("CARGO_BIN_EXE_tosi")).await?;
    let result = if structured {
        structured_follow_up(&mut client, backend, directory.path()).await
    } else {
        workspace_write(&mut client, backend, directory.path()).await
    };
    let cleanup = client.finish().await;
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}\nCleanup also failed: {cleanup}")),
    }
}

#[tokio::test]
async fn claude_structured_output_and_follow_up() -> TestResult {
    run_case("claude", true).await
}

#[tokio::test]
async fn codex_structured_output_and_follow_up() -> TestResult {
    run_case("codex", true).await
}

#[tokio::test]
async fn claude_workspace_write() -> TestResult {
    run_case("claude", false).await
}

#[tokio::test]
async fn codex_workspace_write() -> TestResult {
    run_case("codex", false).await
}

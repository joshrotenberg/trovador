//! The CLI and the MCP tool must return what the library returns. spike had four separate
//! bugs from its two doors computing the same answer differently.

mod common;

use std::process::Command;

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::sessions::{self, ListOpts};

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "claude question"),
            claude_tool_use("Read", json!({"file_path": "/w/a.rs"})),
            claude_assistant("claude answer"),
        ],
        20,
    );
    f.codex(
        ID_B,
        &[
            codex_meta("/w", json!("exec")),
            codex_user_item("codex question"),
            codex_agent_item("codex answer"),
        ],
        10,
    );
    f
}

fn cli(f: &Fixture, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(args)
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_HOME", &f.paths.home)
        .output()
        .expect("run trovador");
    assert!(
        out.status.success(),
        "trovador {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is one JSON document")
}

async fn mcp(f: &Fixture, args: Value) -> Value {
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    client.call_tool_json("sessions", args).await
}

#[tokio::test]
async fn list_is_identical_from_the_library_the_cli_and_the_mcp_tool() {
    let f = fixture();
    let lib = serde_json::to_value(sessions::list(&f.ctx(), &ListOpts::default())).unwrap();
    assert_eq!(lib["returned"], 2);
    assert_eq!(cli(&f, &["sessions", "list"]), lib);
    assert_eq!(mcp(&f, json!({"action": "list"})).await, lib);
}

#[tokio::test]
async fn show_and_transcript_are_identical_through_both_doors() {
    let f = fixture();
    let ctx = f.ctx();
    let show = serde_json::to_value(sessions::show(&ctx, "bbbb", None).unwrap()).unwrap();
    assert_eq!(cli(&f, &["sessions", "show", "bbbb"]), show);
    assert_eq!(
        mcp(&f, json!({"action": "show", "session_id": "bbbb"})).await,
        show
    );

    let page =
        serde_json::to_value(sessions::transcript(&ctx, ID_A, None, 1, Some(1)).unwrap()).unwrap();
    assert_eq!(page["records"][0]["type"], "tool_call");
    assert_eq!(
        cli(
            &f,
            &[
                "sessions",
                "transcript",
                ID_A,
                "--offset",
                "1",
                "--limit",
                "1"
            ]
        ),
        page
    );
    assert_eq!(
        mcp(
            &f,
            json!({"action": "transcript", "session_id": ID_A, "offset": 1, "limit": 1})
        )
        .await,
        page
    );
}

#[tokio::test]
async fn an_unknown_session_is_a_readable_tool_error_not_a_protocol_failure() {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let result = client
        .call_tool("sessions", json!({"action": "show", "session_id": "ffff"}))
        .await;
    assert!(result.is_error);
    assert!(
        result
            .all_text()
            .contains("no archived session matches \"ffff\"")
    );

    let missing = client
        .call_tool("sessions", json!({"action": "transcript"}))
        .await;
    assert!(missing.is_error);
    assert!(missing.all_text().contains("session_id is required"));
}

#[tokio::test]
async fn the_session_resource_matches_show() {
    let f = fixture();
    let show = serde_json::to_value(sessions::show(&f.ctx(), ID_B, None).unwrap()).unwrap();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let res = client
        .read_resource(&format!("session://codex/{ID_B}"))
        .await;
    let body: Value = serde_json::from_str(res.first_text().unwrap()).unwrap();
    assert_eq!(body, show);

    let t = client
        .read_resource(&format!("session://claude/{ID_A}/transcript"))
        .await;
    let body: Value = serde_json::from_str(t.first_text().unwrap()).unwrap();
    assert_eq!(body["records"][0]["text"], "claude question");
}

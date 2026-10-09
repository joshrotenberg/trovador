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
            codex_meta("/w", json!("cli")),
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
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("TROVADOR_HOME", &f.paths.home)
        .env("TROVADOR_ACTIVE_WINDOW_S", "0")
        .env(
            "TROVADOR_CLAUDE_BIN",
            f.paths.claude_bin.clone().unwrap_or_default(),
        )
        .env(
            "TROVADOR_CODEX_BIN",
            f.paths.codex_bin.clone().unwrap_or_default(),
        )
        .output()
        .expect("run trovador");
    assert!(
        out.status.success(),
        "trovador {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is one JSON document")
}

async fn mcp_tool(f: &Fixture, tool: &str, args: Value) -> Value {
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    client.call_tool_json(tool, args).await
}

async fn mcp(f: &Fixture, args: Value) -> Value {
    mcp_tool(f, "sessions", args).await
}

#[tokio::test]
async fn list_is_identical_from_the_library_the_cli_and_the_mcp_tool() {
    let f = fixture();
    let lib =
        serde_json::to_value(sessions::list(&f.ctx(), &ListOpts::default()).unwrap()).unwrap();
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

fn without_timing(mut v: Value) -> Value {
    if let Some(r) = v.pointer_mut("/coverage/refresh") {
        r.as_object_mut().unwrap().remove("ms");
    }
    v
}

#[tokio::test]
async fn search_is_identical_from_the_library_the_cli_and_the_mcp_tool() {
    let f = fixture();
    // Index once so every door sees the same unchanged archive.
    sessions::reindex(&f.ctx(), false).unwrap();
    let opts = trovador::index::SearchOpts {
        query: "question".into(),
        ..Default::default()
    };
    let lib =
        without_timing(serde_json::to_value(sessions::search(&f.ctx(), &opts).unwrap()).unwrap());
    assert_eq!(lib["sessions_matched"], 2);
    assert_eq!(lib["coverage"]["refresh"]["unchanged"], 2);
    assert_eq!(without_timing(cli(&f, &["search", "question"])), lib);
    assert_eq!(
        without_timing(mcp_tool(&f, "search", json!({"query": "question"})).await),
        lib
    );
}

#[tokio::test]
async fn a_bad_query_is_a_readable_tool_error() {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let r = client
        .call_tool("search", json!({"query": "(", "regex": true}))
        .await;
    assert!(r.is_error);
    assert!(r.all_text().starts_with("invalid query"));
}

#[tokio::test]
async fn no_mcp_tool_can_change_consent() {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let names: Vec<String> = client
        .list_tools()
        .await
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    for n in ["sessions", "search", "summarize"] {
        assert!(names.iter().any(|x| x == n));
    }
    assert!(names.iter().all(|n| !n.contains("consent")));
}

#[tokio::test]
async fn a_cached_summary_is_identical_through_both_doors_and_the_resource() {
    let f = fixture().with_fakes();
    let ctx = f.ctx();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    // The resource only serves what exists; it never spends tokens making one.
    let missing = client
        .send_request_expect_error(
            "resources/read",
            Some(json!({"uri": format!("session://claude/{ID_A}/summary")})),
        )
        .await;
    assert!(
        missing.to_string().contains("no current summary"),
        "{missing}"
    );
    assert!(f.calls("claude").is_empty());

    trovador::summary::summarize(&ctx, ID_A, None, false, None)
        .await
        .unwrap();
    let lib = serde_json::to_value(
        trovador::summary::summarize(&ctx, ID_A, None, false, None)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(lib["cached"], true);
    assert_eq!(cli(&f, &["summarize", ID_A]), lib);
    assert_eq!(
        client
            .call_tool_json("summarize", json!({"session_id": ID_A}))
            .await,
        lib
    );
    let res = client
        .read_resource(&format!("session://claude/{ID_A}/summary"))
        .await;
    let body: Value = serde_json::from_str(res.first_text().unwrap()).unwrap();
    assert_eq!(body, lib);
    assert_eq!(
        f.calls("claude").len(),
        1,
        "only the first summary ran a model"
    );
}

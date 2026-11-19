//! Reading the part of a tool record a deep search match found (#107). Search indexes
//! 4,000 characters of a tool result and a transcript shows 600, so a match past 600 opened
//! a record that did not contain it.

mod common;

use std::process::Command;

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::index::SearchOpts;
use trovador::sessions;

const NEEDLE: &str = "zqdeeprecordneedle";
const KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";

/// Record 0 says the needle near its start; record 2, a tool result, 3,000 characters in.
fn fixture() -> Fixture {
    let f = Fixture::new();
    let output = format!(
        "{}{NEEDLE} then the failure {KEY}{}",
        "x ".repeat(1_500),
        " y".repeat(400)
    );
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", &format!("{NEEDLE} please look")),
            claude_tool_use("Bash", json!({"command": "cargo test"})),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "content": output}]}}),
        ],
        100,
    );
    f
}

fn search(f: &Fixture) -> sessions::SearchResult {
    sessions::search(
        &f.ctx(),
        &SearchOpts {
            query: NEEDLE.into(),
            per_session: Some(5),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn only_a_match_past_what_the_transcript_shows_carries_an_offset() {
    let f = fixture();
    let r = search(&f);
    let m = &r.hits[0].matches;
    let user = m.iter().find(|m| m.kind == "user").unwrap();
    let tool = m.iter().find(|m| m.kind == "tool_result").unwrap();
    assert_eq!(user.offset, None);
    assert_eq!(tool.offset, Some(3_000));
}

#[test]
fn the_record_at_a_matchs_offset_holds_the_match() {
    let f = fixture();
    let c = f.ctx();
    let m = search(&f).hits[0]
        .matches
        .iter()
        .find(|m| m.offset.is_some())
        .cloned()
        .unwrap();
    let from = m.offset.unwrap() - 100;
    let r = sessions::record_at(&c, ID_A, None, m.index, from, None).unwrap();
    assert_eq!(r.index, m.index);
    assert!(r.text.contains(NEEDLE), "{}", &r.text[..200]);
    assert!(r.text.contains("[redacted:anthropic]"));
    assert!(!r.text.contains(KEY));
    let t = r.truncated.unwrap();
    assert_eq!(t.offset, from);
    assert_eq!(t.next_offset, None, "the rest fits in one part");
}

#[test]
fn a_long_record_comes_in_parts_that_rebuild_it() {
    let f = fixture();
    let c = f.ctx_with(trovador::Limits {
        response_max_chars: 700,
        ..limits()
    });
    let mut whole = String::new();
    let mut offset = 0;
    let mut revision = None::<String>;
    loop {
        let r = sessions::record_at(&c, ID_A, None, 2, offset, revision.as_deref()).unwrap();
        whole.push_str(&r.text);
        revision = Some(r.revision.clone());
        match r.truncated.and_then(|t| t.next_offset) {
            Some(n) => offset = n,
            None => break,
        }
    }
    assert!(whole.starts_with("x x "));
    assert!(whole.contains(&format!("{NEEDLE} then the failure [redacted:anthropic]")));
    assert!(whole.trim_end().ends_with('y'));
}

#[test]
fn a_record_past_the_end_names_how_many_there_are() {
    let f = fixture();
    let e = sessions::record_at(&f.ctx(), ID_A, None, 99, 0, None).unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("record 99 is past the end of claude session {ID_A}, which has 3 records")
    );
}

#[tokio::test]
async fn record_is_the_same_through_the_library_the_cli_and_mcp() {
    let f = fixture();
    let lib =
        serde_json::to_value(sessions::record_at(&f.ctx(), ID_A, None, 2, 2_900, None).unwrap())
            .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(["sessions", "record", ID_A, "2", "--offset", "2900"])
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("TROVADOR_HOME", &f.paths.home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cli: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(cli, lib);
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let mcp = client
        .call_tool_json(
            "sessions",
            json!({"action": "record", "session_id": ID_A, "index": 2, "offset": 2900}),
        )
        .await;
    assert_eq!(mcp, lib);
    let missing = client
        .call_tool("sessions", json!({"action": "record", "session_id": ID_A}))
        .await;
    assert!(missing.is_error);
    assert!(missing.all_text().contains("index is required"));
}

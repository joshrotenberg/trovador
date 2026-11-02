//! A session's text is held to response_max_chars, and the rest is read by offset
//! (trovador#32).

mod common;

use std::process::Command;

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::sessions::{self, Ctx};

const MAX: usize = 8;

fn small(f: &Fixture) -> Ctx {
    let mut limits = limits();
    limits.response_max_chars = MAX;
    f.ctx_with(limits)
}

fn cli(f: &Fixture, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(args)
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("TROVADOR_HOME", &f.paths.home)
        .env("TROVADOR_ACTIVE_WINDOW_S", "0")
        .env("TROVADOR_RESPONSE_MAX_CHARS", MAX.to_string())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

async fn mcp(f: &Fixture, args: Value) -> Value {
    let mut c = TestClient::from_router(trovador::mcp::router(small(f)));
    c.initialize().await;
    c.call_tool_json("sessions", args).await
}

/// Every part from offset 0, joined.
fn whole(ctx: &Ctx, id: &str) -> String {
    let mut text = String::new();
    let mut next = Some(0);
    while let Some(offset) = next {
        let part = sessions::text(ctx, id, None, offset).unwrap();
        assert!(part.text.chars().count() <= ctx.limits.response_max_chars.max(1));
        text.push_str(&part.text);
        next = part.truncated.and_then(|t| t.next_offset);
    }
    text
}

#[tokio::test]
async fn a_long_session_text_is_paged_identically_through_every_door() {
    let f = Fixture::new();
    let ctx = small(&f);
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "first question"),
            claude_assistant("an answer that runs past the budget"),
        ],
        10,
    );
    let first = sessions::text(&ctx, ID_A, None, 0).unwrap();
    assert_eq!(first.text.chars().count(), MAX);
    let truncated = first.truncated.clone().unwrap();
    assert_eq!(truncated.next_offset, Some(MAX));
    assert!(
        truncated
            .continue_with
            .unwrap()
            .contains("trovador sessions text")
    );

    let joined = whole(&ctx, ID_A);
    assert_eq!(
        joined,
        "USER: first question\nASSISTANT: an answer that runs past the budget\n"
    );
    assert_eq!(truncated.total_chars, joined.chars().count());

    let lib = serde_json::to_value(sessions::text(&ctx, ID_A, None, MAX).unwrap()).unwrap();
    assert_eq!(
        cli(
            &f,
            &["sessions", "text", ID_A, "--offset", &MAX.to_string()]
        ),
        lib
    );
    assert_eq!(
        mcp(
            &f,
            json!({"action": "text", "session_id": ID_A, "offset": MAX})
        )
        .await,
        lib
    );
}

#[test]
fn a_short_session_text_comes_back_whole_and_unmarked() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "hi")], 10);
    let t = sessions::text(&f.ctx(), ID_A, None, 0).unwrap();
    assert_eq!(t.text, "USER: hi\n");
    assert!(t.truncated.is_none());
}

#[test]
fn omitted_and_reasoning_records_are_left_out_of_the_text() {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "hi"),
            claude_attachment("hook_success"),
            json!({"type": "assistant", "message": {"role": "assistant",
                   "content": [{"type": "thinking", "thinking": "private musing"}]}}),
            claude_assistant("hello"),
        ],
        10,
    );
    let t = sessions::text(&f.ctx(), ID_A, None, 0).unwrap();
    assert_eq!(t.text, "USER: hi\nASSISTANT: hello\n");
}

const KEY: &str = "sk-ant-api03-AAAAAAAAAAbbbbbbbbbbCCCCCCCCCCdddddddddd";

/// No part, from any offset, may carry a recognisable piece of KEY.
fn assert_no_fragment(part: &str, at: usize) {
    for piece in ["AAAA", "bbbb", "CCCC", "dddd", "api03", "ant-"] {
        assert!(!part.contains(piece), "offset {at}: {part:?} holds {piece}");
    }
}

#[test]
fn no_offset_returns_part_of_a_secret() {
    // Pre-release review: parts were cut before redaction, so an offset one character into
    // a key, or a key straddling a part boundary, returned the rest of it in the clear.
    let f = Fixture::new();
    let ctx = small(&f);
    f.claude(
        "-w",
        ID_A,
        &[claude_user(
            "/w",
            &format!("the key was {KEY} and it was rotated"),
        )],
        10,
    );
    let total = sessions::text(&ctx, ID_A, None, 0)
        .unwrap()
        .truncated
        .unwrap()
        .total_chars;
    for offset in 0..total {
        assert_no_fragment(
            &sessions::text(&ctx, ID_A, None, offset).unwrap().text,
            offset,
        );
    }

    // --no-redact still returns the key whole, in order.
    let mut raw = ctx.clone();
    raw.limits.redact = false;
    assert!(whole(&raw, ID_A).contains(KEY));
}

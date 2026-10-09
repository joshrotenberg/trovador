//! Summaries are held to response_max_chars, and the rest is read by offset (trovador#32).

mod common;

use std::process::Command;

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::sessions::Ctx;
use trovador::summary;

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
        .env("BOTTEGA_TEST_ROOT", f.dir.path())
        .env("BOTTEGA_HOME", f.paths.shared.parent().unwrap())
        .env("TROVADOR_HOME", &f.paths.home)
        .env("TROVADOR_ACTIVE_WINDOW_S", "0")
        .env("TROVADOR_RESPONSE_MAX_CHARS", MAX.to_string())
        .env("TROVADOR_CLAUDE_BIN", f.paths.claude_bin.clone().unwrap())
        .env("TROVADOR_CODEX_BIN", f.paths.codex_bin.clone().unwrap())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

async fn mcp(f: &Fixture, tool: &str, args: Value) -> Value {
    let mut c = TestClient::from_router(trovador::mcp::router(small(f)));
    c.initialize().await;
    c.call_tool_json(tool, args).await
}

#[tokio::test]
async fn a_long_summary_is_paged_from_the_cache_without_another_provider_call() {
    let f = Fixture::new().with_fakes();
    f.claude("-w", ID_A, &[claude_user("/w", "do the thing")], 10);
    let ctx = small(&f);
    let whole = "single summary";

    let first = summary::part(
        summary::summarize(&ctx, ID_A, None, false, None)
            .await
            .unwrap(),
        0,
        &ctx.limits,
    );
    assert_eq!(first.summary, &whole[..MAX]);
    let next = first.truncated.as_ref().unwrap().next_offset.unwrap();
    assert_eq!(f.calls("claude").len(), 1);

    let rest = summary::part(
        summary::summarize(&ctx, ID_A, None, false, None)
            .await
            .unwrap(),
        next,
        &ctx.limits,
    );
    assert!(rest.cached);
    assert_eq!(rest.summary, &whole[MAX..]);
    assert_eq!(rest.truncated.as_ref().unwrap().next_offset, None);
    assert_eq!(
        f.calls("claude").len(),
        1,
        "the second part came from the cache"
    );

    let lib = serde_json::to_value(&rest).unwrap();
    assert_eq!(
        cli(&f, &["summarize", ID_A, "--offset", &next.to_string()]),
        lib
    );
    assert_eq!(
        mcp(&f, "summarize", json!({"session_id": ID_A, "offset": next})).await,
        lib
    );

    // The cache keeps the whole text, so a consult is seeded with all of it.
    let e = trovador::archive::find_session(&ctx.paths, ID_A, None).unwrap();
    let cached = summary::cached(&ctx, &e).unwrap();
    assert_eq!(cached.summary, whole);
    assert!(cached.truncated.is_none());
}

const KEY: &str = "sk-ant-api03-AAAAAAAAAAbbbbbbbbbbCCCCCCCCCCdddddddddd";

/// No part, from any offset, may carry a recognisable piece of KEY.
fn assert_no_fragment(part: &str, at: usize) {
    for piece in ["AAAA", "bbbb", "CCCC", "dddd", "api03", "ant-"] {
        assert!(!part.contains(piece), "offset {at}: {part:?} holds {piece}");
    }
}

#[tokio::test]
async fn no_offset_returns_part_of_a_secret() {
    // Pre-release review: parts were cut before redaction, so an offset one character into
    // a key, or a key straddling a part boundary, returned the rest of it in the clear.
    let f = Fixture::new().with_fakes();
    let ctx = small(&f);
    f.claude("-w", ID_A, &[claude_user("/w", "x")], 10);
    let mut s = summary::summarize(&ctx, ID_A, None, false, None)
        .await
        .unwrap();
    s.summary = format!("the key was {KEY} and it was rotated");
    let total = summary::part(s.clone(), 0, &ctx.limits)
        .truncated
        .unwrap()
        .total_chars;
    for offset in 0..total {
        assert_no_fragment(
            &summary::part(s.clone(), offset, &ctx.limits).summary,
            offset,
        );
    }

    // --no-redact still returns the key whole, in order.
    let mut raw = ctx.clone();
    raw.limits.redact = false;
    let mut text = String::new();
    let mut next = Some(0);
    while let Some(offset) = next {
        let part = summary::part(s.clone(), offset, &raw.limits);
        text.push_str(&part.summary);
        next = part.truncated.and_then(|t| t.next_offset);
    }
    assert!(text.contains(KEY), "{text}");
}

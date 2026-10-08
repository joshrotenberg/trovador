//! Model text is held to response_max_chars, and the rest can be read back (#32).

mod common;

use std::process::Command;
use std::time::Duration;

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::output::{self, OutputError};
use trovador::run::{self, AskOpts};
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
async fn a_long_reply_is_cut_saved_and_read_back_the_same_through_both_doors() {
    let f = Fixture::new().with_fakes();
    let ctx = small(&f);
    let reply = "echo: tell me everything";
    let r = run::ask(
        &ctx,
        &AskOpts {
            prompt: "tell me everything".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(r.result, &reply[..MAX]);
    let t = r.result_truncated.clone().unwrap();
    assert_eq!(
        (t.total_chars, t.offset, t.returned_chars, t.next_offset),
        (reply.len(), 0, MAX, Some(MAX))
    );
    assert!(t.continue_with.unwrap().contains(&r.run_id));

    // Following next_offset reads the whole reply, once.
    let mut text = r.result.clone();
    let mut next = Some(MAX);
    while let Some(offset) = next {
        let part = output::read(&ctx, &r.run_id, offset).unwrap();
        text.push_str(&part.text);
        next = part.truncated.and_then(|t| t.next_offset);
    }
    assert_eq!(text, reply);

    let lib = serde_json::to_value(output::read(&ctx, &r.run_id, MAX).unwrap()).unwrap();
    let id = r.run_id.as_str();
    let from_cli = cli(
        &f,
        &["sessions", "output", id, "--offset", &MAX.to_string()],
    );
    let from_mcp = mcp(
        &f,
        "sessions",
        json!({"action": "output", "run_id": id, "offset": MAX}),
    )
    .await;
    assert_eq!(from_cli, lib);
    assert_eq!(from_mcp, lib);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let saved = output::dir(&ctx.paths).join(format!("{id}.txt"));
        let mode = std::fs::metadata(saved).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[tokio::test]
async fn a_reply_within_the_budget_is_whole_and_nothing_is_saved() {
    let f = Fixture::new().with_fakes();
    let r = run::ask(
        &f.ctx(),
        &AskOpts {
            prompt: "hi".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(r.result, "echo: hi");
    assert!(r.result_truncated.is_none());
    assert!(!output::dir(&f.paths).exists());
}

#[test]
fn only_a_uuid_names_a_saved_output() {
    let f = Fixture::new();
    let ctx = f.ctx();
    for bad in ["../consent", "", "not-a-uuid", "/etc/passwd"] {
        assert!(
            matches!(output::read(&ctx, bad, 0), Err(OutputError::BadId(_))),
            "{bad}"
        );
    }
    assert!(matches!(
        output::read(&ctx, "00000000-0000-4000-8000-000000000000", 0),
        Err(OutputError::NotFound(_))
    ));
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
    run::ask(
        &ctx,
        &AskOpts {
            prompt: "what was decided?".into(),
            about: Some(ID_A.into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(f.prompts("codex").iter().any(|p| p.contains(whole)));
}

#[tokio::test]
async fn gc_removes_saved_replies_past_its_cutoff() {
    let f = Fixture::new().with_fakes();
    let ctx = small(&f);
    let r = run::ask(
        &ctx,
        &AskOpts {
            prompt: "tell me everything".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let kept = trovador::gc::gc(&ctx, Duration::from_secs(86_400), true).unwrap();
    assert_eq!((kept.outputs.kept, kept.outputs.old), (1, 0));
    std::thread::sleep(Duration::from_millis(20));
    let dry = trovador::gc::gc(&ctx, Duration::ZERO, false).unwrap();
    assert_eq!(dry.outputs.old, 1);
    assert!(
        output::read(&ctx, &r.run_id, 0).is_ok(),
        "a dry run removes nothing"
    );
    trovador::gc::gc(&ctx, Duration::ZERO, true).unwrap();
    assert!(matches!(
        output::read(&ctx, &r.run_id, 0),
        Err(OutputError::NotFound(_))
    ));
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
    let r = run::ask(
        &ctx,
        &AskOpts {
            prompt: format!("use {KEY} now"),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_no_fragment(&r.result, 0);
    let total = r.result_truncated.as_ref().unwrap().total_chars;
    for offset in 0..total {
        let part = output::read(&ctx, &r.run_id, offset).unwrap();
        assert_no_fragment(&part.text, offset);
    }

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
        let part = output::read(&raw, &r.run_id, offset).unwrap();
        text.push_str(&part.text);
        next = part.truncated.and_then(|t| t.next_offset);
    }
    assert!(text.contains(KEY), "{text}");
}

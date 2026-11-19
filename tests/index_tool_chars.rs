//! Tool output is indexed past what a transcript shows (#101). A search for a word 2,278
//! characters into a tool result found nothing when the index kept only the 600
//! characters a transcript displays.

mod common;

use common::*;
use serde_json::json;
use trovador::index::SearchOpts;
use trovador::sessions;
use trovador::transcript::RecordKind;
use trovador::{Ctx, Limits};

const NEEDLE: &str = "zqdeeptoolneedle";

/// A Claude session whose tool result names the needle 3,000 characters in.
fn fixture() -> Fixture {
    let f = Fixture::new();
    let output = format!(
        "{}{NEEDLE} at the end{}",
        "x ".repeat(1_500),
        " y".repeat(500)
    );
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "read the file"),
            claude_tool_use("Read", json!({"file_path": "/w/big.txt"})),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "content": output}]}}),
            claude_assistant("done"),
        ],
        100,
    );
    f
}

fn ctx(f: &Fixture, edit: impl FnOnce(&mut Limits)) -> Ctx {
    let mut l = limits();
    edit(&mut l);
    f.ctx_with(l)
}

fn search(ctx: &Ctx) -> sessions::SearchResult {
    sessions::search(
        ctx,
        &SearchOpts {
            query: NEEDLE.into(),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn a_word_deep_in_a_tool_result_is_found_with_an_excerpt_around_it() {
    let f = fixture();
    let r = search(&f.ctx());
    assert_eq!(r.sessions_matched, 1);
    let m = &r.hits[0].matches[0];
    assert_eq!(m.kind, "tool_result");
    assert!(m.excerpt.contains(&format!("[{NEEDLE}]")), "{}", m.excerpt);
    assert!(m.excerpt.len() < 400, "an excerpt, not the record");
}

#[test]
fn the_transcript_at_that_match_still_shows_the_display_cap() {
    let f = fixture();
    let c = f.ctx();
    let at = search(&c).hits[0].matches[0].index;
    let page = sessions::transcript(&c, ID_A, None, at, Some(1))
        .unwrap()
        .page;
    let rec = &page.records[0];
    assert_eq!(rec.kind, RecordKind::ToolResult);
    let text = rec.text.as_deref().unwrap();
    assert!(text.ends_with(" ...[truncated]"));
    assert!(text.chars().count() <= c.limits.tool_chars + 15);
    assert!(!text.contains(NEEDLE));
}

#[test]
fn an_index_cap_at_the_display_cap_misses_it_as_before() {
    let f = fixture();
    let c = ctx(&f, |l| l.index_tool_chars = l.tool_chars);
    assert_eq!(search(&c).sessions_matched, 0);
}

#[test]
fn changing_the_index_cap_rebuilds_the_index_and_the_display_cap_does_not() {
    let f = fixture();
    let narrow = ctx(&f, |l| l.index_tool_chars = 600);
    assert_eq!(search(&narrow).sessions_matched, 0);
    let wide = f.ctx();
    let r = search(&wide);
    assert_eq!(r.coverage.refresh.reindexed, 1, "a fresh index");
    assert_eq!(r.sessions_matched, 1);
    let shorter = ctx(&f, |l| l.tool_chars = 100);
    let r = search(&shorter);
    assert_eq!(
        r.coverage.refresh.reindexed, 0,
        "display caps are not indexed"
    );
    assert_eq!(r.sessions_matched, 1);
}

#[test]
fn tool_call_arguments_are_indexed_past_the_display_cap_too() {
    let f = Fixture::new();
    let command = format!("{} && echo {NEEDLE}", "true ".repeat(400));
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "run it"),
            claude_tool_use("Bash", json!({"command": command})),
        ],
        100,
    );
    let r = search(&f.ctx());
    assert_eq!(r.sessions_matched, 1);
    assert_eq!(r.hits[0].matches[0].kind, "tool_call");
}

#[test]
fn the_index_cap_is_a_setting() {
    assert_eq!(Limits::default().index_tool_chars, 4_000);
    let l = Limits::from_lookup(|k| (k == "TROVADOR_INDEX_TOOL_CHARS").then(|| "9000".into()));
    assert_eq!(l.index_tool_chars, 9_000);
    assert_eq!(l.for_index().tool_chars, 9_000);
    assert_eq!(l.tool_chars, 600);
}

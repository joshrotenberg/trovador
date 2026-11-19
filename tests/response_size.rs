//! Every response fits a context. A caller once could ask for `--limit 5000
//! --per-session 100` and get 31 MB back, and a default text part was 100,000 characters,
//! twice what Claude Code shows a model inline before saving the result to a file.

mod common;

use std::collections::HashSet;

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::index::SearchOpts;
use trovador::sessions::{self, ListOpts};
use trovador::{Ctx, Limits};

const NEEDLE: &str = "zqsizeneedle";

fn id(i: usize) -> String {
    format!("{i:08x}-0000-4000-8000-{i:012x}")
}

/// `n` sessions, newest first by index, each mentioning the needle in several records.
fn fixture(n: usize, records_each: usize) -> Fixture {
    let f = Fixture::new();
    for i in 0..n {
        let records: Vec<Value> = (0..records_each)
            .map(|r| claude_user("/w", &format!("{NEEDLE} record {r} of session {i}")))
            .collect();
        f.claude("-w", &id(i), &records, 100 + i as u64);
    }
    f
}

fn ctx(f: &Fixture, edit: impl FnOnce(&mut Limits)) -> Ctx {
    let mut l = limits();
    edit(&mut l);
    f.ctx_with(l)
}

fn json_len(v: &impl serde::Serialize) -> usize {
    serde_json::to_string(v).unwrap().len()
}

/// What the budget counts: each item as JSON, without the brackets and commas between.
fn items_len<T: serde::Serialize>(items: &[T]) -> usize {
    items.iter().map(json_len).sum()
}

fn list(ctx: &Ctx, limit: Option<usize>, offset: usize) -> sessions::ListResult {
    sessions::list(
        ctx,
        &ListOpts {
            limit,
            offset,
            ..Default::default()
        },
    )
    .unwrap()
}

fn search(
    ctx: &Ctx,
    limit: Option<usize>,
    per_session: Option<usize>,
    offset: usize,
) -> sessions::SearchResult {
    sessions::search(
        ctx,
        &SearchOpts {
            query: NEEDLE.into(),
            limit,
            per_session,
            offset,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn a_list_over_the_size_budget_pages_through_every_session_once() {
    let f = fixture(12, 1);
    let c = ctx(&f, |l| l.response_max_chars = 1_500);
    let mut seen = Vec::new();
    let mut offset = 0;
    loop {
        let page = list(&c, Some(100), offset);
        assert!(!page.sessions.is_empty());
        if page.sessions.len() > 1 {
            assert!(items_len(&page.sessions) <= 1_500, "page over the budget");
        }
        seen.extend(page.sessions.iter().map(|s| s.session_id.clone()));
        match page.next_offset {
            Some(next) => {
                let why = page.truncated_by.as_deref().unwrap_or_default();
                assert!(why.contains("TROVADOR_RESPONSE_MAX_CHARS (1500)"), "{why}");
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(seen.len(), 12, "{seen:?}");
    assert_eq!(seen.iter().collect::<HashSet<_>>().len(), 12, "repeats");
}

#[test]
fn a_search_over_the_size_budget_pages_through_every_session_once() {
    let f = fixture(12, 3);
    let c = ctx(&f, |l| l.response_max_chars = 2_500);
    let mut seen = Vec::new();
    let mut offset = 0;
    let mut pages = 0;
    loop {
        let page = search(&c, Some(50), None, offset);
        pages += 1;
        assert_eq!(page.sessions_matched, 12);
        assert!(!page.hits.is_empty());
        if page.hits.len() > 1 {
            assert!(items_len(&page.hits) <= 2_500, "page over the budget");
        }
        seen.extend(page.hits.iter().map(|h| h.row.session_id.clone()));
        match page.next_offset {
            Some(next) => {
                let why = page.truncated_by.as_deref().unwrap_or_default();
                assert!(why.contains("TROVADOR_RESPONSE_MAX_CHARS"), "{why}");
                assert_eq!(next, offset + page.hits.len());
                offset = next;
            }
            None => break,
        }
    }
    assert!(pages > 1, "the budget never cut a page");
    assert_eq!(seen.len(), 12, "{seen:?}");
    assert_eq!(seen.iter().collect::<HashSet<_>>().len(), 12, "repeats");
}

#[test]
fn an_item_larger_than_the_whole_budget_is_still_returned_one_per_page() {
    let f = fixture(3, 2);
    let c = ctx(&f, |l| l.response_max_chars = 10);
    let l = list(&c, None, 0);
    assert_eq!((l.returned, l.next_offset), (1, Some(1)));
    let s = search(&c, None, None, 0);
    assert_eq!((s.returned, s.next_offset), (1, Some(1)));
    assert!(s.absence_is_proof.is_none());
}

#[test]
fn a_limit_above_the_ceiling_is_lowered_and_says_so() {
    let f = fixture(5, 1);
    let c = ctx(&f, |l| {
        l.list_max = 3;
        l.search_max = 2;
    });
    let l = list(&c, Some(10), 0);
    assert_eq!((l.returned, l.next_offset), (3, Some(3)));
    assert_eq!(
        l.truncated_by.as_deref(),
        Some("limit 10 is above TROVADOR_LIST_MAX (3)")
    );
    let s = search(&c, Some(10), None, 0);
    assert_eq!((s.returned, s.next_offset), (2, Some(2)));
    assert_eq!(
        s.truncated_by.as_deref(),
        Some("limit 10 is above TROVADOR_SEARCH_MAX (2)")
    );
}

#[test]
fn a_limit_within_the_ceilings_reports_nothing() {
    let f = fixture(5, 1);
    let c = f.ctx();
    let l = list(&c, Some(2), 0);
    assert_eq!((l.returned, l.truncated_by), (2, None));
    let s = search(&c, Some(2), None, 0);
    assert_eq!((s.returned, s.truncated_by), (2, None));
    // A ceiling that caps a request already covering every session cuts nothing.
    let c = ctx(&f, |l| l.list_max = 5);
    assert_eq!(list(&c, Some(10), 0).truncated_by, None);
}

#[test]
fn per_session_above_the_ceiling_is_lowered_and_match_count_still_counts_all() {
    let f = fixture(1, 6);
    let c = ctx(&f, |l| l.per_session_max = 2);
    let s = search(&c, None, Some(5), 0);
    assert_eq!(s.hits[0].match_count, 6);
    assert_eq!(s.hits[0].matches.len(), 2);
    assert_eq!(
        s.truncated_by.as_deref(),
        Some("per_session 5 is above TROVADOR_PER_SESSION_MAX (2)")
    );
}

#[test]
fn a_transcript_page_is_budgeted_on_its_records_as_json() {
    // Short records: their text alone is a fraction of what the page serializes to.
    let f = fixture(1, 100);
    let c = ctx(&f, |l| l.response_max_chars = 3_000);
    let page = sessions::transcript(&c, &id(0), None, 0, None)
        .unwrap()
        .page;
    assert!(page.records.len() > 1 && page.has_more);
    let n = items_len(&page.records);
    assert!(n <= 3_000, "{n}");
    assert!(
        page.truncated_by
            .unwrap()
            .contains("TROVADOR_RESPONSE_MAX_CHARS (3000)")
    );
}

#[test]
fn defaults_keep_a_large_search_under_what_hosts_show_inline() {
    // 60 sessions with long matching records, asked for far past every ceiling.
    let f = Fixture::new();
    let long = format!("{NEEDLE} {}", "word ".repeat(2_000));
    for i in 0..60 {
        f.claude(
            "-w",
            &id(i),
            &vec![claude_user("/w", &long); 20],
            100 + i as u64,
        );
    }
    let s = search(&f.ctx(), Some(5_000), Some(100), 0);
    assert!(s.returned < 60 && s.next_offset.is_some());
    assert!(json_len(&s) < 50_000, "{}", json_len(&s));
}

#[tokio::test]
async fn mcp_text_content_is_compact_json_equal_to_the_structured_content() {
    let f = fixture(2, 1);
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    for (tool, args) in [
        ("search", json!({"query": NEEDLE})),
        ("sessions", json!({"action": "list"})),
    ] {
        let result = client.call_tool_raw(tool, args).await;
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains('\n'), "{tool}: pretty-printed");
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed, result["structuredContent"], "{tool}");
        assert_eq!(text, serde_json::to_string(&parsed).unwrap(), "{tool}");
    }
}

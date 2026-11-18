//! Paging through list and search with `offset` and `next_offset`, and the `archived`
//! filter. Every page is read through the library the doors share.

mod common;

use common::*;
use serde_json::json;
use trovador::Limits;
use trovador::index::{Order, SearchOpts};
use trovador::sessions::{self, Ctx, Error, ListOpts};

fn id(n: u32) -> String {
    format!("{n:08x}-0000-4000-8000-000000000000")
}

/// Five Claude sessions that all contain "needle", newest first by number. Longer text for
/// the older ones so relevance order differs from recency order.
fn five() -> Fixture {
    let f = Fixture::new();
    for n in 1..=5u32 {
        let filler = "padding ".repeat((6 - n as usize) * 12);
        f.claude(
            "-w",
            &id(n),
            &[claude_user("/w", &format!("needle {filler}"))],
            u64::from(n) * 100,
        );
    }
    f
}

fn list_page(ctx: &Ctx, offset: usize, limit: usize) -> sessions::ListResult {
    sessions::list(
        ctx,
        &ListOpts {
            offset,
            limit: Some(limit),
            ..Default::default()
        },
    )
    .unwrap()
}

fn search_page(
    ctx: &Ctx,
    opts: &SearchOpts,
    offset: usize,
    limit: usize,
) -> sessions::SearchResult {
    sessions::search(
        ctx,
        &SearchOpts {
            offset,
            limit: Some(limit),
            ..opts.clone()
        },
    )
    .unwrap()
}

fn needle() -> SearchOpts {
    SearchOpts {
        query: "needle".into(),
        ..Default::default()
    }
}

fn list_ids(r: &sessions::ListResult) -> Vec<String> {
    r.sessions.iter().map(|s| s.session_id.clone()).collect()
}

fn hit_ids(r: &sessions::SearchResult) -> Vec<String> {
    r.hits.iter().map(|h| h.row.session_id.clone()).collect()
}

#[test]
fn list_pages_follow_next_offset_and_visit_every_session_once_newest_first() {
    let f = five();
    let ctx = f.ctx();
    let whole = list_ids(&list_page(&ctx, 0, 100));
    assert_eq!(whole, (1..=5).map(id).collect::<Vec<_>>());

    let mut seen = Vec::new();
    let mut offset = 0;
    let mut pages = 0;
    loop {
        let page = list_page(&ctx, offset, 2);
        assert_eq!(page.offset, offset);
        seen.extend(list_ids(&page));
        pages += 1;
        match page.next_offset {
            Some(next) => {
                assert!(page.truncated, "next_offset and truncated agree");
                assert_eq!(next, offset + page.returned);
                offset = next;
            }
            None => {
                assert!(!page.truncated);
                break;
            }
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, whole, "no session repeated or skipped, same order");
}

#[test]
fn a_list_page_that_ends_exactly_at_the_last_session_has_no_next_offset() {
    let f = five();
    let ctx = f.ctx();
    let exact = list_page(&ctx, 0, 5);
    assert_eq!(exact.returned, 5);
    assert_eq!(exact.next_offset, None);
    assert!(!exact.truncated);
    let tail = list_page(&ctx, 3, 2);
    assert_eq!(list_ids(&tail), vec![id(4), id(5)]);
    assert_eq!(tail.next_offset, None);
}

#[test]
fn a_list_offset_past_the_end_is_an_empty_page_not_an_error() {
    let f = five();
    let ctx = f.ctx();
    let r = list_page(&ctx, 50, 2);
    assert_eq!(r.returned, 0);
    assert_eq!(r.offset, 50);
    assert_eq!(r.next_offset, None);
    assert!(!r.truncated);
}

#[test]
fn a_list_offset_counts_sessions_the_filters_keep_not_files_scanned() {
    let f = Fixture::new();
    // An automated session sits between the sessions a person started. It is hidden by
    // default, so it must not occupy a position in the offset.
    f.claude("-w", &id(1), &[claude_user("/w", "one")], 100);
    f.codex(
        &id(2),
        &[codex_meta("/w", json!("exec")), codex_user_item("two")],
        200,
    );
    f.claude("-w", &id(3), &[claude_user("/w", "three")], 300);
    f.claude("-w", &id(4), &[claude_user("/w", "four")], 400);
    let ctx = f.ctx();

    let first = list_page(&ctx, 0, 2);
    assert_eq!(list_ids(&first), vec![id(1), id(3)]);
    assert_eq!(first.next_offset, Some(2));
    let second = list_page(&ctx, 2, 2);
    assert_eq!(list_ids(&second), vec![id(4)]);
    assert_eq!(second.next_offset, None);
    assert_eq!(second.hidden_automated, 1, "counted among those examined");
}

#[test]
fn search_pages_visit_every_session_once_in_each_ordering() {
    let f = five();
    let ctx = f.ctx();
    let orderings = [
        ("relevance", needle()),
        (
            "recent",
            SearchOpts {
                order: Order::Recent,
                ..needle()
            },
        ),
        (
            "regex",
            SearchOpts {
                regex: true,
                ..needle()
            },
        ),
    ];
    for (name, opts) in orderings {
        let whole = search_page(&ctx, &opts, 0, 100);
        assert_eq!(whole.sessions_matched, 5, "{name}");
        assert_eq!(whole.next_offset, None, "{name}");
        let whole_ids = hit_ids(&whole);
        assert_eq!(whole_ids.len(), 5, "{name}");

        let mut seen = Vec::new();
        let mut offset = 0;
        let mut next_offsets = Vec::new();
        loop {
            let page = search_page(&ctx, &opts, offset, 2);
            assert_eq!(page.offset, offset, "{name}");
            assert_eq!(page.sessions_matched, 5, "{name}: the same on every page");
            seen.extend(hit_ids(&page));
            next_offsets.push(page.next_offset);
            match page.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert_eq!(next_offsets, vec![Some(2), Some(4), None], "{name}");
        assert_eq!(seen, whole_ids, "{name}: no repeat, no gap, same order");
    }
}

#[test]
fn relevance_order_differs_from_recent_order_in_the_paging_fixture() {
    // Guards the fixture: if both orderings agreed, the ordering test above would pass
    // for a paging bug that ignored the order.
    let f = five();
    let ctx = f.ctx();
    let relevance = hit_ids(&search_page(&ctx, &needle(), 0, 100));
    let recent = hit_ids(&search_page(
        &ctx,
        &SearchOpts {
            order: Order::Recent,
            ..needle()
        },
        0,
        100,
    ));
    assert_ne!(relevance, recent);
    assert_eq!(recent, (1..=5).map(id).collect::<Vec<_>>());
}

#[test]
fn search_sessions_with_equal_scores_keep_one_order_across_pages() {
    let f = Fixture::new();
    for n in 1..=4u32 {
        // Identical text, so score ties and only the time and id keys decide.
        f.claude("-w", &id(n), &[claude_user("/w", "needle")], u64::from(n));
    }
    let ctx = f.ctx();
    let whole = hit_ids(&search_page(&ctx, &needle(), 0, 100));
    let mut seen = Vec::new();
    for offset in 0..4 {
        seen.extend(hit_ids(&search_page(&ctx, &needle(), offset, 1)));
    }
    assert_eq!(seen, whole);
}

#[test]
fn coverage_fields_describe_the_whole_query_on_a_later_page() {
    let f = five();
    // A matching automated session the default hides, counted on every page.
    f.codex(
        &id(9),
        &[codex_meta("/w", json!("exec")), codex_user_item("needle")],
        50,
    );
    let ctx = f.ctx();
    let first = search_page(&ctx, &needle(), 0, 2);
    let later = search_page(&ctx, &needle(), 2, 2);
    assert_eq!(later.hits.len(), 2);
    assert_eq!(later.sessions_matched, first.sessions_matched);
    assert_eq!(later.hidden.automated, 1);
    assert_eq!(first.hidden.automated, 1);
    assert_eq!(later.exhaustive, first.exhaustive);
    assert!(later.exhaustive);
    assert_eq!(later.absence_is_proof, None, "there are hits on this page");
    assert!(later.note.unwrap().contains("include_automated"));
}

#[test]
fn an_empty_later_page_does_not_claim_proof_of_absence() {
    let f = five();
    let ctx = f.ctx();
    // Past the matches: nothing is hidden and the index is current, so the first page of
    // an empty result would be proof. A later page is not.
    let past = search_page(&ctx, &needle(), 5, 2);
    assert!(past.hits.is_empty());
    assert!(past.exhaustive);
    assert!(!past.hidden.any());
    assert_eq!(past.absence_is_proof, Some(false));
    assert_eq!(past.next_offset, None);
    assert!(past.note.unwrap().contains("5 matched"));

    let nothing = SearchOpts {
        query: "absentword".into(),
        ..Default::default()
    };
    let first = search_page(&ctx, &nothing, 0, 2);
    assert_eq!(
        first.absence_is_proof,
        Some(true),
        "offset zero still proves"
    );
    let later = search_page(&ctx, &nothing, 1, 2);
    assert_eq!(later.absence_is_proof, Some(false));
}

fn archived_fixture() -> Fixture {
    let f = Fixture::new();
    f.claude("-w", &id(1), &[claude_user("/w", "needle claude")], 30);
    f.codex(
        &id(2),
        &[
            codex_meta("/w", json!("cli")),
            codex_user_item("needle live"),
        ],
        20,
    );
    f.codex_archived(
        &id(3),
        &[
            codex_meta("/w", json!("cli")),
            codex_user_item("needle archived"),
        ],
        10,
    );
    f
}

#[test]
fn the_archived_filter_selects_archived_unarchived_or_both_in_list() {
    let f = archived_fixture();
    let ctx = f.ctx();
    let list = |archived: Option<bool>| {
        let r = sessions::list(
            &ctx,
            &ListOpts {
                archived,
                ..Default::default()
            },
        )
        .unwrap();
        (list_ids(&r), r.total_sessions)
    };
    assert_eq!(list(Some(true)), (vec![id(3)], 1));
    assert_eq!(list(Some(false)), (vec![id(2), id(1)], 2));
    assert_eq!(
        list(None),
        (vec![id(3), id(2), id(1)], 3),
        "the default keeps today's behavior: both"
    );
}

#[test]
fn the_archived_filter_selects_archived_unarchived_or_both_in_search() {
    let f = archived_fixture();
    let ctx = f.ctx();
    let run = |archived: Option<bool>| {
        let r = sessions::search(
            &ctx,
            &SearchOpts {
                archived,
                order: Order::Recent,
                ..needle()
            },
        )
        .unwrap();
        (hit_ids(&r), r.sessions_matched, r.hidden.any())
    };
    assert_eq!(run(Some(true)), (vec![id(3)], 1, false));
    assert_eq!(run(Some(false)), (vec![id(2), id(1)], 2, false));
    assert_eq!(run(None), (vec![id(3), id(2), id(1)], 3, false));
}

#[test]
fn archived_true_pages_like_any_other_filter() {
    let f = Fixture::new();
    for n in 1..=3u32 {
        f.codex_archived(
            &id(n),
            &[codex_meta("/w", json!("cli")), codex_user_item("needle")],
            u64::from(n) * 10,
        );
    }
    f.claude("-w", &id(7), &[claude_user("/w", "needle")], 5);
    let ctx = f.ctx();
    let first = sessions::list(
        &ctx,
        &ListOpts {
            archived: Some(true),
            limit: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(list_ids(&first), vec![id(1), id(2)]);
    assert_eq!(first.next_offset, Some(2));
    let second = sessions::list(
        &ctx,
        &ListOpts {
            archived: Some(true),
            limit: Some(2),
            offset: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(list_ids(&second), vec![id(3)]);
    assert_eq!(second.next_offset, None);
}

#[test]
fn an_offset_above_the_limit_is_rejected_with_a_message_that_names_it() {
    let f = five();
    let ctx = f.ctx_with(Limits {
        offset_max: 10,
        ..limits()
    });
    let err = sessions::list(
        &ctx,
        &ListOpts {
            offset: 11,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::Offset(_)));
    let text = err.to_string();
    assert!(text.starts_with("invalid offset"), "{text}");
    assert!(text.contains("11") && text.contains("10"), "{text}");

    let err = sessions::search(
        &ctx,
        &SearchOpts {
            offset: 11,
            ..needle()
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::Offset(_)), "{err}");
    assert!(
        !trovador::index::db_path(&ctx.paths).exists(),
        "rejected before the index was opened"
    );

    // The ceiling itself is allowed: it is an empty page, not an error.
    let ok = sessions::list(
        &ctx,
        &ListOpts {
            offset: 10,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(ok.returned, 0);
}

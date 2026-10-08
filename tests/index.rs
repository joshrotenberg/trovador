mod common;

use std::io::Write;

use common::*;
use serde_json::{Value, json};
use trovador::index::{Index, Order, RefreshOpts, SearchOpts};
use trovador::sessions::{self, Ctx};
use trovador::transcript::RecordKind;
use trovador::{Backend, Limits};

fn append(path: &std::path::Path, records: &[Value]) {
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    for r in records {
        writeln!(f, "{r}").unwrap();
    }
}

fn refresh(ctx: &Ctx) -> trovador::index::RefreshReport {
    let mut idx = Index::open(&ctx.paths).unwrap();
    idx.refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())
        .unwrap()
}

/// What the index holds for a session must be exactly the text-bearing records a full
/// transcript read produces, with the same indexes, or a search hit's index would open the
/// transcript at the wrong record.
fn assert_index_matches_transcript(ctx: &Ctx, backend: Backend, id: &str) {
    let idx = Index::open(&ctx.paths).unwrap();
    let indexed = idx.session_records(backend, id).unwrap();
    let page = sessions::transcript(ctx, id, Some(backend), 0, Some(200))
        .unwrap()
        .page;
    let expected: Vec<(usize, String, String)> = page
        .records
        .into_iter()
        .filter(|r| r.kind != RecordKind::Omitted)
        .map(|r| {
            let kind = serde_json::to_value(r.kind)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string();
            (r.index, kind, r.text.unwrap_or_default())
        })
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(indexed, expected);
}

fn search(ctx: &Ctx, query: &str) -> sessions::SearchResult {
    sessions::search(
        ctx,
        &SearchOpts {
            query: query.into(),
            ..Default::default()
        },
    )
    .unwrap()
}

// --- incremental -----------------------------------------------------------

#[test]
fn an_append_continues_the_open_omitted_run_and_keeps_indexes_aligned() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let path = f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "first"),
            claude_attachment("total_tokens_reminder"),
            claude_attachment("total_tokens_reminder"),
        ],
        0,
    );
    let r1 = refresh(&ctx);
    assert_eq!(r1.reindexed, 1);
    assert_index_matches_transcript(&ctx, Backend::Claude, ID_A);

    // The omitted run that was open at the end of the first pass continues here. A full
    // read collapses all three attachments into one record, so "second" is index 2.
    append(
        &path,
        &[
            claude_attachment("total_tokens_reminder"),
            claude_assistant("second"),
        ],
    );
    let r2 = refresh(&ctx);
    assert_eq!((r2.appended, r2.reindexed), (1, 0));
    assert_index_matches_transcript(&ctx, Backend::Claude, ID_A);
    let records = Index::open(&ctx.paths)
        .unwrap()
        .session_records(Backend::Claude, ID_A)
        .unwrap();
    assert_eq!(records.last().unwrap().0, 2);

    let r3 = refresh(&ctx);
    assert_eq!(r3.unchanged, 1);
}

#[test]
fn a_young_codex_file_that_later_shows_its_item_stream_is_reindexed_not_appended() {
    // Under the pre-scan window with no item event yet, the first pass projects the
    // response_item user turn. Once item events appear, a fresh read would not, so the
    // second pass must start over rather than continue.
    let f = Fixture::new();
    let ctx = f.ctx();
    let path = f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("cli")),
            codex_response("user", "early turn"),
        ],
        0,
    );
    refresh(&ctx);
    append(
        &path,
        &[
            codex_user_item("early turn"),
            codex_response("assistant", "reply"),
            codex_agent_item("reply"),
        ],
    );
    let r = refresh(&ctx);
    assert_eq!((r.appended, r.reindexed), (0, 1));
    assert_index_matches_transcript(&ctx, Backend::Codex, ID_A);
    let idx = Index::open(&ctx.paths).unwrap();
    let users = idx
        .session_records(Backend::Codex, ID_A)
        .unwrap()
        .into_iter()
        .filter(|(_, k, _)| k == "user")
        .count();
    assert_eq!(
        users, 1,
        "the user turn must not be indexed from both streams"
    );
}

#[test]
fn indexing_split_between_a_turn_and_its_copy_in_the_other_stream_matches_a_full_read() {
    // The first pass ends after a first turn larger than the pre-scan window, so its mode
    // is final and its last turn is remembered; the copy arrives in the next pass.
    let f = Fixture::new();
    let ctx = f.ctx();
    let huge = "q".repeat(600_000);
    let path = f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("vscode")),
            codex_response("user", &huge),
        ],
        0,
    );
    refresh(&ctx);
    append(
        &path,
        &[
            codex_user_item(&huge),
            codex_response("assistant", "fine"),
            codex_agent_item("fine"),
        ],
    );
    let r = refresh(&ctx);
    assert_eq!((r.appended, r.reindexed), (1, 0));
    assert_index_matches_transcript(&ctx, Backend::Codex, ID_A);
    let users = Index::open(&ctx.paths)
        .unwrap()
        .session_records(Backend::Codex, ID_A)
        .unwrap()
        .into_iter()
        .filter(|(_, k, _)| k == "user")
        .count();
    assert_eq!(users, 1);
}

#[test]
fn a_torn_final_line_is_left_for_the_next_pass_and_indexed_once() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let path = f.claude("-w", ID_A, &[claude_user("/w", "start")], 0);
    let line = claude_assistant("finished later").to_string();
    let (head, tail) = line.split_at(20);
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(file, "{head}").unwrap();
    }
    refresh(&ctx);
    assert_eq!(search(&ctx, "finished later").sessions_matched, 0);
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{tail}").unwrap();
    }
    let hit = search(&ctx, "finished later");
    assert_eq!(hit.coverage.refresh.appended, 1);
    assert_eq!(hit.sessions_matched, 1);
    assert_eq!(hit.hits[0].match_count, 1);
    assert_index_matches_transcript(&ctx, Backend::Claude, ID_A);
}

#[test]
fn a_rewritten_file_is_reindexed_and_loses_its_old_text() {
    let f = Fixture::new();
    let ctx = f.ctx();
    f.claude("-w", ID_A, &[claude_user("/w", "alpha alpha alpha")], 0);
    refresh(&ctx);
    f.claude("-w", ID_A, &[claude_user("/w", "beta")], 0);
    let r = search(&ctx, "alpha");
    assert_eq!(r.coverage.refresh.reindexed, 1);
    assert_eq!(r.sessions_matched, 0);
    assert_eq!(search(&ctx, "beta").sessions_matched, 1);
}

#[test]
fn a_vanished_session_is_dropped_but_an_unreadable_root_drops_nothing() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let a = f.claude("-w", ID_A, &[claude_user("/w", "kept")], 0);
    f.claude("-w", ID_B, &[claude_user("/w", "gone")], 0);
    f.codex(
        ID_C,
        &[
            codex_meta("/w", json!("cli")),
            codex_user_item("codex kept"),
        ],
        0,
    );
    refresh(&ctx);

    std::fs::remove_file(
        f.paths
            .claude_archive
            .join("-w")
            .join(format!("{ID_B}.jsonl")),
    )
    .unwrap();
    assert_eq!(refresh(&ctx).removed, 1);

    // The whole Claude root disappearing (an unmounted volume, a permissions change) says
    // nothing about whether its sessions still exist.
    let _ = a;
    std::fs::remove_dir_all(&f.paths.claude_archive).unwrap();
    assert_eq!(refresh(&ctx).removed, 0);
    let idx = Index::open(&ctx.paths).unwrap();
    assert_eq!(idx.status().unwrap().sessions, 2);
}

// --- coverage --------------------------------------------------------------

#[test]
fn guardian_text_is_not_indexed_and_search_says_so() {
    let f = Fixture::new();
    let ctx = f.ctx();
    f.codex(
        ID_A,
        &[
            json!({"type": "session_meta", "payload": {"cwd": "/w",
                "source": {"subagent": {"other": "guardian"}}}}),
            codex_user_item("copied parent history mentions zebra"),
        ],
        0,
    );
    f.claude("-w", ID_B, &[claude_user("/w", "the real zebra")], 0);
    let r = sessions::search(
        &ctx,
        &SearchOpts {
            query: "zebra".into(),
            include_subagents: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.sessions_matched, 1);
    assert_eq!(r.hits[0].row.session_id, ID_B);
    assert_eq!(r.coverage.text_excluded.get("guardian"), Some(&1));
    assert!(r.note.unwrap().contains("1 guardian"));
}

#[test]
fn a_search_that_ran_out_of_budget_is_not_proof_of_absence() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "needle")], 0);
    let ctx = f.ctx_with(Limits {
        index_budget_ms: 0,
        ..limits()
    });
    let r = search(&ctx, "needle");
    assert_eq!(r.coverage.refresh.pending, 1);
    assert!(!r.exhaustive);
    assert_eq!(r.absence_is_proof, Some(false));
    assert!(r.note.unwrap().contains("trovador index"));

    // Once indexed, the same search is exhaustive and finds it.
    sessions::reindex(&f.ctx(), false).unwrap();
    let r = search(&ctx, "needle");
    assert!(r.exhaustive);
    assert_eq!(r.sessions_matched, 1);
}

// --- queries ---------------------------------------------------------------

fn corpus() -> Fixture {
    let f = Fixture::new();
    f.claude(
        "-alpha",
        ID_A,
        &[
            claude_user(
                "/code/alpha",
                "look up 550e8400-e29b-41d4-a716-446655440000 please",
            ),
            claude_tool_use("Bash", json!({"command": "grep idempotency_key src/"})),
        ],
        30,
    );
    f.codex(
        ID_B,
        &[
            codex_meta("/code/beta", json!("vscode")),
            codex_user_item("the idempotency key is missing"),
            codex_agent_item("added idempotency_key to the request"),
        ],
        10,
    );
    f.claude(
        "-gamma",
        ID_C,
        &[claude_user("/code/gamma", "unrelated work")],
        20,
    );
    f
}

#[test]
fn a_uuid_and_a_snake_case_name_match_as_written() {
    let f = corpus();
    let ctx = f.ctx();
    let r = search(&ctx, "550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(r.sessions_matched, 1);
    assert_eq!(r.hits[0].matches[0].index, 0);
    let excerpt = &r.hits[0].matches[0].excerpt;
    assert!(
        excerpt.contains("[550e8400-e29b-41d4-a716-446655440000]"),
        "{excerpt}"
    );

    let r = search(&ctx, "idempotency_key");
    assert_eq!(r.sessions_matched, 2);
    // "idempotency key" as a phrase also matches the snake_case form: same tokens.
    assert_eq!(search(&ctx, "\"idempotency key\"").sessions_matched, 2);
}

#[test]
fn every_term_is_required_unless_fts_syntax_says_otherwise() {
    let f = corpus();
    let ctx = f.ctx();
    assert_eq!(search(&ctx, "idempotency unrelated").sessions_matched, 0);
    let r = sessions::search(
        &ctx,
        &SearchOpts {
            query: "idempotency OR unrelated".into(),
            fts: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.sessions_matched, 3);
    let bad = sessions::search(
        &ctx,
        &SearchOpts {
            query: "AND OR".into(),
            fts: true,
            ..Default::default()
        },
    );
    assert!(matches!(bad, Err(sessions::Error::Query(_))));
}

#[test]
fn a_regex_searches_raw_text_and_brackets_the_match() {
    let f = corpus();
    let ctx = f.ctx();
    let r = sessions::search(
        &ctx,
        &SearchOpts {
            query: r"idempotency[_ ]key".into(),
            regex: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.sessions_matched, 2);
    // Regex results are ordered by recency.
    assert_eq!(r.hits[0].row.session_id, ID_B);
    assert!(r.hits[0].matches[0].excerpt.contains("[idempotency key]"));
    let bad = sessions::search(
        &ctx,
        &SearchOpts {
            query: "(".into(),
            regex: true,
            ..Default::default()
        },
    );
    assert!(matches!(bad, Err(sessions::Error::Query(_))));
}

#[test]
fn filters_narrow_by_backend_project_and_origin() {
    let f = corpus();
    let ctx = f.ctx();
    let with = |o: SearchOpts| sessions::search(&ctx, &o).unwrap().sessions_matched;
    let base = || SearchOpts {
        query: "idempotency".into(),
        ..Default::default()
    };
    assert_eq!(
        with(SearchOpts {
            backend: Some(Backend::Codex),
            ..base()
        }),
        1
    );
    assert_eq!(
        with(SearchOpts {
            project: Some("alpha".into()),
            ..base()
        }),
        1
    );
    assert_eq!(
        with(SearchOpts {
            origin: Some("vscode".into()),
            ..base()
        }),
        1
    );
    // LIKE wildcards in a project filter are literal.
    assert_eq!(
        with(SearchOpts {
            project: Some("%".into()),
            ..base()
        }),
        0
    );
}

#[test]
fn order_recent_puts_the_newest_session_first() {
    let f = corpus();
    let ctx = f.ctx();
    let r = sessions::search(
        &ctx,
        &SearchOpts {
            query: "idempotency".into(),
            order: Order::Recent,
            ..Default::default()
        },
    )
    .unwrap();
    let ids: Vec<_> = r.hits.iter().map(|h| h.row.session_id.as_str()).collect();
    assert_eq!(ids, vec![ID_B, ID_A]);
}

#[test]
fn listing_reads_the_index_when_present_but_never_creates_it() {
    let f = corpus();
    let ctx = f.ctx();
    let before = sessions::list(&ctx, &Default::default());
    assert!(!trovador::index::db_path(&f.paths).exists());
    sessions::reindex(&ctx, false).unwrap();
    let after = sessions::list(&ctx, &Default::default());
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
}

#[test]
fn a_rebuild_reindexes_everything_from_an_empty_file() {
    let f = corpus();
    let ctx = f.ctx();
    sessions::reindex(&ctx, false).unwrap();
    let again = sessions::reindex(&ctx, false).unwrap();
    assert_eq!(again.unchanged, 3);
    let rebuilt = sessions::reindex(&ctx, true).unwrap();
    assert_eq!((rebuilt.reindexed, rebuilt.unchanged), (3, 0));
    assert_eq!(search(&ctx, "idempotency_key").sessions_matched, 2);
}

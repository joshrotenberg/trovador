mod common;

use std::io::Write;

use common::*;
use serde_json::{Value, json};
use trovador::index::{Index, Order, RefreshOpts, SearchOpts};
use trovador::sessions::{self, Ctx};
use trovador::transcript::RecordKind;
use trovador::{Limits, Provider};

fn append(path: &std::path::Path, records: &[Value]) {
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    for r in records {
        writeln!(f, "{r}").unwrap();
    }
}

fn refresh(ctx: &Ctx) -> trovador::index::RefreshReport {
    let mut idx = Index::open(&ctx.paths, &ctx.limits).unwrap();
    idx.refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())
        .unwrap()
}

/// The text-bearing records of a full transcript read, as (index, type, text).
fn transcript_records(ctx: &Ctx, provider: Provider, id: &str) -> Vec<(usize, String, String)> {
    sessions::transcript(ctx, id, Some(provider), 0, Some(200))
        .unwrap()
        .page
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
        .collect()
}

/// What the index holds for a session must be exactly the text-bearing records a full
/// transcript read produces, with the same indexes, or a search hit's index would open the
/// transcript at the wrong record. Only the text differs, where the index keeps more of a
/// tool record (`index_tool_chars`) than a transcript shows (`tool_chars`).
fn assert_index_matches_transcript(ctx: &Ctx, provider: Provider, id: &str) {
    let idx = Index::open(&ctx.paths, &ctx.limits).unwrap();
    let indexed = idx.session_records(provider, id).unwrap();
    let wide = Ctx {
        paths: ctx.paths.clone(),
        limits: ctx.limits.for_index(),
    };
    let expected = transcript_records(&wide, provider, id);
    assert!(!expected.is_empty());
    assert_eq!(indexed, expected);
    let shown = transcript_records(ctx, provider, id);
    let positions = |v: &[(usize, String, String)]| {
        v.iter()
            .map(|(i, k, _)| (*i, k.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(positions(&indexed), positions(&shown));
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

fn assert_custom_title_on_both_doors(ctx: &Ctx, title: &str) {
    use trovador::archive::TitleSource;
    let listed = sessions::list(ctx, &sessions::ListOpts::default()).unwrap();
    assert_eq!(listed.sessions.len(), 1);
    assert_eq!(listed.sessions[0].meta.title.as_deref(), Some(title));
    assert_eq!(
        listed.sessions[0].meta.title_source,
        Some(TitleSource::Custom)
    );
    let found = search(ctx, "needle");
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].row.meta.title.as_deref(), Some(title));
    assert_eq!(
        found.hits[0].row.meta.title_source,
        Some(TitleSource::Custom)
    );
    let shown = sessions::show(ctx, ID_A, None).unwrap();
    assert_eq!(shown.row.meta.title.as_deref(), Some(title));
    assert_eq!(shown.row.meta.title_source, Some(TitleSource::Custom));
}

#[test]
fn an_appended_custom_title_survives_later_ai_titles_and_incremental_refresh() {
    let f = Fixture::new();
    let ctx = f.ctx_with(Limits {
        title_scan_max_bytes: 0,
        ..limits()
    });
    let path = f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "needle"),
            claude_assistant(&"x".repeat(600 * 1024)),
            json!({"type": "ai-title", "aiTitle": "generated"}),
        ],
        0,
    );
    assert_eq!(refresh(&ctx).reindexed, 1);
    // Issue #36: the old file stays above the fallback budget, but this append fits it.
    let ctx = f.ctx_with(Limits {
        title_scan_max_bytes: 350 * 1024,
        ..limits()
    });
    append(
        &path,
        &[
            json!({"type": "custom-title", "customTitle": "typed"}),
            claude_assistant(&"y".repeat(300 * 1024)),
            json!({"type": "ai-title", "aiTitle": "later generated"}),
        ],
    );
    // Issue #36: list and show must use the stored offset even before search refreshes it.
    let listed = sessions::list(&ctx, &sessions::ListOpts::default()).unwrap();
    assert_eq!(listed.sessions[0].meta.title.as_deref(), Some("typed"));
    let shown = sessions::show(&ctx, ID_A, None).unwrap();
    assert_eq!(shown.row.meta.title.as_deref(), Some("typed"));
    let r = refresh(&ctx);
    assert_eq!((r.appended, r.reindexed, r.metadata_refreshed), (1, 0, 0));
    assert_custom_title_on_both_doors(&ctx, "typed");
    append(
        &path,
        &[json!({"type": "ai-title", "aiTitle": "new generated"})],
    );
    assert_eq!(refresh(&ctx).appended, 1);
    assert_custom_title_on_both_doors(&ctx, "typed");
}

#[test]
fn old_cached_generated_titles_are_backfilled_without_reprojecting_search_records() {
    let f = Fixture::new();
    let ctx = f.ctx_with(Limits {
        title_scan_max_bytes: 0,
        ..limits()
    });
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "needle"),
            claude_assistant(&"x".repeat(100 * 1024)),
            json!({"type": "custom-title", "customTitle": "typed"}),
            claude_assistant(&"y".repeat(500 * 1024)),
            json!({"type": "ai-title", "aiTitle": "generated"}),
        ],
        0,
    );
    assert_eq!(refresh(&ctx).reindexed, 1);
    let index = Index::open(&ctx.paths, &ctx.limits).unwrap();
    let original_records = index.session_records(Provider::Claude, ID_A).unwrap();
    drop(index);
    let conn = rusqlite::Connection::open(trovador::index::db_path(&ctx.paths)).unwrap();
    let raw: String = conn
        .query_row("SELECT meta FROM sessions", [], |r| r.get(0))
        .unwrap();
    let mut meta: Value = serde_json::from_str(&raw).unwrap();
    meta["title"] = json!("generated");
    meta["title_source"] = json!("generated");
    conn.execute("UPDATE sessions SET meta = ?1", [meta.to_string()])
        .unwrap();
    // Issue #36: reproduce the previous schema, where no title state or version existed.
    conn.execute_batch(
        "ALTER TABLE sessions DROP COLUMN custom_title;
                        ALTER TABLE sessions DROP COLUMN title_metadata_version;",
    )
    .unwrap();
    drop(conn);
    let r = refresh(&ctx);
    assert_eq!((r.metadata_refreshed, r.reindexed, r.appended), (1, 0, 0));
    let index = Index::open(&ctx.paths, &ctx.limits).unwrap();
    assert_eq!(
        index.session_records(Provider::Claude, ID_A).unwrap(),
        original_records
    );
    assert_custom_title_on_both_doors(&ctx, "typed");
    assert_eq!(refresh(&ctx).unchanged, 1);
}

fn simulate_old_build_update(ctx: &Ctx) {
    let entry = trovador::archive::find_session(&ctx.paths, ID_A, None).unwrap();
    let meta = trovador::archive::read_meta(&entry, &ctx.limits);
    let mtime = entry
        .mtime
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let bytes = std::fs::read(&entry.file).unwrap();
    let region = bytes.len().min(4096);
    let head =
        trovador::transcript::text_hash(std::str::from_utf8(&bytes[..region]).unwrap()) as i64;
    let tail = trovador::transcript::text_hash(
        std::str::from_utf8(&bytes[bytes.len() - region..]).unwrap(),
    ) as i64;
    let conn = rusqlite::Connection::open(trovador::index::db_path(&ctx.paths)).unwrap();
    // Issue #36: the old binary's upsert changes its checkpoint but leaves new columns alone.
    conn.execute(
        "UPDATE sessions SET offset = ?1, size = ?1, mtime_ns = ?2, meta = ?3,
                  head_hash = ?4, tail_hash = ?5",
        rusqlite::params![
            entry.size_bytes as i64,
            mtime,
            serde_json::to_string(&meta).unwrap(),
            head,
            tail
        ],
    )
    .unwrap();
}

#[test]
fn an_old_build_resume_cannot_advance_past_an_untracked_custom_title() {
    let f = Fixture::new();
    let ctx = f.ctx_with(Limits {
        title_scan_max_bytes: 0,
        ..limits()
    });
    let path = f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "needle"),
            claude_assistant(&"x".repeat(600 * 1024)),
            json!({"type": "ai-title", "aiTitle": "generated"}),
        ],
        0,
    );
    assert_eq!(refresh(&ctx).reindexed, 1);
    append(
        &path,
        &[
            json!({"type": "custom-title", "customTitle": "typed"}),
            json!({"type": "progress", "payload": "y".repeat(300 * 1024)}),
            json!({"type": "ai-title", "aiTitle": "later generated"}),
        ],
    );
    simulate_old_build_update(&ctx);
    let index = Index::open(&ctx.paths, &ctx.limits).unwrap();
    assert!(index.meta_snapshot().is_empty());
    let r = refresh(&ctx);
    assert_eq!((r.metadata_refreshed, r.reindexed, r.appended), (1, 0, 0));
    assert_custom_title_on_both_doors(&ctx, "typed");
    assert_eq!(refresh(&ctx).unchanged, 1);
}

#[test]
fn an_old_build_title_backfill_still_indexes_bytes_appended_after_its_checkpoint() {
    let f = Fixture::new();
    let ctx = f.ctx_with(Limits {
        title_scan_max_bytes: 0,
        ..limits()
    });
    let path = f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "needle"),
            claude_assistant(&"x".repeat(600 * 1024)),
            json!({"type": "ai-title", "aiTitle": "generated"}),
        ],
        0,
    );
    assert_eq!(refresh(&ctx).reindexed, 1);
    append(
        &path,
        &[
            json!({"type": "custom-title", "customTitle": "typed"}),
            json!({"type": "progress", "payload": "y".repeat(300 * 1024)}),
            json!({"type": "ai-title", "aiTitle": "later generated"}),
        ],
    );
    simulate_old_build_update(&ctx);
    append(
        &path,
        &[
            claude_assistant("continued needle"),
            json!({"type": "ai-title", "aiTitle": "new generated"}),
        ],
    );
    let r = refresh(&ctx);
    assert_eq!((r.metadata_refreshed, r.reindexed), (1, 0));
    assert_custom_title_on_both_doors(&ctx, "typed");
    let found = search(&ctx, "continued");
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].matches[0].excerpt, "[continued] needle");
    assert_eq!(refresh(&ctx).unchanged, 1);
}

#[test]
fn an_old_build_full_reindex_cannot_keep_a_custom_title_from_the_old_file() {
    use trovador::archive::TitleSource;
    let f = Fixture::new();
    let ctx = f.ctx_with(Limits {
        title_scan_max_bytes: 0,
        ..limits()
    });
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "needle"),
            json!({"type": "custom-title", "customTitle": "obsolete"}),
            claude_assistant(&"x".repeat(600 * 1024)),
        ],
        0,
    );
    assert_eq!(refresh(&ctx).reindexed, 1);
    let path = f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "needle"),
            claude_assistant(&"y".repeat(700 * 1024)),
            json!({"type": "ai-title", "aiTitle": "rewritten"}),
        ],
        0,
    );
    simulate_old_build_update(&ctx);
    let r = refresh(&ctx);
    assert_eq!((r.metadata_refreshed, r.reindexed, r.appended), (1, 0, 0));
    append(
        &path,
        &[json!({"type": "ai-title", "aiTitle": "latest generated"})],
    );
    assert_eq!(refresh(&ctx).appended, 1);
    let shown = sessions::show(&ctx, ID_A, None).unwrap();
    assert_eq!(shown.row.meta.title.as_deref(), Some("latest generated"));
    assert_eq!(shown.row.meta.title_source, Some(TitleSource::Generated));
    let listed = sessions::list(&ctx, &sessions::ListOpts::default()).unwrap();
    assert_eq!(
        listed.sessions[0].meta.title.as_deref(),
        Some("latest generated")
    );
    let found = search(&ctx, "needle");
    assert_eq!(
        found.hits[0].row.meta.title.as_deref(),
        Some("latest generated")
    );
}

#[test]
fn indexed_and_unindexed_complete_final_custom_titles_without_newlines_agree() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let path = f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "needle"),
            claude_assistant(&"x".repeat(100 * 1024)),
            json!({"type": "custom-title", "customTitle": "A"}),
            claude_assistant(&"y".repeat(500 * 1024)),
            json!({"type": "custom-title", "customTitle": "B"}),
        ],
        0,
    );
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(file.metadata().unwrap().len() - 1).unwrap();
    let unindexed = sessions::show(&ctx, ID_A, None).unwrap();
    assert_eq!(unindexed.row.meta.title.as_deref(), Some("B"));
    assert_eq!(refresh(&ctx).reindexed, 1);
    assert_custom_title_on_both_doors(&ctx, "B");
    let conn = rusqlite::Connection::open(trovador::index::db_path(&ctx.paths)).unwrap();
    let stored: Option<String> = conn
        .query_row("SELECT custom_title FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored.as_deref(), Some("A"));
    let raw: String = conn
        .query_row("SELECT meta FROM sessions", [], |r| r.get(0))
        .unwrap();
    let persisted: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(persisted["title"], "A");
    drop(conn);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file).unwrap();
    drop(file);
    assert_eq!(refresh(&ctx).appended, 1);
    assert_custom_title_on_both_doors(&ctx, "B");
}

#[test]
fn indexed_metadata_does_not_scan_an_append_above_the_title_scan_budget() {
    let f = Fixture::new();
    let ctx = f.ctx_with(Limits {
        title_scan_max_bytes: 1024,
        ..limits()
    });
    let path = f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "needle"),
            json!({"type": "custom-title", "customTitle": "stored"}),
        ],
        0,
    );
    assert_eq!(refresh(&ctx).reindexed, 1);
    append(
        &path,
        &[
            json!({"type": "custom-title", "customTitle": "appended"}),
            claude_assistant(&"y".repeat(300 * 1024)),
            json!({"type": "custom-title", "customTitle": "tail"}),
            json!({"type": "ai-title", "aiTitle": "generated"}),
        ],
    );
    let listed = sessions::list(&ctx, &sessions::ListOpts::default()).unwrap();
    assert_eq!(listed.sessions[0].meta.title.as_deref(), Some("stored"));
    let shown = sessions::show(&ctx, ID_A, None).unwrap();
    assert_eq!(shown.row.meta.title.as_deref(), Some("stored"));
    assert_eq!(refresh(&ctx).appended, 1);
    assert_custom_title_on_both_doors(&ctx, "tail");
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
    assert_index_matches_transcript(&ctx, Provider::Claude, ID_A);

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
    assert_index_matches_transcript(&ctx, Provider::Claude, ID_A);
    let records = Index::open(&ctx.paths, &ctx.limits)
        .unwrap()
        .session_records(Provider::Claude, ID_A)
        .unwrap();
    assert_eq!(records.last().unwrap().0, 2);

    let r3 = refresh(&ctx);
    assert_eq!(r3.unchanged, 1);
}

#[test]
fn a_codex_file_that_later_shows_its_item_stream_is_continued_without_a_duplicate() {
    // The first pass sees only the response stream. The item stream appears after it,
    // starting with a copy of the turn already indexed, which the switch drops.
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
    assert_eq!((r.appended, r.reindexed), (1, 0));
    assert_index_matches_transcript(&ctx, Provider::Codex, ID_A);
    let idx = Index::open(&ctx.paths, &ctx.limits).unwrap();
    let users = idx
        .session_records(Provider::Codex, ID_A)
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
    assert_index_matches_transcript(&ctx, Provider::Codex, ID_A);
    let users = Index::open(&ctx.paths, &ctx.limits)
        .unwrap()
        .session_records(Provider::Codex, ID_A)
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
    assert_index_matches_transcript(&ctx, Provider::Claude, ID_A);
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
    let idx = Index::open(&ctx.paths, &ctx.limits).unwrap();
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
            include_subagents: Some(true),
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
fn filters_narrow_by_provider_project_and_origin() {
    let f = corpus();
    let ctx = f.ctx();
    let with = |o: SearchOpts| sessions::search(&ctx, &o).unwrap().sessions_matched;
    let base = || SearchOpts {
        query: "idempotency".into(),
        ..Default::default()
    };
    assert_eq!(
        with(SearchOpts {
            provider: Some(Provider::Codex),
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
    let before = sessions::list(&ctx, &Default::default()).unwrap();
    assert!(!trovador::index::db_path(&f.paths).exists());
    sessions::reindex(&ctx, false).unwrap();
    let after = sessions::list(&ctx, &Default::default()).unwrap();
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

#[test]
fn reasoning_can_be_left_out_of_the_index_and_changing_that_rebuilds_it() {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "why is it slow"),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "suspect the quadratic merge", "signature": "s"},
                {"type": "text", "text": "Found it."}
            ]}}),
        ],
        0,
    );
    let on = f.ctx();
    let off = f.ctx_with(Limits {
        index_reasoning: false,
        ..limits()
    });
    assert_eq!(search(&on, "quadratic merge").sessions_matched, 1);

    // The same home with the setting flipped: the file is replaced, not mixed.
    let r = search(&off, "quadratic merge");
    assert_eq!(r.sessions_matched, 0);
    assert_eq!(r.coverage.refresh.reindexed, 1);
    let kinds: Vec<String> = Index::open(&off.paths, &off.limits)
        .unwrap()
        .session_records(Provider::Claude, ID_A)
        .unwrap()
        .into_iter()
        .map(|(_, k, _)| k)
        .collect();
    assert_eq!(kinds, vec!["user", "assistant"]);
    // Transcripts still show it.
    let t = sessions::transcript(&off, ID_A, None, 0, None).unwrap();
    assert!(
        t.page
            .records
            .iter()
            .any(|r| r.kind == RecordKind::Reasoning)
    );
}

// --- #25, #26, #31 -------------------------------------------------------------

#[test]
fn a_rewrite_of_the_same_size_or_larger_is_reindexed_not_continued() {
    // #25: growing, or keeping the size, did not prove the indexed bytes were unchanged.
    for replacement in ["bravo bravo bravo", "bravo bravo bravo and then some more"] {
        let f = Fixture::new();
        let ctx = f.ctx();
        f.claude("-w", ID_A, &[claude_user("/w", "alpha alpha alpha")], 100);
        refresh(&ctx);
        f.claude("-w", ID_A, &[claude_user("/w", replacement)], 50);
        let r = search(&ctx, "bravo");
        assert_eq!(
            (r.coverage.refresh.appended, r.coverage.refresh.reindexed),
            (0, 1),
            "{replacement}"
        );
        assert_eq!(r.sessions_matched, 1);
        assert_eq!(search(&ctx, "alpha").sessions_matched, 0, "{replacement}");
        assert_index_matches_transcript(&ctx, Provider::Claude, ID_A);
    }
}

#[test]
fn a_plain_append_is_still_continued() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let path = f.claude("-w", ID_A, &[claude_user("/w", "first")], 100);
    refresh(&ctx);
    append(&path, &[claude_assistant("second")]);
    let r = refresh(&ctx);
    assert_eq!((r.appended, r.reindexed), (1, 0));
}

#[test]
fn changing_a_record_cap_rebuilds_the_index() {
    // #26: rows built with one cap were served after it changed.
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[claude_user(
            "/w",
            "opening words then the tailword at the end",
        )],
        100,
    );
    let full = f.ctx();
    let short = f.ctx_with(Limits {
        record_chars: 10,
        ..limits()
    });
    assert_eq!(search(&full, "tailword").sessions_matched, 1);
    let r = search(&short, "tailword");
    assert_eq!(
        r.coverage.refresh.reindexed, 1,
        "a fresh file for the new cap"
    );
    assert_eq!(r.sessions_matched, 0);
    let back = search(&full, "tailword");
    assert_eq!(back.coverage.refresh.reindexed, 1);
    assert_eq!(back.sessions_matched, 1);
}

#[test]
fn older_format_codex_turns_survive_a_newer_client_continuing_the_session() {
    // #31: the pre-scan started in the newest stream it found and dropped the older turns.
    let f = Fixture::new();
    let ctx = f.ctx();
    let path = f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("cli")),
            codex_response("user", "historicalzephyr question"),
            codex_event("user_message", "historicalzephyr question"),
            codex_response("assistant", "historical answer"),
            codex_event("agent_message", "historical answer"),
        ],
        100,
    );
    assert_eq!(search(&ctx, "historicalzephyr").sessions_matched, 1);
    append(
        &path,
        &[
            codex_response("user", "current question"),
            codex_user_item("current question"),
            codex_response("assistant", "current answer"),
            codex_agent_item("current answer"),
        ],
    );
    let turns = |ctx: &trovador::sessions::Ctx| -> Vec<(RecordKind, String)> {
        sessions::transcript(ctx, ID_A, None, 0, None)
            .unwrap()
            .page
            .records
            .into_iter()
            .filter(|r| matches!(r.kind, RecordKind::User | RecordKind::Assistant))
            .map(|r| (r.kind, r.text.unwrap()))
            .collect()
    };
    let want = vec![
        (RecordKind::User, "historicalzephyr question".to_string()),
        (RecordKind::Assistant, "historical answer".to_string()),
        (RecordKind::User, "current question".to_string()),
        (RecordKind::Assistant, "current answer".to_string()),
    ];
    assert_eq!(turns(&ctx), want);
    let r = search(&ctx, "historicalzephyr");
    assert_eq!(r.coverage.refresh.appended, 1);
    assert_eq!(r.sessions_matched, 1);
    assert_index_matches_transcript(&ctx, Provider::Codex, ID_A);
    sessions::reindex(&ctx, true).unwrap();
    assert_index_matches_transcript(&ctx, Provider::Codex, ID_A);
    assert_eq!(search(&ctx, "historicalzephyr").sessions_matched, 1);
}

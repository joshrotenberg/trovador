//! What list and search leave out by default, and that they always say so.

mod common;

use common::*;
use serde_json::json;
use trovador::Limits;
use trovador::index::{Index, SearchOpts};
use trovador::sessions::{self, Ctx, ListOpts};

fn search(ctx: &Ctx, opts: SearchOpts) -> sessions::SearchResult {
    sessions::search(ctx, &opts).unwrap()
}

fn q(query: &str) -> SearchOpts {
    SearchOpts {
        query: query.into(),
        ..Default::default()
    }
}

fn ids(r: &sessions::SearchResult) -> Vec<String> {
    r.hits.iter().map(|h| h.row.session_id.clone()).collect()
}

#[test]
fn the_session_doing_the_search_is_left_out_while_it_is_active() {
    let f = Fixture::new();
    // The caller's own session, written to as it searches, and an older one.
    f.claude(
        "-w",
        ID_A,
        &[claude_user("/w", "where did we fix the parser")],
        0,
    );
    f.claude(
        "-w",
        ID_B,
        &[claude_user("/w", "we fix the parser here")],
        3_600,
    );
    let ctx = f.ctx_with(Limits {
        active_window_s: 600,
        ..limits()
    });

    let r = search(&ctx, q("fix the parser"));
    assert_eq!(ids(&r), vec![ID_B]);
    assert_eq!(r.hidden.active, 1);
    assert!(r.note.unwrap().contains("include_active"));

    let all = search(
        &ctx,
        SearchOpts {
            include_active: Some(true),
            ..q("fix the parser")
        },
    );
    assert_eq!(all.sessions_matched, 2);
    assert_eq!(all.hidden.active, 0);
}

#[test]
fn finding_nothing_is_not_proof_when_a_default_hid_a_match() {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[claude_user("/w", "only in the active session")],
        0,
    );
    let ctx = f.ctx_with(Limits {
        active_window_s: 600,
        ..limits()
    });
    let r = search(&ctx, q("only in the active session"));
    assert!(r.hits.is_empty());
    assert!(r.exhaustive);
    assert_eq!(r.absence_is_proof, Some(false));
}

#[test]
fn exclude_session_leaves_out_exactly_that_session() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "shared words")], 10);
    f.claude("-w", ID_B, &[claude_user("/w", "shared words")], 20);
    let ctx = f.ctx();
    let r = search(
        &ctx,
        SearchOpts {
            exclude_session: Some(ID_A.into()),
            ..q("shared words")
        },
    );
    assert_eq!(ids(&r), vec![ID_B]);
    assert_eq!(r.hidden.excluded, 1);
}

fn automated_fixture() -> Fixture {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "the deploy script")], 30);
    f.codex(
        ID_B,
        &[
            codex_meta("/w", json!("exec")),
            codex_user_item("the deploy script"),
        ],
        20,
    );
    f.codex(
        ID_C,
        &[
            codex_meta("/w", json!("exec")),
            codex_user_item("the deploy script"),
        ],
        10,
    );
    f
}

#[test]
fn automated_sessions_are_left_out_unless_asked_for() {
    let f = automated_fixture();
    let ctx = f.ctx();

    let r = search(&ctx, q("deploy script"));
    assert_eq!(ids(&r), vec![ID_A]);
    assert_eq!(r.hidden.automated, 2);
    assert!(r.note.unwrap().contains("include_automated"));

    let all = search(
        &ctx,
        SearchOpts {
            include_automated: Some(true),
            ..q("deploy script")
        },
    );
    assert_eq!(all.sessions_matched, 3);
    let asked = search(
        &ctx,
        SearchOpts {
            origin: Some("exec".into()),
            ..q("deploy script")
        },
    );
    assert_eq!(
        asked.sessions_matched, 2,
        "an explicit origin includes them"
    );

    let listed = sessions::list(&ctx, &ListOpts::default()).unwrap();
    let mut listed_ids: Vec<_> = listed
        .sessions
        .iter()
        .map(|s| s.session_id.clone())
        .collect();
    listed_ids.sort();
    assert_eq!(listed_ids, vec![ID_A]);
    assert_eq!(listed.hidden_automated, 2);
    let listed_all = sessions::list(
        &ctx,
        &ListOpts {
            include_automated: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(listed_all.returned, 3);
}

#[test]
fn a_projection_version_change_starts_a_fresh_index_file() {
    let f = automated_fixture();
    let ctx = f.ctx();
    sessions::reindex(&ctx, false).unwrap();
    let db = trovador::index::db_path(&ctx.paths);
    {
        // An index left by an older trovador.
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("UPDATE meta SET value = 0 WHERE key = 'projection'", [])
            .unwrap();
        conn.execute("CREATE TABLE left_by_old_version (x INTEGER)", [])
            .unwrap();
    }
    let idx = Index::open(&ctx.paths, &ctx.limits).unwrap();
    assert_eq!(idx.status().unwrap().sessions, 0, "the old rows are gone");
    drop(idx);
    let conn = rusqlite::Connection::open(&db).unwrap();
    let leftover: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'left_by_old_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(leftover, 0, "the file was replaced, not migrated in place");
    let r = sessions::reindex(&ctx, false).unwrap();
    assert_eq!(r.reindexed, 3);
}

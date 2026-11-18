#![cfg(unix)]

mod common;

use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use rusqlite::Connection;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::index::{Index, IndexError, SearchOpts, Visibility, db_path};
use trovador::sessions;

struct ReadOnly(Vec<(PathBuf, Permissions)>);

impl ReadOnly {
    fn new(dir: &Path) -> Self {
        let mut paths = vec![dir.to_path_buf()];
        paths.extend(fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()));
        let guard = Self(
            paths
                .into_iter()
                .map(|p| {
                    let permissions = fs::metadata(&p).unwrap().permissions();
                    (p, permissions)
                })
                .collect(),
        );
        for (path, _) in guard.0.iter().rev() {
            let mode = if path.is_dir() { 0o500 } else { 0o400 };
            fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
        }
        // Verify that the test exercises OS denial, not just a mode-bit branch.
        assert_eq!(
            fs::File::create(dir.join("write-probe"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        guard
    }
}

impl Drop for ReadOnly {
    fn drop(&mut self) {
        // Restore the directory first so TempDir can clean up even after an assertion.
        for (path, permissions) in &self.0 {
            fs::set_permissions(path, permissions.clone()).unwrap();
        }
    }
}

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "checkpointneedle")], 100);
    sessions::reindex(&f.ctx(), false).unwrap();
    f
}

fn opts(query: &str) -> SearchOpts {
    SearchOpts {
        query: query.into(),
        ..Default::default()
    }
}

fn assert_stale(f: &Fixture, result: &sessions::SearchResult) {
    assert!(!result.exhaustive);
    let note = result.note.as_deref().unwrap();
    assert!(note.contains("could not be updated"), "{note}");
    assert!(
        note.contains(&db_path(&f.paths).display().to_string()),
        "{note}"
    );
    assert!(note.contains("read-only"), "{note}");
}

fn assert_build_advice(f: &Fixture) {
    let error = sessions::search(&f.ctx(), &opts("checkpointneedle")).unwrap_err();
    assert!(
        matches!(
            error,
            sessions::Error::Index(IndexError::Unavailable { .. })
        ),
        "{error}"
    );
    let message = error.to_string();
    for expected in [
        "has not been built or is out of date",
        "trovador index",
        "outside the sandbox",
        "MCP server",
    ] {
        assert!(message.contains(expected), "{message}");
    }
    assert!(message.contains(&db_path(&f.paths).display().to_string()));
}

fn cli(f: &Fixture, query: &str) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(["search", query])
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("TROVADOR_HOME", &f.paths.home)
        .env("TROVADOR_ACTIVE_WINDOW_S", "0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn checkpointed_index_without_writable_files_or_sidecars_still_returns_hits() {
    let f = fixture();
    assert!(!f.paths.home.join("index.db-wal").exists());
    assert!(!f.paths.home.join("index.db-shm").exists());
    let before = fs::read(db_path(&f.paths)).unwrap();
    let _readonly = ReadOnly::new(&f.paths.home);
    assert_eq!(
        fs::OpenOptions::new()
            .write(true)
            .open(db_path(&f.paths))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let result = sessions::search(&f.ctx(), &opts("checkpointneedle")).unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].row.session_id, ID_A);
    assert_stale(&f, &result);
    assert_eq!(before, fs::read(db_path(&f.paths)).unwrap());
    assert!(!f.paths.home.join("index.db-shm").exists());
    assert_eq!(
        fs::metadata(&f.paths.home).unwrap().permissions().mode() & 0o777,
        0o500
    );
}

#[test]
fn immutable_uri_preserves_reserved_characters_and_unicode_in_paths() {
    let mut f = fixture();
    let home = f.dir.path().join("state ?mode=rw&immutable=0#%é");
    fs::rename(&f.paths.home, &home).unwrap();
    f.paths.home = home;
    let _readonly = ReadOnly::new(&f.paths.home);
    let result = sessions::search(&f.ctx(), &opts("checkpointneedle")).unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].row.session_id, ID_A);
    assert_stale(&f, &result);
    assert_eq!(
        cli(&f, "checkpointneedle"),
        serde_json::to_value(result).unwrap()
    );
    assert!(!f.paths.home.join("index.db-shm").exists());
}

#[test]
fn sessions_added_after_readonly_indexing_cannot_be_proven_absent() {
    let f = fixture();
    let _readonly = ReadOnly::new(&f.paths.home);
    f.codex(
        ID_B,
        &[codex_meta("/w", json!("cli")), codex_user_item("newneedle")],
        50,
    );
    let result = sessions::search(&f.ctx(), &opts("newneedle")).unwrap();
    assert!(result.hits.is_empty());
    assert_eq!(result.sessions_matched, 0);
    assert_eq!(result.absence_is_proof, Some(false));
    assert_stale(&f, &result);
}

#[test]
fn missing_readonly_index_explains_how_the_host_can_build_it() {
    let f = Fixture::new();
    fs::create_dir_all(&f.paths.home).unwrap();
    let _readonly = ReadOnly::new(&f.paths.home);
    assert_build_advice(&f);
}

#[test]
fn incompatible_readonly_index_never_serves_rows_from_another_build() {
    for key in ["schema", "projection", "settings"] {
        let f = fixture();
        let conn = Connection::open(db_path(&f.paths)).unwrap();
        conn.execute("UPDATE meta SET value = value + 1 WHERE key = ?1", [key])
            .unwrap();
        drop(conn);
        let before = fs::read(db_path(&f.paths)).unwrap();
        let _readonly = ReadOnly::new(&f.paths.home);
        assert_build_advice(&f);
        assert_eq!(before, fs::read(db_path(&f.paths)).unwrap());
    }
}

#[test]
fn corrupt_readonly_index_is_a_database_error_not_build_advice() {
    let f = Fixture::new();
    fs::create_dir_all(&f.paths.home).unwrap();
    fs::write(db_path(&f.paths), b"not a database").unwrap();
    let _readonly = ReadOnly::new(&f.paths.home);
    let error = sessions::search(&f.ctx(), &opts("needle")).unwrap_err();
    assert!(
        matches!(error, sessions::Error::Index(IndexError::Sql(_))),
        "{error}"
    );
    assert!(error.to_string().contains("not a database"), "{error}");
}

#[test]
fn invalid_fts_queries_remain_query_errors_with_a_readonly_index() {
    let f = fixture();
    let _readonly = ReadOnly::new(&f.paths.home);
    let error = sessions::search(
        &f.ctx(),
        &SearchOpts {
            fts: true,
            ..opts("(")
        },
    )
    .unwrap_err();
    assert!(matches!(error, sessions::Error::Query(_)), "{error}");
}

#[tokio::test]
async fn readonly_search_json_agrees_between_library_cli_and_mcp() {
    let f = fixture();
    let _readonly = ReadOnly::new(&f.paths.home);
    for query in ["checkpointneedle", "absentneedle"] {
        let lib = serde_json::to_value(sessions::search(&f.ctx(), &opts(query)).unwrap()).unwrap();
        assert_eq!(cli(&f, query), lib);
        let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
        client.initialize().await;
        assert_eq!(
            client
                .call_tool_json("search", json!({"query": query}))
                .await,
            lib
        );
    }
}

fn wal_writer(f: &Fixture) -> Connection {
    let writer = Connection::open(db_path(&f.paths)).unwrap();
    writer
        .execute_batch("PRAGMA wal_autocheckpoint = 0; UPDATE records SET text = 'walneedle';")
        .unwrap();
    assert!(
        fs::metadata(f.paths.home.join("index.db-wal"))
            .unwrap()
            .len()
            > 0
    );
    writer
}

#[test]
fn readonly_wal_search_includes_committed_rows_not_yet_checkpointed() {
    let f = fixture();
    let writer = wal_writer(&f);
    let _readonly = ReadOnly::new(&f.paths.home);
    let result = sessions::search(&f.ctx(), &opts("walneedle")).unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].row.session_id, ID_A);
    assert_stale(&f, &result);
    assert_eq!(cli(&f, "walneedle"), serde_json::to_value(result).unwrap());
    // Keep the writer connected until the guard restores its sidecar permissions.
    drop(_readonly);
    drop(writer);
}

#[test]
fn rollback_journal_cannot_leak_uncommitted_rows_into_a_snapshot() {
    let f = fixture();
    let writer = Connection::open(db_path(&f.paths)).unwrap();
    writer.execute_batch("PRAGMA journal_mode = DELETE; BEGIN IMMEDIATE; UPDATE records SET text = 'uncommittedneedle';").unwrap();
    assert!(
        fs::metadata(f.paths.home.join("index.db-journal"))
            .unwrap()
            .len()
            > 0
    );
    let readonly = ReadOnly::new(&f.paths.home);
    let result = sessions::search(&f.ctx(), &opts("uncommittedneedle")).unwrap();
    assert_eq!(result.sessions_matched, 0);
    assert_eq!(result.absence_is_proof, Some(false));
    assert_stale(&f, &result);
    let committed = sessions::search(&f.ctx(), &opts("checkpointneedle")).unwrap();
    assert_eq!(committed.sessions_matched, 1);
    drop(readonly);
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn nonpermission_open_errors_are_not_replaced_by_readonly_build_advice() {
    let f = Fixture::new();
    fs::write(&f.paths.home, b"not a directory").unwrap();
    let error = sessions::search(&f.ctx(), &opts("needle")).unwrap_err();
    assert!(
        matches!(error, sessions::Error::Index(IndexError::Dir { ref source, .. })
        if source.kind() == std::io::ErrorKind::NotADirectory),
        "{error}"
    );
    assert!(!error.to_string().contains("trovador index"));
}

#[test]
fn concurrent_wal_commits_cannot_mix_readonly_counts_and_excerpts() {
    let f = fixture();
    let writer = wal_writer(&f);
    let reader = Index::open_read_only(&f.paths, &f.ctx().limits).unwrap();
    writer
        .execute_batch("UPDATE records SET text = 'laterneedle';")
        .unwrap();
    let old = reader
        .search(&opts("walneedle"), &Visibility::default(), 10, 3, false)
        .unwrap();
    assert_eq!(old.sessions_matched, 1);
    assert_eq!(old.hits.len(), 1);
    assert_eq!(old.hits[0].matches.len(), 1);
    assert!(old.hits[0].matches[0].excerpt.contains("walneedle"));
    let fresh = Index::open_read_only(&f.paths, &f.ctx().limits).unwrap();
    assert_eq!(
        fresh
            .search(&opts("laterneedle"), &Visibility::default(), 10, 3, false)
            .unwrap()
            .sessions_matched,
        1
    );
    assert_eq!(
        reader
            .search(&opts("laterneedle"), &Visibility::default(), 10, 3, false)
            .unwrap()
            .sessions_matched,
        0
    );
    reader.verify_snapshot().unwrap();
    fresh.verify_snapshot().unwrap();
}

#[test]
fn immutable_reader_rejects_results_when_the_host_rebuilds_the_index() {
    let f = fixture();
    let reader = Index::open_read_only(&f.paths, &f.ctx().limits).unwrap();
    f.claude("-w", ID_A, &[claude_user("/w", "replacementneedle")], 20);
    sessions::reindex(&f.ctx(), true).unwrap();
    let old = reader
        .search(
            &opts("checkpointneedle"),
            &Visibility::default(),
            10,
            3,
            false,
        )
        .unwrap();
    assert_eq!(old.sessions_matched, 1);
    assert_eq!(old.hits[0].matches.len(), 1);
    assert!(old.hits[0].matches[0].excerpt.contains("checkpointneedle"));
    assert!(matches!(
        reader.verify_snapshot(),
        Err(IndexError::SnapshotChanged { .. })
    ));
    assert_eq!(
        sessions::search(&f.ctx(), &opts("replacementneedle"))
            .unwrap()
            .sessions_matched,
        1
    );
}

#[test]
fn main_file_checkpoint_between_open_and_search_completion_returns_snapshot_changed() {
    let f = fixture();
    let before = fs::read(db_path(&f.paths)).unwrap();
    let reader = Index::open_read_only(&f.paths, &f.ctx().limits).unwrap();
    reader.verify_snapshot().unwrap();
    let writer = Connection::open(db_path(&f.paths)).unwrap();
    let result = reader
        .search(
            &opts("checkpointneedle"),
            &Visibility::default(),
            10,
            3,
            false,
        )
        .and_then(|result| {
            assert_eq!(result.sessions_matched, 1);
            assert_eq!(result.hits[0].matches.len(), 1);
            // Interleave a completed checkpoint after reading results but before
            // validation, without a timing-dependent race between test threads.
            writer
                .execute_batch(
                    "PRAGMA wal_autocheckpoint = 0; UPDATE records SET text = 'changedneedle';",
                )
                .unwrap();
            let busy: i64 = writer
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
                .unwrap();
            assert_eq!(busy, 0);
            assert_eq!(
                fs::metadata(f.paths.home.join("index.db-wal"))
                    .unwrap()
                    .len(),
                0
            );
            assert_ne!(before, fs::read(db_path(&f.paths)).unwrap());
            reader.verify_snapshot()?;
            Ok(result)
        });
    assert!(matches!(result, Err(IndexError::SnapshotChanged { .. })));
}

#[test]
fn a_new_wal_or_journal_invalidates_immutable_results_even_without_main_file_changes() {
    for mode in ["WAL", "DELETE"] {
        let f = fixture();
        let writer = Connection::open(db_path(&f.paths)).unwrap();
        writer.pragma_update(None, "journal_mode", mode).unwrap();
        let before = fs::read(db_path(&f.paths)).unwrap();
        let reader = Index::open_read_only(&f.paths, &f.ctx().limits).unwrap();
        let result = reader
            .search(
                &opts("checkpointneedle"),
                &Visibility::default(),
                10,
                3,
                false,
            )
            .unwrap();
        assert_eq!(result.sessions_matched, 1);
        reader.verify_snapshot().unwrap();
        writer
            .execute_batch("BEGIN IMMEDIATE; UPDATE records SET text = 'changedneedle';")
            .unwrap();
        if mode == "WAL" {
            writer.execute_batch("COMMIT").unwrap();
        }
        assert_eq!(before, fs::read(db_path(&f.paths)).unwrap());
        assert!(matches!(
            reader.verify_snapshot(),
            Err(IndexError::SnapshotChanged { .. })
        ));
        if mode == "DELETE" {
            writer.execute_batch("ROLLBACK").unwrap();
        }
    }
}

#[test]
fn a_removed_main_file_invalidates_immutable_results() {
    let f = fixture();
    let reader = Index::open_read_only(&f.paths, &f.ctx().limits).unwrap();
    let result = reader
        .search(
            &opts("checkpointneedle"),
            &Visibility::default(),
            10,
            3,
            false,
        )
        .unwrap();
    assert_eq!(result.sessions_matched, 1);
    fs::remove_file(db_path(&f.paths)).unwrap();
    assert!(matches!(
        reader.verify_snapshot(),
        Err(IndexError::SnapshotChanged { .. })
    ));
}

#[test]
fn writable_search_refreshes_new_sessions_and_remains_exhaustive() {
    let f = fixture();
    f.codex(
        ID_B,
        &[codex_meta("/w", json!("cli")), codex_user_item("newneedle")],
        50,
    );
    let result = sessions::search(&f.ctx(), &opts("newneedle")).unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].row.session_id, ID_B);
    assert!(result.exhaustive);
    assert!(result.note.is_none());
    assert_eq!(result.coverage.refresh.reindexed, 1);
}

fn many(n: u32) -> Fixture {
    let f = Fixture::new();
    for i in 1..=n {
        f.claude(
            "-w",
            &format!("{i:08x}-0000-4000-8000-000000000000"),
            &[claude_user("/w", "pagedneedle")],
            u64::from(i) * 100,
        );
    }
    sessions::reindex(&f.ctx(), false).unwrap();
    f
}

fn sid(i: u32) -> String {
    format!("{i:08x}-0000-4000-8000-000000000000")
}

fn recent(offset: usize) -> SearchOpts {
    SearchOpts {
        offset,
        limit: Some(2),
        order: trovador::index::Order::Recent,
        ..opts("pagedneedle")
    }
}

#[test]
fn paging_visits_every_remaining_session_once_when_a_matched_file_is_gone() {
    let f = many(5);
    // Removed after indexing. Read-only, the refresh cannot drop it, so the index still
    // matches it but the archive scan cannot return it.
    fs::remove_file(
        f.paths
            .claude_archive
            .join("-w")
            .join(format!("{}.jsonl", sid(1))),
    )
    .unwrap();
    let _readonly = ReadOnly::new(&f.paths.home);
    let mut seen = Vec::new();
    let mut nexts = Vec::new();
    let mut offset = 0;
    let mut unavailable = 0;
    loop {
        let page = sessions::search(&f.ctx(), &recent(offset)).unwrap();
        seen.extend(page.hits.iter().map(|h| h.row.session_id.clone()));
        nexts.push(page.next_offset);
        unavailable += page.hidden.unavailable;
        match page.next_offset {
            Some(n) => offset = n,
            None => break,
        }
    }
    assert_eq!(seen, (2..=5).map(sid).collect::<Vec<_>>());
    assert_eq!(nexts, vec![Some(2), Some(4), None]);
    assert_eq!(unavailable, 1, "counted on the page that held it");
}

#[test]
fn a_page_whose_matches_are_all_unavailable_is_not_reported_as_past_the_end() {
    let f = many(5);
    for i in [1, 2] {
        fs::remove_file(
            f.paths
                .claude_archive
                .join("-w")
                .join(format!("{}.jsonl", sid(i))),
        )
        .unwrap();
    }
    let _readonly = ReadOnly::new(&f.paths.home);
    let page = sessions::search(&f.ctx(), &recent(0)).unwrap();
    assert!(page.hits.is_empty());
    assert_eq!(page.hidden.unavailable, 2);
    assert_eq!(page.next_offset, Some(2), "more follow");
    assert_eq!(page.absence_is_proof, Some(false));
    let note = page.note.unwrap();
    assert!(note.contains("could not be shown"), "{note}");
    assert!(!note.contains("no sessions at offset"), "{note}");
}

#[test]
fn a_row_under_an_unknown_backend_is_counted_and_next_offset_still_advances() {
    let f = many(3);
    // An index written by a build that knew another backend.
    let conn = Connection::open(db_path(&f.paths)).unwrap();
    let changed = conn
        .execute(
            "UPDATE sessions SET backend = 'mystery' WHERE session_id = ?1",
            [sid(1)],
        )
        .unwrap();
    assert_eq!(changed, 1);
    drop(conn);
    let _readonly = ReadOnly::new(&f.paths.home);
    // The unknown row is newest, so it fills the whole first page of one.
    let first = sessions::search(
        &f.ctx(),
        &SearchOpts {
            limit: Some(1),
            ..recent(0)
        },
    )
    .unwrap();
    assert!(first.hits.is_empty());
    assert_eq!(first.hidden.unrecognized, 1);
    assert_eq!(
        first.next_offset,
        Some(1),
        "advances by the row it consumed"
    );
    assert_eq!(first.absence_is_proof, Some(false));
    assert!(
        first
            .note
            .unwrap()
            .contains("backend this version does not know")
    );
    let second = sessions::search(
        &f.ctx(),
        &SearchOpts {
            limit: Some(1),
            ..recent(1)
        },
    )
    .unwrap();
    assert_eq!(second.hits.len(), 1);
    assert_eq!(second.hidden.unrecognized, 0);
}

#[test]
fn the_archived_filter_works_on_the_read_only_fallback() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "archneedle")], 100);
    f.codex_archived(
        ID_B,
        &[
            codex_meta("/w", json!("cli")),
            codex_user_item("archneedle"),
        ],
        50,
    );
    sessions::reindex(&f.ctx(), false).unwrap();
    let _readonly = ReadOnly::new(&f.paths.home);
    for (archived, expected) in [
        (Some(true), vec![ID_B]),
        (Some(false), vec![ID_A]),
        (None, vec![ID_B, ID_A]),
    ] {
        let r = sessions::search(
            &f.ctx(),
            &SearchOpts {
                archived,
                order: trovador::index::Order::Recent,
                ..opts("archneedle")
            },
        )
        .unwrap();
        assert_stale(&f, &r);
        let ids: Vec<_> = r.hits.iter().map(|h| h.row.session_id.as_str()).collect();
        assert_eq!(ids, expected, "{archived:?}");
    }
}

mod common;

use common::*;
use serde_json::json;
use trovador::index::{Index, SearchOpts};
use trovador::sessions;

#[test]
fn search_refresh_scopes_out_removed_roots_without_deleting_their_rows() {
    assert_removed_roots(false);
}

#[test]
fn index_removes_sessions_from_roots_no_longer_configured() {
    assert_removed_roots(true);
}

fn assert_removed_roots(explicit_index: bool) {
    for root in 0..3 {
        let f = Fixture::new();
        f.claude("-w", ID_A, &[claude_user("/w", "alpha")], 0);
        let records = [codex_meta("/w", json!("cli")), codex_user_item("alpha")];
        f.codex(ID_B, &records, 0);
        f.codex_archived(ID_C, &records, 0);
        let mut ctx = f.ctx();
        assert_eq!(sessions::reindex(&ctx, false).unwrap().reindexed, 3);
        let replacement = f.dir.path().join("replacement");
        std::fs::create_dir(&replacement).unwrap();
        match root {
            0 => ctx.paths.claude_archive = replacement,
            1 => ctx.paths.codex_archive = replacement,
            _ => ctx.paths.codex_archived = replacement,
        }
        if explicit_index {
            assert_eq!(sessions::reindex(&ctx, false).unwrap().removed, 1);
        }
        let result = sessions::search(
            &ctx,
            &SearchOpts {
                query: "alpha".into(),
                ..SearchOpts::default()
            },
        )
        .unwrap();
        if !explicit_index {
            assert_eq!(result.coverage.refresh.removed, 0);
        }
        assert_eq!(result.sessions_matched, 2);
        assert_eq!(result.hits.len(), 2);
        assert_eq!(result.hidden.unavailable, 0);
        assert!(result.coverage.refresh.unavailable_roots.is_empty());
        let absent = sessions::search(
            &ctx,
            &SearchOpts {
                query: "missing".into(),
                ..SearchOpts::default()
            },
        )
        .unwrap();
        assert_eq!(absent.absence_is_proof, Some(true));
        assert!(absent.note.is_none());
        assert_eq!(
            Index::open(&ctx.paths, &ctx.limits)
                .unwrap()
                .status()
                .unwrap()
                .sessions,
            if explicit_index { 2 } else { 3 }
        );
    }
}

fn search(ctx: &trovador::Ctx, query: &str) -> sessions::SearchResult {
    sessions::search(
        ctx,
        &SearchOpts {
            query: query.into(),
            ..SearchOpts::default()
        },
    )
    .unwrap()
}

#[test]
fn alternating_search_environments_keep_each_others_rows_without_reindexing_or_losing_proof() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "alpha")], 0);
    let other = Fixture::new();
    other.claude("-w", ID_B, &[claude_user("/w", "beta")], 0);
    let a = f.ctx();
    let mut b = f.ctx();
    b.paths.claude_archive = other.paths.claude_archive.clone();
    assert_eq!(search(&a, "alpha").coverage.refresh.reindexed, 1);
    assert_eq!(search(&b, "beta").coverage.refresh.reindexed, 1);

    for (ctx, own, foreign, id) in [(&a, "alpha", "beta", ID_A), (&b, "beta", "alpha", ID_B)] {
        for (regex, fts) in [(false, false), (true, false), (false, true)] {
            for (query, expected) in [(own, 1), (foreign, 0)] {
                let result = sessions::search(
                    ctx,
                    &SearchOpts {
                        query: query.into(),
                        regex,
                        fts,
                        ..SearchOpts::default()
                    },
                )
                .unwrap();
                assert_eq!(result.coverage.refresh.reindexed, 0);
                assert_eq!(result.coverage.refresh.removed, 0);
                assert_eq!(result.sessions_matched, expected);
                assert_eq!(result.hits.len(), expected);
                assert!(!result.hidden.any());
                assert!(result.exhaustive);
                assert!(result.note.is_none());
                if expected == 0 {
                    assert_eq!(result.absence_is_proof, Some(true));
                } else {
                    assert_eq!(result.hits[0].row.session_id, id);
                }
            }
        }
    }
    assert_eq!(
        Index::open(&a.paths, &a.limits)
            .unwrap()
            .status()
            .unwrap()
            .sessions,
        2
    );
}

#[test]
fn index_keeps_out_of_root_rows_until_every_configured_root_is_readable() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "alpha")], 0);
    let mut ctx = f.ctx();
    for root in [&ctx.paths.codex_archive, &ctx.paths.codex_archived] {
        std::fs::create_dir_all(root).unwrap();
    }
    sessions::reindex(&ctx, false).unwrap();
    ctx.paths.claude_archive = f.dir.path().join("replacement");
    std::fs::create_dir(&ctx.paths.claude_archive).unwrap();
    let offline = f.dir.path().join("offline");
    std::fs::rename(&ctx.paths.codex_archive, &offline).unwrap();
    std::fs::write(&ctx.paths.codex_archive, "not a directory").unwrap();
    let report = sessions::reindex(&ctx, false).unwrap();
    assert_eq!(report.removed, 0);
    assert!(
        report
            .cleanup_skipped_reason
            .unwrap()
            .contains(&ctx.paths.codex_archive.display().to_string())
    );
    assert_eq!(
        Index::open(&ctx.paths, &ctx.limits)
            .unwrap()
            .status()
            .unwrap()
            .sessions,
        1
    );
    let result = search(&ctx, "alpha");
    assert_eq!(result.sessions_matched, 0);
    assert!(!result.hidden.any());
    assert_eq!(result.absence_is_proof, Some(false));
    assert!(
        result
            .note
            .unwrap()
            .contains(&ctx.paths.codex_archive.display().to_string())
    );
    std::fs::remove_file(&ctx.paths.codex_archive).unwrap();
    std::fs::rename(offline, &ctx.paths.codex_archive).unwrap();
    assert_eq!(sessions::reindex(&ctx, false).unwrap().removed, 1);
    assert_eq!(
        Index::open(&ctx.paths, &ctx.limits)
            .unwrap()
            .status()
            .unwrap()
            .sessions,
        0
    );
}

#[test]
fn unreadable_root_with_a_different_spelling_cannot_claim_absence_proof() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "alpha")], 0);
    let mut ctx = f.ctx();
    sessions::reindex(&ctx, false).unwrap();
    ctx.paths.claude_archive = f.dir.path().join("other-spelling");
    std::fs::write(&ctx.paths.claude_archive, "unreadable archive directory").unwrap();
    let result = search(&ctx, "alpha");
    assert_eq!(result.coverage.refresh.removed, 0);
    assert_eq!(result.sessions_matched, 0);
    assert!(!result.hidden.any());
    assert_eq!(result.absence_is_proof, Some(false));
    assert!(!result.exhaustive);
    assert_eq!(
        result.coverage.refresh.unavailable_roots,
        vec![ctx.paths.claude_archive.display().to_string()]
    );
    assert!(
        result
            .note
            .unwrap()
            .contains(&ctx.paths.claude_archive.display().to_string())
    );
}

#[test]
fn index_prunes_removed_claude_roots_when_codex_roots_do_not_exist() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "alpha")], 0);
    let mut ctx = f.ctx();
    sessions::reindex(&ctx, false).unwrap();
    assert!(!ctx.paths.codex_archive.exists());
    assert!(!ctx.paths.codex_archived.exists());
    ctx.paths.claude_archive = f.dir.path().join("replacement");
    std::fs::create_dir(&ctx.paths.claude_archive).unwrap();
    let report = sessions::reindex(&ctx, false).unwrap();
    assert_eq!(report.removed, 1);
    assert!(report.cleanup_skipped_reason.is_none());
    assert_eq!(
        Index::open(&ctx.paths, &ctx.limits)
            .unwrap()
            .status()
            .unwrap()
            .sessions,
        0
    );
    let result = search(&ctx, "alpha");
    assert_eq!(result.sessions_matched, 0);
    assert_eq!(result.absence_is_proof, Some(true));
    assert!(result.note.is_none());
}

#[test]
fn unavailable_configured_root_keeps_rows_and_explains_nonproof_search() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "alpha")], 0);
    let ctx = f.ctx();
    sessions::reindex(&ctx, false).unwrap();
    std::fs::rename(&ctx.paths.claude_archive, f.dir.path().join("offline")).unwrap();
    let result = sessions::search(
        &ctx,
        &SearchOpts {
            query: "alpha".into(),
            ..SearchOpts::default()
        },
    )
    .unwrap();
    assert_eq!(result.coverage.refresh.removed, 0);
    assert_eq!(result.sessions_matched, 1);
    assert_eq!(result.hidden.unavailable, 1);
    assert_eq!(
        result.coverage.refresh.unavailable_roots,
        vec![ctx.paths.claude_archive.display().to_string()]
    );
    assert_eq!(result.absence_is_proof, Some(false));
    assert!(
        result
            .note
            .as_deref()
            .unwrap()
            .contains(&ctx.paths.claude_archive.display().to_string())
    );
    assert_eq!(
        Index::open(&ctx.paths, &ctx.limits)
            .unwrap()
            .status()
            .unwrap()
            .sessions,
        1
    );
}

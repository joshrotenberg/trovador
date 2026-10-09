//! One file per session, and saying so when part of the archive could not be checked.

mod common;

use common::*;
use serde_json::json;
use trovador::index::SearchOpts;
use trovador::sessions::{self, ListOpts};

#[test]
fn a_relocated_session_with_an_old_copy_is_one_session_everywhere() {
    // #28: the old copy made the session list twice, every prefix ambiguous, and search
    // rows mix the two files.
    let f = Fixture::new();
    f.claude("-old", ID_A, &[claude_user("/fixture/old", "oldword")], 300);
    f.claude(
        "-new",
        ID_A,
        &[
            claude_user("/fixture/old", "oldword"),
            claude_assistant("newword added after the move"),
            json!({"type": "relocated", "relocatedCwd": "/fixture/new"}),
        ],
        100,
    );
    let ctx = f.ctx();
    let list = sessions::list(&ctx, &ListOpts::default()).unwrap();
    assert_eq!(list.total_sessions, 1);
    assert_eq!(list.returned, 1);

    let by_prefix = sessions::show(&ctx, "aaaa", None).unwrap();
    let exact = sessions::show(&ctx, ID_A, None).unwrap();
    assert_eq!(by_prefix.file, exact.file);
    assert!(exact.file.contains("-new"));
    assert_eq!(exact.row.project.as_deref(), Some("/fixture/new"));

    let r = sessions::search(
        &ctx,
        &SearchOpts {
            query: "newword".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.hits.len(), 1);
    let hit = &r.hits[0].row;
    assert_eq!(hit.modified, exact.row.modified);
    assert_eq!(hit.size_kb, exact.row.size_kb);
    assert_eq!(hit.project, exact.row.project);
}

#[test]
fn an_unreadable_root_is_reported_and_search_does_not_claim_absence() {
    // #27: the index kept the root's rows, search could not return them, and still said
    // exhaustive: true and absence_is_proof: true.
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "mountedword")], 100);
    f.codex(
        ID_B,
        &[codex_meta("/w", json!("cli")), codex_user_item("other")],
        100,
    );
    let ctx = f.ctx();
    let q = || SearchOpts {
        query: "mountedword".into(),
        ..Default::default()
    };
    assert_eq!(sessions::search(&ctx, &q()).unwrap().hits.len(), 1);

    let away = f.dir.path().join("claude-away");
    std::fs::rename(&f.paths.claude_archive, &away).unwrap();
    let r = sessions::search(&ctx, &q()).unwrap();
    assert!(r.hits.is_empty());
    assert!(!r.exhaustive);
    assert_eq!(r.absence_is_proof, Some(false));
    assert_eq!(r.hidden.unavailable, 1);
    assert_eq!(
        r.coverage.refresh.unavailable_roots,
        vec![f.paths.claude_archive.display().to_string()]
    );
    assert!(r.note.unwrap().contains("could not be read"));

    // The rows survived, so restoring the root brings the hit back without a rebuild.
    std::fs::rename(&away, &f.paths.claude_archive).unwrap();
    let back = sessions::search(&ctx, &q()).unwrap();
    assert_eq!(back.hits.len(), 1);
    assert!(back.exhaustive);
    assert_eq!(back.coverage.refresh.reindexed, 0);
}

mod common;

use common::*;
use trovador::Backend;
use trovador::doctor::{self, Severity};

fn report(f: &Fixture) -> doctor::Report {
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(doctor::report(&f.ctx()))
}

#[test]
fn a_healthy_machine_reports_providers_archives_and_no_errors() {
    let f = Fixture::new().with_fakes();
    f.claude("-w", ID_A, &[claude_user("/w", "hi")], 0);
    trovador::sessions::reindex(&f.ctx(), false).unwrap();
    let r = report(&f);
    assert!(r.ok, "{:?}", r.problems);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    let claude = r
        .providers
        .iter()
        .find(|p| p.backend == Backend::Claude)
        .unwrap();
    assert_eq!(claude.version.as_deref(), Some("9.9.9 (fake claude)"));
    let codex = r
        .providers
        .iter()
        .find(|p| p.backend == Backend::Codex)
        .unwrap();
    assert_eq!(codex.version.as_deref(), Some("codex-cli 0.0.0-fake"));
    assert_eq!(r.archives[0].sessions, 1);
    assert_eq!(r.index.unwrap().sessions, 1);
}

#[test]
fn a_missing_provider_is_a_warning_because_reading_still_works() {
    let mut f = Fixture::new();
    f.paths.codex_bin = Some(f.dir.path().join("no-such-codex"));
    f.paths.claude_bin = Some(f.dir.path().join("no-such-claude"));
    f.claude("-w", ID_A, &[claude_user("/w", "hi")], 0);
    let r = report(&f);
    assert!(r.ok);
    let warns: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.severity == Severity::Warn)
        .map(|p| p.what.clone())
        .collect();
    assert!(
        warns
            .iter()
            .any(|w| w.starts_with("codex:") && w.contains("does not exist")),
        "{warns:?}"
    );
    assert!(
        warns.iter().any(|w| w.contains("no search index yet")),
        "{warns:?}"
    );
}

#[test]
fn no_archives_is_an_error_and_an_unreadable_provenance_record_a_warning() {
    let f = Fixture::new().with_fakes();
    std::fs::create_dir_all(&f.paths.home).unwrap();
    std::fs::write(trovador::provenance::path(&f.paths), "{ broken").unwrap();
    let r = report(&f);
    assert!(!r.ok);
    assert!(
        r.problems
            .iter()
            .any(|p| p.severity == Severity::Warn && p.what.contains("is invalid")),
        "{:?}",
        r.problems
    );
    let errors: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .map(|p| p.what.clone())
        .collect();
    assert!(
        errors.iter().any(|e| e == "no session archive can be read"),
        "{errors:?}"
    );
}

#[test]
fn an_index_from_another_version_is_reported_as_due_for_a_rebuild() {
    let f = Fixture::new().with_fakes();
    f.claude("-w", ID_A, &[claude_user("/w", "hi")], 0);
    trovador::sessions::reindex(&f.ctx(), false).unwrap();
    let conn = rusqlite::Connection::open(trovador::index::db_path(&f.paths)).unwrap();
    conn.execute("DELETE FROM meta WHERE key = 'projection'", [])
        .unwrap();
    drop(conn);
    let r = report(&f);
    assert!(r.index.is_none());
    assert!(
        r.problems
            .iter()
            .any(|p| p.what.contains("built by another version")),
        "{:?}",
        r.problems
    );
}

#[tokio::test]
async fn migration_warning_reaches_the_shared_doctor_report() {
    let f = Fixture::new();
    let ctx = f.ctx();
    std::fs::create_dir(f.dir.path().join(".trovador")).unwrap();
    let report = trovador::doctor::report(&ctx).await;
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.what.contains("without merging"))
    );
}

#[tokio::test]
async fn doctor_rechecks_recovery_conditions_and_provides_separate_fixes() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let root = ctx.paths.state.root.clone();
    std::fs::create_dir(f.dir.path().join(".trovador")).unwrap();
    std::fs::create_dir(f.dir.path().join(".trovador.migrated")).unwrap();
    std::fs::create_dir(root.join("trovador.partial")).unwrap();
    std::fs::write(ctx.paths.home.join("spawned.json"), "source").unwrap();
    std::fs::write(ctx.paths.shared.join("spawned.json"), "target").unwrap();
    let report = doctor::report(&ctx).await;
    let conditions = [
        "without merging",
        "incomplete migration copy",
        "retired migration source",
        "shared state conflict",
    ];
    let fixes: std::collections::HashSet<_> = conditions
        .iter()
        .map(|condition| {
            &report
                .problems
                .iter()
                .find(|p| p.what.contains(condition))
                .unwrap()
                .fix
        })
        .collect();
    assert_eq!(fixes.len(), 4);
    std::fs::remove_dir(f.dir.path().join(".trovador")).unwrap();
    std::fs::remove_dir(f.dir.path().join(".trovador.migrated")).unwrap();
    std::fs::remove_dir(root.join("trovador.partial")).unwrap();
    std::fs::remove_file(ctx.paths.home.join("spawned.json")).unwrap();
    let report = doctor::report(&ctx).await;
    assert!(
        conditions
            .iter()
            .all(|condition| !report.problems.iter().any(|p| p.what.contains(condition)))
    );
}

mod common;

use common::*;
use trovador::doctor::{self, Severity};

fn report(f: &Fixture) -> doctor::Report {
    doctor::report(&f.ctx())
}

#[test]
fn a_healthy_machine_reports_archives_and_no_errors() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "hi")], 0);
    trovador::sessions::reindex(&f.ctx(), false).unwrap();
    let r = report(&f);
    assert!(r.ok, "{:?}", r.problems);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.archives[0].sessions, 1);
    assert_eq!(r.index.unwrap().sessions, 1);
}

#[test]
fn a_missing_index_is_a_warning_because_reading_still_works() {
    let f = Fixture::new();
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
        warns.iter().any(|w| w.contains("no search index yet")),
        "{warns:?}"
    );
}

#[test]
fn no_archives_is_an_error() {
    let f = Fixture::new();
    let r = report(&f);
    assert!(!r.ok);
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
    let f = Fixture::new();
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

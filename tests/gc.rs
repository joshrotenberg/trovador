mod common;

use std::io::Write;
use std::time::Duration;

use common::*;
use serde_json::json;
use trovador::summary;

fn summarize(f: &Fixture, id: &str) {
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(summary::summarize(&f.ctx(), id, None, false, None))
        .unwrap();
}

/// Four summaries: one for a session since deleted, one for a session that changed after
/// it was made, one current but made long ago, and one current and recent.
fn fixture() -> (Fixture, std::path::PathBuf) {
    let f = Fixture::new().with_fakes();
    let gone = f.claude("-w", ID_A, &[claude_user("/w", "a")], 100);
    let changed = f.claude("-w", ID_B, &[claude_user("/w", "b")], 100);
    f.claude("-w", ID_C, &[claude_user("/w", "c")], 100);
    f.claude("-w", ID_D, &[claude_user("/w", "d")], 100);
    for id in [ID_A, ID_B, ID_C, ID_D] {
        summarize(&f, id);
    }
    std::fs::remove_file(gone).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(changed)
        .unwrap();
    writeln!(file, "{}", claude_assistant("more")).unwrap();
    // Backdate ID_C's summary.
    let p = summary::dir(&f.ctx()).join(format!("claude-{ID_C}.json"));
    let mut s: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    s["created"] = json!("2025-01-01T00:00:00.000Z");
    std::fs::write(&p, s.to_string()).unwrap();

    let log = f.paths.home.join("runs.jsonl");
    std::fs::write(
        &log,
        [
            json!({"phase": "end", "at": "2025-01-01T00:00:00.000Z"}).to_string(),
            "not json".to_string(),
            json!({"phase": "end", "at": humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string()})
                .to_string(),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    (f, log)
}

#[test]
fn durations_before_the_epoch_keep_age_eligible_data_without_panicking() {
    for older_than in [Duration::from_secs(86_400 * 365 * 100), Duration::MAX] {
        for apply in [false, true] {
            let (f, log) = fixture();
            let before_log = std::fs::read_to_string(&log).unwrap();
            let outputs = f.paths.home.join("outputs");
            std::fs::create_dir_all(&outputs).unwrap();
            let output = outputs.join("reply.txt");
            std::fs::write(&output, "saved reply").unwrap();

            let r = trovador::gc::gc(&f.ctx(), older_than, apply).unwrap();

            assert_eq!(r.dry_run, !apply);
            assert_eq!(
                (
                    r.summaries.session_gone,
                    r.summaries.stale,
                    r.summaries.old,
                    r.summaries.kept
                ),
                (1, 1, 0, 2)
            );
            assert_eq!((r.outputs.old, r.outputs.kept), (0, 1));
            assert_eq!((r.runs.removed, r.runs.kept), (0, 3));
            assert_eq!(std::fs::read_to_string(&log).unwrap(), before_log);
            assert_eq!(std::fs::read_to_string(&output).unwrap(), "saved reply");
            for id in [ID_C, ID_D] {
                assert!(
                    summary::dir(&f.ctx())
                        .join(format!("claude-{id}.json"))
                        .exists()
                );
            }
        }
    }
}

#[test]
fn a_dry_run_reports_and_changes_nothing() {
    let (f, log) = fixture();
    let before_log = std::fs::read_to_string(&log).unwrap();
    let r = trovador::gc::gc(&f.ctx(), Duration::from_secs(90 * 86_400), false).unwrap();
    assert!(r.dry_run);
    let s = &r.summaries;
    assert_eq!((s.session_gone, s.stale, s.old, s.kept), (1, 1, 1, 1));
    assert_eq!((r.runs.removed, r.runs.kept), (1, 2));
    assert_eq!(
        std::fs::read_dir(summary::dir(&f.ctx())).unwrap().count(),
        4
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap(), before_log);
}

#[test]
fn apply_removes_exactly_what_will_never_be_used_and_leaves_provenance_alone() {
    let (f, log) = fixture();
    let provenance_before = std::fs::read_to_string(trovador::provenance::path(&f.paths)).ok();
    let r = trovador::gc::gc(&f.ctx(), Duration::from_secs(90 * 86_400), true).unwrap();
    assert!(!r.dry_run);
    let left: Vec<String> = std::fs::read_dir(summary::dir(&f.ctx()))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(left, vec![format!("claude-{ID_D}.json")]);
    let lines: Vec<String> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        lines.len(),
        2,
        "the unreadable line and the recent line stay"
    );
    assert_eq!(lines[0], "not json");
    assert_eq!(
        std::fs::read_to_string(trovador::provenance::path(&f.paths)).ok(),
        provenance_before
    );
    assert!(r.summaries.bytes_removed > 0);
}

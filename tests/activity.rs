//! A session's `modified` is when it was last active, from its records, not the file's
//! mtime (#102). 56 Claude session files on the development machine shared one mtime from
//! a bulk touch, and one of them had last recorded anything seven weeks earlier.

mod common;

use std::path::Path;
use std::time::{Duration, SystemTime};

use common::*;
use serde_json::{Value, json};
use trovador::index::{Order, SearchOpts};
use trovador::sessions::{self, ListOpts};
use trovador::{Ctx, Limits};

const DAY: u64 = 86_400;
const NEEDLE: &str = "zqactivityneedle";

fn ago(secs: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(secs)
}

fn rfc3339(t: SystemTime) -> String {
    humantime::format_rfc3339_millis(t).to_string()
}

/// `record` with the `timestamp` both providers write, `secs` ago.
fn stamped(mut record: Value, secs: u64) -> Value {
    record["timestamp"] = json!(rfc3339(ago(secs)));
    record
}

fn touch(path: &Path, secs: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(ago(secs))
        .unwrap();
}

fn list(ctx: &Ctx, opts: ListOpts) -> Vec<(String, String)> {
    sessions::list(ctx, &opts)
        .unwrap()
        .sessions
        .into_iter()
        .map(|s| (s.session_id, s.modified))
        .collect()
}

fn search(ctx: &Ctx, opts: SearchOpts) -> sessions::SearchResult {
    sessions::search(
        ctx,
        &SearchOpts {
            query: NEEDLE.into(),
            ..opts
        },
    )
    .unwrap()
}

/// Seconds between a reported `modified` and `t`.
fn off_by(modified: &str, t: SystemTime) -> u64 {
    let m = humantime::parse_rfc3339(modified).unwrap();
    m.duration_since(t)
        .or_else(|_| t.duration_since(m))
        .unwrap()
        .as_secs()
}

/// A: last recorded 30 days ago, file touched just now. B: recorded and written a day ago.
fn touched_fixture() -> Fixture {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[
            stamped(claude_user("/w", &format!("{NEEDLE} old work")), 31 * DAY),
            stamped(claude_assistant("done"), 30 * DAY),
        ],
        0,
    );
    f.claude(
        "-w",
        ID_B,
        &[
            stamped(claude_user("/w", &format!("{NEEDLE} recent work")), DAY),
            stamped(claude_assistant("done"), DAY),
        ],
        DAY,
    );
    f
}

#[test]
fn a_touched_file_reports_and_sorts_by_its_last_record() {
    let f = touched_fixture();
    // Without an index, from the metadata read, and again from the index's copy.
    for indexed in [false, true] {
        let c = f.ctx();
        if indexed {
            sessions::reindex(&c, false).unwrap();
        }
        let rows = list(&c, ListOpts::default());
        assert_eq!(rows[0].0, ID_B, "indexed={indexed}: {rows:?}");
        assert_eq!(rows[1].0, ID_A);
        assert!(off_by(&rows[1].1, ago(30 * DAY)) < 5, "{rows:?}");
        let recent = list(
            &c,
            ListOpts {
                since: Some("7d".into()),
                ..Default::default()
            },
        );
        assert_eq!(recent.len(), 1, "indexed={indexed}: {recent:?}");
        assert_eq!(recent[0].0, ID_B);
        let older = list(
            &c,
            ListOpts {
                until: Some("7d".into()),
                ..Default::default()
            },
        );
        assert_eq!(older.len(), 1, "indexed={indexed}: {older:?}");
        assert_eq!(older[0].0, ID_A);
    }
}

#[test]
fn search_filters_and_orders_by_activity_too() {
    let f = touched_fixture();
    let c = f.ctx();
    let r = search(
        &c,
        SearchOpts {
            order: Order::Recent,
            ..Default::default()
        },
    );
    let ids: Vec<_> = r.hits.iter().map(|h| h.row.session_id.as_str()).collect();
    assert_eq!(ids, [ID_B, ID_A]);
    assert!(off_by(&r.hits[1].row.modified, ago(30 * DAY)) < 5);
    let r = search(
        &c,
        SearchOpts {
            since: Some("7d".into()),
            ..Default::default()
        },
    );
    assert_eq!(r.sessions_matched, 1);
    assert_eq!(r.hits[0].row.session_id, ID_B);
}

#[test]
fn a_touched_file_is_not_active() {
    let f = touched_fixture();
    f.claude(
        "-w",
        ID_C,
        &[stamped(
            claude_user("/w", &format!("{NEEDLE} running now")),
            5,
        )],
        5,
    );
    let c = f.ctx_with(Limits {
        active_window_s: 600,
        ..limits()
    });
    let r = search(&c, SearchOpts::default());
    assert_eq!(r.hidden.active, 1, "only the session written just now");
    let ids: Vec<_> = r.hits.iter().map(|h| h.row.session_id.as_str()).collect();
    assert!(ids.contains(&ID_A) && !ids.contains(&ID_C), "{ids:?}");
}

#[test]
fn a_session_without_timestamps_uses_the_mtime() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "no times")], 3 * DAY);
    let rows = list(&f.ctx(), ListOpts::default());
    assert!(off_by(&rows[0].1, ago(3 * DAY)) < 5, "{rows:?}");
}

#[test]
fn a_record_later_than_the_mtime_is_held_to_the_mtime() {
    // A record clock ahead of the filesystem's must not put a session in the future.
    let f = Fixture::new();
    let mut r = claude_user("/w", "skewed");
    r["timestamp"] = json!(rfc3339(SystemTime::now() + Duration::from_secs(DAY)));
    f.claude("-w", ID_A, &[r], 2 * DAY);
    let rows = list(&f.ctx(), ListOpts::default());
    assert!(off_by(&rows[0].1, ago(2 * DAY)) < 5, "{rows:?}");
}

#[test]
fn an_appended_record_moves_activity_forward_after_an_index_resume() {
    use std::io::Write;
    let f = Fixture::new();
    let path = f.claude(
        "-w",
        ID_A,
        &[stamped(
            claude_user("/w", &format!("{NEEDLE} start")),
            10 * DAY,
        )],
        10 * DAY,
    );
    let c = f.ctx();
    let first = search(&c, SearchOpts::default());
    assert!(off_by(&first.hits[0].row.modified, ago(10 * DAY)) < 5);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "{}", stamped(claude_assistant("later"), 2 * DAY)).unwrap();
    drop(file);
    touch(&path, DAY);
    let r = search(&c, SearchOpts::default());
    assert_eq!(r.coverage.refresh.appended, 1);
    assert!(off_by(&r.hits[0].row.modified, ago(2 * DAY)) < 5);
}

#[test]
fn codex_rollouts_report_their_last_record_plain_and_compressed() {
    let f = Fixture::new();
    let records = |text: &str| {
        vec![
            stamped(codex_meta("/w", json!("vscode")), 20 * DAY),
            stamped(codex_user_item(&format!("{NEEDLE} {text}")), 20 * DAY),
            stamped(codex_agent_item("ok"), 19 * DAY),
        ]
    };
    f.codex(ID_A, &records("plain"), 0);
    f.codex_zstd(ID_B, &records("compressed"), 0, true);
    let c = f.ctx();
    // The plain rollout from its tail, before any index exists.
    let rows = list(&c, ListOpts::default());
    let plain = rows.iter().find(|(id, _)| id == ID_A).unwrap();
    assert!(off_by(&plain.1, ago(19 * DAY)) < 5, "{rows:?}");
    // The compressed one from the index, which reads every record.
    let r = search(&c, SearchOpts::default());
    assert_eq!(r.sessions_matched, 2);
    for h in &r.hits {
        assert!(
            off_by(&h.row.modified, ago(19 * DAY)) < 5,
            "{}: {}",
            h.row.session_id,
            h.row.modified
        );
    }
}

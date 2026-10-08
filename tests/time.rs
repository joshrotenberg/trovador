//! since and until on list and search.

mod common;

use common::*;
use trovador::index::SearchOpts;
use trovador::sessions::{self, ListOpts};

const HOUR: u64 = 3_600;
const DAY: u64 = 24 * HOUR;

/// Sessions last written 1 hour, 3 days, and 10 days ago, all mentioning "the build".
fn fixture() -> Fixture {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[claude_user("/w", "fix the build today")],
        HOUR,
    );
    f.claude(
        "-w",
        ID_B,
        &[claude_user("/w", "fix the build midweek")],
        3 * DAY,
    );
    f.claude(
        "-w",
        ID_C,
        &[claude_user("/w", "fix the build long ago")],
        10 * DAY,
    );
    f
}

fn listed(f: &Fixture, since: Option<&str>, until: Option<&str>) -> Vec<String> {
    sessions::list(
        &f.ctx(),
        &ListOpts {
            since: since.map(str::to_string),
            until: until.map(str::to_string),
            ..Default::default()
        },
    )
    .unwrap()
    .sessions
    .into_iter()
    .map(|r| r.session_id)
    .collect()
}

fn found(f: &Fixture, since: Option<&str>, until: Option<&str>) -> Vec<String> {
    let mut ids: Vec<String> = sessions::search(
        &f.ctx(),
        &SearchOpts {
            query: "the build".into(),
            since: since.map(str::to_string),
            until: until.map(str::to_string),
            ..Default::default()
        },
    )
    .unwrap()
    .hits
    .into_iter()
    .map(|h| h.row.session_id)
    .collect();
    ids.sort();
    ids
}

#[test]
fn since_keeps_sessions_written_within_the_window() {
    let f = fixture();
    assert_eq!(listed(&f, Some("2d"), None), vec![ID_A]);
    assert_eq!(listed(&f, Some("1w"), None), vec![ID_A, ID_B]);
    assert_eq!(found(&f, Some("2d"), None), vec![ID_A]);
    assert_eq!(found(&f, Some("1w"), None), vec![ID_A, ID_B]);
}

#[test]
fn until_keeps_sessions_written_before_it_and_combines_with_since() {
    let f = fixture();
    assert_eq!(listed(&f, None, Some("2d")), vec![ID_B, ID_C]);
    assert_eq!(listed(&f, Some("1w"), Some("2d")), vec![ID_B]);
    assert_eq!(found(&f, None, Some("1w")), vec![ID_C]);
    assert_eq!(found(&f, Some("1w"), Some("2d")), vec![ID_B]);
}

#[test]
fn an_absolute_date_works_like_a_duration() {
    let f = fixture();
    let five_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(5 * DAY);
    let date = humantime::format_rfc3339_seconds(five_days_ago).to_string();
    assert_eq!(listed(&f, Some(&date), None), vec![ID_A, ID_B]);
}

#[test]
fn a_value_that_does_not_parse_is_an_error_not_an_ignored_filter() {
    let f = fixture();
    let err = sessions::list(
        &f.ctx(),
        &ListOpts {
            since: Some("last tuesday".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("since"), "{err}");
    let err = sessions::search(
        &f.ctx(),
        &SearchOpts {
            query: "build".into(),
            until: Some("soon".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("until"), "{err}");
}

#[test]
fn an_oversized_session_names_the_call_that_continues_its_work() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/", &"z".repeat(950_000))], 0);
    let d = sessions::show(&f.ctx(), ID_A, None).unwrap();
    let reason = d.resume_blocked_by.unwrap();
    assert!(reason.contains("ask with about"), "{reason}");
}

//! Session rows carry each fact once (#104). `cwd` and `relocated_cwd` repeated `project`,
//! and `resource` and `open_with` are derived from the provider, id and project; a
//! 20-session list was 14,199 characters with them and 8,665 without.

mod common;

use common::*;
use serde_json::{Value, json};
use trovador::index::SearchOpts;
use trovador::sessions::{self, ListOpts};

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "zqrowneedle here")], 200);
    f.claude(
        "-old",
        ID_B,
        &[
            claude_user("/old/home", "zqrowneedle moved"),
            json!({"type": "relocated", "relocatedCwd": "/new/home"}),
        ],
        100,
    );
    f
}

fn rows(f: &Fixture) -> Vec<Value> {
    let l = sessions::list(&f.ctx(), &ListOpts::default()).unwrap();
    serde_json::to_value(l).unwrap()["sessions"]
        .as_array()
        .unwrap()
        .clone()
}

fn by_id<'a>(rows: &'a [Value], id: &str) -> &'a Value {
    rows.iter().find(|r| r["session_id"] == id).unwrap()
}

#[test]
fn a_row_names_its_directory_once() {
    let f = fixture();
    let rows = rows(&f);
    let plain = by_id(&rows, ID_A);
    assert_eq!(plain["project"], "/w");
    assert!(plain.get("cwd").is_none(), "{plain}");
    assert!(plain.get("relocated_cwd").is_none(), "{plain}");
}

#[test]
fn a_relocated_row_keeps_where_it_started_because_that_differs() {
    let f = fixture();
    let rows = rows(&f);
    let moved = by_id(&rows, ID_B);
    assert_eq!(moved["project"], "/new/home");
    assert_eq!(moved["cwd"], "/old/home");
    assert!(moved.get("relocated_cwd").is_none(), "{moved}");
}

#[test]
fn list_and_search_rows_leave_out_derived_fields() {
    let f = fixture();
    let search = sessions::search(
        &f.ctx(),
        &SearchOpts {
            query: "zqrowneedle".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let hits = serde_json::to_value(search).unwrap()["hits"].clone();
    for r in rows(&f).iter().chain(hits.as_array().unwrap()) {
        assert!(r.get("resource").is_none(), "{r}");
        assert!(r.get("open_with").is_none(), "{r}");
    }
}

#[test]
fn show_still_gives_the_resume_command_and_resources() {
    let f = fixture();
    let d = serde_json::to_value(sessions::show(&f.ctx(), ID_B, None).unwrap()).unwrap();
    assert_eq!(
        d["open_with"],
        format!("cd '/new/home' && claude --resume {ID_B}")
    );
    assert_eq!(d["resource"], format!("session://claude/{ID_B}"));
    assert_eq!(
        d["transcript_resource"],
        format!("session://claude/{ID_B}/transcript")
    );
    assert_eq!(d["cwd"], "/old/home");
}

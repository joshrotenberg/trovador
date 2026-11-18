//! A transcript or text page reports the revision of the session it was read from, and a
//! continuation that names an earlier revision is refused instead of read (trovador#83).

mod common;

use std::fs;
use std::io::Write;
use std::process::Command;
use std::time::SystemTime;

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::archive;
use trovador::revision;
use trovador::sessions::{self, Error};

fn claude_fixture() -> (Fixture, std::path::PathBuf) {
    let f = Fixture::new();
    let path = f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "first question"),
            claude_assistant("first answer"),
            claude_user("/w", "second question"),
            claude_assistant("second answer"),
        ],
        60,
    );
    (f, path)
}

fn append(path: &std::path::Path, record: &Value) {
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{record}").unwrap();
}

/// Rewrites `path` with `records` and puts the old mtime back, as a restore or `cp -p` would.
fn rewrite_keeping_mtime(path: &std::path::Path, records: &[Value]) {
    let mtime = fs::metadata(path).unwrap().modified().unwrap();
    let body: String = records.iter().map(|r| format!("{r}\n")).collect();
    fs::write(path, body).unwrap();
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

/// The fixture's records with one letter of the last answer changed: the same size.
fn same_size_rewrite() -> Vec<Value> {
    vec![
        claude_user("/w", "first question"),
        claude_assistant("first answer"),
        claude_user("/w", "second question"),
        claude_assistant("secOnd answer"),
    ]
}

fn assert_stale(result: Result<impl std::fmt::Debug, Error>, expected: &str, current: &str) {
    match result {
        Err(Error::StaleRevision {
            expected: e,
            current: c,
            ..
        }) => {
            assert_eq!(
                e, expected,
                "the error names the revision the caller passed"
            );
            assert_eq!(c, current, "the error names the revision to restart from");
        }
        other => panic!("expected StaleRevision, got {other:?}"),
    }
}

#[test]
fn an_unchanged_session_pages_exactly_as_before_under_one_stable_revision() {
    let (f, _) = claude_fixture();
    let ctx = f.ctx();
    let whole = sessions::transcript(&ctx, ID_A, None, 0, Some(200)).unwrap();
    let mut records = Vec::new();
    let mut offset = 0;
    loop {
        let page =
            sessions::transcript_at(&ctx, ID_A, None, offset, Some(1), Some(&whole.revision))
                .unwrap();
        assert_eq!(
            page.revision, whole.revision,
            "revision moved between pages"
        );
        records.extend(page.page.records);
        match page.page.next_offset {
            Some(n) => offset = n,
            None => break,
        }
    }
    assert_eq!(
        records, whole.page.records,
        "paging with the guard lost or repeated records"
    );
    let again = sessions::transcript(&ctx, ID_A, None, 0, Some(200)).unwrap();
    assert_eq!(
        again.revision, whole.revision,
        "reading changed the revision"
    );
    assert_eq!(whole.revision.len(), 16);
}

#[test]
fn text_parts_of_an_unchanged_session_share_one_revision_and_continue_with_it() {
    let (f, _) = claude_fixture();
    let ctx = f.ctx_with(trovador::Limits {
        response_max_chars: 40,
        ..limits()
    });
    let first = sessions::text(&ctx, ID_A, None, 0).unwrap();
    let truncated = first
        .truncated
        .clone()
        .expect("the text spans several parts");
    let hint = truncated.continue_with.unwrap();
    assert!(
        hint.contains(&format!("expected_revision={}", first.revision))
            && hint.contains(&format!("--expected-revision {}", first.revision)),
        "the continuation hint drops the revision: {hint}"
    );
    let mut offset = truncated.next_offset.unwrap();
    let mut joined = first.text.clone();
    loop {
        let part = sessions::text_at(&ctx, ID_A, None, offset, Some(&first.revision)).unwrap();
        assert_eq!(part.revision, first.revision);
        joined.push_str(&part.text);
        match part.truncated.and_then(|t| t.next_offset) {
            Some(n) => offset = n,
            None => break,
        }
    }
    let whole = sessions::text(&f.ctx(), ID_A, None, 0).unwrap().text;
    assert_eq!(joined, whole);
}

#[test]
fn an_append_that_leaves_earlier_records_alone_still_invalidates_a_transcript_continuation() {
    let (f, path) = claude_fixture();
    let ctx = f.ctx();
    let first = sessions::transcript(&ctx, ID_A, None, 0, Some(2)).unwrap();
    append(&path, &claude_assistant("third answer"));
    let now = sessions::transcript(&ctx, ID_A, None, 0, Some(2)).unwrap();
    assert_ne!(
        now.revision, first.revision,
        "an append left the revision alone"
    );
    assert_stale(
        sessions::transcript_at(&ctx, ID_A, None, 2, Some(2), Some(&first.revision)),
        &first.revision,
        &now.revision,
    );
}

#[test]
fn an_append_invalidates_a_text_continuation() {
    let (f, path) = claude_fixture();
    let ctx = f.ctx();
    let first = sessions::text(&ctx, ID_A, None, 0).unwrap();
    append(&path, &claude_assistant("third answer"));
    let now = sessions::text(&ctx, ID_A, None, 0).unwrap();
    assert_stale(
        sessions::text_at(&ctx, ID_A, None, 10, Some(&first.revision)),
        &first.revision,
        &now.revision,
    );
}

#[test]
fn a_rewrite_with_the_same_size_and_mtime_invalidates_both_kinds_of_continuation() {
    let (f, path) = claude_fixture();
    let ctx = f.ctx();
    let t = sessions::transcript(&ctx, ID_A, None, 0, Some(2)).unwrap();
    let x = sessions::text(&ctx, ID_A, None, 0).unwrap();
    assert_eq!(t.revision, x.revision, "one session, two revisions");
    let size = fs::metadata(&path).unwrap().len();
    rewrite_keeping_mtime(&path, &same_size_rewrite());
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        size,
        "the fixture must keep the size"
    );
    let now = sessions::transcript(&ctx, ID_A, None, 0, Some(2)).unwrap();
    assert_ne!(
        now.revision, t.revision,
        "a same-size rewrite went unnoticed"
    );
    assert_stale(
        sessions::transcript_at(&ctx, ID_A, None, 2, Some(2), Some(&t.revision)),
        &t.revision,
        &now.revision,
    );
    assert_stale(
        sessions::text_at(&ctx, ID_A, None, 5, Some(&x.revision)),
        &x.revision,
        &now.revision,
    );
}

#[test]
fn a_rewrite_with_a_new_mtime_and_a_different_size_invalidates_a_continuation() {
    let (f, _) = claude_fixture();
    let ctx = f.ctx();
    let first = sessions::transcript(&ctx, ID_A, None, 0, Some(2)).unwrap();
    f.claude(
        "-w",
        ID_A,
        &[claude_user("/w", "something else entirely")],
        0,
    );
    let now = sessions::transcript(&ctx, ID_A, None, 0, Some(2)).unwrap();
    assert_stale(
        sessions::transcript_at(&ctx, ID_A, None, 2, Some(2), Some(&first.revision)),
        &first.revision,
        &now.revision,
    );
}

#[test]
fn a_call_without_an_expected_revision_reads_a_changed_session_as_it_always_did() {
    let (f, path) = claude_fixture();
    let ctx = f.ctx();
    let before = sessions::transcript(&ctx, ID_A, None, 0, Some(200)).unwrap();
    append(&path, &claude_assistant("third answer"));
    let after = sessions::transcript(&ctx, ID_A, None, 0, Some(200)).unwrap();
    let n = before.page.records.len();
    assert_eq!(after.page.records.len(), n + 1);
    assert_eq!(after.page.records[..n], before.page.records[..]);
    let text = sessions::text(&ctx, ID_A, None, 0).unwrap();
    assert!(text.text.contains("third answer"));
    let at_none = sessions::text_at(&ctx, ID_A, None, 0, None).unwrap();
    assert_eq!(at_none.text, text.text);
}

#[test]
fn a_matching_expected_revision_returns_the_same_page_as_no_expectation() {
    let (f, _) = claude_fixture();
    let ctx = f.ctx();
    let plain = sessions::transcript(&ctx, ID_A, None, 1, Some(2)).unwrap();
    let guarded =
        sessions::transcript_at(&ctx, ID_A, None, 1, Some(2), Some(&plain.revision)).unwrap();
    assert_eq!(
        serde_json::to_value(&plain).unwrap(),
        serde_json::to_value(&guarded).unwrap()
    );
}

#[test]
fn an_empty_expected_revision_is_a_mismatch_not_an_absent_one() {
    let (f, _) = claude_fixture();
    let ctx = f.ctx();
    let now = sessions::transcript(&ctx, ID_A, None, 0, None).unwrap();
    assert_stale(
        sessions::transcript_at(&ctx, ID_A, None, 0, None, Some("")),
        "",
        &now.revision,
    );
}

#[test]
fn a_read_never_returns_bytes_appended_after_the_revision_was_taken() {
    // The revision is taken from the scan; a read that went on to the end of the file
    // would return a newer record under that older label.
    let (f, path) = claude_fixture();
    let entry = archive::find_session(&f.paths, ID_A, None).unwrap();
    let before = trovador::transcript::page(&entry, 0, Some(200), &limits()).unwrap();
    append(&path, &claude_assistant("appended after the scan"));
    let during = trovador::transcript::page(&entry, 0, Some(200), &limits()).unwrap();
    assert_eq!(
        during.records, before.records,
        "the page read past the scanned size"
    );
    let text = trovador::text::render(&entry, &limits()).unwrap();
    assert!(!text.contains("appended after the scan"));
}

#[test]
fn a_rewrite_during_a_read_is_detected_and_an_append_during_it_is_not_an_error() {
    let (f, path) = claude_fixture();
    let entry = archive::find_session(&f.paths, ID_A, None).unwrap();
    let rev = revision::of(&entry, &limits()).unwrap();
    append(&path, &claude_assistant("appended during the read"));
    assert!(
        rev.still_holds(&entry).unwrap(),
        "growth alone failed the read"
    );
    let mut rewritten = same_size_rewrite();
    rewritten.push(claude_assistant("appended during the read"));
    rewrite_keeping_mtime(&path, &rewritten);
    assert!(
        !rev.still_holds(&entry).unwrap(),
        "a rewrite during the read went unnoticed"
    );
    fs::write(&path, "{}\n").unwrap();
    assert!(
        !rev.still_holds(&entry).unwrap(),
        "a truncation went unnoticed"
    );
}

#[test]
fn a_touch_with_no_content_change_is_reported_as_a_new_revision() {
    // Documented behaviour: mtime is part of the revision, so this fails safe.
    let (f, path) = claude_fixture();
    let ctx = f.ctx();
    let first = sessions::transcript(&ctx, ID_A, None, 0, None).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now())
        .unwrap();
    let now = sessions::transcript(&ctx, ID_A, None, 0, None).unwrap();
    assert_ne!(now.revision, first.revision);
    assert_eq!(now.page.records, first.page.records);
}

#[test]
fn a_different_record_cap_changes_the_revision_because_text_offsets_move() {
    let (f, _) = claude_fixture();
    let a = sessions::text(&f.ctx(), ID_A, None, 0).unwrap();
    let b = sessions::text(
        &f.ctx_with(trovador::Limits {
            record_chars: 10,
            ..limits()
        }),
        ID_A,
        None,
        0,
    )
    .unwrap();
    assert_ne!(a.revision, b.revision);
}

fn codex_records() -> Vec<Value> {
    vec![
        codex_meta("/w", json!("cli")),
        codex_user_item("codex question"),
        codex_agent_item("codex answer"),
    ]
}

#[test]
fn a_compressed_rollout_has_a_stable_revision_and_guards_both_kinds_of_page() {
    let f = Fixture::new();
    let path = f.codex_zstd(ID_B, &codex_records(), 60, false);
    let ctx = f.ctx();
    let t = sessions::transcript(&ctx, ID_B, None, 0, None).unwrap();
    let x = sessions::text(&ctx, ID_B, None, 0).unwrap();
    assert_eq!(t.revision, x.revision);
    assert_eq!(
        sessions::transcript(&ctx, ID_B, None, 0, None)
            .unwrap()
            .revision,
        t.revision,
        "reading a compressed rollout changed its revision"
    );
    sessions::transcript_at(&ctx, ID_B, None, 0, None, Some(&t.revision)).unwrap();
    sessions::text_at(&ctx, ID_B, None, 0, Some(&x.revision)).unwrap();

    let mut longer = codex_records();
    longer.push(codex_agent_item("a later answer"));
    let replaced = f.codex_zstd(ID_B, &longer, 0, false);
    assert_eq!(replaced, path);
    let now = sessions::transcript(&ctx, ID_B, None, 0, None).unwrap();
    assert_stale(
        sessions::transcript_at(&ctx, ID_B, None, 0, None, Some(&t.revision)),
        &t.revision,
        &now.revision,
    );
    assert_stale(
        sessions::text_at(&ctx, ID_B, None, 0, Some(&x.revision)),
        &x.revision,
        &now.revision,
    );
}

#[test]
fn compressing_a_rollout_without_changing_it_is_reported_as_a_new_revision() {
    // Documented behaviour: the revision names the stored file, and decoding it to hash
    // the decompressed content would read the whole session. The caller restarts, and the
    // records it gets are the same.
    let f = Fixture::new();
    let plain = f.codex(ID_B, &codex_records(), 60);
    let ctx = f.ctx();
    let before = sessions::transcript(&ctx, ID_B, None, 0, None).unwrap();
    let text_before = sessions::text(&ctx, ID_B, None, 0).unwrap();
    fs::remove_file(&plain).unwrap();
    f.codex_zstd(ID_B, &codex_records(), 60, false);
    let after = sessions::transcript(&ctx, ID_B, None, 0, None).unwrap();
    assert_ne!(after.revision, before.revision);
    assert_eq!(
        after.page.records, before.page.records,
        "compression changed the records"
    );
    assert_eq!(
        sessions::text(&ctx, ID_B, None, 0).unwrap().text,
        text_before.text
    );
    assert_stale(
        sessions::transcript_at(&ctx, ID_B, None, 0, None, Some(&before.revision)),
        &before.revision,
        &after.revision,
    );
}

// --- both doors -------------------------------------------------------------

fn cli_output(f: &Fixture, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(args)
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("TROVADOR_HOME", &f.paths.home)
        .env("TROVADOR_ACTIVE_WINDOW_S", "0")
        .output()
        .expect("run trovador")
}

fn cli(f: &Fixture, args: &[&str]) -> Value {
    let out = cli_output(f, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Whether the call was an error, and its text.
async fn mcp_sessions(f: &Fixture, args: Value) -> (bool, String) {
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let r = client.call_tool("sessions", args).await;
    (r.is_error, r.all_text())
}

#[tokio::test]
async fn both_doors_return_the_library_page_for_a_matching_expected_revision() {
    let (f, _) = claude_fixture();
    let ctx = f.ctx();
    let t = sessions::transcript(&ctx, ID_A, None, 1, Some(2)).unwrap();
    let lib_t = serde_json::to_value(&t).unwrap();
    let x = sessions::text(&ctx, ID_A, None, 0).unwrap();
    let lib_x = serde_json::to_value(&x).unwrap();
    assert_eq!(lib_t["revision"], t.revision);

    let r = t.revision.as_str();
    assert_eq!(
        cli(
            &f,
            &[
                "sessions",
                "transcript",
                ID_A,
                "--offset",
                "1",
                "--limit",
                "2",
                "--expected-revision",
                r
            ]
        ),
        lib_t
    );
    let (err, text) = mcp_sessions(
        &f,
        json!({"action": "transcript", "session_id": ID_A, "offset": 1, "limit": 2, "expected_revision": r}),
    )
    .await;
    assert!(!err, "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), lib_t);

    assert_eq!(
        cli(
            &f,
            &["sessions", "text", ID_A, "--expected-revision", &x.revision]
        ),
        lib_x
    );
    let (err, text) = mcp_sessions(
        &f,
        json!({"action": "text", "session_id": ID_A, "expected_revision": x.revision}),
    )
    .await;
    assert!(!err, "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), lib_x);
}

#[tokio::test]
async fn both_doors_refuse_a_stale_continuation_with_the_same_message_naming_both_revisions() {
    let (f, path) = claude_fixture();
    let ctx = f.ctx();
    let old = sessions::transcript(&ctx, ID_A, None, 0, Some(2))
        .unwrap()
        .revision;
    append(&path, &claude_assistant("third answer"));
    let now = sessions::transcript(&ctx, ID_A, None, 0, Some(2))
        .unwrap()
        .revision;

    for action in ["transcript", "text"] {
        let lib = if action == "transcript" {
            sessions::transcript_at(&ctx, ID_A, None, 2, Some(2), Some(&old)).map(|_| ())
        } else {
            sessions::text_at(&ctx, ID_A, None, 2, Some(&old)).map(|_| ())
        }
        .unwrap_err()
        .to_string();
        assert!(lib.contains(&old) && lib.contains(&now), "{lib}");

        let out = cli_output(
            &f,
            &[
                "sessions",
                action,
                ID_A,
                "--offset",
                "2",
                "--expected-revision",
                &old,
            ],
        );
        assert!(!out.status.success(), "{action}: the CLI returned a page");
        assert!(
            out.stdout.is_empty(),
            "{action}: the CLI printed a page on stdout"
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stderr).trim(),
            format!("trovador: {lib}")
        );

        let (err, text) = mcp_sessions(
            &f,
            json!({"action": action, "session_id": ID_A, "offset": 2, "expected_revision": old}),
        )
        .await;
        assert!(err, "{action}: the MCP tool returned a page");
        assert_eq!(text, lib, "{action}: the doors worded it differently");
    }
}

#[tokio::test]
async fn the_transcript_resource_reports_the_revision_the_tool_reports() {
    let (f, _) = claude_fixture();
    let tool = sessions::transcript(&f.ctx(), ID_A, None, 0, None)
        .unwrap()
        .revision;
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let read = client
        .read_resource(&format!("session://claude/{ID_A}/transcript"))
        .await;
    let json: Value = serde_json::from_str(read.first_text().unwrap()).unwrap();
    assert_eq!(json["revision"], tool);
}

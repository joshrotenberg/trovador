//! `TROVADOR_INDEX_REDACT`: secrets redacted before they are written to the index, not only
//! on output. Off by default, where the index keeps them so a search for a token still
//! finds its session.

mod common;

use common::*;
use trovador::index::{Index, SearchOpts};
use trovador::sessions;
use trovador::transcript::RecordKind;
use trovador::{Ctx, Limits};

const KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", &format!("deploy with {KEY} on zqredactneedle")),
            serde_json::json!({"type": "custom-title", "customTitle": format!("key {KEY}")}),
            claude_assistant("deploying"),
        ],
        100,
    );
    f
}

fn ctx(f: &Fixture, index_redact: bool, redact: bool) -> Ctx {
    f.ctx_with(Limits {
        index_redact,
        redact,
        ..limits()
    })
}

fn search(ctx: &Ctx, query: &str) -> sessions::SearchResult {
    sessions::search(
        ctx,
        &SearchOpts {
            query: query.into(),
            ..Default::default()
        },
    )
    .unwrap()
}

/// Every byte trovador keeps: the index and its WAL and shared-memory files.
fn home_holds(f: &Fixture, needle: &str) -> bool {
    std::fs::read_dir(&f.paths.home)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .any(|e| {
            let bytes = std::fs::read(e.path()).unwrap();
            bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
        })
}

#[test]
fn by_default_the_index_keeps_secrets_so_a_search_for_one_finds_its_session() {
    let f = fixture();
    let c = ctx(&f, false, true);
    let r = search(&c, KEY);
    assert_eq!(r.sessions_matched, 1);
    let excerpt = &r.hits[0].matches[0].excerpt;
    assert!(excerpt.contains("[redacted:anthropic]"), "{excerpt}");
    assert!(home_holds(&f, KEY), "the default index holds the key");
}

#[test]
fn with_index_redaction_no_byte_of_the_index_holds_the_secret() {
    let f = fixture();
    let c = ctx(&f, true, true);
    assert_eq!(search(&c, "zqredactneedle").sessions_matched, 1);
    // Record text, the first prompt in the stored metadata, and the custom title.
    assert!(!home_holds(&f, KEY));
    assert!(!home_holds(&f, "abcdefghijklmnopqrstuvwxyz0123456789"));
    let idx = Index::open(&c.paths, &c.limits).unwrap();
    let rows = idx
        .session_records(trovador::Provider::Claude, ID_A)
        .unwrap();
    assert_eq!(
        rows[0].2,
        "deploy with [redacted:anthropic] on zqredactneedle"
    );
}

#[test]
fn with_index_redaction_a_search_for_the_secret_finds_nothing() {
    let f = fixture();
    let c = ctx(&f, true, true);
    let r = search(&c, KEY);
    assert_eq!(r.sessions_matched, 0);
}

#[test]
fn a_redacted_match_still_points_at_its_transcript_record() {
    let f = fixture();
    let c = ctx(&f, true, true);
    let r = search(&c, "zqredactneedle");
    let at = r.hits[0].matches[0].index;
    let page = sessions::transcript(&c, ID_A, None, at, Some(1))
        .unwrap()
        .page;
    assert_eq!(page.records[0].kind, RecordKind::User);
    assert!(
        page.records[0]
            .text
            .as_deref()
            .unwrap()
            .contains("zqredactneedle")
    );
}

#[test]
fn without_output_redaction_the_index_still_cannot_return_the_secret_but_a_transcript_can() {
    // --no-redact turns off output redaction only. What the index never stored, an excerpt
    // cannot show; transcripts read the provider's file and still can.
    let f = fixture();
    let c = ctx(&f, true, false);
    let r = search(&c, "zqredactneedle");
    let excerpt = &r.hits[0].matches[0].excerpt;
    assert!(excerpt.contains("[redacted:anthropic]"), "{excerpt}");
    assert!(!excerpt.contains(KEY));
    let page = sessions::transcript(&c, ID_A, None, 0, None).unwrap().page;
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref().is_some_and(|t| t.contains(KEY)))
    );
}

#[test]
fn turning_index_redaction_on_replaces_an_index_that_held_secrets() {
    let f = fixture();
    assert_eq!(search(&ctx(&f, false, true), KEY).sessions_matched, 1);
    assert!(home_holds(&f, KEY));
    let on = ctx(&f, true, true);
    let r = search(&on, "zqredactneedle");
    assert_eq!(r.coverage.refresh.reindexed, 1, "a fresh index file");
    assert!(!home_holds(&f, KEY), "no page of the old file survives");
}

#[test]
fn appended_records_are_redacted_too() {
    use std::io::Write;
    let f = fixture();
    let c = ctx(&f, true, true);
    assert_eq!(search(&c, "zqredactneedle").sessions_matched, 1);
    let path = f
        .paths
        .claude_archive
        .join("-w")
        .join(format!("{ID_A}.jsonl"));
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    let later = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
    writeln!(
        file,
        "{}",
        claude_user("/w", &format!("also {later} zqappendneedle"))
    )
    .unwrap();
    drop(file);
    let r = search(&c, "zqappendneedle");
    assert_eq!(r.coverage.refresh.appended, 1);
    assert!(!home_holds(&f, later));
}

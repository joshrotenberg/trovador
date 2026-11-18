//! Prompts typed while a Claude turn runs (#89).
//!
//! Claude Code records the prompt being queued (`queue-operation` enqueue) and then either
//! dequeues it into an ordinary user record or delivers it into the running turn as a
//! `queued_command` attachment. The attachment was shown as `omitted: attachment`, so 3,067
//! of 15,331 local queued prompts never reached a transcript, session text or search.

mod common;

use common::*;
use serde_json::{Value, json};
use trovador::index::SearchOpts;
use trovador::sessions;
use trovador::transcript::{Record, RecordKind};

fn enqueue(text: &str) -> Value {
    json!({"type": "queue-operation", "operation": "enqueue", "content": text,
           "sessionId": ID_A, "timestamp": "2026-11-18T20:00:00Z"})
}

fn dequeue() -> Value {
    json!({"type": "queue-operation", "operation": "dequeue",
           "sessionId": ID_A, "timestamp": "2026-11-18T20:00:01Z"})
}

fn remove(text: &str) -> Value {
    json!({"type": "queue-operation", "operation": "remove", "content": text,
           "sessionId": ID_A, "timestamp": "2026-11-18T20:00:01Z"})
}

/// A `queued_command` attachment as Claude Code 2.1 writes it for a typed prompt.
fn queued(prompt: Value) -> Value {
    json!({"type": "attachment", "attachment": {
        "type": "queued_command", "prompt": prompt, "commandMode": "prompt",
        "origin": {"kind": "user"}, "source_uuid": "5f1c7a52-0000-4000-8000-000000000009",
        "timestamp": "2026-11-18T20:00:02Z"}})
}

fn tool_turn() -> Vec<Value> {
    vec![
        claude_tool_use("Bash", json!({"command": "cargo test"})),
        json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]}}),
    ]
}

fn session(f: &Fixture, middle: Vec<Value>) {
    let mut recs = vec![claude_user("/w", "run the tests")];
    recs.extend(middle);
    recs.push(claude_assistant("done"));
    f.claude("-w", ID_A, &recs, 100);
}

fn records(f: &Fixture) -> Vec<Record> {
    sessions::transcript(&f.ctx(), ID_A, None, 0, None)
        .unwrap()
        .page
        .records
}

fn users(f: &Fixture) -> Vec<(Option<String>, String)> {
    records(f)
        .into_iter()
        .filter(|r| r.kind == RecordKind::User)
        .map(|r| (r.name, r.text.unwrap_or_default()))
        .collect()
}

fn matched(f: &Fixture, query: &str) -> Vec<String> {
    sessions::search(
        &f.ctx(),
        &SearchOpts {
            query: query.into(),
            ..Default::default()
        },
    )
    .unwrap()
    .hits
    .into_iter()
    .flat_map(|h| h.matches.into_iter().map(|m| m.kind))
    .collect()
}

fn queued_user(text: &str) -> (Option<String>, String) {
    (Some("queued".into()), text.into())
}

fn plain_user(text: &str) -> (Option<String>, String) {
    (None, text.into())
}

#[test]
fn a_prompt_delivered_into_the_running_turn_is_a_queued_user_record() {
    let f = Fixture::new();
    let mut middle = tool_turn();
    middle.extend([
        enqueue("also check zqqueuedneedle"),
        remove("also check zqqueuedneedle"),
        queued(json!("also check zqqueuedneedle")),
    ]);
    session(&f, middle);
    assert_eq!(
        users(&f),
        [
            plain_user("run the tests"),
            queued_user("also check zqqueuedneedle")
        ]
    );
    assert_eq!(matched(&f, "zqqueuedneedle"), ["user"]);
    let text = sessions::text(&f.ctx(), ID_A, None, 0).unwrap().text;
    assert!(text.contains("USER: also check zqqueuedneedle"), "{text}");
}

#[test]
fn a_queued_prompt_dequeued_into_an_ordinary_user_record_appears_once() {
    let f = Fixture::new();
    let mut middle = tool_turn();
    middle.extend([
        enqueue("zqdequeuedneedle next"),
        dequeue(),
        claude_user("/w", "zqdequeuedneedle next"),
    ]);
    session(&f, middle);
    assert_eq!(
        users(&f),
        [
            plain_user("run the tests"),
            plain_user("zqdequeuedneedle next")
        ]
    );
    assert_eq!(matched(&f, "zqdequeuedneedle"), ["user"]);
}

#[test]
fn a_prompt_removed_before_delivery_is_not_a_user_record() {
    // The model never received it; the queue-operation stays omitted.
    let f = Fixture::new();
    session(
        &f,
        vec![enqueue("zqwithdrawnneedle"), remove("zqwithdrawnneedle")],
    );
    assert_eq!(users(&f), [plain_user("run the tests")]);
    assert!(matched(&f, "zqwithdrawnneedle").is_empty());
    assert!(records(&f).iter().any(|r| r.kind == RecordKind::Omitted
        && r.name.as_deref() == Some("queue-operation")
        && r.count == Some(2)));
}

#[test]
fn a_task_notification_in_the_same_attachment_stays_omitted() {
    let f = Fixture::new();
    session(
        &f,
        vec![json!({"type": "attachment", "attachment": {
            "type": "queued_command", "commandMode": "task-notification",
            "prompt": "<task-notification>zqnotificationneedle</task-notification>"}})],
    );
    assert_eq!(users(&f), [plain_user("run the tests")]);
    assert!(matched(&f, "zqnotificationneedle").is_empty());
    assert!(
        records(&f)
            .iter()
            .any(|r| r.kind == RecordKind::Omitted && r.name.as_deref() == Some("attachment"))
    );
}

#[test]
fn a_meta_queued_command_without_a_mode_stays_omitted() {
    let f = Fixture::new();
    session(
        &f,
        vec![json!({"type": "attachment", "attachment": {
            "type": "queued_command", "isMeta": true, "prompt": "zqmetaneedle"}})],
    );
    assert_eq!(users(&f), [plain_user("run the tests")]);
    assert!(matched(&f, "zqmetaneedle").is_empty());
}

#[test]
fn a_queued_prompt_of_blocks_keeps_its_text_and_omits_its_image() {
    let f = Fixture::new();
    session(
        &f,
        vec![queued(json!([
            {"type": "text", "text": "what is in zqscreenshotneedle"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                                         "data": "QUJD".repeat(500)}},
        ]))],
    );
    assert_eq!(
        users(&f),
        [
            plain_user("run the tests"),
            queued_user("what is in zqscreenshotneedle")
        ]
    );
    assert!(
        records(&f)
            .iter()
            .any(|r| r.kind == RecordKind::Omitted && r.name.as_deref() == Some("image"))
    );
}

#[test]
fn a_compact_summary_quoting_a_queued_prompt_is_not_a_second_user_turn() {
    // 777 of the 818 later user records that contained a queued prompt were compact
    // summaries; they are system records, so the prompt is still one user turn.
    let f = Fixture::new();
    session(
        &f,
        vec![
            queued(json!("zqonceneedle please")),
            json!({"type": "user", "isCompactSummary": true, "isVisibleInTranscriptOnly": true,
                   "message": {"role": "user",
                               "content": "Summary: the user asked zqonceneedle please."}}),
        ],
    );
    assert_eq!(
        users(&f),
        [
            plain_user("run the tests"),
            queued_user("zqonceneedle please")
        ]
    );
    let systems: Vec<_> = records(&f)
        .into_iter()
        .filter(|r| r.kind == RecordKind::System)
        .map(|r| r.name)
        .collect();
    assert_eq!(systems, [Some("compact_summary".to_string())]);
}

#[test]
fn an_empty_queued_prompt_stays_omitted() {
    let f = Fixture::new();
    session(&f, vec![queued(json!("   "))]);
    assert_eq!(users(&f), [plain_user("run the tests")]);
}

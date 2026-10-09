mod common;

use common::*;
use serde_json::json;
use trovador::{index::SearchOpts, sessions, transcript::RecordKind};

#[test]
fn question_answers_are_user_text_in_transcripts_and_search_in_every_codex_stream() {
    for stream in 0..3 {
        let f = Fixture::new();
        let reply = format!(
            "<send_user_message_question_reply>{}</send_user_message_question_reply>",
            json!([
                {"question": "Which workflow?", "answer": "Issue branch, then PR to main (recommended)"},
                {"question": "When?", "answer": "After verification"}
            ])
        );
        let message = match stream {
            0 => codex_user_item(&reply),
            1 => codex_response("user", &reply),
            _ => codex_event("user_message", &reply),
        };
        f.codex(ID_A, &[codex_meta("/w", json!("cli")), message], 0);
        let ctx = f.ctx();
        let page = sessions::transcript(&ctx, ID_A, None, 0, None)
            .unwrap()
            .page;
        let records: Vec<_> = page
            .records
            .iter()
            .filter(|r| r.name.as_deref() != Some("session_meta"))
            .collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, RecordKind::User);
        assert_eq!(
            records[0].text.as_deref(),
            Some(
                "Which workflow? → Issue branch, then PR to main (recommended)\nWhen? → After verification"
            )
        );
        let result = sessions::search(
            &ctx,
            &SearchOpts {
                query: "verification".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.sessions_matched, 1);
    }
}

#[test]
fn an_entry_without_an_answer_does_not_hide_other_question_answers() {
    let f = Fixture::new();
    let reply = format!(
        "<send_user_message_question_reply>{}</send_user_message_question_reply>",
        json!([
            {"question": "Which workflow?", "answer": "Issue branch"},
            {"question": "Unanswered?"},
            {"answer": "After verification"}
        ])
    );
    f.codex(
        ID_A,
        &[codex_meta("/w", json!("cli")), codex_user_item(&reply)],
        0,
    );
    let page = sessions::transcript(&f.ctx(), ID_A, None, 0, None)
        .unwrap()
        .page;
    let users: Vec<_> = page
        .records
        .iter()
        .filter(|r| r.kind == RecordKind::User)
        .collect();
    assert_eq!(users.len(), 1);
    assert_eq!(
        users[0].text.as_deref(),
        Some("Which workflow? → Issue branch\nAfter verification")
    );
}

#[test]
fn question_reply_copies_at_response_and_event_to_item_switches_are_one_user_record() {
    let reply = format!(
        "<send_user_message_question_reply>{}</send_user_message_question_reply>",
        json!([{"question": "When?", "answer": "After verification"}])
    );
    for first in [
        codex_response("user", &reply),
        codex_event("user_message", &reply),
    ] {
        let f = Fixture::new();
        f.codex(
            ID_A,
            &[
                codex_meta("/w", json!("cli")),
                first,
                codex_user_item(&reply),
            ],
            0,
        );
        let page = sessions::transcript(&f.ctx(), ID_A, None, 0, None)
            .unwrap()
            .page;
        let users: Vec<_> = page
            .records
            .iter()
            .filter(|r| r.kind == RecordKind::User)
            .collect();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].text.as_deref(), Some("When? → After verification"));
    }
}

#[test]
fn injected_wrappers_and_invalid_question_replies_stay_omitted() {
    let wrappers = [
        "<environment_context>hidden</environment_context>",
        "<INSTRUCTIONS>hidden</INSTRUCTIONS>",
        "<developer_instructions>hidden</developer_instructions>",
        "<recommended_plugins>hidden</recommended_plugins>",
        "<permissions instructions>hidden</permissions instructions>",
        "<unknown_context>hidden</unknown_context>",
        "# AGENTS.md instructions for /w\nhidden",
        "<send_user_message_question_reply>invalid</send_user_message_question_reply>",
        "<send_user_message_question_reply>[{\"question\":\"hidden\"}]</send_user_message_question_reply>",
        "<send_user_message_question_reply>[{\"answer\":\"hidden\"}]",
    ];
    for stream in 0..3 {
        let f = Fixture::new();
        let mut records = vec![codex_meta("/w", json!("cli"))];
        records.extend(wrappers.iter().map(|text| match stream {
            0 => codex_user_item(text),
            1 => codex_response("user", text),
            _ => codex_event("user_message", text),
        }));
        f.codex(ID_A, &records, 0);
        let page = sessions::transcript(&f.ctx(), ID_A, None, 0, None)
            .unwrap()
            .page;
        let records: Vec<_> = page
            .records
            .iter()
            .filter(|r| r.name.as_deref() != Some("session_meta"))
            .collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, RecordKind::Omitted);
        assert_eq!(records[0].name.as_deref(), Some("injected_context"));
        assert_eq!(records[0].count, Some(wrappers.len()));
    }
}

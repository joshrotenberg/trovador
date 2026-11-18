//! Records, content blocks and items of types the projection does not know (#84).
//!
//! Before, an unknown record type was `omitted` with no text and an unknown Claude content
//! block produced nothing, so text in a shape the projection had not learned could not be
//! found. Now each one is an `unrecognized` record with a bounded copy of its JSON that
//! search finds, while types known to be bookkeeping stay `omitted`.

mod common;

use common::*;
use serde_json::{Value, json};
use trovador::index::SearchOpts;
use trovador::sessions;
use trovador::transcript::{Record, RecordKind};
use trovador::{Ctx, Limits};

const MARK: &str = " ...[truncated]";

fn records(ctx: &Ctx, id: &str) -> Vec<Record> {
    sessions::transcript(ctx, id, None, 0, None)
        .unwrap()
        .page
        .records
}

fn of_kind(ctx: &Ctx, id: &str, kind: RecordKind) -> Vec<Record> {
    records(ctx, id)
        .into_iter()
        .filter(|r| r.kind == kind)
        .collect()
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

/// The kind of the first match of `query`, or None when nothing matched.
fn match_kind(ctx: &Ctx, query: &str) -> Option<String> {
    let r = search(ctx, query);
    r.hits
        .first()
        .and_then(|h| h.matches.first())
        .map(|m| m.kind.clone())
}

fn claude(f: &Fixture, extra: Vec<Value>) {
    let mut recs = vec![claude_user("/w", "start here")];
    recs.extend(extra);
    recs.push(claude_assistant("done"));
    f.claude("-w", ID_A, &recs, 100);
}

fn codex(f: &Fixture, extra: Vec<Value>) {
    let mut recs = vec![
        codex_meta("/w", json!("vscode")),
        codex_user_item("start here"),
    ];
    recs.extend(extra);
    recs.push(codex_agent_item("done"));
    f.codex(ID_A, &recs, 100);
}

// --- claude ----------------------------------------------------------------

#[test]
fn an_unknown_claude_record_type_is_unrecognized_and_searchable() {
    let f = Fixture::new();
    claude(
        &f,
        vec![
            json!({"type": "hologram-note", "uuid": "u-1", "timestamp": "2026-11-18T00:00:00Z",
                    "note": "zqhologramneedle in a new record"}),
        ],
    );
    let ctx = f.ctx();
    let u = of_kind(&ctx, ID_A, RecordKind::Unrecognized);
    assert_eq!(u.len(), 1, "{u:?}");
    assert_eq!(u[0].name.as_deref(), Some("hologram-note"));
    let text = u[0].text.as_deref().unwrap();
    assert!(text.contains("zqhologramneedle"), "{text}");
    // The envelope says where the record sits, not what it is, and would spend the budget.
    assert!(
        !text.contains("u-1") && !text.contains("2026-11-18"),
        "{text}"
    );
    assert_eq!(
        match_kind(&ctx, "zqhologramneedle").as_deref(),
        Some("unrecognized")
    );
}

#[test]
fn an_unknown_claude_content_block_is_unrecognized_beside_the_known_ones() {
    let f = Fixture::new();
    claude(
        &f,
        vec![
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "text", "text": "before the widget"},
                {"type": "server_widget", "payload": "zqwidgetneedle"},
            ]}}),
        ],
    );
    let ctx = f.ctx();
    let recs = records(&ctx, ID_A);
    let i = recs
        .iter()
        .position(|r| r.text.as_deref() == Some("before the widget"))
        .expect("the known block is still projected");
    let next = &recs[i + 1];
    assert_eq!(next.kind, RecordKind::Unrecognized);
    assert_eq!(next.name.as_deref(), Some("server_widget"));
    assert_eq!(
        match_kind(&ctx, "zqwidgetneedle").as_deref(),
        Some("unrecognized")
    );
}

#[test]
fn an_unknown_block_in_a_claude_user_message_is_unrecognized() {
    let f = Fixture::new();
    claude(
        &f,
        vec![
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "voice_note", "transcript": "zqvoiceneedle"},
            ]}}),
        ],
    );
    let u = of_kind(&f.ctx(), ID_A, RecordKind::Unrecognized);
    assert_eq!(u.len(), 1, "{u:?}");
    assert_eq!(u[0].name.as_deref(), Some("voice_note"));
}

#[test]
fn a_claude_image_block_is_omitted_and_its_data_is_nowhere() {
    let f = Fixture::new();
    let data = "QUJD".repeat(5_000);
    claude(
        &f,
        vec![
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}},
            ]}}),
        ],
    );
    let recs = records(&f.ctx(), ID_A);
    assert!(
        recs.iter()
            .any(|r| r.kind == RecordKind::Omitted && r.name.as_deref() == Some("image")),
        "{recs:?}"
    );
    assert!(recs.iter().all(|r| r.kind != RecordKind::Unrecognized));
    assert!(
        recs.iter()
            .all(|r| !r.text.as_deref().unwrap_or("").contains("QUJDQUJD"))
    );
}

#[test]
fn known_claude_bookkeeping_stays_omitted_without_text() {
    let f = Fixture::new();
    claude(
        &f,
        vec![
            json!({"type": "cost-state", "totalCostUSD": 1.5, "note": "zqcostneedle"}),
            json!({"type": "pr-link", "prNumber": 7, "prUrl": "https://example.invalid/pr/7"}),
            json!({"type": "attachment", "attachment": {"type": "total_tokens_reminder",
                   "text": "zqreminderneedle"}}),
        ],
    );
    let ctx = f.ctx();
    let recs = records(&ctx, ID_A);
    assert!(
        recs.iter().all(|r| r.kind != RecordKind::Unrecognized),
        "{recs:?}"
    );
    let omitted: Vec<_> = recs
        .iter()
        .filter(|r| r.kind == RecordKind::Omitted)
        .map(|r| (r.name.as_deref().unwrap(), r.text.is_none()))
        .collect();
    assert_eq!(
        omitted,
        [
            ("cost-state", true),
            ("pr-link", true),
            ("attachment", true)
        ]
    );
    assert_eq!(search(&ctx, "zqcostneedle").sessions_matched, 0);
    assert_eq!(search(&ctx, "zqreminderneedle").sessions_matched, 0);
}

#[test]
fn an_unknown_claude_attachment_subtype_is_unrecognized_without_its_rendered_copy() {
    let f = Fixture::new();
    claude(
        &f,
        vec![json!({"type": "attachment",
                    "attachment": {"type": "sticky_note", "body": "zqstickyneedle"},
                    "rendered": [{"content": "<system-reminder>zqrenderedneedle</system-reminder>"}]})],
    );
    let ctx = f.ctx();
    let u = of_kind(&ctx, ID_A, RecordKind::Unrecognized);
    assert_eq!(u.len(), 1, "{u:?}");
    assert_eq!(u[0].name.as_deref(), Some("attachment:sticky_note"));
    let text = u[0].text.as_deref().unwrap();
    assert!(text.contains("zqstickyneedle"), "{text}");
    assert!(!text.contains("zqrenderedneedle"), "{text}");
    assert_eq!(search(&ctx, "zqstickyneedle").sessions_matched, 1);
    assert_eq!(search(&ctx, "zqrenderedneedle").sessions_matched, 0);
}

// --- codex -----------------------------------------------------------------

#[test]
fn an_unknown_codex_item_is_unrecognized_and_searchable() {
    let f = Fixture::new();
    codex(
        &f,
        vec![codex_item(json!({"type": "DynamicToolCall", "id": "dt-1",
                               "tool": "lookup", "arguments": {"q": "zqdynamicneedle"}}))],
    );
    let ctx = f.ctx();
    let u = of_kind(&ctx, ID_A, RecordKind::Unrecognized);
    assert_eq!(u.len(), 1, "{u:?}");
    assert_eq!(u[0].name.as_deref(), Some("DynamicToolCall"));
    assert!(u[0].text.as_deref().unwrap().contains("zqdynamicneedle"));
    assert_eq!(
        match_kind(&ctx, "zqdynamicneedle").as_deref(),
        Some("unrecognized")
    );
}

#[test]
fn an_unknown_codex_payload_and_record_type_are_unrecognized() {
    let f = Fixture::new();
    codex(
        &f,
        vec![
            json!({"type": "response_item", "timestamp": "t", "ordinal": 3,
                   "payload": {"type": "local_shell_call", "action": {"command": ["zqshellneedle"]}}}),
            json!({"type": "event_msg", "payload": {"type": "exited_review_mode",
                   "review_output": "zqreviewneedle"}}),
            json!({"type": "hologram_record", "payload": {"note": "zqrecordneedle"}}),
        ],
    );
    let ctx = f.ctx();
    let names: Vec<_> = of_kind(&ctx, ID_A, RecordKind::Unrecognized)
        .into_iter()
        .map(|r| r.name.unwrap())
        .collect();
    assert_eq!(
        names,
        ["local_shell_call", "exited_review_mode", "hologram_record"]
    );
    for needle in ["zqshellneedle", "zqreviewneedle", "zqrecordneedle"] {
        assert_eq!(search(&ctx, needle).sessions_matched, 1, "{needle}");
    }
}

#[test]
fn known_codex_bookkeeping_stays_omitted_without_text() {
    let f = Fixture::new();
    codex(
        &f,
        vec![
            json!({"type": "token_usage_record", "payload": {"usage": {"input_tokens": 5}}}),
            json!({"type": "world_state", "payload": {"state": {"agents_md": "zqworldneedle"}}}),
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": null}}),
            json!({"type": "response_item", "payload": {"type": "compaction",
                   "encrypted_content": "gAAAA"}}),
        ],
    );
    let ctx = f.ctx();
    let recs = records(&ctx, ID_A);
    assert!(
        recs.iter().all(|r| r.kind != RecordKind::Unrecognized),
        "{recs:?}"
    );
    let omitted: Vec<_> = recs
        .iter()
        .filter(|r| r.kind == RecordKind::Omitted)
        .map(|r| r.name.clone().unwrap())
        .collect();
    assert_eq!(
        omitted,
        [
            "session_meta",
            "token_usage_record",
            "world_state",
            "token_count",
            "compaction"
        ]
    );
    assert_eq!(search(&ctx, "zqworldneedle").sessions_matched, 0);
}

// --- shape of the text -----------------------------------------------------

#[test]
fn binary_and_encrypted_values_are_replaced_in_unrecognized_text() {
    let f = Fixture::new();
    let blob = "QUJD".repeat(2_000);
    claude(
        &f,
        vec![json!({"type": "hologram-note", "label": "zqlabel",
                    "data": blob, "signature": "c2lnbmF0dXJl", "encrypted_content": "gAAAAB",
                    "preview": format!("data:image/png;base64,{blob}")})],
    );
    let u = of_kind(&f.ctx(), ID_A, RecordKind::Unrecognized);
    let text = u[0].text.as_deref().unwrap();
    assert!(text.contains("zqlabel"), "{text}");
    assert!(!text.contains("QUJDQUJD"), "{text}");
    assert!(
        !text.contains("c2lnbmF0dXJl") && !text.contains("gAAAAB"),
        "{text}"
    );
    assert_eq!(text.matches("[binary]").count(), 4, "{text}");
}

#[test]
fn unrecognized_text_is_cut_to_its_limit_and_says_so() {
    let f = Fixture::new();
    let long = "word ".repeat(20_000);
    claude(&f, vec![json!({"type": "hologram-note", "body": long})]);
    for n in [1_000, 50] {
        let ctx = f.ctx_with(Limits {
            unrecognized_chars: n,
            ..limits()
        });
        let u = of_kind(&ctx, ID_A, RecordKind::Unrecognized);
        let text = u[0].text.as_deref().unwrap();
        assert!(text.ends_with(MARK), "{text}");
        assert_eq!(text.chars().count(), n + MARK.chars().count());
    }
}

#[test]
fn a_cut_inside_multibyte_text_still_marks_the_cut() {
    // The writer stops on bytes and the cap counts characters; four-byte characters must not
    // let a cut text through unmarked.
    let f = Fixture::new();
    claude(
        &f,
        vec![json!({"type": "hologram-note", "body": "𝄞".repeat(5_000)})],
    );
    let ctx = f.ctx_with(Limits {
        unrecognized_chars: 100,
        ..limits()
    });
    let text = of_kind(&ctx, ID_A, RecordKind::Unrecognized)[0]
        .text
        .clone()
        .unwrap();
    assert!(text.ends_with(MARK), "{text}");
    assert_eq!(text.chars().count(), 100 + MARK.chars().count());
}

#[test]
fn consecutive_unrecognized_records_each_keep_their_own_text() {
    let f = Fixture::new();
    claude(
        &f,
        vec![
            json!({"type": "hologram-note", "body": "zqfirstnote"}),
            json!({"type": "hologram-note", "body": "zqsecondnote"}),
        ],
    );
    let ctx = f.ctx();
    let u = of_kind(&ctx, ID_A, RecordKind::Unrecognized);
    assert_eq!(u.len(), 2, "{u:?}");
    assert!(u.iter().all(|r| r.count.is_none()));
    assert_eq!(search(&ctx, "zqsecondnote").sessions_matched, 1);
}

#[test]
fn a_zero_limit_shows_unrecognized_records_as_omitted_and_keeps_them_out_of_search() {
    let f = Fixture::new();
    claude(
        &f,
        vec![
            json!({"type": "hologram-note", "body": "zqfirstnote"}),
            json!({"type": "hologram-note", "body": "zqsecondnote"}),
        ],
    );
    let ctx = f.ctx_with(Limits {
        unrecognized_chars: 0,
        ..limits()
    });
    let recs = records(&ctx, ID_A);
    assert!(recs.iter().all(|r| r.kind != RecordKind::Unrecognized));
    let run: Vec<_> = recs
        .iter()
        .filter(|r| r.kind == RecordKind::Omitted)
        .map(|r| (r.name.as_deref().unwrap(), r.count))
        .collect();
    assert_eq!(run, [("hologram-note", Some(2))]);
    assert_eq!(search(&ctx, "zqfirstnote").sessions_matched, 0);
    // The index is rebuilt for the default limit rather than serving the zero-limit rows.
    assert_eq!(search(&f.ctx(), "zqfirstnote").sessions_matched, 1);
}

#[test]
fn session_text_leaves_unrecognized_records_out_and_render_labels_them() {
    let f = Fixture::new();
    claude(
        &f,
        vec![json!({"type": "hologram-note", "body": "zqtextneedle"})],
    );
    let text = sessions::text(&f.ctx(), ID_A, None, 0).unwrap().text;
    assert!(text.contains("USER: start here"), "{text}");
    assert!(!text.contains("zqtextneedle"), "{text}");
    let rendered: Vec<_> = records(&f.ctx(), ID_A)
        .iter()
        .filter(|r| r.kind == RecordKind::Unrecognized)
        .map(trovador::transcript::render)
        .collect();
    assert_eq!(rendered.len(), 1);
    assert!(
        rendered[0].starts_with("UNRECOGNIZED (hologram-note): {"),
        "{}",
        rendered[0]
    );
}

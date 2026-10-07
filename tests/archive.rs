mod common;

use common::*;
use serde_json::json;
use trovador::Backend;
use trovador::archive::{self, LookupError, TitleSource};
use trovador::sessions::{self, ListOpts};
use trovador::transcript::RecordKind;

fn kinds(t: &sessions::TranscriptResult) -> Vec<(RecordKind, String)> {
    t.page
        .records
        .iter()
        .map(|r| {
            (
                r.kind,
                r.text.clone().or(r.name.clone()).unwrap_or_default(),
            )
        })
        .collect()
}

// --- metadata --------------------------------------------------------------

#[test]
fn a_codex_user_message_past_the_first_64kb_is_still_found() {
    // Codex writes a large developer preamble first; one real session put the first user
    // message at byte 119,106.
    let f = Fixture::new();
    let preamble = "x".repeat(100 * 1024);
    f.codex(
        ID_A,
        &[
            codex_meta("/work/a", json!("exec")),
            codex_response("developer", &preamble),
            codex_user_item("fix the flaky test"),
        ],
        0,
    );
    let e = archive::find_session(&f.paths, ID_A, None).unwrap();
    let meta = archive::read_meta(&e, &f.ctx().limits);
    assert_eq!(meta.cwd.as_deref(), Some("/work/a"));
    assert_eq!(meta.snippet.as_deref(), Some("fix the flaky test"));
}

#[test]
fn the_codex_snippet_skips_injected_agents_md_and_context_tags() {
    let f = Fixture::new();
    f.codex(
        ID_A,
        &[
            codex_meta("/work/a", json!("vscode")),
            codex_response("user", "<environment_context>cwd</environment_context>"),
            codex_item(json!({"type": "UserMessage", "content": [
                {"type": "text", "text": "# AGENTS.md instructions for /work/a\n<INSTRUCTIONS>..."},
                {"type": "text", "text": "what does this repo do"}
            ]})),
        ],
        0,
    );
    let e = archive::find_session(&f.paths, ID_A, None).unwrap();
    let meta = archive::read_meta(&e, &f.ctx().limits);
    assert_eq!(meta.snippet.as_deref(), Some("what does this repo do"));
}

#[test]
fn a_claude_snippet_skips_a_system_reminder_block_ahead_of_the_typed_text() {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[json!({"type": "user", "cwd": "/w", "message": {"role": "user", "content": [
            {"type": "text", "text": "<system-reminder>\nYou are in a worktree</system-reminder>"},
            {"type": "text", "text": "In the carolina repo, fix the build"}
        ]}})],
        0,
    );
    let e = archive::find_session(&f.paths, ID_A, None).unwrap();
    let meta = archive::read_meta(&e, &f.ctx().limits);
    assert_eq!(
        meta.snippet.as_deref(),
        Some("In the carolina repo, fix the build")
    );
}

#[test]
fn a_relocated_claude_session_resumes_from_where_it_lives_now() {
    let f = Fixture::new();
    f.claude(
        "-new-home",
        ID_A,
        &[
            claude_user("/old/scratch", "start here"),
            json!({"type": "relocated", "relocatedCwd": "/new/home"}),
            json!({"type": "pr-link", "prUrl": "https://github.com/o/r/pull/2"}),
            json!({"type": "pr-link", "prUrl": "https://github.com/o/r/pull/2"}),
        ],
        0,
    );
    let d = sessions::show(&f.ctx(), ID_A, None).unwrap();
    assert_eq!(d.row.meta.cwd.as_deref(), Some("/old/scratch"));
    assert_eq!(d.row.project.as_deref(), Some("/new/home"));
    assert_eq!(
        d.row.open_with,
        format!("cd '/new/home' && claude --resume {ID_A}")
    );
    assert_eq!(
        d.row.meta.pull_requests,
        vec!["https://github.com/o/r/pull/2"]
    );
}

#[test]
fn a_title_rewritten_past_the_head_read_comes_from_the_tail() {
    let f = Fixture::new();
    let filler: Vec<_> = (0..200)
        .map(|i| claude_assistant(&format!("{i} {}", "y".repeat(1000))))
        .collect();
    let mut records = vec![
        claude_user("/w", "hello"),
        json!({"type": "ai-title", "aiTitle": "early"}),
    ];
    records.extend(filler);
    records.push(json!({"type": "custom-title", "customTitle": "the real name"}));
    f.claude("-w", ID_A, &records, 0);
    let e = archive::find_session(&f.paths, ID_A, None).unwrap();
    assert!(
        e.size_bytes > 64 * 1024,
        "the title must sit past the first head read"
    );
    let meta = archive::read_meta(&e, &f.ctx().limits);
    assert_eq!(meta.title.as_deref(), Some("the real name"));
    assert_eq!(meta.title_source, Some(TitleSource::Custom));
}

#[test]
fn a_guardian_codex_session_is_marked_as_a_subagent_with_its_parent() {
    let f = Fixture::new();
    f.codex(
        ID_A,
        &[json!({"type": "session_meta", "payload": {
            "cwd": "/w", "source": {"subagent": {"other": "guardian"}},
            "parent_thread_id": ID_B}})],
        0,
    );
    let e = archive::find_session(&f.paths, ID_A, None).unwrap();
    let meta = archive::read_meta(&e, &f.ctx().limits);
    assert!(meta.subagent);
    assert_eq!(meta.origin.as_deref(), Some("guardian"));
    assert_eq!(meta.parent_session_id.as_deref(), Some(ID_B));
}

// --- lookup ----------------------------------------------------------------

#[test]
fn an_exact_id_wins_over_a_longer_id_it_prefixes() {
    let f = Fixture::new();
    f.claude("-w", ID_B, &[claude_user("/w", "b")], 0);
    f.claude("-w", ID_D, &[claude_user("/w", "d")], 0);
    assert!(matches!(
        archive::find_session(&f.paths, "ab", None),
        Ok(e) if e.session_id == ID_D
    ));
    match archive::find_session(&f.paths, "", None) {
        Err(LookupError::NotFound { .. }) => {}
        other => panic!("an empty id must not match everything: {other:?}"),
    }
}

#[test]
fn an_ambiguous_prefix_names_every_candidate() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "a")], 0);
    f.codex(ID_D, &[codex_meta("/w", json!("exec"))], 0);
    match archive::find_session(&f.paths, "a", None) {
        Err(LookupError::Ambiguous { candidates, .. }) => {
            assert_eq!(candidates.len(), 2);
            assert!(candidates.iter().any(|c| c == &format!("codex/{ID_D}")));
        }
        other => panic!("expected ambiguity, got {other:?}"),
    }
    // Narrowing by backend resolves it.
    assert!(archive::find_session(&f.paths, "a", Some(Backend::Claude)).is_ok());
}

// --- listing ---------------------------------------------------------------

#[test]
fn list_is_newest_first_and_hides_subagents_unless_asked() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "oldest")], 300);
    f.codex(
        ID_B,
        &[json!({"type": "session_meta", "payload": {"cwd": "/w",
            "source": {"subagent": {"other": "guardian"}}}})],
        10,
    );
    f.codex(ID_C, &[codex_meta("/w", json!("cli"))], 100);

    let ids = |r: &sessions::ListResult| -> Vec<String> {
        r.sessions.iter().map(|s| s.session_id.clone()).collect()
    };
    let default = sessions::list(&f.ctx(), &ListOpts::default());
    assert_eq!(ids(&default), vec![ID_C, ID_A]);
    assert_eq!(default.total_sessions, 3);

    let all = sessions::list(
        &f.ctx(),
        &ListOpts {
            include_subagents: true,
            ..Default::default()
        },
    );
    assert_eq!(ids(&all), vec![ID_B, ID_C, ID_A]);

    let cli_only = sessions::list(
        &f.ctx(),
        &ListOpts {
            origin: Some("cli".into()),
            ..Default::default()
        },
    );
    assert_eq!(ids(&cli_only), vec![ID_C, ID_A]);
}

#[test]
fn a_project_filter_matches_where_the_session_lives_now() {
    let f = Fixture::new();
    f.claude(
        "-moved",
        ID_A,
        &[
            claude_user("/scratch", "x"),
            json!({"type": "relocated", "relocatedCwd": "/code/trovador"}),
        ],
        0,
    );
    f.claude("-other", ID_B, &[claude_user("/code/other", "y")], 0);
    let r = sessions::list(
        &f.ctx(),
        &ListOpts {
            project: Some("trovador".into()),
            ..Default::default()
        },
    );
    assert_eq!(r.returned, 1);
    assert_eq!(r.sessions[0].session_id, ID_A);
}

#[test]
fn a_session_over_the_resume_ceiling_says_why_it_cannot_resume() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", &"z".repeat(950_000))], 0);
    let d = sessions::show(&f.ctx(), ID_A, None).unwrap();
    assert!(!d.row.resumable);
    assert!(
        d.resume_blocked_by
            .unwrap()
            .contains("summarize or consult")
    );
}

// --- transcripts -----------------------------------------------------------

#[test]
fn a_current_codex_turn_is_projected_once_not_once_per_stream() {
    // Codex writes each message as a model-facing response_item and a UI-facing
    // item_completed. The first user response_item comes before the first item event.
    let f = Fixture::new();
    f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("cli")),
            codex_response("developer", "you are codex"),
            codex_response("user", "list the files"),
            codex_user_item("list the files"),
            codex_response("assistant", "here they are"),
            codex_agent_item("here they are"),
            json!({"type": "response_item", "payload": {"type": "custom_tool_call",
                   "name": "exec", "input": "ls"}}),
            codex_item(
                json!({"type": "CommandExecution", "parsed_cmd": [{"cmd": "ls"}],
                              "stdout": "a.rs", "stderr": ""}),
            ),
        ],
        0,
    );
    let t = sessions::transcript(&f.ctx(), ID_A, None, 0, None).unwrap();
    let conversational: Vec<_> = kinds(&t)
        .into_iter()
        .filter(|(k, _)| *k != RecordKind::Omitted)
        .collect();
    assert_eq!(
        conversational,
        vec![
            (RecordKind::User, "list the files".into()),
            (RecordKind::Assistant, "here they are".into()),
            (RecordKind::ToolCall, "ls".into()),
            (RecordKind::ToolResult, "a.rs".into()),
        ]
    );
}

#[test]
fn an_older_codex_session_uses_its_event_stream() {
    let f = Fixture::new();
    f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("cli")),
            codex_response("user", "hi"),
            codex_event("user_message", "hi"),
            codex_response("assistant", "hello"),
            codex_event("agent_message", "hello"),
        ],
        0,
    );
    let t = sessions::transcript(&f.ctx(), ID_A, None, 0, None).unwrap();
    let texts: Vec<_> = kinds(&t)
        .into_iter()
        .filter(|(k, _)| *k != RecordKind::Omitted)
        .collect();
    assert_eq!(
        texts,
        vec![
            (RecordKind::User, "hi".into()),
            (RecordKind::Assistant, "hello".into())
        ]
    );
}

#[test]
fn the_oldest_codex_records_without_an_envelope_are_still_read() {
    let f = Fixture::new();
    f.codex(
        ID_A,
        &[
            json!({"id": "x", "timestamp": "2025-09-01T00:00:00Z", "instructions": ""}),
            json!({"type": "message", "role": "user",
                   "content": [{"type": "input_text", "text": "old times"}]}),
        ],
        0,
    );
    let t = sessions::transcript(&f.ctx(), ID_A, None, 0, None).unwrap();
    assert_eq!(kinds(&t), vec![(RecordKind::User, "old times".into())]);
    let d = sessions::show(&f.ctx(), ID_A, None).unwrap();
    assert_eq!(d.row.meta.snippet.as_deref(), Some("old times"));
}

#[test]
fn consecutive_omitted_records_collapse_into_one_with_a_count() {
    let f = Fixture::new();
    let mut records = vec![claude_user("/w", "go")];
    records.extend((0..5).map(|_| claude_attachment("total_tokens_reminder")));
    records.push(claude_assistant("done"));
    records.push(json!({"type": "queue-operation"}));
    f.claude("-w", ID_A, &records, 0);
    let t = sessions::transcript(&f.ctx(), ID_A, None, 0, None).unwrap();
    let recs = &t.page.records;
    assert_eq!(recs.len(), 4);
    assert_eq!(recs[1].kind, RecordKind::Omitted);
    assert_eq!(recs[1].name.as_deref(), Some("attachment"));
    assert_eq!(recs[1].count, Some(5));
    assert_eq!(recs[3].name.as_deref(), Some("queue-operation"));
    assert_eq!(recs[3].count, Some(1));
    assert!(recs.iter().enumerate().all(|(i, r)| r.index == i));
}

#[test]
fn paging_reports_next_offset_until_the_end_and_a_total_only_at_the_end() {
    let f = Fixture::new();
    let mut records = vec![claude_user("/w", "q0")];
    records.extend((1..10).map(|i| claude_assistant(&format!("a{i}"))));
    f.claude("-w", ID_A, &records, 0);
    let ctx = f.ctx();

    let p1 = sessions::transcript(&ctx, ID_A, None, 0, Some(4))
        .unwrap()
        .page;
    assert_eq!(p1.records.len(), 4);
    assert!(p1.has_more);
    assert_eq!(p1.next_offset, Some(4));
    assert_eq!(p1.total, None);

    let p3 = sessions::transcript(&ctx, ID_A, None, 8, Some(4))
        .unwrap()
        .page;
    assert_eq!(p3.records.len(), 2);
    assert!(!p3.has_more);
    assert_eq!(p3.total, Some(10));
    assert_eq!(p3.records[0].index, 8);
}

#[test]
fn a_page_stops_at_the_response_size_cap_and_says_so() {
    let f = Fixture::new();
    let records: Vec<_> = (0..10)
        .map(|i| claude_assistant(&format!("{i}{}", "w".repeat(3000))))
        .collect();
    f.claude("-w", ID_A, &records, 0);
    let limits = trovador::Limits {
        response_max_chars: 10_000,
        ..Default::default()
    };
    let p = sessions::transcript(&f.ctx_with(limits), ID_A, None, 0, None)
        .unwrap()
        .page;
    assert_eq!(p.records.len(), 3);
    assert!(p.has_more);
    assert_eq!(p.next_offset, Some(3));
    assert!(p.truncated_by.unwrap().contains("response_max_chars"));
}

#[test]
fn a_torn_final_line_in_a_live_session_is_skipped_not_fatal() {
    let f = Fixture::new();
    let path = f.claude("-w", ID_A, &[claude_user("/w", "still going")], 0);
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    write!(file, "{{\"type\":\"assistant\",\"mess").unwrap();
    let t = sessions::transcript(&f.ctx(), ID_A, None, 0, None).unwrap();
    assert_eq!(kinds(&t), vec![(RecordKind::User, "still going".into())]);
}

#[test]
fn claude_subagent_transcripts_are_not_listed_as_sessions() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", "parent")], 0);
    f.claude(
        &format!("-w/{ID_A}/subagents"),
        "agent-1234",
        &[claude_user("/w", "child")],
        0,
    );
    let r = sessions::list(
        &f.ctx(),
        &ListOpts {
            include_subagents: true,
            ..Default::default()
        },
    );
    assert_eq!(r.total_sessions, 1);
}

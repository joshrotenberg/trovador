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
    let default = sessions::list(&f.ctx(), &ListOpts::default()).unwrap();
    assert_eq!(ids(&default), vec![ID_C, ID_A]);
    assert_eq!(default.total_sessions, 3);

    let all = sessions::list(
        &f.ctx(),
        &ListOpts {
            include_subagents: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(ids(&all), vec![ID_B, ID_C, ID_A]);

    let cli_only = sessions::list(
        &f.ctx(),
        &ListOpts {
            origin: Some("cli".into()),
            ..Default::default()
        },
    )
    .unwrap();
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
    )
    .unwrap();
    assert_eq!(r.returned, 1);
    assert_eq!(r.sessions[0].session_id, ID_A);
}

#[test]
fn a_session_over_the_resume_ceiling_says_why_it_cannot_resume() {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", &"z".repeat(950_000))], 0);
    let d = sessions::show(&f.ctx(), ID_A, None).unwrap();
    assert!(!d.row.resumable);
    assert!(d.resume_blocked_by.unwrap().contains("ask with about"));
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
fn a_first_turn_larger_than_the_pre_scan_window_is_projected_once() {
    // Seen on a real machine: a 477 KB pasted prompt written to the response stream carried
    // the pre-scan past its 512 KB window, so the item stream was only found at the copy.
    let f = Fixture::new();
    let huge = "p".repeat(600_000);
    f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("vscode")),
            codex_response("user", &huge),
            codex_user_item(&huge),
            codex_response("assistant", "ok"),
            codex_agent_item("ok"),
            codex_user_item("next question"),
        ],
        0,
    );
    let t = sessions::transcript(&f.ctx(), ID_A, None, 0, None).unwrap();
    let turns: Vec<_> = kinds(&t)
        .into_iter()
        .filter(|(k, _)| *k != RecordKind::Omitted)
        .map(|(k, text)| (k, text.len().min(20)))
        .collect();
    assert_eq!(
        turns,
        vec![
            (RecordKind::User, 20),
            (RecordKind::Assistant, 2),
            (RecordKind::User, 13)
        ]
    );
}

#[test]
fn a_prompt_a_person_really_sent_twice_is_projected_twice() {
    let f = Fixture::new();
    f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("cli")),
            codex_user_item("run the tests again"),
            codex_agent_item("done"),
            codex_user_item("run the tests again"),
            codex_user_item("run the tests again"),
        ],
        0,
    );
    let t = sessions::transcript(&f.ctx(), ID_A, None, 0, None).unwrap();
    let users = kinds(&t)
        .into_iter()
        .filter(|(k, _)| *k == RecordKind::User)
        .count();
    assert_eq!(users, 3);
}

#[test]
fn codex_reasoning_search_and_tool_items_are_projected_from_their_real_shapes() {
    let f = Fixture::new();
    f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("vscode")),
            codex_user_item("look into the MCP spec"),
            // Most reasoning items carry no summary; those stay omitted.
            codex_item(json!({"type": "Reasoning", "id": "rs_1", "summary_text": [], "raw_content": []})),
            codex_item(json!({"type": "Reasoning", "id": "rs_2",
                "summary_text": ["**Planning repository exploration**"], "raw_content": []})),
            // The response-item twin of the reasoning above, which the item stream supersedes.
            json!({"type": "response_item", "payload": {"type": "reasoning",
                "summary": [{"type": "summary_text", "text": "**Planning repository exploration**"}],
                "encrypted_content": "gAAAA"}}),
            codex_item(json!({"type": "WebSearch", "id": "ws_1", "query": "mcp changelog",
                "action": {"type": "search", "queries": ["mcp changelog", "mcp tasks"]},
                "results": [{"type": "text_result", "title": "Changelog", "url": "https://example.com/c",
                             "snippet": "s", "domain": "example.com", "ref_id": "turn0search0"}]})),
            codex_item(json!({"type": "FunctionCallOutput", "id": "fo_1", "name": "read_file",
                "namespace": "workspace", "output": "fn main() {}"})),
            codex_item(json!({"type": "ImageView", "id": "iv_1", "path": "/tmp/shot.png"})),
            codex_item(json!({"type": "CollabAgentToolCall", "id": "c_1", "tool": "wait", "status": "completed",
                "sender_thread_id": ID_B, "receiver_thread_ids": [], "receiver_agents": [], "agents_states": {}})),
            json!({"type": "response_item", "payload": {"type": "tool_search_call", "call_id": "c",
                "status": "completed", "execution": "client", "arguments": {"query": "github issues"}}}),
            json!({"type": "response_item", "payload": {"type": "tool_search_output", "call_id": "c",
                "status": "completed", "execution": "client",
                "tools": [{"type": "namespace", "name": "mcp__github", "tools": []}]}}),
            json!({"type": "compacted", "payload": {"message": "", "replacement_history": [],
                "window_number": 1}}),
        ],
        0,
    );
    let t = sessions::transcript(&f.ctx(), ID_A, None, 0, None).unwrap();
    let shown: Vec<(RecordKind, Option<String>, String)> = t
        .page
        .records
        .iter()
        .filter(|r| r.kind != RecordKind::Omitted)
        .map(|r| (r.kind, r.name.clone(), r.text.clone().unwrap_or_default()))
        .collect();
    let named = |n: &str| Some(n.to_string());
    assert_eq!(
        shown,
        vec![
            (RecordKind::User, None, "look into the MCP spec".into()),
            (
                RecordKind::Reasoning,
                None,
                "**Planning repository exploration**".into()
            ),
            (
                RecordKind::ToolCall,
                named("web_search"),
                "mcp changelog\nmcp tasks".into()
            ),
            (
                RecordKind::ToolResult,
                named("web_search"),
                "Changelog https://example.com/c".into()
            ),
            (
                RecordKind::ToolResult,
                named("workspace/read_file"),
                "fn main() {}".into()
            ),
            (
                RecordKind::ToolCall,
                named("view_image"),
                "/tmp/shot.png".into()
            ),
            (
                RecordKind::ToolCall,
                named("tool_search"),
                "github issues".into()
            ),
            (
                RecordKind::ToolResult,
                named("tool_search"),
                "mcp__github".into()
            ),
        ]
    );
    let omitted: Vec<_> = t
        .page
        .records
        .iter()
        .filter(|r| r.kind == RecordKind::Omitted)
        .filter_map(|r| r.name.clone())
        .collect();
    for cat in ["Reasoning", "reasoning", "CollabAgentToolCall", "compacted"] {
        assert!(
            omitted.iter().any(|c| c == cat),
            "{cat} should be counted as omitted: {omitted:?}"
        );
    }
}

#[test]
fn an_older_codex_file_projects_reasoning_and_search_from_response_items() {
    let f = Fixture::new();
    f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("cli")),
            codex_response("user", "find it"),
            json!({"type": "response_item", "payload": {"type": "reasoning",
                "summary": [{"type": "summary_text", "text": "Searching the docs"}]}}),
            json!({"type": "response_item", "payload": {"type": "web_search_call", "status": "completed",
                "action": {"type": "search", "query": "codex mcp config"}}}),
        ],
        0,
    );
    let t = sessions::transcript(&f.ctx(), ID_A, None, 0, None).unwrap();
    assert_eq!(
        kinds(&t)
            .into_iter()
            .filter(|(k, _)| *k != RecordKind::Omitted)
            .collect::<Vec<_>>(),
        vec![
            (RecordKind::User, "find it".into()),
            (RecordKind::Reasoning, "Searching the docs".into()),
            (RecordKind::ToolCall, "codex mcp config".into()),
        ]
    );
}

#[test]
fn claude_thinking_with_text_is_reasoning_and_signature_only_thinking_is_dropped() {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "why is it slow"),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "", "signature": "abc"},
                {"type": "thinking", "thinking": "The index is rebuilt on every call.", "signature": "def"},
                {"type": "text", "text": "It rebuilds the index each time."}
            ]}}),
        ],
        0,
    );
    let t = sessions::transcript(&f.ctx(), ID_A, None, 0, None).unwrap();
    assert_eq!(
        kinds(&t),
        vec![
            (RecordKind::User, "why is it slow".into()),
            (
                RecordKind::Reasoning,
                "The index is rebuilt on every call.".into()
            ),
            (
                RecordKind::Assistant,
                "It rebuilds the index each time.".into()
            ),
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
        ..limits()
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
fn an_archived_codex_thread_is_still_listed_and_marked() {
    let f = Fixture::new();
    f.codex(ID_A, &[codex_meta("/w", json!("cli"))], 10);
    f.codex_archived(
        ID_B,
        &[codex_meta("/w", json!("cli")), codex_user_item("old work")],
        20,
    );
    let r = sessions::list(&f.ctx(), &ListOpts::default()).unwrap();
    assert_eq!(r.total_sessions, 2);
    let archived: Vec<_> = r.sessions.iter().filter(|s| s.archived).collect();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].session_id, ID_B);
    assert_eq!(archived[0].meta.snippet.as_deref(), Some("old work"));
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
    )
    .unwrap();
    assert_eq!(r.total_sessions, 1);
}

mod common;

use common::*;
use serde_json::{Value, json};
use trovador::{
    sessions,
    transcript::{Record, RecordKind},
};

fn response(blocks: &[&str]) -> Value {
    json!({"type":"response_item", "payload":{"type":"message", "role":"user",
        "content":blocks.iter().map(|text| json!({"type":"input_text", "text":text})).collect::<Vec<_>>()}})
}

fn records(rows: Vec<Value>) -> Vec<Record> {
    let f = Fixture::new();
    let mut all = vec![codex_meta("/w", json!({"subagent":{"other":"guardian"}}))];
    all.extend(rows);
    f.codex(ID_A, &all, 0);
    sessions::transcript(&f.ctx(), ID_A, None, 0, Some(200))
        .unwrap()
        .page
        .records
}

fn users(rows: Vec<Value>) -> Vec<String> {
    records(rows)
        .into_iter()
        .filter(|r| r.kind == RecordKind::User)
        .map(|r| r.text.unwrap())
        .collect()
}

#[test]
fn items_mode_keeps_response_only_user_turns() {
    assert_eq!(
        users(vec![codex_agent_item("ready"), response(&["review this"])]),
        ["review this"]
    );
}

#[test]
fn split_response_and_concatenated_item_are_one_turn_at_switch() {
    assert_eq!(
        users(vec![
            response(&["review\n", "action\n", "please"]),
            codex_user_item("review\naction\nplease")
        ])
        .len(),
        1
    );
}

#[test]
fn every_paired_turn_is_emitted_once_in_items_and_events() {
    for items in [true, false] {
        let mut rows = Vec::new();
        for text in ["first", "second", "third"] {
            rows.push(response(&[&text[..1], &text[1..]]));
            rows.push(if items {
                codex_user_item(text)
            } else {
                codex_event("user_message", text)
            });
            rows.push(if items {
                codex_agent_item("ok")
            } else {
                codex_event("agent_message", "ok")
            });
        }
        assert_eq!(users(rows), ["first", "second", "third"]);
    }
}

#[test]
fn injected_blocks_among_response_text_do_not_leak_through_item_copy() {
    let blocks = [
        "review\n",
        "<environment_context>secret</environment_context>\n",
        "# AGENTS.md instructions\nsecret\n",
        "please",
    ];
    let result = records(vec![
        response(&["<instructions>secret</instructions>"]),
        response(&blocks),
        codex_user_item(&blocks.concat()),
    ]);
    assert!(
        result
            .iter()
            .any(|r| r.name.as_deref() == Some("injected_context"))
    );
    let turns: Vec<_> = result
        .iter()
        .filter(|r| r.kind == RecordKind::User)
        .collect();
    assert_eq!(turns.len(), 1);
    assert!(!turns[0].text.as_ref().unwrap().contains("secret"));
}

#[test]
fn guardian_unknown_user_content_is_omitted() {
    let result = records(vec![
        codex_agent_item("ready"),
        json!({"type":"response_item", "payload":{"type":"message", "role":"user", "content":[{"type":"future_content", "data":"unknown"}]}}),
    ]);
    assert!(
        result
            .iter()
            .any(|r| r.kind == RecordKind::Omitted && r.name.as_deref() == Some("message"))
    );
}

#[test]
fn separate_identical_user_turns_do_not_collapse() {
    assert_eq!(
        users(vec![
            response(&["y", "es"]),
            codex_user_item("yes"),
            response(&["y", "es"]),
            codex_user_item("yes"),
            codex_user_item("yes"),
            codex_user_item("yes")
        ]),
        ["yes", "yes", "yes", "yes"]
    );
    assert_eq!(
        users(vec![
            codex_agent_item("ready"),
            response(&["continue"]),
            response(&["continue"])
        ]),
        ["continue", "continue"]
    );
}

#[test]
fn identical_users_in_different_tasks_remain_separate() {
    assert_eq!(
        users(vec![
            codex_agent_item("ready"),
            response(&["yes"]),
            codex_event("task_complete", ""),
            codex_event("task_started", ""),
            codex_user_item("yes")
        ]),
        ["yes", "yes"]
    );
}

#[test]
fn response_copy_eligibility_survives_index_resume_and_tracks_each_stream() {
    use std::io::Write;
    use trovador::{
        archive::{ArchiveEntry, Backend},
        transcript::Projection,
    };
    let f = Fixture::new();
    let path = f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("cli")),
            response(&["review\n", "please"]),
        ],
        0,
    );
    let mut entry = ArchiveEntry {
        backend: Backend::Codex,
        session_id: ID_A.into(),
        file: path.clone(),
        size_bytes: std::fs::metadata(&path).unwrap().len(),
        mtime: std::time::SystemTime::now(),
        archived: false,
    };
    let mut first = Projection::for_index(&entry, &limits(), None).unwrap();
    let mut all: Vec<_> = first.by_ref().collect();
    let state = first.state().clone();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "{}", codex_event("user_message", "review\nplease")).unwrap();
    entry.size_bytes = file.metadata().unwrap().len();
    let mut middle = Projection::for_index(&entry, &limits(), Some(state)).unwrap();
    all.extend(middle.by_ref());
    let state = middle.state().clone();
    writeln!(file, "{}", codex_user_item("review\nplease")).unwrap();
    writeln!(file, "{}", codex_user_item("review\nplease")).unwrap();
    entry.size_bytes = file.metadata().unwrap().len();
    all.extend(Projection::for_index(&entry, &limits(), Some(state)).unwrap());
    let full: Vec<_> = Projection::for_index(&entry, &limits(), None)
        .unwrap()
        .collect();
    assert_eq!(all, full);
    assert_eq!(all.iter().filter(|r| r.kind == RecordKind::User).count(), 2);
    assert_eq!(
        all.iter()
            .filter(|r| r.kind == RecordKind::User)
            .map(|r| r.text.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["review\nplease", "review\nplease"]
    );
}

#[test]
fn filtered_copy_hash_survives_reopening_the_index() {
    use std::io::Write;
    use trovador::{
        Backend,
        index::{Index, RefreshOpts},
    };
    let f = Fixture::new();
    let ctx = f.ctx();
    let path = f.codex(
        ID_A,
        &[
            codex_meta("/w", json!("cli")),
            response(&["review\n", "please"]),
        ],
        0,
    );
    {
        let mut index = Index::open(&ctx.paths, &ctx.limits).unwrap();
        index
            .refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())
            .unwrap();
    }
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap(),
        "{}",
        codex_event("user_message", "review\nplease")
    )
    .unwrap();
    {
        let mut index = Index::open(&ctx.paths, &ctx.limits).unwrap();
        index
            .refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())
            .unwrap();
    }
    let item = codex_item(json!({"type":"UserMessage", "content":[
        {"type":"text", "text":"# AGENTS.md instructions\nsecret"},
        {"type":"text", "text":"review\nplease"}
    ]}));
    writeln!(
        std::fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{item}"
    )
    .unwrap();
    let mut index = Index::open(&ctx.paths, &ctx.limits).unwrap();
    let report = index
        .refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())
        .unwrap();
    assert_eq!((report.appended, report.reindexed), (1, 0));
    let indexed = index.session_records(Backend::Codex, ID_A).unwrap();
    assert_eq!(
        indexed.iter().filter(|(_, kind, _)| kind == "user").count(),
        1
    );
}

#[test]
fn different_user_text_after_the_display_cap_is_not_a_copy() {
    let prefix = "x".repeat(5000);
    assert_eq!(
        users(vec![
            response(&[&format!("{prefix}a")]),
            codex_user_item(&format!("{prefix}b"))
        ])
        .len(),
        2
    );
}

#[test]
fn all_three_stream_orders_project_each_turn_once() {
    for order in ["REI", "RIE", "ERI", "EIR", "IRE", "IER"] {
        let mut rows = Vec::new();
        for _ in 0..2 {
            rows.push(codex_event("task_started", ""));
            for stream in order.chars() {
                rows.push(match stream {
                    'R' => response(&["review\n", "please"]),
                    'E' => codex_event("user_message", "review\nplease"),
                    'I' => codex_user_item("review\nplease"),
                    _ => unreachable!(),
                });
            }
            rows.push(codex_event("task_complete", ""));
        }
        assert_eq!(users(rows), ["review\nplease", "review\nplease"], "{order}");
    }
}

#[test]
fn non_user_response_roles_stay_silent_outside_responses_mode() {
    for signal in [
        codex_agent_item("ready"),
        codex_event("agent_message", "ready"),
    ] {
        let result = records(vec![
            signal,
            codex_response("developer", "instructions"),
            codex_response("system", "instructions"),
        ]);
        assert!(
            !result
                .iter()
                .any(|r| matches!(r.name.as_deref(), Some("developer" | "system")))
        );
    }
}

#[test]
fn question_reply_event_and_decoded_item_match_the_filtered_hash() {
    let reply = format!(
        "<send_user_message_question_reply>{}</send_user_message_question_reply>",
        json!([{"question":"Proceed?", "answer":"yes"}])
    );
    assert_eq!(
        users(vec![
            codex_event("user_message", &reply),
            codex_user_item("Proceed? → yes")
        ]),
        ["Proceed? → yes"]
    );
}

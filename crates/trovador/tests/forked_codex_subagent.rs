mod common;

use common::*;
use serde_json::{Value, json};
use trovador::archive;
use trovador::index::{Index, RefreshOpts};
use trovador::sessions;
use trovador::transcript::RecordKind;

fn fork_records() -> Vec<Value> {
    let mut child = codex_meta("/child", json!({"subagent": {"thread_spawn": {}}}));
    child["payload"]["forked_from_id"] = json!(ID_A);
    child["payload"]["subagent_history_start_ordinal"] = json!(5);
    let mut records = vec![
        child,
        codex_meta("/parent", json!("cli")),
        codex_user_item("parent-only question"),
        codex_agent_item("parent-only answer"),
        json!({"type":"response_item", "payload":{"type":"agent_message",
            "author":"/ancestor", "recipient":"/root", "content":[{"type":"input_text", "text":"parent-only delegation"}]}}),
        json!({"type":"response_item", "payload":{"type":"agent_message",
            "author":"/root", "recipient":"/root/child", "content":[{"type":"input_text", "text":"child-only task"}]}}),
        codex_response("assistant", "child-only answer"),
    ];
    for (ordinal, record) in records.iter_mut().enumerate() {
        record["ordinal"] = json!(ordinal);
    }
    records
}

#[test]
fn forked_child_snippet_comes_from_its_own_first_agent_message() {
    let f = Fixture::new();
    f.codex(ID_B, &fork_records(), 0);
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    let meta = archive::read_meta(&e, &f.ctx().limits);
    assert_eq!(meta.snippet.as_deref(), Some("child-only task"));
    assert_eq!(meta.cwd.as_deref(), Some("/child"));
}

#[test]
fn copied_parent_turns_are_omitted_without_changing_the_child_stream() {
    let f = Fixture::new();
    f.codex(ID_B, &fork_records(), 0);
    let page = sessions::transcript(&f.ctx(), ID_B, None, 0, Some(200))
        .unwrap()
        .page;
    let inherited: Vec<_> = page
        .records
        .iter()
        .filter(|r| r.name.as_deref() == Some("forked_history"))
        .collect();
    assert_eq!(inherited.len(), 1);
    assert_eq!(inherited[0].kind, RecordKind::Omitted);
    assert_eq!(inherited[0].count, Some(4));
    assert!(
        page.records
            .iter()
            .all(|r| !r.text.as_deref().unwrap_or("").contains("parent-only"))
    );
    assert!(
        page.records
            .iter()
            .any(|r| r.kind == RecordKind::System && r.text.as_deref() == Some("child-only task"))
    );
    assert!(
        page.records
            .iter()
            .any(|r| r.kind == RecordKind::Assistant
                && r.text.as_deref() == Some("child-only answer"))
    );
}

#[test]
fn incremental_indexing_inside_a_fork_prefix_never_indexes_parent_text() {
    let f = Fixture::new();
    let records = fork_records();
    let path = f.codex(ID_B, &records[..4], 0);
    let ctx = f.ctx();
    let mut idx = Index::open(&ctx.paths, &ctx.limits).unwrap();
    let opts = RefreshOpts::default();
    idx.refresh(&ctx.paths, &ctx.limits, &opts).unwrap();
    assert!(
        idx.session_records(trovador::Backend::Codex, ID_B)
            .unwrap()
            .is_empty()
    );
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    for record in &records[4..] {
        writeln!(file, "{record}").unwrap();
    }
    idx.refresh(&ctx.paths, &ctx.limits, &opts).unwrap();
    let indexed = idx.session_records(trovador::Backend::Codex, ID_B).unwrap();
    assert_eq!(indexed.len(), 2);
    assert_eq!(indexed[0].2, "child-only task");
    assert_eq!(indexed[1].2, "child-only answer");
}

#[test]
fn a_child_with_only_inherited_history_has_no_snippet_or_turns() {
    let f = Fixture::new();
    f.codex(ID_B, &fork_records()[..5], 0);
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    assert!(archive::read_meta(&e, &f.ctx().limits).snippet.is_none());
    let page = sessions::transcript(&f.ctx(), ID_B, None, 0, Some(200))
        .unwrap()
        .page;
    assert!(page.records.iter().all(|r| r.kind == RecordKind::Omitted));
    assert_eq!(page.records.last().unwrap().count, Some(4));
}

#[test]
fn a_fork_without_explicit_record_ordinals_uses_the_rollout_positions() {
    let f = Fixture::new();
    let mut records = fork_records();
    for record in &mut records {
        record.as_object_mut().unwrap().remove("ordinal");
    }
    f.codex(ID_B, &records, 0);
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    assert_eq!(
        archive::read_meta(&e, &f.ctx().limits).snippet.as_deref(),
        Some("child-only task")
    );
    let page = sessions::transcript(&f.ctx(), ID_B, None, 0, Some(200))
        .unwrap()
        .page;
    assert!(
        page.records
            .iter()
            .all(|r| !r.text.as_deref().unwrap_or("").contains("parent-only"))
    );
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only answer"))
    );
}

#[test]
fn explicit_ordinals_determine_the_boundary_instead_of_line_numbers() {
    let f = Fixture::new();
    let mut records = fork_records();
    records[0]["payload"]["subagent_history_start_ordinal"] = json!(105);
    for (ordinal, record) in records.iter_mut().enumerate() {
        record["ordinal"] = json!(100 + ordinal);
    }
    f.codex(ID_B, &records, 0);
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    assert_eq!(
        archive::read_meta(&e, &f.ctx().limits).snippet.as_deref(),
        Some("child-only task")
    );
    let page = sessions::transcript(&f.ctx(), ID_B, None, 0, Some(200))
        .unwrap()
        .page;
    assert!(
        page.records
            .iter()
            .all(|r| !r.text.as_deref().unwrap_or("").contains("parent-only"))
    );
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only answer"))
    );
}

#[test]
fn child_summaries_exclude_parent_text_and_reject_pre_fix_cached_summaries() {
    let f = Fixture::new().with_fakes();
    f.codex(ID_B, &fork_records(), 0);
    let ctx = f.ctx();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let summary = rt
        .block_on(trovador::summary::summarize(&ctx, ID_B, None, false, None))
        .unwrap();
    assert!(!summary.cached);
    let prompts = f.prompts("claude");
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("child-only task"));
    assert!(prompts[0].contains("ASSISTANT: child-only answer"));
    assert!(!prompts[0].contains("parent-only"));

    let cache = trovador::summary::dir(&ctx).join(format!("codex-{ID_B}.json"));
    let mut stale = serde_json::to_value(&summary).unwrap();
    stale["prompt_version"] = json!(1);
    stale.as_object_mut().unwrap().remove("fork_aware");
    stale["summary"] = json!("parent-only stale summary");
    std::fs::write(cache, serde_json::to_vec(&stale).unwrap()).unwrap();
    let refreshed = rt
        .block_on(trovador::summary::summarize(&ctx, ID_B, None, false, None))
        .unwrap();
    assert!(!refreshed.cached);
    assert!(!refreshed.summary.contains("parent-only"));
    assert_eq!(f.prompts("claude").len(), 2);
}

fn real_layout_records() -> Vec<Value> {
    let short = fork_records();
    let mut records = short[..5].to_vec();
    records[0]["payload"]["subagent_history_start_ordinal"] = json!(41);
    records.extend((5..41).map(|_| json!({"type":"turn_context", "payload":{}})));
    records.extend([
        json!({"type":"event_msg", "payload":{"type":"thread_settings_applied"}}),
        json!({"type":"event_msg", "payload":{"type":"task_started"}}),
        codex_response("developer", "child setup"),
        json!({"type":"world_state", "payload":{}}),
        json!({"type":"turn_context", "payload":{}}),
        json!({"type":"inter_agent_communication_metadata", "payload":{"trigger_turn":true}}),
        short[5].clone(),
        short[6].clone(),
    ]);
    for (ordinal, record) in records.iter_mut().enumerate() {
        record["ordinal"] = json!(ordinal);
    }
    records
}

#[test]
fn real_layout_boundary_precedes_child_setup_and_assignment_by_six_records() {
    let f = Fixture::new();
    let records = real_layout_records();
    assert_eq!(records[41]["payload"]["type"], "thread_settings_applied");
    assert_eq!(records[47]["payload"]["type"], "agent_message");
    f.codex(ID_B, &records, 0);
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    assert_eq!(
        archive::read_meta(&e, &f.ctx().limits).snippet.as_deref(),
        Some("child-only task")
    );
    let page = sessions::transcript(&f.ctx(), ID_B, None, 0, Some(200))
        .unwrap()
        .page;
    let inherited = page
        .records
        .iter()
        .find(|r| r.name.as_deref() == Some("forked_history"))
        .unwrap();
    assert_eq!(inherited.count, Some(40));
    assert!(
        page.records
            .iter()
            .all(|r| !r.text.as_deref().unwrap_or("").contains("parent-only"))
    );
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only answer"))
    );
}

#[test]
fn appending_during_an_unknown_boundary_pass_does_not_checkpoint_child_turns() {
    use std::io::Write;
    use trovador::transcript::Projection;
    let f = Fixture::new();
    let records = real_layout_records();
    let path = f.codex(ID_B, &records[..20], 0);
    let scanned = std::fs::metadata(&path).unwrap().len();
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    let mut projection = Projection::for_index(&e, &f.ctx().limits, None).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    for record in &records[20..] {
        writeln!(file, "{record}").unwrap();
    }
    let first: Vec<_> = projection.by_ref().collect();
    assert!(first.iter().all(|r| r.text.is_none()));
    assert_eq!(projection.state().offset, scanned);
    let resumed: Vec<_> =
        Projection::for_index(&e, &f.ctx().limits, Some(projection.state().clone()))
            .unwrap()
            .collect();
    assert!(
        resumed
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only task"))
    );
    assert!(
        resumed
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only answer"))
    );
}

#[test]
fn a_typed_child_message_keeps_item_then_event_then_response_snippet_preference() {
    for with_item in [false, true] {
        let f = Fixture::new();
        let mut records = fork_records()[..5].to_vec();
        records.push(codex_response("user", "model-facing injected instructions"));
        records.push(codex_event("user_message", "child typed event"));
        if with_item {
            records.push(codex_user_item("child typed item"));
        }
        for (ordinal, record) in records.iter_mut().enumerate() {
            record["ordinal"] = json!(ordinal);
        }
        f.codex(ID_B, &records, 0);
        let e = archive::find_session(&f.paths, ID_B, None).unwrap();
        assert_eq!(
            archive::read_meta(&e, &f.ctx().limits).snippet.as_deref(),
            Some(if with_item {
                "child typed item"
            } else {
                "child typed event"
            })
        );
    }
}

#[test]
fn old_normal_summary_stays_cached_without_spending_provider_tokens() {
    let f = Fixture::new().with_fakes();
    f.codex(
        ID_A,
        &[
            codex_meta("/normal", json!("cli")),
            codex_response("user", "normal task"),
        ],
        0,
    );
    let ctx = f.ctx();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let summary = rt
        .block_on(trovador::summary::summarize(&ctx, ID_A, None, false, None))
        .unwrap();
    let mut old = serde_json::to_value(summary).unwrap();
    old["prompt_version"] = json!(1);
    old.as_object_mut().unwrap().remove("fork_aware");
    std::fs::write(
        trovador::summary::dir(&ctx).join(format!("codex-{ID_A}.json")),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    let cached = rt
        .block_on(trovador::summary::summarize(&ctx, ID_A, None, false, None))
        .unwrap();
    assert!(cached.cached);
    assert_eq!(f.prompts("claude").len(), 1);
}

#[test]
fn spike_import_preserves_normal_summaries_but_rejects_old_fork_summaries() {
    let f = Fixture::new();
    let normal = f.codex(ID_A, &[codex_meta("/normal", json!("cli"))], 0);
    let fork = f.codex(ID_B, &fork_records(), 0);
    let spike = f.dir.path().join("spike");
    std::fs::create_dir_all(spike.join("summaries")).unwrap();
    for (id, path) in [(ID_A, normal), (ID_B, fork)] {
        let mtime = std::fs::metadata(path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        std::fs::write(spike.join("summaries").join(format!("{id}.json")), json!({"session_id":id,"backend":"codex","project":"/work","source_mtime":mtime,"created":"2026-10-08T00:00:00Z","summary":"old spike summary"}).to_string()).unwrap();
    }
    for dry_run in [true, false] {
        let report = trovador::import::import(&f.ctx(), &spike, dry_run).unwrap();
        assert_eq!(report.summaries_imported, 1);
        assert_eq!(report.summaries_stale, 1);
    }
    let normal = archive::find_session(&f.paths, ID_A, None).unwrap();
    let fork = archive::find_session(&f.paths, ID_B, None).unwrap();
    assert!(trovador::summary::cached(&f.ctx(), &normal).is_some());
    assert!(trovador::summary::cached(&f.ctx(), &fork).is_none());
}

#[test]
fn a_finished_fork_with_an_impossible_boundary_keeps_its_transcript() {
    let f = Fixture::new();
    let mut records = fork_records();
    records[0]["payload"]["subagent_history_start_ordinal"] = json!(1000);
    records.push(json!({"type":"event_msg", "payload":{"type":"task_complete"}}));
    f.codex_archived(ID_B, &records, 0);
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    assert_eq!(
        archive::read_meta(&e, &f.ctx().limits).snippet.as_deref(),
        Some("parent-only question")
    );
    let page = sessions::transcript(&f.ctx(), ID_B, None, 0, Some(200))
        .unwrap()
        .page;
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref() == Some("parent-only question"))
    );
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only task"))
    );
    assert!(
        page.records
            .iter()
            .all(|r| r.name.as_deref() != Some("forked_history"))
    );
}

#[test]
fn blank_and_malformed_lines_do_not_shift_the_shared_fork_boundary() {
    let f = Fixture::new();
    let mut records = fork_records();
    records[0]["payload"]["subagent_history_start_ordinal"] = json!(7);
    for record in &mut records {
        record.as_object_mut().unwrap().remove("ordinal");
    }
    let path = f.codex(ID_B, &records, 0);
    let mut lines: Vec<_> = records.iter().map(|r| format!("{r}\n")).collect();
    lines.insert(2, "\nnot-json\n".into());
    std::fs::write(path, lines.concat()).unwrap();
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    assert_eq!(
        archive::read_meta(&e, &f.ctx().limits).snippet.as_deref(),
        Some("child-only task")
    );
    let page = sessions::transcript(&f.ctx(), ID_B, None, 0, Some(200))
        .unwrap()
        .page;
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only task"))
    );
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only answer"))
    );
    assert!(
        page.records
            .iter()
            .all(|r| !r.text.as_deref().unwrap_or("").contains("parent-only"))
    );
}

#[test]
fn known_fork_boundaries_survive_projection_resumes_and_index_reopens() {
    use std::io::Write;
    use trovador::transcript::Projection;
    let f = Fixture::new();
    let records = real_layout_records();
    let path = f.codex(ID_B, &records, 0);
    let ctx = f.ctx();
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    let expected = records[..41]
        .iter()
        .map(|r| format!("{r}\n").len() as u64)
        .sum::<u64>();
    let mut projection = Projection::for_index(&e, &ctx.limits, None).unwrap();
    let projected: Vec<_> = projection.by_ref().collect();
    assert!(
        projected
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only answer"))
    );
    let checkpoint = projection.state().clone();
    assert_eq!(checkpoint.codex_inherited_end, Some(expected));

    let mut idx = Index::open(&ctx.paths, &ctx.limits).unwrap();
    idx.refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())
        .unwrap();
    drop(idx);
    let conn = rusqlite::Connection::open(trovador::index::db_path(&ctx.paths)).unwrap();
    let stored: Option<i64> = conn
        .query_row(
            "SELECT codex_inherited_end FROM sessions WHERE session_id = ?1",
            [ID_B],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, Some(expected as i64));
    drop(conn);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(
        file,
        "{}",
        codex_response("assistant", "child follow-up answer")
    )
    .unwrap();
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    let mut resumed = Projection::for_index(&e, &ctx.limits, Some(checkpoint)).unwrap();
    let added: Vec<_> = resumed.by_ref().collect();
    assert_eq!(resumed.state().codex_inherited_end, Some(expected));
    assert!(
        added
            .iter()
            .any(|r| r.text.as_deref() == Some("child follow-up answer"))
    );
    let mut idx = Index::open(&ctx.paths, &ctx.limits).unwrap();
    let report = idx
        .refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())
        .unwrap();
    assert_eq!(report.appended, 1);
    assert!(
        idx.session_records(trovador::Backend::Codex, ID_B)
            .unwrap()
            .iter()
            .any(|r| r.2 == "child follow-up answer")
    );
}

#[test]
fn completed_unarchived_forks_with_impossible_boundaries_also_keep_their_turns() {
    let f = Fixture::new();
    let mut records = fork_records();
    records[0]["payload"]["subagent_history_start_ordinal"] = json!(1000);
    records.push(json!({"type":"event_msg", "payload":{"type":"task_complete"}}));
    f.codex(ID_B, &records, 0);
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    assert_eq!(
        archive::read_meta(&e, &f.ctx().limits).snippet.as_deref(),
        Some("parent-only question")
    );
    let page = sessions::transcript(&f.ctx(), ID_B, None, 0, Some(200))
        .unwrap()
        .page;
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref() == Some("parent-only question"))
    );
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only task"))
    );
    assert!(
        page.records
            .iter()
            .all(|r| r.name.as_deref() != Some("forked_history"))
    );
}

#[test]
fn payload_ordinals_do_not_override_the_rollout_record_position() {
    let f = Fixture::new();
    let mut records = fork_records();
    for record in &mut records {
        record.as_object_mut().unwrap().remove("ordinal");
    }
    records[1]["payload"]["ordinal"] = json!(999);
    f.codex(ID_B, &records, 0);
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    assert_eq!(
        archive::read_meta(&e, &f.ctx().limits).snippet.as_deref(),
        Some("child-only task")
    );
    let page = sessions::transcript(&f.ctx(), ID_B, None, 0, Some(200))
        .unwrap()
        .page;
    assert!(
        page.records
            .iter()
            .all(|r| !r.text.as_deref().unwrap_or("").contains("parent-only"))
    );
    assert!(
        page.records
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only answer"))
    );
}

#[test]
fn a_torn_fork_header_does_not_persist_a_non_fork_boundary() {
    use std::io::Write;
    use trovador::transcript::Projection;
    let f = Fixture::new();
    let records = fork_records();
    let path = f.codex(ID_B, &[], 0);
    let header = records[0].to_string();
    std::fs::write(&path, &header[..header.len() / 2]).unwrap();
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    let mut projection = Projection::for_index(&e, &f.ctx().limits, None).unwrap();
    assert!(projection.next().is_none());
    assert_eq!(projection.state().offset, 0);
    assert_eq!(projection.state().codex_inherited_end, None);
    let state = projection.state().clone();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "{}", &header[header.len() / 2..]).unwrap();
    for record in &records[1..] {
        writeln!(file, "{record}").unwrap();
    }
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    let resumed: Vec<_> = Projection::for_index(&e, &f.ctx().limits, Some(state))
        .unwrap()
        .collect();
    assert!(
        resumed
            .iter()
            .all(|r| !r.text.as_deref().unwrap_or("").contains("parent-only"))
    );
    assert!(
        resumed
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only task"))
    );
    assert!(
        resumed
            .iter()
            .any(|r| r.text.as_deref() == Some("child-only answer"))
    );
}

#[test]
fn copied_parent_completion_never_persists_a_zero_boundary_before_child_history_arrives() {
    use std::io::Write;
    for completion in ["task_complete", "turn_aborted"] {
        let f = Fixture::new();
        let mut records = fork_records();
        records.insert(4, codex_event(completion, ""));
        records[0]["payload"]["subagent_history_start_ordinal"] = json!(6);
        for (ordinal, record) in records.iter_mut().enumerate() {
            record["ordinal"] = json!(ordinal);
        }
        let path = f.codex(ID_B, &records[..5], 0);
        let ctx = f.ctx();
        let mut idx = Index::open(&ctx.paths, &ctx.limits).unwrap();
        idx.refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())
            .unwrap();
        assert!(
            idx.session_records(trovador::Backend::Codex, ID_B)
                .unwrap()
                .is_empty()
        );
        drop(idx);
        let conn = rusqlite::Connection::open(trovador::index::db_path(&ctx.paths)).unwrap();
        let boundary: Option<i64> = conn
            .query_row(
                "SELECT codex_inherited_end FROM sessions WHERE session_id = ?1",
                [ID_B],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(boundary, None);
        drop(conn);
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        for record in &records[5..] {
            writeln!(file, "{record}").unwrap();
        }
        let mut idx = Index::open(&ctx.paths, &ctx.limits).unwrap();
        let report = idx
            .refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())
            .unwrap();
        assert_eq!(report.appended, 1);
        let indexed = idx.session_records(trovador::Backend::Codex, ID_B).unwrap();
        assert_eq!(indexed.len(), 2);
        assert_eq!(indexed[0].2, "child-only task");
        assert_eq!(indexed[1].2, "child-only answer");
        assert!(indexed.iter().all(|r| !r.2.contains("parent-only")));
    }
}

#[test]
fn an_unmarked_summary_for_a_fork_without_a_start_ordinal_stays_cached() {
    let f = Fixture::new().with_fakes();
    let mut records = fork_records();
    records[0]["payload"]
        .as_object_mut()
        .unwrap()
        .remove("subagent_history_start_ordinal");
    f.codex(ID_B, &records, 0);
    let ctx = f.ctx();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let summary = rt
        .block_on(trovador::summary::summarize(&ctx, ID_B, None, false, None))
        .unwrap();
    let mut old = serde_json::to_value(summary).unwrap();
    old.as_object_mut().unwrap().remove("fork_aware");
    std::fs::write(
        trovador::summary::dir(&ctx).join(format!("codex-{ID_B}.json")),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    let reused = rt
        .block_on(trovador::summary::summarize(&ctx, ID_B, None, false, None))
        .unwrap();
    assert!(reused.cached);
    assert_eq!(f.prompts("claude").len(), 1);
}

#[test]
fn spike_summary_import_for_a_fork_without_a_start_ordinal_remains_reusable() {
    let f = Fixture::new();
    let mut records = fork_records();
    records[0]["payload"]
        .as_object_mut()
        .unwrap()
        .remove("subagent_history_start_ordinal");
    let path = f.codex(ID_B, &records, 0);
    let spike = f.dir.path().join("spike");
    std::fs::create_dir_all(spike.join("summaries")).unwrap();
    let mtime = std::fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    std::fs::write(spike.join("summaries").join(format!("{ID_B}.json")), json!({"session_id":ID_B,"backend":"codex","project":"/work","source_mtime":mtime,"created":"2026-10-08T00:00:00Z","summary":"old spike summary"}).to_string()).unwrap();
    for dry_run in [true, false] {
        let report = trovador::import::import(&f.ctx(), &spike, dry_run).unwrap();
        assert_eq!(report.summaries_imported, 1);
        assert_eq!(report.summaries_stale, 0);
    }
    let e = archive::find_session(&f.paths, ID_B, None).unwrap();
    let cached = trovador::summary::cached(&f.ctx(), &e).unwrap();
    assert_eq!(cached.summary, "old spike summary");
}

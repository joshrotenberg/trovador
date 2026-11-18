//! Codex command output in both shapes Codex has written. 0.162.0 dropped `stdout` and
//! `stderr` from `CommandExecution` items and kept only `aggregated_output`; reading
//! `stdout` alone projected every later command's result as empty.

mod common;

use common::*;
use serde_json::{Value, json};
use trovador::index::SearchOpts;
use trovador::sessions;
use trovador::transcript::RecordKind;

/// A `CommandExecution` item as Codex 0.165.0 writes it: no stdout or stderr.
fn current(cmd: &str, output: &str, exit_code: i64) -> Value {
    codex_item(json!({
        "type": "CommandExecution",
        "id": "exec-a635e007-8ef4-46d9-a299-f8db70ae904b",
        "process_id": "84614",
        "command": ["/bin/zsh", "-lc", cmd],
        "cwd": "file:///w",
        "parsed_cmd": [{"type": "unknown", "cmd": cmd}],
        "source": "unified_exec_startup",
        "status": "completed",
        "aggregated_output": output,
        "exit_code": exit_code,
        "duration": {"secs": 0, "nanos": 22916}
    }))
}

/// The same item as Codex 0.160 and earlier wrote it.
fn older(cmd: &str, stdout: &str, stderr: &str) -> Value {
    codex_item(json!({
        "type": "CommandExecution",
        "command": ["/bin/zsh", "-lc", cmd],
        "parsed_cmd": [{"type": "unknown", "cmd": cmd}],
        "status": "completed",
        "stdout": stdout,
        "stderr": stderr,
        "aggregated_output": format!("{stdout}{stderr}"),
        "formatted_output": stdout,
        "exit_code": 0
    }))
}

fn session(f: &Fixture, id: &str, command: Value) {
    f.codex(
        id,
        &[
            codex_meta("/w", json!("vscode")),
            codex_user_item("where are we"),
            command,
            codex_agent_item("in the candidate checkout"),
        ],
        0,
    );
}

fn tool_results(f: &Fixture, id: &str) -> Vec<String> {
    sessions::transcript(&f.ctx(), id, None, 0, None)
        .unwrap()
        .page
        .records
        .into_iter()
        .filter(|r| r.kind == RecordKind::ToolResult)
        .map(|r| r.text.unwrap_or_default())
        .collect()
}

#[test]
fn current_codex_command_output_appears_in_the_transcript() {
    let f = Fixture::new();
    session(
        &f,
        ID_A,
        current("pwd", "/private/tmp/due-rpc/candidate\n", 0),
    );
    assert_eq!(tool_results(&f, ID_A), ["/private/tmp/due-rpc/candidate\n"]);
}

#[test]
fn current_codex_command_output_is_in_session_text_and_search() {
    let f = Fixture::new();
    session(
        &f,
        ID_A,
        current("cargo test", "test result: FAILED. zqoutputneedle", 101),
    );
    let text = sessions::text(&f.ctx(), ID_A, None, 0).unwrap().text;
    assert!(
        text.contains("TOOL RESULT: test result: FAILED. zqoutputneedle"),
        "{text}"
    );
    let found = sessions::search(
        &f.ctx(),
        &SearchOpts {
            query: "zqoutputneedle".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(found.sessions_matched, 1);
    assert_eq!(found.hits[0].matches[0].kind, "tool_result");
}

#[test]
fn older_codex_command_output_projects_as_before() {
    // Before 0.162.0 aggregated_output equalled stdout (6,991 of 6,991 sampled commands),
    // so preferring it must leave older sessions' text unchanged.
    let f = Fixture::new();
    session(&f, ID_A, older("ls", "a.rs\nb.rs\n", ""));
    assert_eq!(tool_results(&f, ID_A), ["a.rs\nb.rs\n"]);
}

#[test]
fn an_item_with_only_stdout_and_stderr_still_shows_both() {
    let f = Fixture::new();
    session(
        &f,
        ID_A,
        codex_item(
            json!({"type": "CommandExecution", "parsed_cmd": [{"cmd": "make"}],
                          "stdout": "built", "stderr": "warning: unused"}),
        ),
    );
    assert_eq!(tool_results(&f, ID_A), ["built\n[stderr] warning: unused"]);
}

//! Progress lines and diagnostics on stderr. stdout carries one JSON document whatever
//! either of them prints.
//!
//! `init` and the first search after an install both ran for seconds with nothing on
//! stderr, and the library emitted almost no `tracing` events, so `TROVADOR_LOG=debug`
//! showed nothing either.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use common::*;
use serde_json::Value;

const NEEDLE: &str = "zqprogressneedle";

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", NEEDLE)], 3_600);
    f.claude("-w", ID_B, &[claude_user("/w", "other")], 3_600);
    f
}

fn command(f: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_trovador"));
    c.args(args)
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("TROVADOR_HOME", &f.paths.home)
        .env_remove("TROVADOR_LOG")
        .env_remove("TROVADOR_PROGRESS")
        .env_remove("TROVADOR_PROGRESS_AFTER_MS")
        .env_remove("TROVADOR_RECORD_CHARS")
        .envs(env.iter().copied());
    c
}

/// Runs a command to completion: its stdout parsed as one JSON document, and its stderr.
fn run(f: &Fixture, env: &[(&str, &str)], args: &[&str]) -> (Value, String) {
    let out = command(f, env, args).output().expect("run trovador");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "trovador {args:?} failed: {stderr}");
    let stdout = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}); stderr: {stderr}"));
    (stdout, stderr)
}

const PROGRESS_NOW: &[(&str, &str)] = &[
    ("TROVADOR_PROGRESS", "1"),
    ("TROVADOR_PROGRESS_AFTER_MS", "0"),
];

#[test]
fn an_index_build_prints_its_plan_and_summary_on_stderr_and_stdout_stays_json() {
    let f = fixture();
    let (out, stderr) = run(&f, PROGRESS_NOW, &["index"]);
    assert_eq!(out["refresh"]["reindexed"], 2);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(
        lines[0], "trovador: indexing 2 session files (2 new)",
        "{stderr}"
    );
    assert!(
        lines
            .last()
            .unwrap()
            .starts_with("trovador: indexed 2 session files in "),
        "{stderr}"
    );
}

#[test]
fn a_refresh_shorter_than_the_delay_prints_nothing() {
    // A search usually finds a few grown files; announcing them on every search at a
    // terminal would be noise.
    let f = fixture();
    let (_, stderr) = run(&f, &[("TROVADOR_PROGRESS", "1")], &["index"]);
    assert_eq!(stderr, "");
}

#[test]
fn a_refresh_with_nothing_to_do_prints_nothing() {
    let f = fixture();
    run(&f, &[], &["index"]);
    let (out, stderr) = run(&f, PROGRESS_NOW, &["search", NEEDLE]);
    assert_eq!(out["coverage"]["refresh"]["unchanged"], 2);
    assert_eq!(stderr, "");
}

#[test]
fn with_progress_off_and_the_default_filter_stderr_is_empty() {
    let f = fixture();
    let (_, stderr) = run(
        &f,
        &[
            ("TROVADOR_PROGRESS", "0"),
            ("TROVADOR_PROGRESS_AFTER_MS", "0"),
        ],
        &["search", NEEDLE],
    );
    assert_eq!(stderr, "");
}

#[test]
fn a_search_that_discards_the_index_says_why_it_is_slow() {
    let f = fixture();
    run(&f, &[], &["index"]);
    let mut env = PROGRESS_NOW.to_vec();
    env.push(("TROVADOR_RECORD_CHARS", "1234"));
    let (out, stderr) = run(&f, &env, &["search", NEEDLE]);
    assert_eq!(out["sessions_matched"], 1);
    assert!(
        stderr.starts_with(
            "trovador: the index was built by another version or with other settings; building it again\n"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("trovador: indexing 2 session files (2 new); for up to "),
        "{stderr}"
    );
}

#[test]
fn info_logging_at_a_terminal_does_not_print_progress_twice() {
    let f = fixture();
    let mut env = PROGRESS_NOW.to_vec();
    env.push(("TROVADOR_LOG", "info"));
    let (_, stderr) = run(&f, &env, &["index"]);
    assert_eq!(
        stderr.matches("indexing 2 session files").count(),
        1,
        "{stderr}"
    );
}

#[test]
fn with_progress_off_info_logging_still_carries_the_progress_events() {
    let f = fixture();
    let (_, stderr) = run(
        &f,
        &[
            ("TROVADOR_PROGRESS", "0"),
            ("TROVADOR_PROGRESS_AFTER_MS", "0"),
            ("TROVADOR_LOG", "info"),
        ],
        &["index"],
    );
    assert!(stderr.contains("trovador::progress"), "{stderr}");
    assert!(stderr.contains("indexing 2 session files"), "{stderr}");
}

#[test]
fn debug_logging_names_each_phase_and_leaves_stdout_alone() {
    let f = fixture();
    let (out, stderr) = run(
        &f,
        &[("TROVADOR_LOG", "debug"), ("TROVADOR_PROGRESS", "0")],
        &["search", NEEDLE],
    );
    assert_eq!(out["sessions_matched"], 1);
    for phase in [
        "scanned archive root",
        "creating index",
        "refresh plan",
        "refresh done",
        "search",
    ] {
        assert!(stderr.contains(phase), "no {phase:?} in {stderr}");
    }
    assert!(
        !stderr.contains("\x1b["),
        "colour codes when stderr is not a terminal"
    );
    assert!(!stderr.contains("projected"), "per-file lines are trace");
}

#[test]
fn trace_logging_adds_one_line_per_file_indexed() {
    let f = fixture();
    let (_, stderr) = run(
        &f,
        &[
            ("TROVADOR_LOG", "trovador=trace"),
            ("TROVADOR_PROGRESS", "0"),
        ],
        &["index"],
    );
    assert_eq!(stderr.matches(" projected ").count(), 2, "{stderr}");
}

#[test]
fn logged_queries_are_redacted() {
    let f = fixture();
    let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";
    let (_, stderr) = run(
        &f,
        &[("TROVADOR_LOG", "debug"), ("TROVADOR_PROGRESS", "0")],
        &["search", key],
    );
    assert!(stderr.contains("[redacted:anthropic]"), "{stderr}");
    assert!(!stderr.contains(key));
}

#[test]
fn serve_never_prints_progress_even_when_asked() {
    let f = fixture();
    let mut child = command(&f, PROGRESS_NOW, &["serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn trovador serve");
    let mut stdin = child.stdin.take().unwrap();
    for msg in [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "search", "arguments": {"query": NEEDLE}}}),
    ] {
        writeln!(stdin, "{msg}").unwrap();
    }
    stdin.flush().unwrap();
    // Wait for the search result before closing stdin, so the refresh has run.
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    let mut answered = false;
    while stdout.read_line(&mut line).unwrap() > 0 {
        let v: Value = serde_json::from_str(&line).expect("stdout is JSON-RPC");
        if v["id"] == 2 {
            answered = true;
            break;
        }
        line.clear();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(answered, "no search result");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("trovador: indexing"), "{stderr}");
}

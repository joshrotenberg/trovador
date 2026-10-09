//! The CLI and the MCP tool must return what the library returns. spike had four separate
//! bugs from its two doors computing the same answer differently.

mod common;

use std::process::{Command, Output};

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::sessions::{self, ListOpts};

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "claude question"),
            claude_tool_use("Read", json!({"file_path": "/w/a.rs"})),
            claude_assistant("claude answer"),
        ],
        20,
    );
    f.codex(
        ID_B,
        &[
            codex_meta("/w", json!("cli")),
            codex_user_item("codex question"),
            codex_agent_item("codex answer"),
        ],
        10,
    );
    f
}

fn cli_output(f: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(args)
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("BOTTEGA_TEST_ROOT", f.dir.path())
        .env("BOTTEGA_HOME", f.paths.shared.parent().unwrap())
        .env("TROVADOR_HOME", &f.paths.home)
        .env("TROVADOR_ACTIVE_WINDOW_S", "0")
        .env(
            "TROVADOR_CLAUDE_BIN",
            f.paths.claude_bin.clone().unwrap_or_default(),
        )
        .env(
            "TROVADOR_CODEX_BIN",
            f.paths.codex_bin.clone().unwrap_or_default(),
        )
        .output()
        .expect("run trovador")
}

fn cli(f: &Fixture, args: &[&str]) -> Value {
    let out = cli_output(f, args);
    assert!(
        out.status.success(),
        "trovador {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is one JSON document")
}

async fn mcp_tool(f: &Fixture, tool: &str, args: Value) -> Value {
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    client.call_tool_json(tool, args).await
}

async fn mcp(f: &Fixture, args: Value) -> Value {
    mcp_tool(f, "sessions", args).await
}

#[tokio::test]
async fn list_is_identical_from_the_library_the_cli_and_the_mcp_tool() {
    let f = fixture();
    let lib =
        serde_json::to_value(sessions::list(&f.ctx(), &ListOpts::default()).unwrap()).unwrap();
    assert_eq!(lib["returned"], 2);
    assert_eq!(cli(&f, &["sessions", "list"]), lib);
    assert_eq!(mcp(&f, json!({"action": "list"})).await, lib);
}

#[tokio::test]
async fn show_and_transcript_are_identical_through_both_doors() {
    let f = fixture();
    let ctx = f.ctx();
    let show = serde_json::to_value(sessions::show(&ctx, "bbbb", None).unwrap()).unwrap();
    assert_eq!(cli(&f, &["sessions", "show", "bbbb"]), show);
    assert_eq!(
        mcp(&f, json!({"action": "show", "session_id": "bbbb"})).await,
        show
    );

    let page =
        serde_json::to_value(sessions::transcript(&ctx, ID_A, None, 1, Some(1)).unwrap()).unwrap();
    assert_eq!(page["records"][0]["type"], "tool_call");
    assert_eq!(
        cli(
            &f,
            &[
                "sessions",
                "transcript",
                ID_A,
                "--offset",
                "1",
                "--limit",
                "1"
            ]
        ),
        page
    );
    assert_eq!(
        mcp(
            &f,
            json!({"action": "transcript", "session_id": ID_A, "offset": 1, "limit": 1})
        )
        .await,
        page
    );
}

#[tokio::test]
async fn an_unknown_session_is_a_readable_tool_error_not_a_protocol_failure() {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let result = client
        .call_tool("sessions", json!({"action": "show", "session_id": "ffff"}))
        .await;
    assert!(result.is_error);
    assert!(
        result
            .all_text()
            .contains("no archived session matches \"ffff\"")
    );

    let missing = client
        .call_tool("sessions", json!({"action": "transcript"}))
        .await;
    assert!(missing.is_error);
    assert!(missing.all_text().contains("session_id is required"));
}

#[tokio::test]
async fn the_session_resource_matches_show() {
    let f = fixture();
    let show = serde_json::to_value(sessions::show(&f.ctx(), ID_B, None).unwrap()).unwrap();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let res = client
        .read_resource(&format!("session://codex/{ID_B}"))
        .await;
    let body: Value = serde_json::from_str(res.first_text().unwrap()).unwrap();
    assert_eq!(body, show);

    let t = client
        .read_resource(&format!("session://claude/{ID_A}/transcript"))
        .await;
    let body: Value = serde_json::from_str(t.first_text().unwrap()).unwrap();
    assert_eq!(body["records"][0]["text"], "claude question");
}

fn without_timing(mut v: Value) -> Value {
    if let Some(r) = v.pointer_mut("/coverage/refresh") {
        r.as_object_mut().unwrap().remove("ms");
    }
    v
}

#[tokio::test]
async fn project_filter_is_ascii_case_insensitive_in_list_and_search_through_both_doors() {
    let f = Fixture::new();
    f.claude(
        "-w-MyProj",
        ID_A,
        &[claude_user("/w/MyProj/É", "alpha")],
        20,
    );
    // Issue #31: listing before searching must not hide a project that search finds.
    let cold = cli(&f, &["sessions", "list", "--project", "myproj"]);
    assert_eq!(cold["returned"], 1);
    assert_eq!(
        mcp(&f, json!({"action": "list", "project": "myproj"})).await,
        cold
    );
    sessions::reindex(&f.ctx(), false).unwrap();
    for (project, expected) in [
        ("myproj", 1),
        ("MYPROJ", 1),
        ("MyProj/É", 1),
        ("myproj/é", 0),
        ("missing", 0),
    ] {
        let listed = cli(&f, &["sessions", "list", "--project", project]);
        assert_eq!(listed["returned"], expected, "{project}");
        assert_eq!(
            mcp(&f, json!({"action": "list", "project": project})).await,
            listed
        );
        let searched = without_timing(cli(&f, &["search", "alpha", "--project", project]));
        assert_eq!(searched["sessions_matched"], expected, "{project}");
        assert_eq!(
            without_timing(
                mcp_tool(&f, "search", json!({"query": "alpha", "project": project})).await
            ),
            searched
        );
    }
}

#[tokio::test]
async fn search_is_identical_from_the_library_the_cli_and_the_mcp_tool() {
    let f = fixture();
    // Index once so every door sees the same unchanged archive.
    sessions::reindex(&f.ctx(), false).unwrap();
    let opts = trovador::index::SearchOpts {
        query: "question".into(),
        ..Default::default()
    };
    let lib =
        without_timing(serde_json::to_value(sessions::search(&f.ctx(), &opts).unwrap()).unwrap());
    assert_eq!(lib["sessions_matched"], 2);
    assert_eq!(lib["coverage"]["refresh"]["unchanged"], 2);
    assert_eq!(without_timing(cli(&f, &["search", "question"])), lib);
    assert_eq!(
        without_timing(mcp_tool(&f, "search", json!({"query": "question"})).await),
        lib
    );
}

#[tokio::test]
async fn a_bad_query_is_a_readable_tool_error() {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let r = client
        .call_tool("search", json!({"query": "(", "regex": true}))
        .await;
    assert!(r.is_error);
    assert!(r.all_text().starts_with("invalid query"));
}

#[tokio::test]
async fn no_mcp_tool_writes_provenance_or_runs_a_turn() {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let names: Vec<String> = client
        .list_tools()
        .await
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    for n in ["sessions", "search", "summarize"] {
        assert!(names.iter().any(|x| x == n));
    }
    assert!(names.iter().all(|n| !n.contains("consent") && n != "ask"));
}

#[tokio::test]
async fn a_cached_summary_is_identical_through_both_doors_and_the_resource() {
    let f = fixture().with_fakes();
    let ctx = f.ctx();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    // The resource only serves what exists; it never spends tokens making one.
    let missing = client
        .send_request_expect_error(
            "resources/read",
            Some(json!({"uri": format!("session://claude/{ID_A}/summary")})),
        )
        .await;
    assert!(
        missing.to_string().contains("no current summary"),
        "{missing}"
    );
    assert!(f.calls("claude").is_empty());

    trovador::summary::summarize(&ctx, ID_A, None, false, None)
        .await
        .unwrap();
    let lib = serde_json::to_value(
        trovador::summary::summarize(&ctx, ID_A, None, false, None)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(lib["cached"], true);
    assert_eq!(cli(&f, &["summarize", ID_A]), lib);
    assert_eq!(
        client
            .call_tool_json("summarize", json!({"session_id": ID_A}))
            .await,
        lib
    );
    let res = client
        .read_resource(&format!("session://claude/{ID_A}/summary"))
        .await;
    let body: Value = serde_json::from_str(res.first_text().unwrap()).unwrap();
    assert_eq!(body, lib);
    assert_eq!(
        f.calls("claude").len(),
        1,
        "only the first summary ran a model"
    );
}

async fn assert_unmatchable_query_is_rejected_through_both_doors(query: &str, fts: bool) {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let response = client
        .call_tool("search", json!({"query": query, "fts": fts}))
        .await;
    assert!(response.is_error, "{query:?}: {}", response.all_text());
    assert!(
        response.all_text().contains("cannot match"),
        "{}",
        response.all_text()
    );
    assert!(!response.all_text().contains("\"absence_is_proof\":true"));

    assert!(!trovador::index::db_path(&f.paths).exists());
    let args = if fts {
        vec!["search", "--fts", "--", query]
    } else {
        vec!["search", "--", query]
    };
    let output = cli_output(&f, &args);
    assert!(
        !output.status.success(),
        "{query:?}: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot match"));
    assert!(output.stdout.is_empty());
    assert!(!trovador::index::db_path(&f.paths).exists());
}

#[tokio::test]
async fn tokenless_terms_cannot_claim_absence_through_cli_or_mcp() {
    for query in ["->", "-", "...", "\"\"", "question ->", "\u{0345}"] {
        assert_unmatchable_query_is_rejected_through_both_doors(query, false).await;
    }
}

#[tokio::test]
async fn ambiguous_empty_fts_results_have_a_note_and_never_claim_absence_through_both_doors() {
    let f = fixture();
    sessions::reindex(&f.ctx(), false).unwrap();
    for (query, reason) in [
        ("kind:user", "unindexed"),
        ("name:Bash", "unindexed"),
        ("\"name\":Bash", "unindexed"),
        ("\"->\"", "no tokens"),
        ("\"\"", "no tokens"),
        ("\"\u{0345}\"", "no tokens"),
    ] {
        let result = cli(&f, &["search", "--fts", "--", query]);
        assert_eq!(result["sessions_matched"], 0, "{query}");
        assert_eq!(result["absence_is_proof"], false, "{query}");
        assert!(
            result["note"].as_str().unwrap().contains(reason),
            "{result}"
        );
        assert_eq!(
            without_timing(result),
            without_timing(mcp_tool(&f, "search", json!({"query": query, "fts": true})).await)
        );
    }
}

#[test]
fn unmatchable_queries_are_library_errors_even_when_punctuation_exists() {
    let f = fixture();
    f.claude(
        "-w",
        ID_A,
        &[claude_user("/w", "alpha deploy - charlie -> bravo ...")],
        20,
    );
    for (query, fts) in [
        ("->", false),
        ("-", false),
        ("...", false),
        ("\"\"", false),
        ("\u{0345}", false),
    ] {
        let error = sessions::search(
            &f.ctx(),
            &trovador::index::SearchOpts {
                query: query.into(),
                fts,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(error, sessions::Error::Query(_)),
            "{query:?}: {error}"
        );
        assert!(
            error.to_string().contains("cannot match"),
            "{query:?}: {error}"
        );
        assert!(!trovador::index::db_path(&f.paths).exists());
    }
    let result = sessions::search(
        &f.ctx(),
        &trovador::index::SearchOpts {
            query: "->".into(),
            regex: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.sessions_matched, 1);
}

#[tokio::test]
async fn searchable_fts_filters_and_quoted_column_like_text_still_work_through_both_doors() {
    let f = fixture();
    sessions::reindex(&f.ctx(), false).unwrap();
    for query in [
        "text:question",
        "{text}:question",
        "\"kind:user\"",
        "text:question OR text:answer",
    ] {
        let cli_result = cli(&f, &["search", "--fts", "--", query]);
        let mcp_result = mcp_tool(&f, "search", json!({"query": query, "fts": true})).await;
        assert_eq!(
            without_timing(cli_result.clone()),
            without_timing(mcp_result)
        );
        if query != "\"kind:user\"" {
            assert_eq!(cli_result["sessions_matched"], 2, "{query}");
        }
    }
}

#[tokio::test]
async fn pua_tokens_and_ignored_empty_quotes_remain_searchable_through_both_doors() {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[claude_user("/w", "error: deploy user \u{e0b0}")],
        20,
    );
    sessions::reindex(&f.ctx(), false).unwrap();
    for query in ["\u{e0b0}", "error: \"", "error: \"\""] {
        let result = cli(&f, &["search", "--", query]);
        assert_eq!(result["sessions_matched"], 1, "{query}");
        assert_eq!(
            without_timing(result),
            without_timing(mcp_tool(&f, "search", json!({"query": query})).await)
        );
    }
    for query in [
        "-kind:user",
        "-{kind name}:deploy",
        "deploy OR kind:user",
        "{text kind}:user",
    ] {
        let result = cli(&f, &["search", "--fts", "--", query]);
        assert_eq!(result["sessions_matched"], 1, "{query}");
        assert_eq!(
            without_timing(result),
            without_timing(mcp_tool(&f, "search", json!({"query": query, "fts": true})).await)
        );
    }
}

#[tokio::test]
async fn fts_syntax_errors_preserve_sqlites_message_without_a_column_hint() {
    let f = fixture();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE VIRTUAL TABLE records USING fts5(text, kind UNINDEXED, name UNINDEXED, tokenize = 'unicode61 remove_diacritics 2');").unwrap();
    for query in ["(", "text:", "deploy OR"] {
        let expected = conn
            .query_row(
                "SELECT count(*) FROM records WHERE records MATCH ?1",
                [query],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_err()
            .to_string();
        let error = sessions::search(
            &f.ctx(),
            &trovador::index::SearchOpts {
                query: query.into(),
                fts: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(error, sessions::Error::Query(_)));
        assert_eq!(error.to_string(), format!("invalid query: {expected}"));
        let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
        client.initialize().await;
        let result = client
            .call_tool("search", json!({"query": query, "fts": true}))
            .await;
        assert!(result.is_error);
        assert_eq!(result.all_text(), format!("invalid query: {expected}"));
        let output = cli_output(&f, &["search", "--fts", "--", query]);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("invalid query: {expected}")),
            "{stderr}"
        );
        assert!(!stderr.contains("column filters"), "{stderr}");
    }
}

#[test]
fn tokenless_terms_fail_before_an_unopenable_index_is_touched() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.paths.home).unwrap();
    std::fs::write(trovador::index::db_path(&f.paths), "not a database").unwrap();
    let error = sessions::search(
        &f.ctx(),
        &trovador::index::SearchOpts {
            query: "->".into(),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(error, sessions::Error::Query(_)), "{error}");
    assert!(error.to_string().contains("->"));
}

#[tokio::test]
async fn unquoted_tokenless_fts_words_cannot_prove_absence_through_cli_or_mcp() {
    let f = fixture();
    sessions::reindex(&f.ctx(), false).unwrap();
    for query in [
        "question AND →",
        "question AND \u{0345}",
        "question AND ☺",
        "question AND →*",
        "deploy →",
        "deploy \u{0345}",
    ] {
        let result = cli(&f, &["search", "--fts", "--", query]);
        assert_eq!(result["sessions_matched"], 0, "{query}");
        assert_eq!(result["absence_is_proof"], false, "{query}");
        assert!(
            result["note"].as_str().unwrap().contains("no tokens"),
            "{result}"
        );
        assert_eq!(
            without_timing(result),
            without_timing(mcp_tool(&f, "search", json!({"query": query, "fts": true})).await)
        );
    }
}

#[tokio::test]
async fn ordinary_unquoted_fts_words_operators_and_prefixes_do_not_add_a_query_note() {
    let f = fixture();
    sessions::reindex(&f.ctx(), false).unwrap();
    for query in [
        "question AND neverseen",
        "question NOT question",
        "neverseen OR nonexistent",
        "NEAR(question neverseen)",
        "text : neverseen*",
    ] {
        let result = cli(&f, &["search", "--fts", "--", query]);
        assert_eq!(result["sessions_matched"], 0, "{query}");
        assert_eq!(result["absence_is_proof"], true, "{query}");
        assert!(result.get("note").is_none(), "{result}");
        assert_eq!(
            without_timing(result),
            without_timing(mcp_tool(&f, "search", json!({"query": query, "fts": true})).await)
        );
    }
}

#[test]
fn oversized_regex_is_an_invalid_query_before_index_refresh_through_cli() {
    let f = fixture();
    let out = cli_output(&f, &["search", "--regex", r"\w{100}"]);
    assert_eq!(out.status.code(), Some(1));
    let error = String::from_utf8(out.stderr).unwrap();
    assert!(error.starts_with("trovador: invalid query"), "{error}");
    assert!(
        error.contains("Compiled regex exceeds size limit"),
        "{error}"
    );
    assert!(!f.paths.home.join("index.db").exists());
}

#[tokio::test]
async fn oversized_regex_is_an_invalid_query_before_index_refresh_through_mcp() {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let result = client
        .call_tool("search", json!({"query": r"\w{100}", "regex": true}))
        .await;
    assert!(result.is_error);
    let error = result.all_text();
    assert!(error.starts_with("invalid query"), "{error}");
    assert!(
        error.contains("Compiled regex exceeds size limit"),
        "{error}"
    );
    assert!(!f.paths.home.join("index.db").exists());
}

#[test]
fn invalid_since_is_an_invalid_query_before_index_refresh_through_cli() {
    let f = fixture();
    let out = cli_output(&f, &["search", "question", "--since", "garbage"]);
    assert_eq!(out.status.code(), Some(1));
    let error = String::from_utf8(out.stderr).unwrap();
    assert!(error.starts_with("trovador: invalid query"), "{error}");
    assert!(!f.paths.home.join("index.db").exists());
}

#[tokio::test]
async fn invalid_since_is_an_invalid_query_before_index_refresh_through_mcp() {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let result = client
        .call_tool("search", json!({"query": "question", "since": "garbage"}))
        .await;
    assert!(result.is_error);
    let error = result.all_text();
    assert!(error.starts_with("invalid query"), "{error}");
    assert!(!f.paths.home.join("index.db").exists());
}

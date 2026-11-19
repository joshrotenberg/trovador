//! Output schemas and `fields` (#105): a caller that knows the shape of a result asks for
//! only the fields it needs, and both doors return the same selection.

mod common;

use std::process::{Command, Output};

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::fields::{Shape, select};
use trovador::index::SearchOpts;
use trovador::sessions::{self, ListOpts};

const NEEDLE: &str = "zqfieldsneedle";

fn fixture() -> Fixture {
    let f = Fixture::new();
    for (i, id) in [ID_A, ID_B, ID_C].into_iter().enumerate() {
        f.claude(
            "-w",
            id,
            &[
                claude_user("/w", &format!("{NEEDLE} question {i}")),
                claude_assistant(&format!("{NEEDLE} answer {i}")),
            ],
            100 + i as u64,
        );
    }
    f
}

fn cli_output(f: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(args)
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("TROVADOR_HOME", &f.paths.home)
        .env("TROVADOR_ACTIVE_WINDOW_S", "0")
        .env_remove("TROVADOR_PROGRESS")
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
    serde_json::from_slice(&out.stdout).unwrap()
}

async fn client(f: &Fixture) -> TestClient {
    let mut c = TestClient::from_router(trovador::mcp::router(f.ctx()));
    c.initialize().await;
    c
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn keys(v: &Value) -> Vec<&str> {
    let mut k: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    k.sort();
    k
}

#[tokio::test]
async fn a_selected_search_is_the_same_through_the_library_the_cli_and_mcp() {
    let f = fixture();
    let fields = names(&["title", "modified", "matches.excerpt"]);
    let r = sessions::search(
        &f.ctx(),
        &SearchOpts {
            query: NEEDLE.into(),
            fields: Some(fields.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let mut lib = serde_json::to_value(r).unwrap();
    select(&mut lib, Shape::Search, &fields).unwrap();
    let hit = &lib["hits"][0];
    assert_eq!(keys(hit), ["matches", "modified", "provider", "session_id"]);
    assert_eq!(keys(&hit["matches"][0]), ["excerpt", "index"]);
    // The envelope is untouched.
    for k in [
        "sessions_matched",
        "exhaustive",
        "hidden",
        "coverage",
        "offset",
    ] {
        assert!(lib.get(k).is_some(), "{k}");
    }
    let via_cli = cli(
        &f,
        &[
            "search",
            NEEDLE,
            "--fields",
            "title,modified,matches.excerpt",
        ],
    );
    let via_mcp = client(&f)
        .await
        .call_tool_json(
            "search",
            json!({"query": NEEDLE, "fields": ["title", "modified", "matches.excerpt"]}),
        )
        .await;
    // Refresh timings differ between runs; everything else must agree.
    let strip = |mut v: Value| {
        v["coverage"]["refresh"] = Value::Null;
        v
    };
    assert_eq!(strip(via_cli), strip(lib.clone()));
    assert_eq!(strip(via_mcp), strip(lib));
}

#[tokio::test]
async fn a_selected_list_and_transcript_agree_through_both_doors() {
    let f = fixture();
    let fields = names(&["title"]);
    let mut lib = serde_json::to_value(
        sessions::list(
            &f.ctx(),
            &ListOpts {
                fields: Some(fields.clone()),
                ..Default::default()
            },
        )
        .unwrap(),
    )
    .unwrap();
    select(&mut lib, Shape::List, &fields).unwrap();
    assert_eq!(keys(&lib["sessions"][0]), ["provider", "session_id"]);
    assert!(lib.get("total_sessions").is_some());
    assert_eq!(cli(&f, &["sessions", "list", "--fields", "title"]), lib);
    let mut c = client(&f).await;
    assert_eq!(
        c.call_tool_json("sessions", json!({"action": "list", "fields": ["title"]}))
            .await,
        lib
    );

    let page = cli(&f, &["sessions", "transcript", ID_A, "--fields", "type"]);
    assert_eq!(keys(&page["records"][0]), ["index", "type"]);
    assert_eq!(keys(&page["records"][1]), ["index", "type"]);
    assert!(page.get("revision").is_some());
    let via_mcp = c
        .call_tool_json(
            "sessions",
            json!({"action": "transcript", "session_id": ID_A, "fields": ["type"]}),
        )
        .await;
    assert_eq!(via_mcp, page);
}

#[tokio::test]
async fn an_unknown_field_fails_the_same_way_through_both_doors() {
    let f = fixture();
    let out = cli_output(&f, &["search", NEEDLE, "--fields", "titel"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("invalid fields: unknown field titel; valid fields: "),
        "{err}"
    );
    let r = client(&f)
        .await
        .call_tool("search", json!({"query": NEEDLE, "fields": ["titel"]}))
        .await;
    assert!(r.is_error);
    assert!(
        r.all_text().contains("unknown field titel"),
        "{}",
        r.all_text()
    );
    let r = sessions::list(
        &f.ctx(),
        &ListOpts {
            fields: Some(names(&["matches.index"])),
            ..Default::default()
        },
    );
    assert!(matches!(r, Err(sessions::Error::Fields(_))));
}

#[tokio::test]
async fn fields_on_show_or_text_is_an_error() {
    let f = fixture();
    let mut c = client(&f).await;
    for action in ["show", "text"] {
        let r = c
            .call_tool(
                "sessions",
                json!({"action": action, "session_id": ID_A, "fields": ["title"]}),
            )
            .await;
        assert!(r.is_error, "{action}");
        assert!(
            r.all_text()
                .contains("fields applies to list and transcript")
        );
    }
}

#[test]
fn the_budget_counts_only_the_selected_fields() {
    let f = fixture();
    let c = f.ctx_with(trovador::Limits {
        response_max_chars: 300,
        ..limits()
    });
    let search = |fields: Option<Vec<String>>| {
        sessions::search(
            &c,
            &SearchOpts {
                query: NEEDLE.into(),
                fields,
                ..Default::default()
            },
        )
        .unwrap()
        .returned
    };
    let whole = search(None);
    let lean = search(Some(names(&["match_count"])));
    assert_eq!(whole, 1, "one full hit is over 300 characters");
    assert_eq!(lean, 3, "three lean hits fit");
}

#[tokio::test]
async fn both_tools_publish_an_output_schema_without_required_fields() {
    let f = fixture();
    let tools = client(&f).await.list_tools().await;
    for name in ["search", "sessions"] {
        let t = tools.iter().find(|t| t["name"] == name).unwrap();
        let schema = &t["outputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert!(!schema.to_string().contains("\"required\":["), "{name}");
    }
    let search = tools.iter().find(|t| t["name"] == "search").unwrap();
    assert_eq!(
        search["outputSchema"],
        trovador::schema::of("search").unwrap()
    );
}

#[test]
fn the_cli_prints_the_published_schema() {
    let f = fixture();
    for name in trovador::schema::NAMES {
        assert_eq!(
            cli(&f, &["schema", name]),
            trovador::schema::of(name).unwrap(),
            "{name}"
        );
    }
    assert!(!cli_output(&f, &["schema", "nope"]).status.success());
}

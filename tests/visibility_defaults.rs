//! Configured defaults for the three groups search and list hide (`TROVADOR_INCLUDE_*`),
//! and explicit choices on each call overriding them in both directions.
//!
//! Before, `include_active`, `include_automated` and `include_subagents` were plain
//! booleans: a default could not be changed, and a caller could not ask to hide a group,
//! since `false` and "not given" were the same value.

mod common;

use std::process::Command;

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::Limits;
use trovador::index::SearchOpts;
use trovador::sessions::{self, Ctx, ListOpts};

const NEEDLE: &str = "zqvisibilityneedle";

/// One session in each group, all matching NEEDLE: A ordinary, B automated (`codex exec`),
/// C a subagent, D written moments ago.
fn fixture() -> Fixture {
    let f = Fixture::new();
    f.claude("-w", ID_A, &[claude_user("/w", NEEDLE)], 3_600);
    f.codex(
        ID_B,
        &[codex_meta("/w", json!("exec")), codex_user_item(NEEDLE)],
        3_600,
    );
    f.codex(
        ID_C,
        &[
            codex_meta(
                "/w",
                json!({"subagent": {"thread_spawn": {"agent_nickname": "Darwin"}}}),
            ),
            codex_user_item(NEEDLE),
        ],
        3_600,
    );
    f.claude("-w", ID_D, &[claude_user("/w", NEEDLE)], 0);
    f
}

fn ctx(f: &Fixture, all: bool) -> Ctx {
    f.ctx_with(Limits {
        active_window_s: 600,
        include_active: all,
        include_automated: all,
        include_subagents: all,
        ..limits()
    })
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn searched(ctx: &Ctx, opts: SearchOpts) -> Vec<String> {
    let r = sessions::search(
        ctx,
        &SearchOpts {
            query: NEEDLE.into(),
            ..opts
        },
    )
    .unwrap();
    sorted(r.hits.into_iter().map(|h| h.row.session_id).collect())
}

fn listed(ctx: &Ctx, opts: ListOpts) -> Vec<String> {
    let r = sessions::list(ctx, &opts).unwrap();
    sorted(r.sessions.into_iter().map(|s| s.session_id).collect())
}

fn ids(v: &[&str]) -> Vec<String> {
    sorted(v.iter().map(|s| s.to_string()).collect())
}

#[test]
fn search_hides_each_group_by_default_and_a_true_call_brings_it_back() {
    let f = fixture();
    let c = ctx(&f, false);
    assert_eq!(searched(&c, SearchOpts::default()), ids(&[ID_A]));
    for (opts, extra) in [
        (
            SearchOpts {
                include_active: Some(true),
                ..Default::default()
            },
            ID_D,
        ),
        (
            SearchOpts {
                include_automated: Some(true),
                ..Default::default()
            },
            ID_B,
        ),
        (
            SearchOpts {
                include_subagents: Some(true),
                ..Default::default()
            },
            ID_C,
        ),
    ] {
        assert_eq!(searched(&c, opts), ids(&[ID_A, extra]));
    }
}

#[test]
fn configured_defaults_include_each_group_and_a_false_call_hides_it() {
    let f = fixture();
    let c = ctx(&f, true);
    assert_eq!(
        searched(&c, SearchOpts::default()),
        ids(&[ID_A, ID_B, ID_C, ID_D])
    );
    for (opts, gone) in [
        (
            SearchOpts {
                include_active: Some(false),
                ..Default::default()
            },
            ID_D,
        ),
        (
            SearchOpts {
                include_automated: Some(false),
                ..Default::default()
            },
            ID_B,
        ),
        (
            SearchOpts {
                include_subagents: Some(false),
                ..Default::default()
            },
            ID_C,
        ),
    ] {
        let mut want = ids(&[ID_A, ID_B, ID_C, ID_D]);
        want.retain(|id| id != gone);
        assert_eq!(searched(&c, opts), want, "{gone} should be hidden");
    }
}

#[test]
fn a_hidden_count_and_note_follow_the_configured_default() {
    let f = fixture();
    let r = sessions::search(
        &ctx(&f, true),
        &SearchOpts {
            query: NEEDLE.into(),
            include_subagents: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        (r.hidden.active, r.hidden.automated, r.hidden.subagents),
        (0, 0, 1)
    );
    assert!(r.note.unwrap().contains("include_subagents"));
}

#[test]
fn list_follows_the_configured_defaults_and_explicit_choices() {
    let f = fixture();
    // list never hides active sessions.
    assert_eq!(
        listed(&ctx(&f, false), ListOpts::default()),
        ids(&[ID_A, ID_D])
    );
    assert_eq!(
        listed(&ctx(&f, true), ListOpts::default()),
        ids(&[ID_A, ID_B, ID_C, ID_D])
    );
    assert_eq!(
        listed(
            &ctx(&f, true),
            ListOpts {
                include_subagents: Some(false),
                include_automated: Some(false),
                ..Default::default()
            }
        ),
        ids(&[ID_A, ID_D])
    );
    assert_eq!(
        listed(
            &ctx(&f, false),
            ListOpts {
                include_subagents: Some(true),
                ..Default::default()
            }
        ),
        ids(&[ID_A, ID_C, ID_D])
    );
}

// --- both doors ------------------------------------------------------------

fn cli(f: &Fixture, env: &[(&str, &str)], args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(args)
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("TROVADOR_HOME", &f.paths.home)
        .env("TROVADOR_ACTIVE_WINDOW_S", "600")
        .env_remove("TROVADOR_INCLUDE_ACTIVE")
        .env_remove("TROVADOR_INCLUDE_AUTOMATED")
        .env_remove("TROVADOR_INCLUDE_SUBAGENTS")
        .envs(env.iter().copied())
        .output()
        .expect("run trovador");
    assert!(
        out.status.success(),
        "trovador {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is one JSON document")
}

fn hit_ids(v: &Value) -> Vec<String> {
    sorted(
        v["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["session_id"].as_str().unwrap().to_string())
            .collect(),
    )
}

fn session_ids(v: &Value) -> Vec<String> {
    sorted(
        v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["session_id"].as_str().unwrap().to_string())
            .collect(),
    )
}

const ALL_ON: &[(&str, &str)] = &[
    ("TROVADOR_INCLUDE_ACTIVE", "1"),
    ("TROVADOR_INCLUDE_AUTOMATED", "true"),
    ("TROVADOR_INCLUDE_SUBAGENTS", "yes"),
];

#[test]
fn the_cli_reads_the_defaults_from_the_environment_and_takes_both_flag_forms() {
    let f = fixture();
    assert_eq!(
        hit_ids(&cli(&f, &[], &["search", NEEDLE])),
        searched(&ctx(&f, false), SearchOpts::default())
    );
    assert_eq!(
        hit_ids(&cli(&f, ALL_ON, &["search", NEEDLE])),
        searched(&ctx(&f, true), SearchOpts::default())
    );
    assert_eq!(
        hit_ids(&cli(
            &f,
            ALL_ON,
            &["search", NEEDLE, "--include-subagents=false"]
        )),
        ids(&[ID_A, ID_B, ID_D])
    );
    assert_eq!(
        hit_ids(&cli(&f, &[], &["search", NEEDLE, "--include-automated"])),
        ids(&[ID_A, ID_B])
    );
    assert_eq!(
        session_ids(&cli(
            &f,
            ALL_ON,
            &["sessions", "list", "--include-automated=false"]
        )),
        ids(&[ID_A, ID_C, ID_D])
    );
}

#[test]
fn doctor_reports_the_configured_defaults() {
    let f = fixture();
    let report = cli(&f, ALL_ON, &["doctor"]);
    for k in ["include_active", "include_automated", "include_subagents"] {
        assert_eq!(report["limits"][k], json!(true), "{k}");
    }
    let report = cli(&f, &[], &["doctor"]);
    assert_eq!(report["limits"]["include_subagents"], json!(false));
}

#[tokio::test]
async fn the_mcp_tools_use_the_configured_default_unless_a_call_says_otherwise() {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(ctx(&f, true)));
    client.initialize().await;
    let all = client
        .call_tool_json("search", json!({"query": NEEDLE}))
        .await;
    assert_eq!(hit_ids(&all), ids(&[ID_A, ID_B, ID_C, ID_D]));
    let fewer = client
        .call_tool_json(
            "search",
            json!({"query": NEEDLE, "include_subagents": false, "include_active": false}),
        )
        .await;
    assert_eq!(hit_ids(&fewer), ids(&[ID_A, ID_B]));
    let list = client
        .call_tool_json(
            "sessions",
            json!({"action": "list", "include_automated": false}),
        )
        .await;
    assert_eq!(session_ids(&list), ids(&[ID_A, ID_C, ID_D]));
}

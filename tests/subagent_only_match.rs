//! Regression coverage for issue #15: default subagent exclusions cannot prove absence.

mod common;

use common::*;
use serde_json::{Value, json};
use std::process::Command;
use tower_mcp::TestClient;
use trovador::index::SearchOpts;
use trovador::sessions;

fn fixture() -> Fixture {
    let f = Fixture::new();
    let mut meta = codex_meta(
        "/w",
        json!({"subagent": {"thread_spawn": {"agent_nickname": "Darwin"}}}),
    );
    meta["payload"]["parent_thread_id"] = json!(ID_B);
    f.codex(ID_A, &[meta, codex_agent_item("quokkafailure")], 3_600);
    f
}

fn assert_hidden_match(r: &Value) {
    assert_eq!(r["sessions_matched"], 0);
    assert_eq!(r["returned"], 0);
    assert_eq!(r["exhaustive"], true);
    assert_eq!(r["absence_is_proof"], false);
    assert_eq!(
        r["hidden"],
        json!({"active": 0, "automated": 0, "subagents": 1, "excluded": 0, "unavailable": 0})
    );
    assert!(r["note"].as_str().unwrap().contains("include_subagents"));
}

#[test]
fn a_match_only_in_a_subagent_does_not_prove_absence_for_terms_fts_or_regex() {
    let f = fixture();
    for (fts, regex) in [(false, false), (true, false), (false, true)] {
        let opts = SearchOpts {
            query: "quokkafailure".into(),
            fts,
            regex,
            ..Default::default()
        };
        let r = sessions::search(&f.ctx(), &opts).unwrap();
        assert_hidden_match(&serde_json::to_value(r).unwrap());
        let included = sessions::search(
            &f.ctx(),
            &SearchOpts {
                include_subagents: Some(true),
                ..opts
            },
        )
        .unwrap();
        assert_eq!(included.sessions_matched, 1);
        assert_eq!(included.hits[0].row.session_id, ID_A);
        assert_eq!(
            serde_json::to_value(&included).unwrap()["hidden"]["subagents"],
            0
        );
        assert_eq!(included.absence_is_proof, None);
        assert!(included.note.is_none());
    }
}

#[tokio::test]
async fn cli_and_mcp_report_the_same_hidden_subagent_and_include_option() {
    let f = fixture();
    let mut client = TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    for included in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_trovador"));
        command.args(["search", "quokkafailure"]);
        if included {
            command.arg("--include-subagents");
        }
        let out = command
            .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
            .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
            .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
            .env("TROVADOR_HOME", &f.paths.home)
            .env("TROVADOR_ACTIVE_WINDOW_S", "0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let cli: Value = serde_json::from_slice(&out.stdout).unwrap();
        let mcp: Value = client
            .call_tool_json(
                "search",
                json!({"query": "quokkafailure", "include_subagents": included}),
            )
            .await;
        assert_eq!(cli["hidden"], mcp["hidden"]);
        if included {
            assert_eq!(cli["sessions_matched"], 1);
            assert_eq!(mcp["sessions_matched"], 1);
            assert_eq!(cli["hidden"]["subagents"], 0);
        } else {
            assert_hidden_match(&cli);
            assert_hidden_match(&mcp);
        }
    }
}

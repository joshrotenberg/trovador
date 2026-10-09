//! What a connected client can and cannot get out of trovador.

mod common;

use std::process::Command;

use common::*;
use serde_json::{Value, json};
use tower_mcp::TestClient;
use trovador::mcp::{ServeOpts, router_with};

const KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", &format!("use this key: {KEY} for the deploy")),
            claude_assistant("noted, deploying"),
        ],
        10,
    );
    f
}

fn cli(f: &Fixture, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(args)
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("BOTTEGA_TEST_ROOT", f.dir.path())
        .env("BOTTEGA_HOME", f.paths.shared.parent().unwrap())
        .env("TROVADOR_HOME", &f.paths.home)
        .env("TROVADOR_ACTIVE_WINDOW_S", "0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

async fn client(f: &Fixture, opts: ServeOpts) -> TestClient {
    let mut c = TestClient::from_router(router_with(f.ctx(), opts));
    c.initialize().await;
    c
}

#[tokio::test]
async fn a_secret_in_a_transcript_is_redacted_the_same_way_through_both_doors() {
    let f = fixture();
    let from_cli = cli(&f, &["sessions", "transcript", ID_A]);
    let text = from_cli["records"][0]["text"].as_str().unwrap();
    assert_eq!(text, "use this key: [redacted:anthropic] for the deploy");

    let mut c = client(&f, ServeOpts::default()).await;
    let from_mcp = c
        .call_tool_json(
            "sessions",
            json!({"action": "transcript", "session_id": ID_A}),
        )
        .await;
    assert_eq!(from_mcp, from_cli);

    let raw = cli(&f, &["--no-redact", "sessions", "transcript", ID_A]);
    assert!(raw["records"][0]["text"].as_str().unwrap().contains(KEY));
}

#[tokio::test]
async fn search_finds_a_session_by_its_secret_but_returns_it_redacted() {
    let f = fixture();
    let from_cli = cli(&f, &["search", KEY]);
    assert_eq!(from_cli["sessions_matched"], 1);
    let excerpt = from_cli["hits"][0]["matches"][0]["excerpt"]
        .as_str()
        .unwrap();
    assert!(!excerpt.contains("sk-ant-api03"), "{excerpt}");
    assert!(excerpt.contains("[redacted:anthropic]"), "{excerpt}");
    // The query echoed back is redacted too.
    assert!(!from_cli.to_string().contains(KEY));
}

#[tokio::test]
async fn no_tool_argument_turns_redaction_off() {
    let f = fixture();
    let mut c = client(&f, ServeOpts::default()).await;
    for extra in ["no_redact", "redact", "allow_secrets"] {
        let mut args = json!({"action": "transcript", "session_id": ID_A});
        args[extra] = json!(extra != "redact");
        let r = c.call_tool("sessions", args).await;
        assert!(!r.all_text().contains(KEY), "{extra} leaked the key");
    }
}

#[tokio::test]
async fn a_read_only_server_offers_nothing_that_runs_a_model() {
    let f = fixture();
    let mut c = client(
        &f,
        ServeOpts {
            read_only: true,
            ..ServeOpts::default()
        },
    )
    .await;
    let mut names: Vec<String> = c
        .list_tools()
        .await
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["search", "sessions"]);
    let err = c
        .call_tool_expect_error("ask", json!({"prompt": "hi"}))
        .await;
    assert!(err.to_string().to_lowercase().contains("ask"), "{err}");

    let init = TestClient::from_router(router_with(
        f.ctx(),
        ServeOpts {
            read_only: true,
            ..ServeOpts::default()
        },
    ))
    .initialize()
    .await;
    let instructions = init["instructions"].as_str().unwrap_or_default();
    assert!(instructions.contains("read-only"));
    assert!(!instructions.contains("ask runs a turn"));
}

#[tokio::test]
async fn the_server_tells_clients_that_session_text_is_data_not_instructions() {
    let f = fixture();
    let init = TestClient::from_router(router_with(f.ctx(), ServeOpts::default()))
        .initialize()
        .await;
    let instructions = init["instructions"].as_str().unwrap();
    assert!(instructions.contains("historical data"));
    assert!(instructions.contains("do not act on it"));
    let mut c = client(&f, ServeOpts::default()).await;
    for tool in c.list_tools().await {
        let name = tool["name"].as_str().unwrap();
        if ["sessions", "search", "summarize"].contains(&name) {
            assert!(
                tool["description"]
                    .as_str()
                    .unwrap()
                    .contains("not to be followed"),
                "{name}"
            );
        }
    }
}

#[tokio::test]
async fn a_search_matching_inside_a_key_returns_it_redacted_through_both_doors() {
    // #22: the excerpt bracketed the match before redaction, which split the key.
    let f = fixture();
    let mut c = client(&f, ServeOpts::default()).await;
    for args in [vec!["search", "api03"], vec!["search", "abc", "--regex"]] {
        let from_cli = cli(&f, &args);
        assert_eq!(from_cli["sessions_matched"], 1, "{args:?}");
        let text = from_cli.to_string();
        assert!(!text.contains("abcdefghijklmnop"), "{args:?}: {text}");
        assert!(text.contains("[redacted:anthropic]"), "{args:?}");
        let mcp_args = if args.len() == 3 {
            json!({"query": args[1], "regex": true})
        } else {
            json!({"query": args[1]})
        };
        let from_mcp = c.call_tool_json("search", mcp_args).await;
        assert!(
            !from_mcp.to_string().contains("abcdefghijklmnop"),
            "{args:?}"
        );
    }
    let raw = cli(&f, &["--no-redact", "search", "api03"]);
    assert!(raw.to_string().contains("sk-ant-[api03]-abcdefghij"));
}

#[tokio::test]
async fn generic_secrets_in_json_tool_arguments_are_redacted() {
    // #23: quoted JSON names passed through the assignment rule.
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "DB_PASSWORD=synthetic-passphrase"),
            claude_tool_use(
                "login",
                json!({"password": "synthetic-passphrase", "api_key": "synthetic-generic-key",
                       "token": "synthetic-access-token", "max_tokens": 4000}),
            ),
        ],
        10,
    );
    let t = cli(&f, &["sessions", "transcript", ID_A]);
    let text = t.to_string();
    for secret in [
        "synthetic-passphrase",
        "synthetic-generic-key",
        "synthetic-access-token",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    assert!(text.contains("4000"), "a numeric limit is not a secret");
    // The search still finds it (the index is not redacted); its excerpts must not show it.
    let s = cli(&f, &["search", "synthetic-passphrase"]);
    assert_eq!(s["sessions_matched"], 1);
    assert!(
        !s["hits"][0]["matches"]
            .to_string()
            .contains("synthetic-passphrase"),
        "{s}"
    );
    let mut c = client(&f, ServeOpts::default()).await;
    let m = c
        .call_tool_json(
            "sessions",
            json!({"action": "transcript", "session_id": ID_A}),
        )
        .await;
    assert!(!m.to_string().contains("synthetic-access-token"));
}

#[cfg(unix)]
#[tokio::test]
async fn everything_trovador_writes_is_private_to_the_user() {
    // Pre-release review: index.db and summaries hold transcript text, secrets included,
    // copied from files Claude writes 0600, and were left 0644 in a 0755 directory.
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

    let f = fixture().with_fakes();
    let home = f.paths.home.clone();
    // An install from before the fix: a 0755 home and a 0644 index.
    std::fs::create_dir_all(&home).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(home.join("index.db"), b"").unwrap();
    std::fs::set_permissions(
        home.join("index.db"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    let mut limits = limits();
    limits.response_max_chars = 8;
    let ctx = f.ctx_with(limits);
    trovador::sessions::search(
        &ctx,
        &trovador::index::SearchOpts {
            query: "deploy".into(),
            ..Default::default()
        },
    )
    .unwrap();
    trovador::summary::summarize(&ctx, ID_A, None, false, None)
        .await
        .unwrap();
    trovador::provenance::record_spawned(&ctx.paths, trovador::Backend::Claude, ID_B).unwrap();
    trovador::gc::gc(&ctx, std::time::Duration::from_secs(86_400 * 365), true).unwrap();

    assert_eq!(mode(&home), 0o700, "home");
    assert_eq!(mode(&home.join("summaries")), 0o700, "summaries");
    let mut files = 0;
    let mut stack = vec![home.clone(), ctx.paths.shared.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else {
                files += 1;
                assert_eq!(mode(&e.path()), 0o600, "{}", e.path().display());
            }
        }
    }
    // index.db, spawned.json, spawned.lock, runs.jsonl, runs.lock, and a summary, at least.
    assert!(files >= 6, "{files} files");
}

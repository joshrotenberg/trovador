//! `trovador init` against fake `claude` and `codex` executables that log their arguments
//! and answer the listing commands from files the test writes.

#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::*;
use serde_json::{Value, json};
use trovador::init::{self, Host, InitOpts, InitReport};

/// Answers `plugin marketplace list`, `plugin list`, and `mcp list|get` from files beside
/// it, fails any command whose arguments contain the text in `<name>.fail`, and logs every
/// call. Anything else succeeds silently.
const FAKE: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
name="$(basename "$0")"
echo "$*" >> "$dir/$name.log"
if [ -f "$dir/$name.fail" ] && echo "$*" | grep -q "$(cat "$dir/$name.fail")"; then
  echo "refused: $*" >&2; exit 1
fi
case "$*" in
  "plugin marketplace list --json") cat "$dir/$name.markets" ;;
  "plugin list --json") cat "$dir/$name.plugins" ;;
  "mcp list --json") cat "$dir/$name.mcp" ;;
  "mcp get trovador") [ -f "$dir/$name.mcpget" ] || exit 1 ;;
esac
exit 0
"#;

struct Hosts {
    bin: PathBuf,
}

impl Hosts {
    fn new(f: &Fixture) -> Self {
        let bin = f.dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        for name in ["claude", "codex"] {
            let p = bin.join(name);
            fs::write(&p, FAKE).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let h = Self { bin };
        h.set("claude", "markets", "[]");
        h.set("claude", "plugins", "[]");
        h.set("codex", "markets", r#"{"marketplaces": []}"#);
        h.set("codex", "plugins", r#"{"marketplaces": []}"#);
        h.set("codex", "mcp", "[]");
        h
    }

    fn set(&self, host: &str, what: &str, body: &str) {
        fs::write(self.bin.join(format!("{host}.{what}")), body).unwrap();
    }

    fn calls(&self, host: &str) -> Vec<String> {
        fs::read_to_string(self.bin.join(format!("{host}.log")))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Calls that change host state, leaving out the read-only inspection.
    fn changes(&self, host: &str) -> Vec<String> {
        self.calls(host)
            .into_iter()
            .filter(|c| !c.ends_with("list --json") && !c.starts_with("mcp "))
            .collect()
    }

    fn opts(&self, f: &Fixture, host: Host, dry_run: bool) -> InitOpts {
        InitOpts {
            dry_run,
            host,
            binary: Some(fake_trovador(f)),
            claude_bin: Some(self.bin.join("claude")),
            codex_bin: Some(self.bin.join("codex")),
        }
    }
}

fn fake_trovador(f: &Fixture) -> PathBuf {
    let p = f.dir.path().join("trovador-bin");
    if !p.exists() {
        fs::write(&p, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::canonicalize(p).unwrap()
}

fn json_file(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn steps(r: &InitReport) -> Vec<String> {
    r.completed
        .iter()
        .map(|c| format!("{} {}", c.host, c.step))
        .collect()
}

#[test]
fn a_dry_run_lists_files_and_commands_and_writes_nothing() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    let r = init::init(&f.paths, &h.opts(&f, Host::Both, true));
    assert!(r.ok, "{:?}", r.issues);
    assert_eq!(r.status, "planned");
    assert!(!init::install_dir(&f.paths).exists());
    assert!(
        r.files
            .iter()
            .any(|p| p.ends_with("plugins/trovador/.mcp.json"))
    );
    let planned: Vec<_> = r.commands.iter().map(|c| c.step.as_str()).collect();
    assert_eq!(
        planned,
        ["add_marketplace", "install", "add_marketplace", "install"]
    );
    assert!(h.changes("claude").is_empty() && h.changes("codex").is_empty());
}

#[test]
fn a_first_install_writes_the_marketplace_and_registers_with_both_hosts() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    let r = init::init(&f.paths, &h.opts(&f, Host::Both, false));
    assert!(r.ok, "{r:?}");
    assert_eq!(r.status, "installed");
    let dir = init::install_dir(&f.paths);
    let d = dir.display().to_string();
    assert_eq!(
        h.changes("claude"),
        [
            format!("plugin marketplace add {d} --scope user"),
            "plugin install trovador@trovador-local --scope user".to_string(),
        ]
    );
    assert_eq!(
        h.changes("codex"),
        [
            format!("plugin marketplace add {d}"),
            "plugin add trovador@trovador-local".to_string(),
        ]
    );

    // Both hosts start the binary by absolute path: a desktop app may lack ~/.cargo/bin.
    let bin = fake_trovador(&f).display().to_string();
    let mcp = json_file(&dir.join("plugins/trovador/.mcp.json"));
    assert_eq!(mcp["mcpServers"]["trovador"]["command"], bin);
    assert_eq!(mcp["mcpServers"]["trovador"]["args"], json!(["serve"]));
    let hooks = json_file(&dir.join("plugins/trovador/hooks/hooks.json"));
    let hook = hooks["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(hook.starts_with(&format!("\"{bin}\" hook session-start --provider claude")));
    assert!(
        hook.ends_with("|| true"),
        "a hook must never block a session"
    );

    for manifest in [".claude-plugin/plugin.json", ".codex-plugin/plugin.json"] {
        let m = json_file(&dir.join("plugins/trovador").join(manifest));
        assert_eq!(m["name"], "trovador");
        assert_eq!(m["version"], env!("CARGO_PKG_VERSION"));
    }
    let codex = json_file(&dir.join("plugins/trovador/.codex-plugin/plugin.json"));
    assert_eq!(codex["mcpServers"], "./.mcp.json");
    assert_eq!(codex["skills"], "./skills/");
    for market in [
        ".claude-plugin/marketplace.json",
        ".agents/plugins/marketplace.json",
    ] {
        assert_eq!(json_file(&dir.join(market))["name"], init::MARKETPLACE);
    }
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn a_re_run_updates_the_plugin_and_drops_files_a_release_no_longer_ships() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    assert!(init::init(&f.paths, &h.opts(&f, Host::Both, false)).ok);
    let dir = init::install_dir(&f.paths);
    let stale = dir.join("plugins/trovador/skills/old/SKILL.md");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, "gone").unwrap();

    let d = dir.display().to_string();
    h.set(
        "claude",
        "markets",
        &json!([{"name": "trovador-local", "path": d}]).to_string(),
    );
    h.set(
        "claude",
        "plugins",
        &json!([{"id": "trovador@trovador-local"}]).to_string(),
    );
    h.set(
        "codex",
        "markets",
        &json!({"marketplaces": [{"name": "trovador-local", "root": d}]}).to_string(),
    );
    h.set(
        "codex",
        "plugins",
        &json!({"marketplaces": [{"plugins": [
        {"pluginId": "trovador@trovador-local", "installed": true}]}]})
        .to_string(),
    );
    fs::remove_file(h.bin.join("claude.log")).unwrap();
    fs::remove_file(h.bin.join("codex.log")).unwrap();

    let r = init::init(&f.paths, &h.opts(&f, Host::Both, false));
    assert!(r.ok, "{r:?}");
    assert_eq!(steps(&r), ["claude update", "codex update"]);
    assert_eq!(
        h.changes("claude"),
        ["plugin update trovador@trovador-local --scope user"]
    );
    assert!(!stale.exists());
    assert!(
        dir.join("plugins/trovador/skills/recall/SKILL.md")
            .is_file()
    );
}

#[test]
fn the_marketplace_registered_at_another_path_stops_init_before_anything_changes() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    h.set(
        "codex",
        "markets",
        &json!({"marketplaces": [{"name": "trovador-local", "root": "/elsewhere"}]}).to_string(),
    );
    let r = init::init(&f.paths, &h.opts(&f, Host::Both, false));
    assert!(!r.ok);
    assert_eq!(r.status, "not_ready");
    assert!(
        r.issues
            .iter()
            .any(|i| i.contains("/elsewhere") && i.contains("marketplace remove")),
        "{:?}",
        r.issues
    );
    assert!(!init::install_dir(&f.paths).exists());
    assert!(h.changes("claude").is_empty() && h.changes("codex").is_empty());
}

#[test]
fn a_directory_init_did_not_write_is_refused() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    let dir = init::install_dir(&f.paths);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("notes.txt"), "mine").unwrap();
    let r = init::init(&f.paths, &h.opts(&f, Host::Both, false));
    assert!(!r.ok);
    assert!(
        r.issues
            .iter()
            .any(|i| i.contains("not written by trovador init"))
    );
    assert_eq!(fs::read_to_string(dir.join("notes.txt")).unwrap(), "mine");
}

#[test]
fn a_failing_host_command_stops_the_run_and_names_the_step() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    h.set("claude", "fail", "plugin install");
    let r = init::init(&f.paths, &h.opts(&f, Host::Both, false));
    assert!(!r.ok);
    assert_eq!(r.status, "failed");
    let failed = r.failed.unwrap();
    assert_eq!(
        (failed.command.host.as_str(), failed.command.step.as_str()),
        ("claude", "install")
    );
    assert!(failed.error.contains("refused"), "{}", failed.error);
    assert!(
        h.changes("codex").is_empty(),
        "codex runs after claude, so it never started"
    );
}

#[test]
fn under_both_a_missing_host_is_skipped_but_a_named_one_is_an_issue() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    let missing = InitOpts {
        codex_bin: Some(f.dir.path().join("no-codex")),
        ..h.opts(&f, Host::Codex, true)
    };
    let r = init::init(&f.paths, &missing);
    assert!(!r.ok);
    assert!(r.issues.iter().any(|i| i.starts_with("codex not found")));

    let claude_only = init::init(&f.paths, &h.opts(&f, Host::Claude, true));
    assert!(claude_only.ok);
    assert!(claude_only.commands.iter().all(|c| c.host == "claude"));
    assert!(claude_only.codex_hook.is_none());
}

#[test]
fn a_hand_configured_trovador_server_is_reported_as_a_duplicate() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    h.set("claude", "mcpget", "");
    h.set(
        "codex",
        "mcp",
        &json!([{"name": "trovador", "transport": {"command": "/old/trovador"}}]).to_string(),
    );
    let r = init::init(&f.paths, &h.opts(&f, Host::Both, true));
    assert!(r.ok, "a duplicate is a note, not a reason to stop");
    for host in ["claude", "codex"] {
        assert!(
            r.notes
                .iter()
                .any(|n| n.starts_with(host) && n.contains(&format!("{host} mcp remove trovador"))),
            "{host}: {:?}",
            r.notes
        );
    }
}

#[test]
fn the_codex_hook_is_reported_and_never_written() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    let hooks = f.paths.codex_archive.parent().unwrap().join("hooks.json");
    let r = init::init(&f.paths, &h.opts(&f, Host::Codex, false));
    let hook = r.codex_hook.unwrap();
    assert!(!hook.present);
    let add = hook.add.unwrap().to_string();
    assert!(add.contains("hook session-start --provider codex"), "{add}");
    assert!(!hooks.exists(), "init never writes under ~/.codex");

    // An entry written before the flag was renamed from --backend still counts.
    fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    fs::write(&hooks, json!({"hooks": {"SessionStart": [{"hooks": [
        {"type": "command", "command": "/x/trovador hook session-start --backend codex || true"}]}]}})
        .to_string()).unwrap();
    let before = fs::read(&hooks).unwrap();
    let r = init::init(&f.paths, &h.opts(&f, Host::Codex, false));
    let hook = r.codex_hook.unwrap();
    assert!(hook.present && hook.add.is_none());
    assert_eq!(fs::read(&hooks).unwrap(), before);
}

#[test]
fn the_cli_prints_the_same_plan_as_the_library() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    let opts = h.opts(&f, Host::Both, true);
    let lib = serde_json::to_value(init::init(&f.paths, &opts)).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(["init", "--dry-run", "--binary"])
        .arg(opts.binary.as_ref().unwrap())
        .arg("--claude-bin")
        .arg(opts.claude_bin.as_ref().unwrap())
        .arg("--codex-bin")
        .arg(opts.codex_bin.as_ref().unwrap())
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("TROVADOR_HOME", &f.paths.home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cli: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(cli, lib);
}

fn cli_init(f: &Fixture, h: &Hosts, progress: &str) -> std::process::Output {
    let opts = h.opts(f, Host::Both, false);
    std::process::Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(["init", "--binary"])
        .arg(opts.binary.as_ref().unwrap())
        .arg("--claude-bin")
        .arg(opts.claude_bin.as_ref().unwrap())
        .arg("--codex-bin")
        .arg(opts.codex_bin.as_ref().unwrap())
        .env("TROVADOR_CLAUDE_ARCHIVE", &f.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &f.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &f.paths.codex_archived)
        .env("TROVADOR_HOME", &f.paths.home)
        .env("TROVADOR_PROGRESS", progress)
        .env_remove("TROVADOR_LOG")
        .output()
        .unwrap()
}

#[test]
fn the_cli_names_each_host_step_on_stderr_as_it_runs() {
    // init runs host commands for seconds; without these lines it looked hung.
    let f = Fixture::new();
    let h = Hosts::new(&f);
    let out = cli_init(&f, &h, "1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let report: Value = serde_json::from_slice(&out.stdout).expect("stdout is one JSON document");
    assert_eq!(report["status"], "installed");
    let dir = init::install_dir(&f.paths);
    let want = [
        "trovador: claude: checking registered marketplaces, plugins and MCP servers".to_string(),
        "trovador: codex: checking registered marketplaces, plugins and MCP servers".to_string(),
        format!("trovador: writing the plugin to {}", dir.display()),
        format!(
            "trovador: claude: add marketplace (claude plugin marketplace add {} --scope user)",
            dir.display()
        ),
        "trovador: claude: install (claude plugin install trovador@trovador-local --scope user)"
            .to_string(),
        "trovador: codex: install (codex plugin add trovador@trovador-local)".to_string(),
        "trovador: installed; restart Claude Code and Codex to load it".to_string(),
    ];
    let lines: Vec<&str> = stderr.lines().collect();
    for w in &want {
        assert!(lines.contains(&w.as_str()), "no {w:?} in\n{stderr}");
    }
    let done = lines
        .iter()
        .filter(|l| l.contains(" done in ") && l.ends_with(" s"))
        .count();
    assert_eq!(done, 4, "one timing line per change\n{stderr}");
}

#[test]
fn a_failing_host_step_is_a_warning_even_with_progress_off() {
    let f = Fixture::new();
    let h = Hosts::new(&f);
    fs::write(h.bin.join("codex.fail"), "plugin add").unwrap();
    let out = cli_init(&f, &h, "0");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("WARN"), "{stderr}");
    assert!(stderr.contains("host command failed"), "{stderr}");
    assert!(stderr.contains("refused: plugin add"), "{stderr}");
    assert!(
        !stderr.contains("trovador: "),
        "no progress lines when off\n{stderr}"
    );
}

#[test]
fn a_hook_line_written_with_the_old_backend_flag_still_names_the_session() {
    // Hook command lines live in host configuration init does not rewrite. Under
    // `|| true`, a flag the parser rejected would silently drop the session's
    // self-exclusion.
    let payload =
        r#"{"session_id":"01a007a0-134e-70e2-84e5-a19ae6996317","transcript_path":"/tmp/x.jsonl"}"#;
    let hook = |flag: &str| {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_trovador"))
            .args(["hook", "session-start", flag, "codex"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), payload.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{flag}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let new = hook("--provider");
    assert!(new.contains("codex session 01a007a0"), "{new}");
    assert_eq!(hook("--backend"), new);
}

#[tokio::test]
async fn the_skill_names_only_tools_and_actions_the_server_offers() {
    // The skill is what an agent reads first; a tool or action it names that does not
    // exist sends the agent looking for something that is not there.
    let skill = include_str!("../plugin/trovador/skills/recall/SKILL.md");
    let f = Fixture::new();
    let mut client = tower_mcp::TestClient::from_router(trovador::mcp::router(f.ctx()));
    client.initialize().await;
    let tools = client.list_tools().await;
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for tool in ["search", "sessions"] {
        assert!(names.contains(&tool) && skill.contains(&format!("`{tool}`")));
    }
    let sessions = tools.iter().find(|t| t["name"] == "sessions").unwrap();
    let schema = sessions["inputSchema"].to_string();
    for action in ["list", "show", "transcript", "text"] {
        assert!(skill.contains(&format!("action={action}")), "{action}");
        assert!(schema.contains(&format!("\"{action}\"")), "{action}");
    }
    let session_params = sessions["inputSchema"]["properties"].as_object().unwrap();
    assert!(session_params.contains_key("expected_revision"));
    assert!(skill.contains("`expected_revision`"));
    let search = tools.iter().find(|t| t["name"] == "search").unwrap();
    let params = search["inputSchema"]["properties"].as_object().unwrap();
    for p in [
        "project",
        "provider",
        "since",
        "until",
        "regex",
        "fts",
        "exclude_session",
        "include_active",
        "include_automated",
        "include_subagents",
    ] {
        assert!(params.contains_key(p), "{p}");
        assert!(skill.contains(&format!("`{p}")), "{p}");
    }
    assert!(!skill.contains("summarize`") && !skill.contains("tosi"));
}

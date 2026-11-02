//! `trovador init`: installs the plugin into Claude Code and Codex.
//!
//! The plugin is written as a local marketplace under trovador's own home
//! (`~/.trovador/plugin`), then registered through each host's own CLI (`claude plugin
//! ...`, `codex plugin ...`). trovador never edits a host's configuration files, so
//! install, update, and removal stay with the hosts, and removing `~/.trovador` leaves
//! nothing of trovador's behind except the registrations the hosts keep.
//!
//! The plan follows due's and hexpm-mcp's installers, which found the same failure modes:
//! a directory that is not ours is refused, a marketplace of the same name registered
//! elsewhere stops the run before anything is installed, and a failing host command
//! stops the run and names the step.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::paths::Paths;

pub const MARKETPLACE: &str = "trovador-local";
pub const PLUGIN: &str = "trovador";
const PLUGIN_ID: &str = "trovador@trovador-local";
/// Marks a directory as one init wrote, so a re-run may replace what is in it.
const MARKER: &str = ".trovador-install.json";
const HOST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_STDERR: usize = 2_000;

const CLAUDE_MANIFEST: &str = include_str!("../plugin/trovador/.claude-plugin/plugin.json");
const CODEX_MANIFEST: &str = include_str!("../plugin/trovador/.codex-plugin/plugin.json");
const SKILL: &str = include_str!("../plugin/trovador/skills/recall/SKILL.md");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Host {
    Both,
    Claude,
    Codex,
}

impl Host {
    fn includes(self, name: &str) -> bool {
        matches!(
            (self, name),
            (Host::Both, _) | (Host::Claude, "claude") | (Host::Codex, "codex")
        )
    }
}

#[derive(Clone, Debug)]
pub struct InitOpts {
    pub dry_run: bool,
    pub host: Host,
    /// The trovador the plugin starts. Defaults to the running executable.
    pub binary: Option<PathBuf>,
    pub claude_bin: Option<PathBuf>,
    pub codex_bin: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HostCommand {
    pub host: String,
    pub step: String,
    pub executable: PathBuf,
    pub args: Vec<String>,
}

/// Whether Codex will run the SessionStart hook. Codex lists `plugin_hooks` as removed
/// (0.162 and 0.165, bottega #52), so a plugin cannot carry it; only `~/.codex/hooks.json`
/// can, and trovador does not write there.
#[derive(Clone, Debug, Serialize)]
pub struct CodexHook {
    pub path: PathBuf,
    pub present: bool,
    /// The entry to add under `hooks.SessionStart`, when it is not present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InitReport {
    /// `planned` (dry run), `installed`, `not_ready`, or `failed`.
    pub status: &'static str,
    pub ok: bool,
    pub install_dir: PathBuf,
    pub binary: PathBuf,
    pub files: Vec<PathBuf>,
    pub commands: Vec<HostCommand>,
    pub completed: Vec<HostCommand>,
    /// What stops init. Nothing is written or run while there are any.
    pub issues: Vec<String>,
    /// What init cannot fix and the user may want to: duplicates, skipped hosts.
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_hook: Option<CodexHook>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<Failed>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Failed {
    pub command: HostCommand,
    pub error: String,
}

pub fn install_dir(paths: &Paths) -> PathBuf {
    paths.home.join("plugin")
}

fn with_version(manifest: &str) -> Value {
    let mut v: Value = serde_json::from_str(manifest).expect("bundled manifest is JSON");
    v["version"] = json!(env!("CARGO_PKG_VERSION"));
    v
}

fn pretty(v: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(v).expect("serializable");
    bytes.push(b'\n');
    bytes
}

/// Every file of the marketplace, relative to the install directory. `.mcp.json` and the
/// hook name the binary by absolute path: a host started from the desktop may not have
/// `~/.cargo/bin` on its PATH.
fn package(binary: &Path) -> Vec<(String, Vec<u8>)> {
    let bin = binary.display().to_string();
    let claude = with_version(CLAUDE_MANIFEST);
    let description = claude["description"].clone();
    let hook = format!("\"{bin}\" hook session-start --backend claude || true");
    vec![
        (
            "plugins/trovador/.claude-plugin/plugin.json".into(),
            pretty(&claude),
        ),
        (
            "plugins/trovador/.codex-plugin/plugin.json".into(),
            pretty(&with_version(CODEX_MANIFEST)),
        ),
        (
            "plugins/trovador/skills/recall/SKILL.md".into(),
            SKILL.as_bytes().to_vec(),
        ),
        (
            "plugins/trovador/.mcp.json".into(),
            pretty(&json!({"mcpServers": {"trovador": {
                "type": "stdio", "command": bin, "args": ["serve"]}}})),
        ),
        (
            "plugins/trovador/hooks/hooks.json".into(),
            pretty(&json!({
                "description": "trovador: tell the session its own id, so search can leave out exactly this conversation",
                "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": hook, "timeout": 5}]}]}
            })),
        ),
        (
            ".claude-plugin/marketplace.json".into(),
            pretty(&json!({
                "name": MARKETPLACE, "owner": {"name": "Josh Rotenberg"},
                "description": "The trovador plugin, written by trovador init",
                "plugins": [{"name": PLUGIN, "source": "./plugins/trovador",
                    "version": env!("CARGO_PKG_VERSION"), "description": description}]
            })),
        ),
        (
            ".agents/plugins/marketplace.json".into(),
            pretty(&json!({
                "name": MARKETPLACE, "interface": {"displayName": "trovador local"},
                "plugins": [{"name": PLUGIN,
                    "source": {"source": "local", "path": "./plugins/trovador"},
                    "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
                    "category": "Developer Tools"}]
            })),
        ),
    ]
}

/// The first `name` on PATH, or the configured path when there is one.
fn find(configured: Option<&Path>, name: &str) -> Option<PathBuf> {
    if let Some(p) = configured {
        return p.is_file().then(|| p.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Runs a host command with no stdin and a deadline. A host that hangs on a prompt must
/// not hang init.
fn run(exe: &Path, args: &[String]) -> Result<Output, String> {
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let (mut o, mut e) = (String::new(), String::new());
        let _ = out.read_to_string(&mut o);
        let _ = err.read_to_string(&mut e);
        (o, e)
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if start.elapsed() > HOST_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "did not finish within {} s",
                HOST_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stderr) = reader.join().unwrap_or_default();
    Ok(Output {
        ok: status.success(),
        stdout,
        stderr,
    })
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Where a host has `trovador-local` registered, if it does.
fn registered_root(host: &str, listing: &Value) -> Option<PathBuf> {
    let entries = match host {
        "claude" => listing.as_array()?,
        _ => listing.get("marketplaces")?.as_array()?,
    };
    entries
        .iter()
        .find(|m| m["name"] == MARKETPLACE)
        .and_then(|m| {
            ["root", "path", "installLocation"]
                .iter()
                .find_map(|k| m[*k].as_str())
        })
        .map(PathBuf::from)
}

fn installed(host: &str, listing: &Value) -> bool {
    match host {
        "claude" => listing
            .as_array()
            .is_some_and(|a| a.iter().any(|p| p["id"] == PLUGIN_ID)),
        // Codex nests plugins under their marketplace; any object carrying our id counts.
        _ => {
            fn walk(v: &Value) -> bool {
                match v {
                    Value::Object(m) => {
                        (m.get("pluginId") == Some(&json!(PLUGIN_ID))
                            && m.get("installed") == Some(&json!(true)))
                            || m.values().any(walk)
                    }
                    Value::Array(a) => a.iter().any(walk),
                    _ => false,
                }
            }
            walk(listing)
        }
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

/// Reads `~/.codex/hooks.json` only to report on it.
fn codex_hook(paths: &Paths, binary: &Path) -> CodexHook {
    let codex_home = paths
        .codex_archive
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let path = codex_home.join("hooks.json");
    let present = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["hooks"]["SessionStart"].as_array().cloned())
        .is_some_and(|entries| {
            entries.iter().any(|e| {
                e["hooks"].as_array().is_some_and(|hs| {
                    hs.iter().any(|h| {
                        h["command"]
                            .as_str()
                            .is_some_and(|c| c.contains("hook session-start"))
                    })
                })
            })
        });
    CodexHook {
        add: (!present).then(|| {
            json!({"hooks": [{"type": "command", "command":
                format!("{} hook session-start --backend codex || true", binary.display())}]})
        }),
        path,
        present,
    }
}

pub fn init(paths: &Paths, opts: &InitOpts) -> InitReport {
    let dir = install_dir(paths);
    let mut report = InitReport {
        status: "planned",
        ok: false,
        install_dir: dir.clone(),
        binary: PathBuf::new(),
        files: Vec::new(),
        commands: Vec::new(),
        completed: Vec::new(),
        issues: Vec::new(),
        notes: Vec::new(),
        codex_hook: None,
        failed: None,
    };
    let binary = match &opts.binary {
        Some(b) => Some(b.clone()),
        None => std::env::current_exe().ok(),
    }
    .map(|b| fs::canonicalize(&b).unwrap_or(b));
    match binary {
        Some(b) if b.is_file() => report.binary = b,
        other => report.issues.push(format!(
            "the trovador binary {} is not a file; pass --binary",
            other.unwrap_or_default().display()
        )),
    }
    let files = package(&report.binary);
    report.files = files.iter().map(|(p, _)| dir.join(p)).collect();
    if let Ok(mut entries) = fs::read_dir(&dir)
        && entries.next().is_some()
        && !dir.join(MARKER).is_file()
    {
        report.issues.push(format!(
            "{} is not empty and was not written by trovador init; move it aside",
            dir.display()
        ));
    }

    for (name, configured) in [
        ("claude", opts.claude_bin.as_deref()),
        ("codex", opts.codex_bin.as_deref()),
    ] {
        if !opts.host.includes(name) {
            continue;
        }
        let Some(exe) = find(configured, name) else {
            if opts.host == Host::Both && configured.is_none() {
                report.notes.push(format!("{name} is not on PATH; skipped"));
            } else {
                report.issues.push(format!(
                    "{name} not found{}",
                    configured
                        .map(|p| format!(" at {}", p.display()))
                        .unwrap_or_default()
                ));
            }
            continue;
        };
        plan_host(&mut report, name, &exe, &dir);
    }
    if opts.host.includes("codex") && report.commands.iter().any(|c| c.host == "codex") {
        report.codex_hook = Some(codex_hook(paths, &report.binary));
    }

    if !report.issues.is_empty() {
        report.status = "not_ready";
        return report;
    }
    if opts.dry_run {
        report.ok = true;
        return report;
    }
    if let Err(e) = write_package(&dir, &files) {
        report.status = "failed";
        report.issues.push(e);
        return report;
    }
    for command in report.commands.clone() {
        match run(&command.executable, &command.args) {
            Ok(out) if out.ok => report.completed.push(command),
            Ok(out) => {
                let mut error = out.stderr.trim().to_string();
                error.truncate(MAX_STDERR);
                report.status = "failed";
                report.failed = Some(Failed { command, error });
                return report;
            }
            Err(error) => {
                report.status = "failed";
                report.failed = Some(Failed { command, error });
                return report;
            }
        }
    }
    report.status = "installed";
    report.ok = true;
    report
}

/// Inspects one host with read-only commands and adds the steps that install or update.
fn plan_host(report: &mut InitReport, name: &str, exe: &Path, dir: &Path) {
    let cmd = |step: &str, a: &[&str]| HostCommand {
        host: name.into(),
        step: step.into(),
        executable: exe.to_path_buf(),
        args: args(a),
    };
    let json_of = |a: &[&str]| -> Result<Value, String> {
        let out = run(exe, &args(a))?;
        if !out.ok {
            return Err(out.stderr.trim().to_string());
        }
        serde_json::from_str(&out.stdout).map_err(|e| format!("not JSON: {e}"))
    };
    let markets = match json_of(&["plugin", "marketplace", "list", "--json"]) {
        Ok(v) => v,
        Err(e) => {
            report
                .issues
                .push(format!("{name} plugin marketplace list failed: {e}"));
            return;
        }
    };
    let dir_text = dir.display().to_string();
    match registered_root(name, &markets) {
        Some(root) if !same_dir(&root, dir) => {
            report.issues.push(format!(
                "{name} has {MARKETPLACE} registered at {}; remove it with `{name} plugin marketplace remove {MARKETPLACE}` and run init again",
                root.display()
            ));
            return;
        }
        Some(_) => {}
        None => {
            let mut add = vec!["plugin", "marketplace", "add", dir_text.as_str()];
            if name == "claude" {
                add.extend(["--scope", "user"]);
            }
            report.commands.push(cmd("add_marketplace", &add));
        }
    }

    let is_installed = json_of(&["plugin", "list", "--json"]).is_ok_and(|p| installed(name, &p));
    report.commands.push(match (name, is_installed) {
        ("claude", false) => cmd(
            "install",
            &["plugin", "install", PLUGIN_ID, "--scope", "user"],
        ),
        ("claude", true) => cmd(
            "update",
            &["plugin", "update", PLUGIN_ID, "--scope", "user"],
        ),
        // Codex copies a local plugin into its cache on add, so adding again refreshes it.
        _ => cmd(
            if is_installed { "update" } else { "install" },
            &["plugin", "add", PLUGIN_ID],
        ),
    });

    // A server named trovador configured by hand runs beside the plugin's, and the agent
    // sees every tool twice.
    let duplicate = match name {
        "claude" => run(exe, &args(&["mcp", "get", PLUGIN])).is_ok_and(|o| o.ok),
        _ => json_of(&["mcp", "list", "--json"]).is_ok_and(|v| {
            v.as_array().is_some_and(|a| {
                a.iter().any(|s| {
                    s["name"] == PLUGIN
                        && s["transport"]["command"].as_str()
                            != Some(report.binary.display().to_string().as_str())
                })
            })
        }),
    };
    if duplicate {
        report.notes.push(format!(
            "{name} also has an MCP server named trovador configured by hand; remove it with `{name} mcp remove trovador` so the tools are not offered twice"
        ));
    }
}

/// Writes the package and removes anything a previous install left that this one no
/// longer ships. Only a directory carrying the marker is cleaned.
fn write_package(dir: &Path, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let err = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    crate::private::private_dir(dir).map_err(|e| err(dir, e))?;
    let keep: std::collections::HashSet<PathBuf> = files
        .iter()
        .map(|(p, _)| dir.join(p))
        .chain([dir.join(MARKER)])
        .collect();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).map_err(|e| err(&d, e))?.flatten() {
            let p = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p);
            } else if !keep.contains(&p) {
                fs::remove_file(&p).map_err(|e| err(&p, e))?;
            }
        }
    }
    for (rel, bytes) in files {
        let p = dir.join(rel);
        let parent = p.parent().expect("package paths have a parent");
        crate::private::private_dir(parent).map_err(|e| err(parent, e))?;
        crate::private::write_private(&p, bytes).map_err(|e| err(&p, e))?;
    }
    let marker =
        pretty(&json!({"managed_by": "trovador init", "version": env!("CARGO_PKG_VERSION")}));
    crate::private::write_private(&dir.join(MARKER), &marker).map_err(|e| err(dir, e))
}

#![cfg(unix)]

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tempfile::TempDir;
use tosi::{
    backend::CliRunner,
    diagnostics::{Authentication, check_environment},
    manager::Manager,
    server::router,
    types::Backend,
};
use tower_mcp::testing::TestClient;
use tracing::instrument::WithSubscriber;

const SECRET: &str = "PROBE_SECRET_DO_NOT_EXPOSE";

struct Fixture {
    dir: TempDir,
    binary: PathBuf,
}

impl Fixture {
    fn new(backend: Backend, stdout: &str, stderr: &str, exit: i32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("fake-cli");
        fs::write(
            &binary,
            r#"#!/bin/sh
fixture=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$fixture/calls"
if IFS= read -r line; then exit 98; fi
if [ "$*" = "--version" ]; then
  cat "$fixture/version"
  cat "$fixture/version-stderr" >&2
  exit 0
fi
case "$*" in
  'auth status --json'|'login status') ;;
  *) exit 99 ;;
esac
printf '%s\n' "$$" > "$fixture/pid"
cat "$fixture/stdout"
cat "$fixture/stderr" >&2
if [ -f "$fixture/hang" ]; then exec /bin/sleep 60; fi
exit "$(cat "$fixture/exit")"
"#,
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            dir.path().join("version"),
            match backend {
                Backend::Claude => "2.1.293 (Claude Code)\n",
                Backend::Codex => "codex-cli 0.157.1\n",
            },
        )
        .unwrap();
        fs::write(dir.path().join("version-stderr"), SECRET).unwrap();
        fs::write(dir.path().join("stdout"), stdout).unwrap();
        fs::write(dir.path().join("stderr"), stderr).unwrap();
        fs::write(dir.path().join("exit"), exit.to_string()).unwrap();
        Self { dir, binary }
    }

    fn runner(&self, backend: Backend) -> CliRunner {
        CliRunner::new().with_binary(backend, &self.binary)
    }

    fn set_version(&self, backend: Backend, version: &str) {
        fs::write(
            self.dir.path().join("version"),
            version_output(backend, version),
        )
        .unwrap();
    }

    fn hang(&self) {
        fs::write(self.dir.path().join("hang"), "").unwrap();
    }

    fn assert_reaped(&self) {
        let pid = fs::read_to_string(self.dir.path().join("pid")).unwrap();
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", pid.trim()])
                .output()
                .unwrap()
                .status
                .success(),
            "probe process was not reaped"
        );
    }
}

fn version_output(backend: Backend, version: &str) -> String {
    match backend {
        Backend::Claude => format!("{version} (Claude Code)\n"),
        Backend::Codex => format!("codex-cli {version}\n"),
    }
}

#[tokio::test]
async fn reports_only_whitelisted_status_and_version_fields() {
    let claude = Fixture::new(
        Backend::Claude,
        &json!({"loggedIn":true,"authMethod":SECRET,"apiProvider":SECRET,"email":SECRET,"orgId":SECRET,"extra":SECRET}).to_string(),
        SECRET,
        0,
    );
    let codex = Fixture::new(
        Backend::Codex,
        SECRET,
        &format!("WARNING: {SECRET}\nLogged in using API key - {SECRET}\n"),
        0,
    );
    let runner = claude
        .runner(Backend::Claude)
        .with_binary(Backend::Codex, &codex.binary);
    let report = check_environment(&runner, None).await;
    assert!(report.is_ready());
    assert_eq!(report.providers.len(), 2);
    assert_eq!(report.providers[0].version.as_deref(), Some("2.1.293"));
    assert_eq!(report.providers[1].version.as_deref(), Some("0.157.1"));
    for provider in &report.providers {
        assert_eq!(provider.authentication, Authentication::Authenticated);
        assert!(provider.executable.as_ref().unwrap().is_file());
    }
    let serialized = serde_json::to_value(&report).unwrap();
    assert!(!serialized.to_string().contains(SECRET));
    assert!(
        serialized["environment"]["auth_env_present"]
            .as_object()
            .unwrap()
            .values()
            .all(Value::is_boolean)
    );
    assert_eq!(
        fs::read_to_string(claude.dir.path().join("calls")).unwrap(),
        "--version\nauth status --json\n"
    );
    assert_eq!(
        fs::read_to_string(codex.dir.path().join("calls")).unwrap(),
        "--version\nlogin status\n"
    );
}

#[tokio::test]
async fn distinguishes_unavailable_unknown_missing_and_failed_probes_without_output_leaks() {
    let cases = [
        (
            Backend::Claude,
            r#"{"loggedIn":false}"#,
            "",
            1,
            Authentication::Unavailable,
        ),
        (
            Backend::Claude,
            r#"{"loggedIn":"true"}"#,
            SECRET,
            0,
            Authentication::Unknown,
        ),
        (
            Backend::Claude,
            "not JSON",
            SECRET,
            0,
            Authentication::Unknown,
        ),
        (
            Backend::Claude,
            r#"{"loggedIn":true}"#,
            SECRET,
            2,
            Authentication::Unknown,
        ),
        (
            Backend::Codex,
            "",
            "Not logged in\n",
            1,
            Authentication::Unavailable,
        ),
        (Backend::Codex, "", SECRET, 0, Authentication::Unknown),
        (
            Backend::Codex,
            "Logged in\nNot logged in\n",
            SECRET,
            0,
            Authentication::Unknown,
        ),
        (
            Backend::Codex,
            "Logged in using ChatGPT\n",
            SECRET,
            2,
            Authentication::Unknown,
        ),
    ];
    for (backend, stdout, stderr, exit, expected) in cases {
        let fixture = Fixture::new(backend, stdout, stderr, exit);
        let report = check_environment(&fixture.runner(backend), Some(backend)).await;
        assert!(!report.is_ready());
        assert_eq!(report.providers[0].authentication, expected);
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(!serialized.contains(SECRET));
        assert!(serialized.contains("launch context"));
    }
    let dir = tempfile::tempdir().unwrap();
    let runner = CliRunner::new().with_binary(Backend::Claude, dir.path().join("missing"));
    let report = check_environment(&runner, Some(Backend::Claude)).await;
    assert!(!report.is_ready());
    // Explicit overrides retain the requested path even if it cannot launch.
    assert_eq!(
        report.providers[0].executable,
        Some(dir.path().join("missing"))
    );
    assert_eq!(report.providers[0].authentication, Authentication::Unknown);
    assert!(
        report.providers[0]
            .hints
            .iter()
            .any(|hint| hint.contains("could not launch"))
    );
}

#[tokio::test]
async fn authenticated_recognized_prereleases_are_ready_for_both_providers() {
    for backend in [Backend::Claude, Backend::Codex] {
        for version in ["0.158.0-alpha.2", "0.158.0-beta.12", "0.158.0-rc.1"] {
            let fixture = Fixture::new(
                backend,
                r#"{"loggedIn":true}"#,
                "Logged in using ChatGPT",
                0,
            );
            fixture.set_version(backend, version);
            let report = check_environment(&fixture.runner(backend), Some(backend)).await;
            assert!(report.is_ready(), "{backend:?} {version}: {report:?}");
            assert_eq!(report.providers[0].version.as_deref(), Some(version));
            assert_eq!(
                report.providers[0].authentication,
                Authentication::Authenticated
            );
            assert!(!serde_json::to_string(&report).unwrap().contains(SECRET));
        }
    }
}

#[tokio::test]
async fn arbitrary_or_malformed_prerelease_versions_are_withheld() {
    let versions = [
        format!("0.158.0-{SECRET}"),
        format!("0.158.0-alpha.{SECRET}"),
        format!("0.158.0+{SECRET}"),
        format!("0.158.0-alpha.2+{SECRET}"),
        "0.158.0-preview.2".into(),
        "0.158.0-alpha".into(),
        "0.158.0-alpha.".into(),
        "0.158.0-alpha.2.3".into(),
        "0.158-alpha.2".into(),
        "0.158.0.1-alpha.2".into(),
        "0.158.x-alpha.2".into(),
        "0.158.0-alpha. 2".into(),
        "0.158. 0-alpha.2".into(),
        "0.158.0- alpha.2".into(),
        "0.158.0-alpha.\t2".into(),
    ];
    for backend in [Backend::Claude, Backend::Codex] {
        for version in &versions {
            let fixture = Fixture::new(
                backend,
                r#"{"loggedIn":true}"#,
                "Logged in using ChatGPT",
                0,
            );
            fixture.set_version(backend, version);
            let report = check_environment(&fixture.runner(backend), Some(backend)).await;
            assert!(!report.is_ready(), "{backend:?} {version}");
            assert!(
                report.providers[0].version.is_none(),
                "{backend:?} {version}"
            );
            assert_eq!(
                report.providers[0].authentication,
                Authentication::Authenticated
            );
            assert!(!serde_json::to_string(&report).unwrap().contains(SECRET));
        }
    }
}

#[tokio::test]
async fn suspicious_version_output_is_not_returned_as_a_version() {
    for backend in [Backend::Claude, Backend::Codex] {
        for version in ["0.158.0", "0.158.0-alpha.2"] {
            let fixture = Fixture::new(
                backend,
                r#"{"loggedIn":true}"#,
                "Logged in using ChatGPT",
                0,
            );
            fs::write(
                fixture.dir.path().join("version"),
                format!("{}{SECRET}", version_output(backend, version)),
            )
            .unwrap();
            let report = check_environment(&fixture.runner(backend), Some(backend)).await;
            assert!(!report.is_ready());
            assert!(report.providers[0].version.is_none());
            assert!(!serde_json::to_string(&report).unwrap().contains(SECRET));
        }
    }
}

#[derive(Clone)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn oversized_probe_output_is_redacted_and_cleanup_is_awaited() {
    for backend in [Backend::Claude, Backend::Codex] {
        let fixture = Fixture::new(backend, &SECRET.repeat(4000), SECRET, 0);
        fixture.hang();
        let capture = Arc::new(Mutex::new(Vec::new()));
        let writer = capture.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || LogCapture(writer.clone()))
            .finish();
        let runner = fixture.runner(backend);
        let report = check_environment(&runner, Some(backend))
            .with_subscriber(subscriber)
            .await;
        assert!(!report.is_ready());
        assert_eq!(report.providers[0].authentication, Authentication::Unknown);
        assert!(
            report.providers[0]
                .hints
                .iter()
                .any(|hint| hint.contains("64 KiB"))
        );
        assert!(!serde_json::to_string(&report).unwrap().contains(SECRET));
        assert!(!String::from_utf8_lossy(&capture.lock().unwrap()).contains(SECRET));
        fixture.assert_reaped();
    }
}

#[tokio::test]
async fn timed_out_probes_are_reaped_and_never_report_partial_credentials() {
    let claude = Fixture::new(Backend::Claude, SECRET, SECRET, 0);
    let codex = Fixture::new(Backend::Codex, SECRET, SECRET, 0);
    claude.hang();
    codex.hang();
    let runner = claude
        .runner(Backend::Claude)
        .with_binary(Backend::Codex, &codex.binary);
    let report = tokio::time::timeout(Duration::from_secs(15), check_environment(&runner, None))
        .await
        .unwrap();
    assert!(!report.is_ready());
    for provider in &report.providers {
        assert_eq!(provider.authentication, Authentication::Unknown);
        assert!(provider.hints.iter().any(|hint| hint.contains("timed out")));
    }
    assert!(!serde_json::to_string(&report).unwrap().contains(SECRET));
    claude.assert_reaped();
    codex.assert_reaped();
}

#[tokio::test]
async fn mcp_check_environment_is_read_only_and_checks_the_selected_provider() {
    let fixture = Fixture::new(Backend::Claude, r#"{"loggedIn":true}"#, SECRET, 0);
    let runner = Arc::new(fixture.runner(Backend::Claude));
    let manager = Manager::new(runner.clone(), Backend::Claude, 1);
    let mut client = TestClient::from_router(router(manager, runner));
    client.initialize().await;
    let tools = client.list_tools().await;
    let check = tools
        .iter()
        .find(|tool| tool["name"] == "check_environment")
        .unwrap();
    assert_eq!(check["annotations"]["readOnlyHint"], true);
    let report = client
        .call_tool_json("check_environment", json!({"backend":"claude"}))
        .await;
    assert_eq!(report["ready"], true);
    assert_eq!(report["providers"].as_array().unwrap().len(), 1);
    assert_eq!(report["providers"][0]["authentication"], "authenticated");
    assert!(!report.to_string().contains(SECRET));
}

#[tokio::test]
async fn doctor_succeeds_for_authenticated_prereleases_even_inside_delegated_workers() {
    for (backend, name, override_name) in [
        (Backend::Claude, "claude", "TOSI_CLAUDE_BIN"),
        (Backend::Codex, "codex", "TOSI_CODEX_BIN"),
    ] {
        let fixture = Fixture::new(
            backend,
            r#"{"loggedIn":true}"#,
            "Logged in using ChatGPT",
            0,
        );
        fixture.set_version(backend, "0.158.0-alpha.2");
        for child_guard in [None, Some("TOSI_CHILD"), Some("TASK_RUNNERS_CHILD")] {
            let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_tosi"));
            command
                .args(["doctor", "--backend", name])
                .env(override_name, &fixture.binary)
                .env_remove("TOSI_CHILD")
                .env_remove("TASK_RUNNERS_CHILD")
                .kill_on_drop(true);
            if let Some(child_guard) = child_guard {
                command.env(child_guard, "1");
            }
            let output = tokio::time::timeout(Duration::from_secs(5), command.output())
                .await
                .unwrap()
                .unwrap();
            assert!(output.status.success(), "{backend:?} {child_guard:?}");
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report["ready"], true);
            assert_eq!(report["providers"][0]["version"], "0.158.0-alpha.2");
            assert_eq!(report["providers"][0]["authentication"], "authenticated");
            assert!(!String::from_utf8_lossy(&output.stdout).contains(SECRET));
            assert!(!String::from_utf8_lossy(&output.stderr).contains(SECRET));
        }
    }
}

#[tokio::test]
async fn doctor_emits_json_and_nonzero_status_when_auth_is_unavailable() {
    let fixture = Fixture::new(
        Backend::Claude,
        &json!({"loggedIn":false,"apiKey":SECRET}).to_string(),
        SECRET,
        1,
    );
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_tosi"))
            .args(["doctor", "--backend", "claude"])
            .env("TOSI_CLAUDE_BIN", &fixture.binary)
            .env("ANTHROPIC_API_KEY", SECRET)
            .env("CODEX_SANDBOX", "test-sandbox")
            .env("CODEX_SANDBOX_NETWORK_DISABLED", "1")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["ready"], false);
    assert_eq!(report["providers"][0]["authentication"], "unavailable");
    assert_eq!(
        report["environment"]["auth_env_present"]["ANTHROPIC_API_KEY"],
        true
    );
    assert_eq!(report["environment"]["network_disabled"], "1");
    assert!(
        report["hints"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hint| hint.as_str().unwrap().contains("survive escalation"))
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SECRET));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SECRET));
}

#[tokio::test]
async fn doctor_honors_legacy_binary_overrides_only_when_the_primary_is_absent() {
    for (backend, name, suffix, auth) in [
        (
            Backend::Claude,
            "claude",
            "CLAUDE_BIN",
            r#"{"loggedIn":true}"#,
        ),
        (
            Backend::Codex,
            "codex",
            "CODEX_BIN",
            "Logged in using ChatGPT\n",
        ),
    ] {
        let fixture = Fixture::new(backend, auth, "", 0);
        let primary_key = format!("TOSI_{suffix}");
        let legacy_key = format!("TASK_RUNNERS_{suffix}");
        for scenario in ["legacy", "primary", "empty", "missing"] {
            let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_tosi"));
            command
                .args(["doctor", "--backend", name])
                .env_remove(&primary_key)
                .env(&legacy_key, &fixture.binary)
                .kill_on_drop(true);
            match scenario {
                "primary" => {
                    command
                        .env(&primary_key, &fixture.binary)
                        .env(&legacy_key, fixture.dir.path().join("missing-legacy"));
                }
                "empty" => {
                    command.env(&primary_key, "");
                }
                "missing" => {
                    command.env(&primary_key, fixture.dir.path().join("missing-primary"));
                }
                _ => {}
            }
            let output = tokio::time::timeout(Duration::from_secs(5), command.output())
                .await
                .unwrap()
                .unwrap();
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            let expected = matches!(scenario, "legacy" | "primary");
            assert_eq!(output.status.success(), expected, "{name}: {scenario}");
            assert_eq!(report["ready"], expected, "{name}: {scenario}");
            if expected {
                assert_eq!(
                    report["providers"][0]["executable"],
                    fixture.binary.to_str().unwrap()
                );
            }
        }
    }
}

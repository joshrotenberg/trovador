//! Read-only CLI and authentication probes for the current launch context.

use std::{collections::BTreeMap, env, path::PathBuf, time::Duration};

use serde::Serialize;
use tracing::instrument::WithSubscriber;

use crate::{backend::CliRunner, types::Backend};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_LIMIT: usize = 64 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_millis(250);

#[derive(Debug, Serialize)]
pub struct EnvironmentReport {
    pub environment: LaunchEnvironment,
    pub providers: Vec<ProviderReport>,
    pub ready: bool,
    pub hints: Vec<String>,
}

impl EnvironmentReport {
    /// Whether every requested CLI returned a recognized version and login status.
    /// This does not establish network access or model availability.
    pub fn is_ready(&self) -> bool {
        self.ready
    }
}

#[derive(Debug, Serialize)]
pub struct LaunchEnvironment {
    pub home: Option<String>,
    pub claude_config_dir: Option<String>,
    pub codex_home: Option<String>,
    pub codex_sandbox: Option<String>,
    pub network_disabled: Option<String>,
    pub auth_env_present: BTreeMap<String, bool>,
}

#[derive(Debug, Serialize)]
pub struct ProviderReport {
    pub backend: Backend,
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
    pub authentication: Authentication,
    pub ready: bool,
    pub hints: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Authentication {
    Authenticated,
    Unavailable,
    Unknown,
}

/// Inspect executable discovery, versions, and CLI login status without a model request.
/// No credentials are read directly, and no environment or authentication state is changed.
pub async fn check_environment(runner: &CliRunner, backend: Option<Backend>) -> EnvironmentReport {
    let environment = LaunchEnvironment {
        home: context_value("HOME"),
        claude_config_dir: context_value("CLAUDE_CONFIG_DIR"),
        codex_home: context_value("CODEX_HOME"),
        codex_sandbox: context_value("CODEX_SANDBOX"),
        network_disabled: context_value("CODEX_SANDBOX_NETWORK_DISABLED"),
        auth_env_present: [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
        ]
        .into_iter()
        .map(|name| (name.to_owned(), env::var_os(name).is_some()))
        .collect(),
    };
    let mut hints = vec![
        "These status-only checks describe this launch context; they do not prove network access, model availability, or task success.".into(),
    ];
    if environment.codex_sandbox.is_some() {
        hints.push(
            "CODEX_SANDBOX is an inherited marker, not proof of the effective sandbox policy."
                .into(),
        );
    }
    if let Some(value) = &environment.network_disabled {
        let interpretation = match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => "suggests restricted network access",
            "0" | "false" | "no" => "does not request disabled networking",
            _ => "has an unrecognized value",
        };
        hints.push(format!(
            "CODEX_SANDBOX_NETWORK_DISABLED {interpretation}. This inherited marker can survive escalation; it does not establish effective network access."
        ));
    }
    let providers = match backend {
        Some(backend) => vec![check_provider(runner, backend).await],
        None => {
            let (claude, codex) = tokio::join!(
                check_provider(runner, Backend::Claude),
                check_provider(runner, Backend::Codex)
            );
            vec![claude, codex]
        }
    };
    let ready = providers.iter().all(|provider| provider.ready);
    EnvironmentReport {
        environment,
        providers,
        ready,
        hints,
    }
}

fn context_value(name: &str) -> Option<String> {
    env::var_os(name).map(|value| value.to_string_lossy().into_owned())
}

async fn check_provider(runner: &CliRunner, backend: Backend) -> ProviderReport {
    let mut report = ProviderReport {
        backend,
        executable: None,
        version: None,
        authentication: Authentication::Unknown,
        ready: false,
        hints: Vec::new(),
    };
    let Ok(binary) = runner.resolved_binary(backend) else {
        report.hints.push("CLI executable could not be resolved in this launch context. Check the installed host's PATH or the provider executable override.".into());
        return report;
    };
    report.executable = Some(binary.clone());
    match probe(backend, &binary, &["--version"]).await {
        Ok(output) if output.success => {
            report.version = parse_version(backend, &output.stdout);
            if report.version.is_none() {
                report
                    .hints
                    .push("CLI version output was not recognized; raw output is withheld.".into());
            }
        }
        Ok(_) => report
            .hints
            .push("CLI version probe exited unsuccessfully; raw output is withheld.".into()),
        Err(error) => report.hints.push(error.hint("CLI version")),
    }
    let args: &[&str] = match backend {
        Backend::Claude => &["auth", "status", "--json"],
        Backend::Codex => &["login", "status"],
    };
    match probe(backend, &binary, args).await {
        Ok(output) => {
            report.authentication = match backend {
                Backend::Claude => claude_authentication(&output),
                Backend::Codex => codex_authentication(&output),
            };
        }
        Err(error) => report.hints.push(error.hint("Authentication status")),
    }
    match report.authentication {
        Authentication::Authenticated => {}
        Authentication::Unavailable => report.hints.push(
            "The CLI reports authentication unavailable in this launch context. Check check_environment from the installed plugin and compare the host environment before deciding that login is required.".into(),
        ),
        Authentication::Unknown => report.hints.push(
            "The status probe could not establish authentication in this launch context. Check the installed plugin's environment; an unknown result is not evidence that credentials are missing.".into(),
        ),
    }
    if cfg!(target_os = "macos") && report.authentication != Authentication::Authenticated {
        report.hints.push("macOS Keychain access can differ between a sandboxed child, the terminal, and the installed plugin host.".into());
    }
    report.ready =
        report.version.is_some() && report.authentication == Authentication::Authenticated;
    report
}

struct ProbeOutput {
    stdout: String,
    stderr: String,
    success: bool,
}

enum ProbeFailure {
    Launch,
    Timeout,
    OutputLimit,
    Failed,
}

impl ProbeFailure {
    fn hint(&self, probe: &str) -> String {
        let reason = match self {
            Self::Launch => "could not launch or communicate with the CLI",
            Self::Timeout => "timed out after 10 seconds; process cleanup was awaited",
            Self::OutputLimit => "exceeded the 64 KiB output limit; process cleanup was awaited",
            Self::Failed => "failed without a recognized status",
        };
        format!("{probe} probe {reason}; raw output is withheld.")
    }
}

async fn probe(
    backend: Backend,
    binary: &std::path::Path,
    args: &[&str],
) -> Result<ProbeOutput, ProbeFailure> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    // The wrappers can log partial command output on timeout/capture failure.
    // Auth probes must suppress those logs as well as redact returned errors.
    let output = async {
        match backend {
            Backend::Claude => {
                let client = claude_wrapper::Claude::builder()
                    .binary(binary)
                    .timeout(PROBE_TIMEOUT)
                    .kill_grace(TERMINATION_GRACE)
                    .output_limit(OUTPUT_LIMIT)
                    .build()
                    .map_err(|_| ProbeFailure::Launch)?;
                let output = claude_wrapper::exec::run_claude_allow_exit_codes(&client, args, &[1])
                    .await
                    .map_err(|error| match error {
                        claude_wrapper::Error::Timeout { .. } => ProbeFailure::Timeout,
                        claude_wrapper::Error::OutputLimitExceeded { .. } => {
                            ProbeFailure::OutputLimit
                        }
                        claude_wrapper::Error::NotFound | claude_wrapper::Error::Io { .. } => {
                            ProbeFailure::Launch
                        }
                        _ => ProbeFailure::Failed,
                    })?;
                Ok(ProbeOutput {
                    stdout: output.stdout,
                    stderr: output.stderr,
                    success: output.success,
                })
            }
            Backend::Codex => {
                let client = codex_wrapper::Codex::builder()
                    .binary(binary)
                    .timeout(PROBE_TIMEOUT)
                    .termination_grace(TERMINATION_GRACE)
                    .output_limit(OUTPUT_LIMIT)
                    .build()
                    .map_err(|_| ProbeFailure::Launch)?;
                let output = codex_wrapper::exec::run_codex_allow_exit_codes(&client, args, &[1])
                    .await
                    .map_err(|error| match error {
                        codex_wrapper::Error::Timeout { .. } => ProbeFailure::Timeout,
                        codex_wrapper::Error::OutputLimitExceeded { .. } => {
                            ProbeFailure::OutputLimit
                        }
                        codex_wrapper::Error::NotFound | codex_wrapper::Error::Io { .. } => {
                            ProbeFailure::Launch
                        }
                        _ => ProbeFailure::Failed,
                    })?;
                Ok(ProbeOutput {
                    stdout: output.stdout,
                    stderr: output.stderr,
                    success: output.success,
                })
            }
        }
    }
    .with_subscriber(tracing::subscriber::NoSubscriber::default())
    .await?;
    if output.stdout.len().saturating_add(output.stderr.len()) > OUTPUT_LIMIT {
        return Err(ProbeFailure::OutputLimit);
    }
    Ok(output)
}

fn parse_version(backend: Backend, stdout: &str) -> Option<String> {
    let line = stdout.trim();
    let token = match backend {
        Backend::Claude => line.strip_suffix(" (Claude Code)")?,
        Backend::Codex => line.strip_prefix("codex-cli ")?,
    };
    // Return numeric releases and only known prerelease labels with a numeric
    // ordinal. Arbitrary SemVer identifiers could expose unrelated CLI output.
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let release = if let Some((release, prerelease)) = token.split_once('-') {
        let (label, ordinal) = prerelease.split_once('.')?;
        if !matches!(label, "alpha" | "beta" | "rc") || !numeric(ordinal) {
            return None;
        }
        release
    } else {
        token
    };
    let parts: Vec<_> = release.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| !numeric(part)) {
        return None;
    }
    Some(token.to_owned())
}

fn claude_authentication(output: &ProbeOutput) -> Authentication {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&output.stdout) else {
        return Authentication::Unknown;
    };
    match value.get("loggedIn").and_then(serde_json::Value::as_bool) {
        Some(true) if output.success => Authentication::Authenticated,
        Some(false) => Authentication::Unavailable,
        _ => Authentication::Unknown,
    }
}

fn codex_authentication(output: &ProbeOutput) -> Authentication {
    let mut logged_in = false;
    let mut logged_out = false;
    for line in output.stdout.lines().chain(output.stderr.lines()) {
        let line = line.trim();
        if line == "Logged in" || line.starts_with("Logged in ") {
            logged_in = true;
        }
        if line == "Not logged in" || line.starts_with("Not logged in ") {
            logged_out = true;
        }
    }
    match (logged_in, logged_out) {
        (true, false) if output.success => Authentication::Authenticated,
        (false, true) => Authentication::Unavailable,
        _ => Authentication::Unknown,
    }
}

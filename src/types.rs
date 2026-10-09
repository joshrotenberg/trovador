use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, clap::ValueEnum, PartialEq, Eq,
)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    #[default]
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    #[default]
    ReadOnly,
    WorkspaceWrite,
}

/// Literal provider levels, not a normalized amount of compute across backends.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RequestedSettings {
    pub backend: Option<Backend>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SelectedSettings {
    pub backend: Backend,
    pub model: Option<String>,
    pub effort: Option<Effort>,
}

/// Unknown without authoritative per-turn evidence, absent from current adapters.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct ReportedSettings {
    pub model: Option<String>,
    pub effort: Option<Effort>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Selection {
    pub requested: RequestedSettings,
    pub selected: SelectedSettings,
    pub reported: ReportedSettings,
}

#[derive(Clone, Debug)]
pub struct RunSpec {
    pub backend: Backend,
    pub task: String,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub output_schema: Option<serde_json::Value>,
    pub session_id: Option<String>,
    /// With `session_id`: resume that session's context into a new session (Claude only).
    pub fork: bool,
    pub timeout_secs: u64,
    pub access: AccessMode,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunOutput {
    pub text: String,
    pub structured_result: Option<serde_json::Value>,
    pub session_id: Option<String>,
    pub usage: serde_json::Value,
}

#[async_trait]
pub trait Runner: Send + Sync {
    /// Return only after the child exits or explicit cancellation has reaped it.
    async fn run(&self, spec: RunSpec, cancel: CancellationToken) -> Result<RunOutput, String>;
}

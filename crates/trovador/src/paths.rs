//! Where the provider archives are and where trovador keeps its own state.
//!
//! Every path the library touches comes from a `Paths` value. Tests build one from a temp
//! directory; only `Paths::from_env` ever looks at the real home directory, so the suite
//! cannot reach real state by forgetting an override.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    /// `~/.claude/projects`: one directory per project, one JSONL file per session.
    pub claude_archive: PathBuf,
    /// `~/.codex/sessions`: date-nested directories of `rollout-*-<uuid>.jsonl` files.
    pub codex_archive: PathBuf,
    /// `~/.codex/archived_sessions`: where Codex moves a thread when it is archived. Flat,
    /// same file names.
    pub codex_archived: PathBuf,
    /// `~/.trovador`: everything trovador owns.
    pub home: PathBuf,
    /// The `claude` binary to run, when not the one on PATH.
    pub claude_bin: Option<PathBuf>,
    /// The `codex` binary to run, when not the one on PATH.
    pub codex_bin: Option<PathBuf>,
}

impl Paths {
    pub fn from_env() -> Self {
        let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            claude_archive: var("TROVADOR_CLAUDE_ARCHIVE")
                .unwrap_or_else(|| home.join(".claude").join("projects")),
            codex_archived: var("TROVADOR_CODEX_ARCHIVED").unwrap_or_else(|| {
                // Beside the sessions root, so overriding that root moves both.
                var("TROVADOR_CODEX_ARCHIVE")
                    .and_then(|p| p.parent().map(|d| d.join("archived_sessions")))
                    .unwrap_or_else(|| home.join(".codex").join("archived_sessions"))
            }),
            codex_archive: var("TROVADOR_CODEX_ARCHIVE")
                .unwrap_or_else(|| home.join(".codex").join("sessions")),
            home: bottega_core::state::trovador_dir(),
            claude_bin: var("TROVADOR_CLAUDE_BIN"),
            codex_bin: var("TROVADOR_CODEX_BIN"),
        }
    }

    /// All three under one root, laid out like a real home directory.
    pub fn under(root: &Path) -> Self {
        Self {
            claude_archive: root.join(".claude").join("projects"),
            codex_archive: root.join(".codex").join("sessions"),
            codex_archived: root.join(".codex").join("archived_sessions"),
            home: bottega_core::state::resolve_trovador_dir(None, root),
            claude_bin: None,
            codex_bin: None,
        }
    }
}

pub use bottega_core::private::{private_dir, private_file, write_private};

/// Tightens a file that already exists to 0600. Missing files are not an error.
pub fn make_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

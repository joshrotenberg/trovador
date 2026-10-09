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
            home: var("TROVADOR_HOME").unwrap_or_else(|| home.join(".trovador")),
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
            home: root.join(".trovador"),
            claude_bin: None,
            codex_bin: None,
        }
    }
}

/// Creates a directory of trovador's state private to the user (0700), and tightens one
/// that already exists. The index, summaries, and saved replies hold transcript text,
/// secrets included, copied from files Claude writes 0600; trovador made them 0644 in a
/// 0755 directory (pre-release review).
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        builder.mode(0o700);
        builder.create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    builder.create(dir)
}

/// Options that create a file readable only by the user (0600).
pub fn private_file() -> std::fs::OpenOptions {
    #[allow(unused_mut)]
    let mut opts = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts
}

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

/// Writes a file readable only by the user, replacing it atomically through a temp file.
pub fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    {
        let mut f = private_file()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(contents)?;
    }
    std::fs::rename(tmp, path)
}

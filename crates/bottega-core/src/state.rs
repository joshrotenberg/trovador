use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub fn trovador_dir() -> PathBuf {
    resolve_trovador_dir(
        std::env::var_os("TROVADOR_HOME"),
        &std::env::home_dir().unwrap_or_else(|| PathBuf::from(".")),
    )
}

// Explicit inputs keep tests independent of process environment and real state.
pub fn resolve_trovador_dir(override_dir: Option<OsString>, home: &Path) -> PathBuf {
    override_dir
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".trovador"))
}

pub fn provenance_path(dir: &Path) -> PathBuf {
    dir.join("spawned.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn environment_override_is_resolved_in_an_isolated_process() {
        const CHILD: &str = "BOTTEGA_CORE_STATE_TEST_CHILD";
        if let Some(expected) = std::env::var_os(CHILD) {
            let expected = PathBuf::from(expected);
            assert_eq!(trovador_dir(), expected);
            assert_eq!(
                provenance_path(&trovador_dir()),
                expected.join("spawned.json")
            );
            std::fs::write(expected.join("child-ran"), b"resolved").unwrap();
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let custom = root.path().join("custom");
        std::fs::create_dir(&custom).unwrap();
        // Libtest omits the crate name from module paths in its exact filters.
        let module = module_path!().split_once("::").unwrap().1;
        let filter = format!("{module}::environment_override_is_resolved_in_an_isolated_process");
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &filter])
            .env("TROVADOR_HOME", &custom)
            .env(CHILD, &custom)
            .status()
            .unwrap();
        assert!(result.success());
        assert_eq!(
            std::fs::read(custom.join("child-ran")).unwrap(),
            b"resolved"
        );
    }

    #[test]
    fn legacy_state_locations_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let default = root.path().join(".trovador");
        let custom = root.path().join("custom");
        assert_eq!(resolve_trovador_dir(None, root.path()), default);
        assert_eq!(
            resolve_trovador_dir(Some(OsString::new()), root.path()),
            default
        );
        assert_eq!(
            resolve_trovador_dir(Some(custom.clone().into_os_string()), root.path()),
            custom
        );
        assert_eq!(provenance_path(&default), default.join("spawned.json"));
        assert!(default.starts_with(root.path()));
        assert!(custom.starts_with(root.path()));
    }
}

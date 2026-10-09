use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::private::{private_dir, private_file, write_private};

#[derive(Clone, Debug)]
pub struct State {
    pub root: PathBuf,
    pub trovador: PathBuf,
    pub tosi: PathBuf,
    pub shared: PathBuf,
    pub warning: Option<String>,
    old: PathBuf,
    migrate: bool,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub what: String,
    pub fix: String,
}

pub fn from_env() -> State {
    resolve(
        std::env::var_os("BOTTEGA_HOME"),
        std::env::var_os("TROVADOR_HOME"),
        &std::env::home_dir().unwrap_or_else(|| PathBuf::from(".")),
    )
}

pub fn trovador_dir() -> PathBuf {
    from_env().trovador
}

pub fn resolve_trovador_dir(override_dir: Option<OsString>, home: &Path) -> PathBuf {
    resolve(None, override_dir, home).trovador
}

pub fn provenance_path(dir: &Path) -> PathBuf {
    dir.join("spawned.json")
}

// Resolution stays pure so library callers cannot accidentally migrate user state.
pub fn resolve(root: Option<OsString>, service: Option<OsString>, home: &Path) -> State {
    let root = root.filter(|v| !v.is_empty());
    let service = service.filter(|v| !v.is_empty());
    let migrate = root.is_none() && service.is_none();
    let root = root
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".bottega"));
    let state = State {
        trovador: service
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("trovador")),
        shared: root.join("shared"),
        tosi: root.join("tosi"),
        root,
        warning: None,
        old: home.join(".trovador"),
        migrate,
    };
    guard(&state);
    state
}

fn guard(state: &State) {
    if let Some(root) = std::env::var_os("BOTTEGA_TEST_ROOT") {
        let root = PathBuf::from(root);
        for path in [&state.root, &state.trovador, &state.shared, &state.tosi] {
            assert!(
                path.starts_with(&root),
                "durable state escaped test root: {}",
                path.display()
            );
        }
        if state.migrate {
            assert!(
                state.old.starts_with(&root),
                "legacy state escaped test root"
            );
        }
    }
}

pub const SHARED: &[&str] = &[
    "spawned.json",
    "spawned.lock",
    "consent.json",
    "consent.lock",
];

/// Entry points prepare state explicitly; path construction has no filesystem effects.
pub fn prepare(state: State) -> io::Result<State> {
    prepare_inner(state, None)
}

/// Forces the cross-device path with a supplied copy operation for isolated recovery tests.
#[doc(hidden)]
pub fn prepare_with_copy(
    state: State,
    mut copy: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<State> {
    prepare_inner(state, Some(&mut copy))
}

type CopyStep<'a> = Option<&'a mut dyn FnMut(&Path, &Path) -> io::Result<()>>;

fn prepare_inner(mut state: State, copy: CopyStep<'_>) -> io::Result<State> {
    if state.migrate {
        state.trovador = state.root.join("trovador");
        state.shared = state.root.join("shared");
    }
    guard(&state);
    let mut held_lock = None;
    let mut warnings = Vec::new();
    let result = (|| -> io::Result<()> {
        private_dir(&state.root)?;
        let lock = private_file()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.root.join("migration.lock"))?;
        lock.lock()?;
        held_lock = Some(lock);
        if state.migrate {
            let partial = state.root.join("trovador.partial");
            if !state.trovador.exists() {
                if partial.exists() {
                    fs::remove_dir_all(&partial)?;
                }
                if state.old.exists() {
                    commit(&state, copy)?;
                    // Shared files must be private before leaving the committed tree.
                    tighten(&state.trovador, &mut warnings);
                }
            }
            // A persisted manifest distinguishes an unretired copy from a recreated old directory.
            if state.trovador.exists()
                && let Err(error) = retire_copy(&state)
            {
                state.warning = Some(format!("state preparation failed: {error}"));
            }
        }
        private_dir(&state.trovador)?;
        private_dir(&state.shared)?;
        private_dir(&state.tosi)?;
        move_shared(&state)?;
        Ok(())
    })();
    if let Err(error) = result {
        if state.migrate
            && !state.trovador.exists()
            && !matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied
                    | io::ErrorKind::Unsupported
                    | io::ErrorKind::InvalidData
            )
        {
            return Err(error);
        }
        state.warning = Some(format!("state preparation failed: {error}"));
        // The committed directory is authoritative even if later recovery steps fail.
        if state.migrate && !state.trovador.exists() && state.old.exists() {
            state.trovador = state.old.clone();
            state.shared = state.old.clone();
        }
    }
    if let Some(warning) = state.warning.take() {
        warnings.push(warning);
    }
    if !warnings.is_empty() {
        state.warning = Some(warnings.join("; "));
    }
    guard(&state);
    Ok(state)
}

fn commit(state: &State, copy: CopyStep<'_>) -> io::Result<()> {
    if copy.is_none() {
        match fs::rename(&state.old, &state.trovador) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {}
            Err(e) => return Err(e),
        }
    }
    let before = manifest(&state.old)?;
    let marker = state.root.join("migration-copy.json");
    write_private(&marker, &serde_json::to_vec(&before)?)?;
    let partial = state.root.join("trovador.partial");
    match copy {
        Some(copy) => copy(&state.old, &partial)?,
        None => copy_tree(&state.old, &partial)?,
    }
    let copied = manifest(&partial)?;
    if before.len() != copied.len()
        || before.iter().any(|(path, entry)| {
            copied
                .get(path)
                .is_none_or(|other| entry.size != other.size || entry.directory != other.directory)
        })
    {
        return Err(io::Error::other("migration copy path or size mismatch"));
    }
    if before != manifest(&state.old)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "migration source changed during copy; retry shortly or stop the old server before retrying",
        ));
    }
    fs::rename(partial, &state.trovador)
}

fn retire_copy(state: &State) -> io::Result<()> {
    let marker = state.root.join("migration-copy.json");
    if !marker.exists() {
        return Ok(());
    }
    if state.old.exists() {
        let before: Manifest = serde_json::from_slice(&fs::read(&marker)?)?;
        if before != manifest(&state.old)? {
            return Err(io::Error::other(
                "committed migration source changed; old directory retained",
            ));
        }
        let retired = state.old.with_extension("migrated");
        if retired.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "retired migration source already exists",
            ));
        }
        fs::rename(&state.old, retired)?;
    }
    fs::remove_file(marker)
}

fn move_shared(state: &State) -> io::Result<()> {
    let mut conflicts = Vec::new();
    for name in SHARED {
        let source = state.trovador.join(name);
        let target = state.shared.join(name);
        let partial = state.shared.join(format!("{name}.partial"));
        if source == target {
            continue;
        }
        if !source.exists() {
            if partial.exists() && target.exists() && same_file(&partial, &target)? {
                fs::remove_file(partial)?;
            }
            continue;
        }
        // The retained link identifies our published cross-device copy after a crash.
        if partial.exists() && target.exists() && same_file(&partial, &target)? {
            let before = fs::metadata(&source)?;
            let identical = fs::read(&source)? == fs::read(&target)?;
            let after = fs::metadata(&source)?;
            if identical && before.len() == after.len() && before.modified()? == after.modified()? {
                fs::remove_file(&source)?;
                fs::remove_file(partial)?;
                continue;
            }
        }
        if partial.exists() && !target.exists() {
            fs::remove_file(&partial)?;
        }
        // Hard links publish without replacing a file created by another writer.
        match fs::hard_link(&source, &target) {
            Ok(()) => fs::remove_file(&source)?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if same_file(&source, &target)? {
                    fs::remove_file(&source)?;
                } else {
                    conflicts.push(*name);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                copy_shared(&source, &target, &partial)?
            }
            Err(e) => return Err(e),
        }
    }
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("shared state conflicts: {}", conflicts.join(", ")),
        ))
    }
}

fn copy_shared(source: &Path, target: &Path, partial: &Path) -> io::Result<()> {
    if partial.exists() {
        fs::remove_file(partial)?;
    }
    let mut input = fs::File::open(source)?;
    let mut output = private_file().write(true).create_new(true).open(partial)?;
    let before = input.metadata()?;
    let size = io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    let after = input.metadata()?;
    if size != before.len()
        || size != after.len()
        || size != output.metadata()?.len()
        || before.modified()? != after.modified()?
    {
        return Err(io::Error::other("shared state changed during copy"));
    }
    fs::hard_link(partial, target)?;
    fs::remove_file(source)?;
    fs::remove_file(partial)
}

fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let a = fs::metadata(a)?;
        let b = fs::metadata(b)?;
        Ok(a.dev() == b.dev() && a.ino() == b.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        Ok(false)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    directory: bool,
    size: u64,
    modified: SystemTime,
}
type Manifest = BTreeMap<PathBuf, Entry>;

fn manifest(root: &Path) -> io::Result<Manifest> {
    fn scan(root: &Path, relative: &Path, out: &mut Manifest) -> io::Result<()> {
        let metadata = fs::symlink_metadata(root.join(relative))?;
        if !metadata.is_dir() && !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "migration refuses special files and symlinks",
            ));
        }
        out.insert(
            relative.to_path_buf(),
            Entry {
                directory: metadata.is_dir(),
                size: if metadata.is_dir() { 0 } else { metadata.len() },
                modified: metadata.modified()?,
            },
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(root.join(relative))? {
                scan(root, &relative.join(entry?.file_name()), out)?;
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    scan(root, Path::new(""), &mut out)?;
    Ok(out)
}

#[doc(hidden)]
pub fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    private_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let dest = target.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else if kind.is_file() {
            let mut input = fs::File::open(entry.path())?;
            let mut output = private_file().write(true).create_new(true).open(dest)?;
            let size = io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            if size != input.metadata()?.len() || size != output.metadata()?.len() {
                return Err(io::Error::other("migration copy size mismatch"));
            }
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "migration refuses special files and symlinks",
            ));
        }
    }
    Ok(())
}

fn tighten(path: &Path, warnings: &mut Vec<String>) {
    tighten_with(path, warnings, &mut |_| {});
}

fn tighten_with(path: &Path, warnings: &mut Vec<String>, before: &mut impl FnMut(&Path)) {
    let result = (|| -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        before(path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.is_file() || metadata.is_dir() {
                fs::set_permissions(
                    path,
                    fs::Permissions::from_mode(if metadata.is_dir() { 0o700 } else { 0o600 }),
                )?;
            }
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                match entry {
                    Ok(entry) => tighten_with(&entry.path(), warnings, before),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => warnings.push(format!("cannot tighten {}: {e}", path.display())),
                }
            }
        }
        Ok(())
    })();
    if let Err(e) = result
        && e.kind() != io::ErrorKind::NotFound
    {
        warnings.push(format!("cannot tighten {}: {e}", path.display()));
    }
}

pub fn diagnostics(state: &State) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if state.migrate && state.old.exists() && state.root.join("trovador").exists() {
        out.push(Diagnostic { what: "old and new state both exist; using the new layout without merging the old directory".into(), fix: "Restart both host apps, inspect ~/.trovador, and remove it only after preserving any newer data".into() });
    }
    let partial = state.root.join("trovador.partial");
    if partial.exists() {
        out.push(Diagnostic { what: format!("incomplete migration copy: {}", partial.display()), fix: "Restart a service to retry an uncommitted migration; if the new layout exists, remove the leftover partial copy".into() });
    }
    let retired = state.old.with_extension("migrated");
    if state.migrate && retired.exists() {
        out.push(Diagnostic {
            what: format!("retired migration source: {}", retired.display()),
            fix: "Verify the new layout, then remove the retired ~/.trovador.migrated directory"
                .into(),
        });
    }
    for name in SHARED {
        let source = state.trovador.join(name);
        let target = state.root.join("shared").join(name);
        if source != target && source.exists() && target.exists() {
            out.push(Diagnostic { what: format!("shared state conflict: {name} exists in both service and shared directories"), fix: format!("Inspect both copies of {name}, preserve the desired data in shared/, then remove the service copy") });
        }
    }
    out
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
                provenance_path(&from_env().shared),
                expected.parent().unwrap().join("shared/spawned.json")
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
            .env("BOTTEGA_HOME", root.path())
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
    fn default_and_override_state_locations_are_isolated() {
        let root = tempfile::tempdir().unwrap();
        let default = root.path().join(".bottega/trovador");
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

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn a_file_vanishing_during_tighten_is_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("vanishing");
        fs::write(&path, b"data").unwrap();
        let mut warnings = Vec::new();
        tighten_with(temp.path(), &mut warnings, &mut |current| {
            if current == path {
                fs::remove_file(current).unwrap();
            }
        });
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!path.exists());
    }

    #[test]
    fn a_crash_after_copy_commit_retires_the_original_on_restart() {
        let temp = tempfile::tempdir().unwrap();
        let state = resolve(None, None, temp.path());
        private_dir(&state.old).unwrap();
        fs::write(state.old.join("index.db"), b"index").unwrap();
        fs::write(state.old.join("spawned.json"), b"provenance").unwrap();
        private_dir(&state.root).unwrap();
        commit(&state, Some(&mut copy_tree)).unwrap();
        assert!(state.old.exists());
        assert!(state.trovador.exists());
        let state = prepare(state).unwrap();
        assert!(!state.old.exists());
        assert_eq!(
            fs::read(state.old.with_extension("migrated").join("index.db")).unwrap(),
            b"index"
        );
        assert_eq!(
            fs::read(state.shared.join("spawned.json")).unwrap(),
            b"provenance"
        );
    }
}

#[cfg(test)]
mod shared_recovery_tests {
    use super::*;

    #[test]
    fn a_published_cross_device_shared_copy_resumes_source_retirement() {
        let temp = tempfile::tempdir().unwrap();
        let state = resolve(None, Some(temp.path().join("service").into()), temp.path());
        private_dir(&state.trovador).unwrap();
        private_dir(&state.shared).unwrap();
        fs::write(state.trovador.join("spawned.json"), b"provenance").unwrap();
        let partial = state.shared.join("spawned.json.partial");
        fs::write(&partial, b"provenance").unwrap();
        fs::hard_link(&partial, state.shared.join("spawned.json")).unwrap();
        let state = prepare(state).unwrap();
        assert!(state.warning.is_none(), "{:?}", state.warning);
        assert!(!state.trovador.join("spawned.json").exists());
        assert!(!partial.exists());
        assert_eq!(
            fs::read(state.shared.join("spawned.json")).unwrap(),
            b"provenance"
        );
    }

    #[test]
    fn an_incomplete_shared_copy_is_retried_without_publishing_partial_data() {
        let temp = tempfile::tempdir().unwrap();
        let state = resolve(None, Some(temp.path().join("service").into()), temp.path());
        private_dir(&state.trovador).unwrap();
        private_dir(&state.shared).unwrap();
        fs::write(state.trovador.join("spawned.json"), b"provenance").unwrap();
        let partial = state.shared.join("spawned.json.partial");
        fs::write(&partial, b"partial").unwrap();
        let state = prepare(state).unwrap();
        assert!(state.warning.is_none(), "{:?}", state.warning);
        assert!(!state.trovador.join("spawned.json").exists());
        assert!(!partial.exists());
        assert_eq!(
            fs::read(state.shared.join("spawned.json")).unwrap(),
            b"provenance"
        );
    }
}

#[cfg(test)]
mod retirement_recovery_tests {
    use super::*;

    #[test]
    fn a_crash_after_retirement_before_marker_cleanup_resumes_shared_moves() {
        let temp = tempfile::tempdir().unwrap();
        let state = resolve(None, None, temp.path());
        private_dir(&state.old).unwrap();
        fs::write(state.old.join("index.db"), b"index").unwrap();
        fs::write(state.old.join("spawned.json"), b"provenance").unwrap();
        private_dir(&state.root).unwrap();
        commit(&state, Some(&mut copy_tree)).unwrap();
        fs::rename(&state.old, state.old.with_extension("migrated")).unwrap();
        let state = prepare(state).unwrap();
        assert!(!state.root.join("migration-copy.json").exists());
        assert_eq!(
            fs::read(state.shared.join("spawned.json")).unwrap(),
            b"provenance"
        );
        assert_eq!(
            fs::read(state.old.with_extension("migrated").join("index.db")).unwrap(),
            b"index"
        );
        assert!(
            diagnostics(&state)
                .iter()
                .any(|d| d.what.contains("retired"))
        );
    }
}

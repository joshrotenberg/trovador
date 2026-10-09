use bottega_core::state::{self, State};
use std::{ffi::OsString, fs, path::Path};

fn guarded(root: &Path, state: State) -> State {
    for path in [
        &state.root,
        &state.trovador,
        &state.shared,
        &state.tosi,
        &state::provenance_path(&state.shared),
    ] {
        assert!(
            path.starts_with(root),
            "escaped temporary root: {}",
            path.display()
        );
    }
    state
}

#[test]
fn fresh_and_overridden_locations_stay_isolated() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let s = guarded(home, state::resolve(None, None, home));
    assert!(!s.root.join("tosi").exists());
    let s = guarded(home, state::prepare(s).unwrap());
    assert_eq!(s.trovador, home.join(".bottega/trovador"));
    assert!(s.shared.is_dir());
    assert!(s.tosi.is_dir());
    assert_eq!(s.tosi, home.join(".bottega/tosi"));
    assert_eq!(
        state::resolve(Some(OsString::new()), Some(OsString::new()), home).trovador,
        s.trovador
    );
    let s = guarded(
        home,
        state::resolve(
            Some(home.join("custom").into()),
            Some(home.join("service").into()),
            home,
        ),
    );
    assert_eq!(s.trovador, home.join("service"));
    assert_eq!(s.shared, home.join("custom/shared"));
}

fn populate(home: &Path) {
    let old = home.join(".trovador");
    fs::create_dir_all(old.join("summaries")).unwrap();
    for name in [
        "index.db",
        "summaries/a",
        "runs.jsonl",
        "spawned.json",
        "consent.json",
        "spawned.lock",
        "index.lock",
    ] {
        fs::write(old.join(name), name).unwrap();
    }
}

#[test]
fn concurrent_migration_preserves_every_file_and_private_permissions() {
    let temp = tempfile::tempdir().unwrap();
    populate(temp.path());
    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            guarded(
                temp.path(),
                state::prepare(state::resolve(None, None, temp.path())).unwrap(),
            )
        });
        let b = scope.spawn(|| {
            guarded(
                temp.path(),
                state::prepare(state::resolve(None, None, temp.path())).unwrap(),
            )
        });
        assert!(a.join().unwrap().warning.is_none());
        assert!(b.join().unwrap().warning.is_none());
    });
    let s = guarded(
        temp.path(),
        state::prepare(state::resolve(None, None, temp.path())).unwrap(),
    );
    for name in [
        "index.db",
        "summaries/a",
        "runs.jsonl",
        "spawned.json",
        "consent.json",
        "spawned.lock",
        "index.lock",
    ] {
        let dir = if name.starts_with("spawned") || name == "consent.json" {
            &s.shared
        } else {
            &s.trovador
        };
        let path = dir.join(name);
        assert_eq!(fs::read_to_string(&path).unwrap(), name);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in [
            &s.root,
            &s.shared,
            &s.trovador,
            &s.trovador.join("summaries"),
        ] {
            assert_eq!(
                fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
    assert!(!temp.path().join(".trovador").exists());
    populate(temp.path());
    assert!(
        state::diagnostics(&state::prepare(state::resolve(None, None, temp.path())).unwrap())
            .iter()
            .any(|d| d.what.contains("without merging"))
    );
    assert!(temp.path().join(".trovador/index.db").exists());
}

#[test]
fn overrides_skip_migration_and_conflicts_keep_committed_state_authoritative() {
    for service in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        populate(temp.path());
        let custom = Some(temp.path().join("custom").into_os_string());
        guarded(
            temp.path(),
            state::resolve(
                if service { None } else { custom.clone() },
                if service { custom } else { None },
                temp.path(),
            ),
        );
        assert!(temp.path().join(".trovador/spawned.json").exists());
    }
    let temp = tempfile::tempdir().unwrap();
    populate(temp.path());
    fs::create_dir_all(temp.path().join(".bottega/shared")).unwrap();
    fs::write(temp.path().join(".bottega/shared/spawned.json"), "existing").unwrap();
    let s = guarded(
        temp.path(),
        state::prepare(state::resolve(None, None, temp.path())).unwrap(),
    );
    assert_eq!(s.trovador, temp.path().join(".bottega/trovador"));
    assert_eq!(s.shared, temp.path().join(".bottega/shared"));
    assert!(s.warning.is_some());
    assert_eq!(
        fs::read_to_string(s.trovador.join("index.db")).unwrap(),
        "index.db"
    );
}

#[cfg(unix)]
#[test]
fn failed_copy_leaves_the_whole_old_directory_untouched() {
    let temp = tempfile::tempdir().unwrap();
    populate(temp.path());
    std::os::unix::fs::symlink("index.db", temp.path().join(".trovador/unsupported-link")).unwrap();
    let s = guarded(
        temp.path(),
        state::prepare_with_copy(state::resolve(None, None, temp.path()), state::copy_tree)
            .unwrap(),
    );
    assert_eq!(s.trovador, temp.path().join(".trovador"));
    assert_eq!(s.shared, s.trovador);
    assert!(s.warning.is_some());
    for name in [
        "index.db",
        "summaries/a",
        "runs.jsonl",
        "spawned.json",
        "consent.json",
    ] {
        assert_eq!(fs::read_to_string(s.trovador.join(name)).unwrap(), name);
    }
    assert!(!s.root.join("migration-stage").exists());
    assert!(!s.root.join("trovador").exists());
}

#[test]
fn resolution_never_creates_or_migrates_directories() {
    let temp = tempfile::tempdir().unwrap();
    populate(temp.path());
    let s = guarded(temp.path(), state::resolve(None, None, temp.path()));
    assert!(!s.root.exists());
    assert!(temp.path().join(".trovador/index.db").exists());
}

#[test]
fn every_committed_shared_move_resumes_after_a_crash() {
    for moved in 0..=state::SHARED.len() {
        let temp = tempfile::tempdir().unwrap();
        populate(temp.path());
        fs::write(temp.path().join(".trovador/consent.lock"), "consent.lock").unwrap();
        let s = state::resolve(None, None, temp.path());
        fs::create_dir_all(&s.root).unwrap();
        fs::rename(temp.path().join(".trovador"), &s.trovador).unwrap();
        fs::create_dir_all(&s.shared).unwrap();
        for name in &state::SHARED[..moved] {
            fs::rename(s.trovador.join(name), s.shared.join(name)).unwrap();
        }
        let s = guarded(temp.path(), state::prepare(s).unwrap());
        assert!(s.warning.is_none(), "{:?}", s.warning);
        assert_eq!(
            fs::read_to_string(s.trovador.join("index.db")).unwrap(),
            "index.db"
        );
        for name in state::SHARED {
            assert_eq!(fs::read_to_string(s.shared.join(name)).unwrap(), *name);
            assert!(!s.trovador.join(name).exists());
        }
    }
}

#[test]
fn crash_between_link_and_unlink_resumes_without_a_conflict() {
    let temp = tempfile::tempdir().unwrap();
    populate(temp.path());
    let s = state::resolve(None, None, temp.path());
    fs::create_dir_all(&s.shared).unwrap();
    fs::rename(temp.path().join(".trovador"), &s.trovador).unwrap();
    fs::hard_link(
        s.trovador.join("spawned.json"),
        s.shared.join("spawned.json"),
    )
    .unwrap();
    let s = guarded(temp.path(), state::prepare(s).unwrap());
    assert!(s.warning.is_none(), "{:?}", s.warning);
    assert!(!s.trovador.join("spawned.json").exists());
    assert_eq!(
        fs::read_to_string(s.shared.join("spawned.json")).unwrap(),
        "spawned.json"
    );
}

#[test]
fn incomplete_copy_retries_and_retires_without_deleting_the_source() {
    let temp = tempfile::tempdir().unwrap();
    populate(temp.path());
    let s = state::resolve(None, None, temp.path());
    fs::create_dir_all(s.root.join("trovador.partial")).unwrap();
    fs::write(s.root.join("trovador.partial/index.db"), "incomplete").unwrap();
    let s = guarded(
        temp.path(),
        state::prepare_with_copy(s, state::copy_tree).unwrap(),
    );
    assert!(!temp.path().join(".trovador").exists());
    assert!(!s.root.join("trovador.partial").exists());
    assert_eq!(
        fs::read_to_string(temp.path().join(".trovador.migrated/index.db")).unwrap(),
        "index.db"
    );
    assert_eq!(
        fs::read_to_string(s.shared.join("spawned.json")).unwrap(),
        "spawned.json"
    );
    assert!(
        state::diagnostics(&s)
            .iter()
            .any(|d| d.what.contains("retired"))
    );
}

#[test]
fn source_changes_during_copy_prevent_commit_and_next_start_retries() {
    let temp = tempfile::tempdir().unwrap();
    populate(temp.path());
    let error =
        state::prepare_with_copy(state::resolve(None, None, temp.path()), |source, target| {
            state::copy_tree(source, target)?;
            fs::write(source.join("index.db"), "changed!")?;
            fs::File::options()
                .write(true)
                .open(source.join("index.db"))?
                .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1))
        })
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    assert!(error.to_string().contains("source changed"));
    assert!(
        error
            .to_string()
            .contains("retry shortly or stop the old server")
    );
    assert!(!temp.path().join(".bottega/trovador").exists());
    assert_eq!(
        fs::read_to_string(temp.path().join(".trovador/index.db")).unwrap(),
        "changed!"
    );
    assert!(temp.path().join(".trovador/spawned.json").exists());
    let s = guarded(
        temp.path(),
        state::prepare_with_copy(state::resolve(None, None, temp.path()), state::copy_tree)
            .unwrap(),
    );
    assert_eq!(
        fs::read_to_string(s.trovador.join("index.db")).unwrap(),
        "changed!"
    );
}

#[test]
fn service_only_override_moves_provenance_to_the_default_shared_root() {
    let temp = tempfile::tempdir().unwrap();
    let service = temp.path().join("service");
    fs::create_dir(&service).unwrap();
    fs::write(service.join("spawned.json"), "provenance").unwrap();
    let s = guarded(
        temp.path(),
        state::prepare(state::resolve(
            None,
            Some(service.clone().into()),
            temp.path(),
        ))
        .unwrap(),
    );
    assert_eq!(s.trovador, service);
    assert_eq!(s.shared, temp.path().join(".bottega/shared"));
    assert_eq!(
        fs::read_to_string(s.shared.join("spawned.json")).unwrap(),
        "provenance"
    );
    assert!(!s.trovador.join("spawned.json").exists());
    fs::write(s.trovador.join("spawned.json"), "other").unwrap();
    let s = state::prepare(s).unwrap();
    assert!(s.warning.unwrap().contains("conflict"));
    assert_eq!(
        fs::read_to_string(s.shared.join("spawned.json")).unwrap(),
        "provenance"
    );
    assert_eq!(
        fs::read_to_string(s.trovador.join("spawned.json")).unwrap(),
        "other"
    );
}

#[test]
fn mismatched_test_root_panics_in_a_subprocess() {
    const CHILD: &str = "BOTTEGA_GUARD_CHILD";
    if std::env::var_os(CHILD).is_some() {
        state::from_env();
        panic!("guard failed to reject escaped paths");
    }
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mismatched_test_root_panics_in_a_subprocess",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("BOTTEGA_TEST_ROOT", temp.path().join("allowed"))
        .env("BOTTEGA_HOME", temp.path().join("escaped"))
        .env_remove("TROVADOR_HOME")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.contains("durable state escaped test root"),
        "{output}"
    );
    assert!(!temp.path().join("escaped").exists());
}

#[test]
fn a_truncated_copy_is_rejected_before_the_commit_point() {
    let temp = tempfile::tempdir().unwrap();
    populate(temp.path());
    let error =
        state::prepare_with_copy(state::resolve(None, None, temp.path()), |source, target| {
            state::copy_tree(source, target)?;
            fs::write(target.join("index.db"), "short")
        })
        .unwrap_err();
    assert!(!temp.path().join(".bottega/trovador").exists());
    assert!(error.to_string().contains("size mismatch"));
    assert_eq!(
        fs::read_to_string(temp.path().join(".trovador/index.db")).unwrap(),
        "index.db"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join(".trovador/spawned.json")).unwrap(),
        "spawned.json"
    );
}

#[cfg(unix)]
#[test]
fn override_roots_preserve_unrelated_files_and_nested_directory_modes() {
    use std::os::unix::fs::PermissionsExt;
    for (root_override, service_override) in [(true, false), (false, true), (true, true)] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let service = temp.path().join("service");
        let resolved = state::resolve(
            root_override.then(|| root.into()),
            service_override.then(|| service.into()),
            temp.path(),
        );
        let mut unrelated = Vec::new();
        for dir in [&resolved.root, &resolved.trovador, &resolved.shared] {
            fs::create_dir_all(dir.join("user-data")).unwrap();
            let file = dir.join("user-data/executable");
            fs::write(&file, b"user file").unwrap();
            for path in [&file, &dir.join("user-data")] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
                unrelated.push(path.clone());
            }
        }
        let s = guarded(temp.path(), state::prepare(resolved).unwrap());
        let s = guarded(temp.path(), state::prepare(s).unwrap());
        for path in unrelated {
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o755,
                "{}",
                path.display()
            );
        }
        for dir in [&s.root, &s.trovador, &s.shared] {
            assert_eq!(
                fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn ordinary_default_start_never_retightens_an_existing_tree() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    populate(temp.path());
    let s = state::prepare(state::resolve(None, None, temp.path())).unwrap();
    for path in [
        s.trovador.join("index.db"),
        s.shared.join("spawned.json"),
        s.trovador.join("summaries"),
    ] {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let next = state::prepare(s.clone()).unwrap();
        assert!(next.warning.is_none());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}

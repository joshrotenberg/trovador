//! The record of sessions the toolkit started: what list and search use to include them.

mod common;

use common::*;
use serde_json::json;
use trovador::Backend;
use trovador::provenance;

#[test]
fn a_session_is_recorded_once() {
    let f = Fixture::new();
    for _ in 0..2 {
        provenance::record_spawned(&f.paths, Backend::Claude, ID_A).unwrap();
    }
    assert_eq!(
        provenance::spawned_keys(&f.paths),
        [format!("claude/{ID_A}")]
    );
    assert!(provenance::is_spawned(&f.paths, Backend::Claude, ID_A));
    assert!(!provenance::is_spawned(&f.paths, Backend::Codex, ID_A));
    assert!(provenance::record_spawned(&f.paths, Backend::Claude, "").is_err());
}

#[test]
fn an_invalid_record_is_never_overwritten() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.paths.home).unwrap();
    let path = provenance::path(&f.paths);
    std::fs::write(&path, "{ not json").unwrap();
    assert!(provenance::record_spawned(&f.paths, Backend::Claude, ID_A).is_err());
    assert!(provenance::spawned_keys(&f.paths).is_empty());
    assert!(provenance::status(&f.paths).error.is_some());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
}

#[cfg(unix)]
#[test]
fn the_record_is_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    provenance::record_spawned(&f.paths, Backend::Claude, ID_A).unwrap();
    let mode = std::fs::metadata(provenance::path(&f.paths))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn spawned_records_carry_over_from_the_old_consent_file() {
    // Before consent was removed, the same records were consent.json's spawned list.
    let f = Fixture::new();
    std::fs::create_dir_all(&f.paths.home).unwrap();
    std::fs::write(
        f.paths.shared.join("consent.json"),
        json!({"version": 1, "level": "approved",
            "spawned": [{"backend": "codex", "session_id": ID_B, "at": "2026-10-08T00:00:00.000Z"}],
            "decisions": [{"backend": "claude", "session_id": ID_C, "decision": "denied",
                           "at": "2026-10-08T00:00:00.000Z"}]})
        .to_string(),
    )
    .unwrap();
    assert!(provenance::is_spawned(&f.paths, Backend::Codex, ID_B));
    assert!(
        !provenance::path(&f.paths).exists(),
        "reading writes nothing"
    );

    provenance::record_spawned(&f.paths, Backend::Claude, ID_A).unwrap();
    let mut keys = provenance::spawned_keys(&f.paths);
    keys.sort();
    assert_eq!(keys, [format!("claude/{ID_A}"), format!("codex/{ID_B}")]);
    assert!(provenance::path(&f.paths).exists());
}

#[test]
fn show_says_whether_the_toolkit_started_a_session() {
    let f = Fixture::new();
    let ctx = f.ctx();
    f.claude("-w", ID_A, &[claude_user("/", "mine")], 0);
    f.claude("-w", ID_B, &[claude_user("/", "theirs")], 0);
    provenance::record_spawned(&ctx.paths, Backend::Claude, ID_A).unwrap();
    assert!(trovador::sessions::show(&ctx, ID_A, None).unwrap().spawned);
    assert!(!trovador::sessions::show(&ctx, ID_B, None).unwrap().spawned);
}

#[test]
fn an_import_with_an_invalid_id_changes_nothing_even_in_a_dry_run() {
    let f = Fixture::new();
    provenance::record_spawned(&f.paths, Backend::Claude, ID_C).unwrap();
    let before = std::fs::read_to_string(provenance::path(&f.paths)).unwrap();
    let spike = f.dir.path().join("spike");
    std::fs::create_dir_all(&spike).unwrap();
    std::fs::write(
        spike.join("consent.json"),
        json!({"level": "all", "spawned": [{"backend": "claude", "session_id": ID_A},
                                           {"backend": "claude", "session_id": ""}]})
        .to_string(),
    )
    .unwrap();
    for dry_run in [true, false] {
        let err = trovador::import::import(&f.ctx(), &spike, dry_run).unwrap_err();
        assert!(err.to_string().contains("invalid session id"), "{err}");
    }
    assert_eq!(
        std::fs::read_to_string(provenance::path(&f.paths)).unwrap(),
        before
    );
}

//! Consent choices made here survive an import, and an import is all or nothing.

mod common;

use common::*;
use serde_json::{Value, json};
use trovador::Backend;
use trovador::consent::{self, Decision, Level, Source};
use trovador::import;

fn spike_with(f: &Fixture, consent: Value) -> std::path::PathBuf {
    let d = f.dir.path().join("spike");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("consent.json"), consent.to_string()).unwrap();
    d
}

fn state(f: &Fixture) -> Option<String> {
    std::fs::read_to_string(consent::path(&f.paths)).ok()
}

#[test]
fn choosing_the_default_policy_is_recorded_and_an_import_keeps_it() {
    // #29: `consent policy spawned` in a fresh home wrote nothing, so a later import
    // adopted spike's `all`.
    let f = Fixture::new();
    let status = consent::set_level(&f.paths, Level::Spawned).unwrap();
    assert_eq!(status.source, Source::File);
    assert!(consent::path(&f.paths).exists());

    let spike = spike_with(&f, json!({"level": "all", "spawned": [], "decisions": []}));
    let r = import::import(&f.ctx(), &spike, false).unwrap();
    assert!(!r.level_adopted);
    assert_eq!(consent::status(&f.paths).level, Level::Spawned);
}

#[test]
fn an_import_with_an_invalid_id_changes_nothing_even_in_a_dry_run() {
    // #30: the level was written before the ids were checked, and a dry run checked none.
    let bad_spawned = json!({"level": "all",
        "spawned": [{"backend": "claude", "session_id": ID_A}, {"backend": "claude", "session_id": ""}],
        "decisions": []});
    let bad_decision = json!({"level": "all",
        "spawned": [{"backend": "claude", "session_id": ID_A}],
        "decisions": [{"backend": "claude", "session_id": ID_B, "decision": "approved"},
                      {"backend": "codex", "session_id": "a\u{0}b", "decision": "denied"}]});
    for incoming in [bad_spawned, bad_decision] {
        // A fresh home stays fresh.
        let f = Fixture::new();
        let spike = spike_with(&f, incoming.clone());
        for dry_run in [true, false] {
            let err = import::import(&f.ctx(), &spike, dry_run).unwrap_err();
            assert!(err.to_string().contains("invalid session id"), "{err}");
        }
        assert_eq!(state(&f), None, "{incoming}");
        assert_eq!(consent::status(&f.paths).source, Source::Default);

        // Existing state is left byte for byte.
        let f = Fixture::new();
        consent::set_level(&f.paths, Level::Approved).unwrap();
        consent::set_decision(&f.paths, Backend::Claude, ID_C, Some(Decision::Denied)).unwrap();
        let before = state(&f);
        let spike = spike_with(&f, incoming.clone());
        for dry_run in [true, false] {
            assert!(import::import(&f.ctx(), &spike, dry_run).is_err());
        }
        assert_eq!(state(&f), before, "{incoming}");
    }
}

#[test]
fn a_dry_run_reports_what_the_import_then_does() {
    let f = Fixture::new();
    consent::set_decision(&f.paths, Backend::Claude, ID_B, Some(Decision::Denied)).unwrap();
    let spike = spike_with(
        &f,
        json!({"level": "approved",
            "spawned": [{"backend": "claude", "session_id": ID_A}, {"backend": "claude", "session_id": ID_A}],
            "decisions": [{"backend": "claude", "session_id": ID_B, "decision": "approved"},
                          {"backend": "codex", "session_id": ID_C, "decision": "approved"}]}),
    );
    let counts = |r: &import::ImportReport| {
        (
            r.level_adopted,
            r.spawned_added,
            r.spawned_present,
            r.decisions_added,
            r.decisions_kept_trovador,
        )
    };
    let before = state(&f);
    let dry = import::import(&f.ctx(), &spike, true).unwrap();
    assert_eq!(state(&f), before, "a dry run writes nothing");
    let real = import::import(&f.ctx(), &spike, false).unwrap();
    // The decision made here for ID_B recorded a level of its own, so spike's is not
    // adopted; the duplicate spawned record counts once as added and once as present.
    assert_eq!(counts(&dry), counts(&real));
    assert_eq!(counts(&real), (false, 1, 1, 1, 1));
    assert!(consent::check(&f.paths, Backend::Claude, ID_A).spawned);
    assert_eq!(
        consent::check(&f.paths, Backend::Claude, ID_B).decision,
        Some(Decision::Denied)
    );
    assert_eq!(
        consent::check(&f.paths, Backend::Codex, ID_C).decision,
        Some(Decision::Approved)
    );
}

// --- moved from tests/ask.rs when ask left trovador; continuing a session is tosi's, and
// --- tosi decides with consent::check, so the rules are tested here directly.

#[test]
fn a_session_trovador_did_not_start_needs_a_person_to_approve_it() {
    let f = Fixture::new();
    let allowed = || consent::check(&f.paths, Backend::Claude, ID_A);
    let first = allowed();
    assert!(!first.allowed);
    assert_eq!(first.level, Level::Spawned);
    assert!(
        first
            .next_steps
            .iter()
            .any(|s| s.contains("trovador consent approve"))
    );

    consent::set_decision(&f.paths, Backend::Claude, ID_A, Some(Decision::Approved)).unwrap();
    assert!(!allowed().allowed, "approval needs level approved");
    consent::set_level(&f.paths, Level::Approved).unwrap();
    assert!(allowed().allowed);

    // An explicit denial wins over every level.
    consent::set_decision(&f.paths, Backend::Claude, ID_A, Some(Decision::Denied)).unwrap();
    consent::set_level(&f.paths, Level::All).unwrap();
    assert!(!allowed().allowed);
    consent::set_decision(&f.paths, Backend::Claude, ID_A, None).unwrap();
    assert!(allowed().allowed);
}

#[test]
fn invalid_consent_state_denies_everything_and_is_never_overwritten() {
    let f = Fixture::new();
    consent::record_spawned(&f.paths, Backend::Claude, ID_A).unwrap();
    let path = consent::path(&f.paths);
    std::fs::write(&path, "{ not json").unwrap();

    let check = consent::check(&f.paths, Backend::Claude, ID_A);
    assert!(!check.allowed, "even a session the toolkit started");
    assert_eq!(check.source, Source::Invalid);
    assert!(consent::set_level(&f.paths, Level::All).is_err());
    assert!(consent::record_spawned(&f.paths, Backend::Claude, ID_B).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
}

#[cfg(unix)]
#[test]
fn the_consent_file_is_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    consent::record_spawned(&f.paths, Backend::Claude, ID_A).unwrap();
    let mode = std::fs::metadata(consent::path(&f.paths))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn show_reports_whether_a_session_may_be_continued() {
    let f = Fixture::new();
    let ctx = f.ctx();
    f.claude("-w", ID_A, &[claude_user("/", "mine")], 0);
    f.claude("-w", ID_B, &[claude_user("/", "theirs")], 0);
    consent::record_spawned(&ctx.paths, Backend::Claude, ID_A).unwrap();
    assert!(
        trovador::sessions::show(&ctx, ID_A, None)
            .unwrap()
            .consent
            .allowed
    );
    assert!(
        !trovador::sessions::show(&ctx, ID_B, None)
            .unwrap()
            .consent
            .allowed
    );
}

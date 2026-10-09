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

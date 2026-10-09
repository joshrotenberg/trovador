mod common;

use std::time::Duration;

use common::*;
use trovador::summary;

#[test]
fn unavailable_archive_roots_keep_summaries_in_dry_run_and_apply() {
    for root_index in 0..3 {
        let f = Fixture::new().with_fakes();
        for root in [
            &f.paths.claude_archive,
            &f.paths.codex_archive,
            &f.paths.codex_archived,
        ] {
            std::fs::create_dir_all(root).unwrap();
        }
        match root_index {
            0 => f.claude("-w", ID_A, &[claude_user("/w", "hello")], 100),
            1 => f.codex(ID_A, &[codex_meta("/w", serde_json::json!("cli"))], 100),
            _ => f.codex_archived(ID_A, &[codex_meta("/w", serde_json::json!("cli"))], 100),
        };
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(summary::summarize(&f.ctx(), ID_A, None, false, None))
            .unwrap();
        let root = [
            &f.paths.claude_archive,
            &f.paths.codex_archive,
            &f.paths.codex_archived,
        ][root_index];
        let backend = if root_index == 0 { "claude" } else { "codex" };
        let cache = summary::dir(&f.ctx()).join(format!("{backend}-{ID_A}.json"));
        let before = std::fs::read(&cache).unwrap();
        std::fs::rename(root, f.dir.path().join("unmounted")).unwrap();
        std::fs::write(root, "unavailable mount").unwrap();
        assert!(std::fs::read_dir(root).is_err());
        for apply in [false, true] {
            let report = trovador::gc::gc(&f.ctx(), Duration::ZERO, apply).unwrap();
            assert_eq!(report.summaries.session_gone, 0);
            assert_eq!(report.summaries.kept, 1);
            assert_eq!(report.summaries.old, 0);
            assert_eq!(report.summaries.stale, 0);
            assert_eq!(report.summaries.bytes_removed, 0);
            assert_eq!(std::fs::read(&cache).unwrap(), before);
            let json = serde_json::to_value(report).unwrap();
            assert_eq!(
                json["unavailable_roots"],
                serde_json::json!([root.display().to_string()])
            );
        }
    }
}

#[test]
fn unavailable_claude_root_does_not_protect_deleted_codex_sessions() {
    let f = Fixture::new().with_fakes();
    f.claude("-w", ID_A, &[claude_user("/w", "hello")], 100);
    let codex = f.codex(ID_B, &[codex_meta("/w", serde_json::json!("cli"))], 100);
    f.codex(ID_C, &[codex_meta("/w", serde_json::json!("cli"))], 100);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for id in [ID_A, ID_B] {
        runtime
            .block_on(summary::summarize(&f.ctx(), id, None, false, None))
            .unwrap();
    }
    std::fs::rename(&f.paths.claude_archive, f.dir.path().join("unmounted")).unwrap();
    std::fs::remove_file(codex).unwrap();
    for apply in [false, true] {
        let report = trovador::gc::gc(&f.ctx(), Duration::MAX, apply).unwrap();
        assert_eq!(report.summaries.kept, 1);
        assert_eq!(report.summaries.session_gone, 1);
        assert!(report.summaries.bytes_removed > 0);
        assert_eq!(
            report.unavailable_roots,
            vec![f.paths.claude_archive.display().to_string()]
        );
        assert!(
            summary::dir(&f.ctx())
                .join(format!("claude-{ID_A}.json"))
                .exists()
        );
        assert_eq!(
            summary::dir(&f.ctx())
                .join(format!("codex-{ID_B}.json"))
                .exists(),
            !apply
        );
    }
}

#[cfg(unix)]
struct RestorePermissions(std::path::PathBuf, std::fs::Permissions);

#[cfg(unix)]
impl Drop for RestorePermissions {
    fn drop(&mut self) {
        std::fs::set_permissions(&self.0, self.1.clone()).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn partial_scans_keep_missing_and_current_summaries_and_report_failed_paths() {
    use std::os::unix::fs::PermissionsExt;
    for case in 0..3 {
        let f = Fixture::new().with_fakes();
        let (blocked, backend) = if case < 2 {
            f.claude("-blocked", ID_A, &[claude_user("/w", "hello")], 100);
            f.claude("-readable", ID_B, &[claude_user("/w", "hello")], 100);
            (
                if case == 0 {
                    f.paths.claude_archive.clone()
                } else {
                    f.paths.claude_archive.join("-blocked")
                },
                "claude",
            )
        } else {
            f.codex(ID_A, &[codex_meta("/w", serde_json::json!("cli"))], 100);
            f.codex_archived(ID_B, &[codex_meta("/w", serde_json::json!("cli"))], 100);
            (f.paths.codex_archive.join("2026/10/07"), "codex")
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for id in [ID_A, ID_B] {
            runtime
                .block_on(summary::summarize(&f.ctx(), id, None, false, None))
                .unwrap();
        }
        let caches: Vec<_> = [ID_A, ID_B]
            .into_iter()
            .map(|id| {
                let path = summary::dir(&f.ctx()).join(format!("{backend}-{id}.json"));
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        let restore = RestorePermissions(
            blocked.clone(),
            std::fs::metadata(&blocked).unwrap().permissions(),
        );
        std::fs::set_permissions(
            &blocked,
            std::fs::Permissions::from_mode(if case == 0 { 0o444 } else { 0o000 }),
        )
        .unwrap();
        let report = trovador::gc::gc(&f.ctx(), Duration::MAX, true).unwrap();
        drop(restore);
        assert_eq!(report.summaries.kept, 2);
        assert_eq!(report.summaries.session_gone, 0);
        assert_eq!(report.summaries.old, 0);
        assert_eq!(report.summaries.stale, 0);
        assert_eq!(report.summaries.bytes_removed, 0);
        assert!(!report.unavailable_roots.is_empty());
        assert!(
            report
                .unavailable_roots
                .iter()
                .all(|p| std::path::Path::new(p).starts_with(&blocked))
        );
        for (path, bytes) in caches {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn empty_backend_keeps_summaries_and_reports_why() {
    let f = Fixture::new().with_fakes();
    f.claude("-w", ID_A, &[claude_user("/w", "hello")], 100);
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(summary::summarize(&f.ctx(), ID_A, None, false, None))
        .unwrap();
    let cache = summary::dir(&f.ctx()).join(format!("claude-{ID_A}.json"));
    let before = std::fs::read(&cache).unwrap();
    std::fs::remove_dir_all(f.paths.claude_archive.join("-w")).unwrap();
    let report = trovador::gc::gc(&f.ctx(), Duration::ZERO, true).unwrap();
    assert_eq!(report.summaries.kept, 1);
    assert_eq!(report.summaries.session_gone, 0);
    assert_eq!(report.summaries.bytes_removed, 0);
    assert_eq!(
        report.empty_backends,
        vec![trovador::archive::Backend::Claude]
    );
    assert!(report.unavailable_roots.is_empty());
    assert_eq!(std::fs::read(cache).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn found_stale_and_old_summaries_are_removed_despite_an_unreadable_project() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().with_fakes();
    f.claude("-blocked", ID_A, &[claude_user("/w", "blocked")], 100);
    let stale = f.claude("-readable", ID_B, &[claude_user("/w", "stale")], 100);
    f.claude("-readable", ID_C, &[claude_user("/w", "old")], 100);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for id in [ID_A, ID_B, ID_C] {
        runtime
            .block_on(summary::summarize(&f.ctx(), id, None, false, None))
            .unwrap();
    }
    let cache = |id| summary::dir(&f.ctx()).join(format!("claude-{id}.json"));
    let blocked_bytes = std::fs::read(cache(ID_A)).unwrap();
    std::fs::write(stale, "changed source").unwrap();
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cache(ID_C)).unwrap()).unwrap();
    old["created"] = serde_json::json!("2025-01-01T00:00:00.000Z");
    std::fs::write(cache(ID_C), old.to_string()).unwrap();
    let blocked = f.paths.claude_archive.join("-blocked");
    let restore = RestorePermissions(
        blocked.clone(),
        std::fs::metadata(&blocked).unwrap().permissions(),
    );
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let report = trovador::gc::gc(&f.ctx(), Duration::from_secs(90 * 86_400), true).unwrap();
    drop(restore);
    assert_eq!(report.summaries.kept, 1);
    assert_eq!(report.summaries.session_gone, 0);
    assert_eq!(report.summaries.stale, 1);
    assert_eq!(report.summaries.old, 1);
    assert_eq!(
        report.unavailable_roots,
        vec![blocked.display().to_string()]
    );
    assert_eq!(std::fs::read(cache(ID_A)).unwrap(), blocked_bytes);
    assert!(!cache(ID_B).exists());
    assert!(!cache(ID_C).exists());
}

#[cfg(unix)]
#[test]
fn dangling_project_symlink_does_not_protect_deleted_sessions() {
    let f = Fixture::new().with_fakes();
    let gone = f.claude("-w", ID_A, &[claude_user("/w", "gone")], 100);
    f.claude("-w", ID_B, &[claude_user("/w", "live")], 100);
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(summary::summarize(&f.ctx(), ID_A, None, false, None))
        .unwrap();
    std::fs::remove_file(gone).unwrap();
    std::os::unix::fs::symlink(
        f.dir.path().join("missing"),
        f.paths.claude_archive.join("-dangling"),
    )
    .unwrap();
    let report = trovador::gc::gc(&f.ctx(), Duration::MAX, true).unwrap();
    assert_eq!(report.summaries.session_gone, 1);
    assert!(report.unavailable_roots.is_empty());
    assert!(report.empty_backends.is_empty());
    assert!(
        !summary::dir(&f.ctx())
            .join(format!("claude-{ID_A}.json"))
            .exists()
    );
}

mod common;

use std::io::Write;

use common::*;
use serde_json::json;
use trovador::consent::{self, Decision, Level};
use trovador::sessions::Ctx;
use trovador::summary::{self, Summary};
use trovador::{Backend, Limits};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().unwrap()
}

fn summarize(ctx: &Ctx, id: &str, force: bool) -> Summary {
    rt().block_on(summary::summarize(ctx, id, None, force, None))
        .unwrap()
}

fn session_with(f: &Fixture, id: &str, turns: usize) -> std::path::PathBuf {
    let mut records = vec![claude_user("/w", "build the parser")];
    records.extend((0..turns).map(|i| claude_assistant(&format!("step {i} {}", "x".repeat(900)))));
    f.claude("-w", id, &records, 0)
}

// --- summaries -------------------------------------------------------------

#[test]
fn a_small_session_is_summarized_in_one_call_and_then_served_from_the_cache() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    session_with(&f, ID_A, 3);
    let s = summarize(&ctx, ID_A, false);
    assert_eq!(s.summary, "single summary");
    assert!(!s.cached);
    assert_eq!((s.chunks, s.chunks_summarized), (1, 1));
    assert!(s.cost_usd > 0.0);
    assert!(f.prompts("claude")[0].contains("USER: build the parser"));

    let again = summarize(&ctx, ID_A, false);
    assert!(again.cached);
    assert_eq!(again.cost_usd, 0.0);
    assert_eq!(f.calls("claude").len(), 1, "a cache hit runs nothing");

    let forced = summarize(&ctx, ID_A, true);
    assert!(!forced.cached);
    assert_eq!(f.calls("claude").len(), 2);
}

#[test]
fn an_append_makes_the_cached_summary_stale() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let path = session_with(&f, ID_A, 2);
    summarize(&ctx, ID_A, false);
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{}", claude_assistant("more work")).unwrap();
    assert!(!summarize(&ctx, ID_A, false).cached);
}

#[test]
fn summary_runs_leave_no_session_behind_and_skip_tools() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    session_with(&f, ID_A, 2);
    summarize(&ctx, ID_A, false);
    assert!(!f.paths.claude_archive.join("-fake").exists());
    let call = &f.calls("claude")[0];
    assert!(call.iter().any(|l| l == "arg=--no-session-persistence"));
    assert!(call.iter().any(|l| l == "arg=haiku"));
    assert_eq!(consent::status(&ctx.paths).spawned, 0);
    let log = std::fs::read_to_string(ctx.paths.home.join("runs.jsonl")).unwrap();
    assert!(log.contains("\"verb\":\"summarize\""));
}

#[test]
fn a_long_session_is_summarized_in_parts_then_merged() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx_with(Limits {
        summary_chunk_chars: 2_000,
        ..limits()
    });
    session_with(&f, ID_A, 5);
    let s = summarize(&ctx, ID_A, false);
    assert_eq!(s.summary, "merged summary");
    assert!(s.chunks > 1);
    assert_eq!(s.chunks, s.chunks_summarized);
    let prompts = f.prompts("claude");
    assert_eq!(prompts.len(), s.chunks + 1);
    let merge = prompts
        .iter()
        .find(|p| p.starts_with("These are sequential partial summaries"))
        .unwrap();
    assert!(!merge.contains("left out"));
}

#[test]
fn past_the_chunk_limit_the_start_and_end_are_kept_and_the_gap_is_named() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx_with(Limits {
        summary_chunk_chars: 1_000,
        summary_max_chunks: 4,
        ..limits()
    });
    session_with(&f, ID_A, 12);
    let s = summarize(&ctx, ID_A, false);
    assert!(s.chunks > 4);
    assert_eq!(s.chunks_summarized, 4);
    let prompts = f.prompts("claude");
    assert_eq!(prompts.len(), 5);
    let (merges, parts): (Vec<_>, Vec<_>) = prompts
        .iter()
        .partition(|p| p.starts_with("These are sequential"));
    assert_eq!((merges.len(), parts.len()), (1, 4));
    let firsts: Vec<_> = parts.iter().map(|p| p.lines().next().unwrap()).collect();
    assert!(firsts.iter().any(|p| p.starts_with("This is part 1 of")));
    assert!(
        firsts
            .iter()
            .any(|p| p.starts_with(&format!("This is part {} of", s.chunks)))
    );
    assert!(merges[0].contains(&format!(
        "Parts 3 to {} of the original were left out",
        s.chunks - 2
    )));
}

// --- consult ---------------------------------------------------------------

// --- import ----------------------------------------------------------------

fn spike_dir(f: &Fixture) -> std::path::PathBuf {
    let d = f.dir.path().join(".agent-sessions");
    std::fs::create_dir_all(d.join("summaries")).unwrap();
    std::fs::create_dir_all(d.join("pads")).unwrap();
    std::fs::write(d.join("pads").join("x.json"), "{}").unwrap();
    std::fs::write(d.join("standing.json"), r#"{"a": {}, "b": {}}"#).unwrap();
    std::fs::write(
        d.join("consent.json"),
        json!({"version": 1, "level": "approved",
            "spawned": [{"backend": "claude", "session_id": ID_A, "at": "2026-08-14T00:00:00.000Z"}],
            "decisions": [
                {"backend": "claude", "session_id": ID_B, "decision": "approved", "at": "2026-08-14T00:00:00.000Z"},
                {"backend": "codex", "session_id": ID_C, "decision": "approved", "at": "2026-08-14T00:00:00.000Z"}
            ]})
        .to_string(),
    )
    .unwrap();
    d
}

fn spike_summary(dir: &std::path::Path, id: &str, source_mtime_ms: f64) {
    std::fs::write(
        dir.join("summaries").join(format!("{id}.json")),
        json!({"session_id": id, "backend": "claude", "project": "/w", "source_mtime": source_mtime_ms,
            "chunks": 1, "transcript_chars": 10, "cost_usd": 0.002,
            "created": "2026-08-14T00:00:00.000Z", "summary": "from spike"})
        .to_string(),
    )
    .unwrap();
}

fn snapshot(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else {
                out.push((
                    e.path().display().to_string(),
                    std::fs::read(e.path()).unwrap(),
                ));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn import_merges_spike_consent_and_fresh_summaries_without_touching_spike() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let fresh = f.claude("-w", ID_A, &[claude_user("/w", "a")], 100);
    f.claude("-w", ID_B, &[claude_user("/w", "b")], 0);
    let spike = spike_dir(&f);
    let mtime_ms = |p: &std::path::Path| {
        std::fs::metadata(p)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as f64
    };
    spike_summary(&spike, ID_A, mtime_ms(&fresh));
    // Summarized before the session was last written: stale.
    spike_summary(&spike, ID_B, 1.0);
    spike_summary(&spike, ID_D, 1.0);
    let before = snapshot(&spike);

    let dry = trovador::import::import(&ctx, &spike, true).unwrap();
    assert_eq!(dry.spawned_added, 1);
    assert!(
        !consent::path(&ctx.paths).exists(),
        "a dry run changes nothing"
    );

    let r = trovador::import::import(&ctx, &spike, false).unwrap();
    assert_eq!(r.level, Some(Level::Approved));
    assert!(r.level_adopted);
    assert_eq!((r.spawned_added, r.decisions_added), (1, 2));
    assert_eq!((r.summaries_imported, r.summaries_stale), (1, 2));
    assert_eq!(r.not_imported.get("pads"), Some(&1));
    assert_eq!(r.not_imported.get("standing"), Some(&2));
    assert_eq!(
        snapshot(&spike),
        before,
        "spike's directory is never written"
    );

    assert!(consent::check(&ctx.paths, Backend::Claude, ID_A).spawned);
    assert!(consent::check(&ctx.paths, Backend::Claude, ID_B).allowed);
    let e = trovador::archive::find_session(&ctx.paths, ID_A, None).unwrap();
    assert_eq!(summary::cached(&ctx, &e).unwrap().summary, "from spike");

    let again = trovador::import::import(&ctx, &spike, false).unwrap();
    assert_eq!(
        (
            again.spawned_added,
            again.decisions_added,
            again.summaries_imported
        ),
        (0, 0, 0)
    );
    assert_eq!((again.spawned_present, again.summaries_present), (1, 1));
}

#[test]
fn import_never_overrides_a_policy_or_decision_already_chosen_here() {
    let f = Fixture::new();
    let ctx = f.ctx();
    consent::set_level(&ctx.paths, Level::Spawned).unwrap();
    consent::set_decision(&ctx.paths, Backend::Claude, ID_B, Some(Decision::Denied)).unwrap();
    let spike = spike_dir(&f);
    let r = trovador::import::import(&ctx, &spike, false).unwrap();
    assert!(!r.level_adopted);
    assert_eq!(r.decisions_kept_trovador, 1);
    assert_eq!(consent::status(&ctx.paths).level, Level::Spawned);
    assert_eq!(
        consent::check(&ctx.paths, Backend::Claude, ID_B).decision,
        Some(Decision::Denied)
    );
}

#[test]
fn import_from_a_directory_that_is_not_spike_is_refused() {
    let f = Fixture::new();
    let r = trovador::import::import(&f.ctx(), f.dir.path(), false);
    assert!(r.is_err());
}

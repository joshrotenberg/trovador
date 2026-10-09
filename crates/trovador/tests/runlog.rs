//! `runs.jsonl` stays valid JSONL under concurrent runs, and every record names its run.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use common::*;
use serde_json::{Value, json};
use trovador::run::{self, AskOpts, RunError};
use trovador::runlog;
use trovador::sessions::Ctx;

/// Every line parsed; a line that is not JSON fails the test with its text.
fn records(ctx: &Ctx) -> Vec<Value> {
    std::fs::read_to_string(runlog::path(&ctx.paths))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
        .collect()
}

fn by_run(records: &[Value]) -> HashMap<String, Vec<Value>> {
    let mut out: HashMap<String, Vec<Value>> = HashMap::new();
    for r in records {
        let id = r["run_id"].as_str().expect("every record has a run_id");
        out.entry(id.to_string()).or_default().push(r.clone());
    }
    out
}

fn prompt(p: &str) -> AskOpts {
    AskOpts {
        prompt: p.into(),
        ..Default::default()
    }
}

#[test]
fn overlapping_turns_in_one_session_are_told_apart_by_run_id() {
    // #18 and #21: two asks through one server shared a session, pid, and start time,
    // and their start records interleaved into lines that were not JSON.
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let seed = rt.block_on(run::ask(&ctx, &prompt("seed"))).unwrap();
    let session = seed.session_id.clone().unwrap();

    // The first half takes longest, so runs end in a different order than they start.
    let opts: Vec<AskOpts> = (0..8)
        .map(|i| AskOpts {
            session: Some(session.clone()),
            ..prompt(&format!(
                "{} {i}",
                if i < 4 { "nap-long" } else { "nap-short" }
            ))
        })
        .collect();
    let results = rt.block_on(futures::future::join_all(
        opts.iter().map(|o| run::ask(&ctx, o)),
    ));

    let runs = by_run(&records(&ctx));
    assert_eq!(runs.len(), 9, "the seed and eight turns");
    for r in results {
        let r = r.unwrap();
        assert_eq!(r.session_id.as_deref(), Some(session.as_str()));
        let phases: Vec<&str> = runs[&r.run_id]
            .iter()
            .map(|x| x["phase"].as_str().unwrap())
            .collect();
        assert_eq!(phases, ["start", "end"], "{}", r.run_id);
        let end = &runs[&r.run_id][1];
        assert_eq!(end["session"], json!(session));
        assert_eq!(end["outcome"], "ok");
    }
}

#[test]
fn a_timeout_and_a_spawn_failure_keep_their_run_id() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let slow = rt
        .block_on(run::ask(
            &ctx,
            &AskOpts {
                timeout_s: Some(1),
                ..prompt("slow")
            },
        ))
        .unwrap();
    assert!(slow.timed_out);
    let runs = by_run(&records(&ctx));
    assert_eq!(runs[&slow.run_id][1]["outcome"], "timeout");

    let mut broken = f.ctx();
    broken.paths.codex_bin = Some(f.dir.path().join("no-such-codex"));
    let err = rt
        .block_on(run::ask(
            &broken,
            &AskOpts {
                backend: Some(trovador::Backend::Codex),
                ..prompt("hello")
            },
        ))
        .unwrap_err();
    let RunError::Spawn { run_id, .. } = &err else {
        panic!("{err}");
    };
    assert!(err.to_string().contains(run_id.as_str()), "{err}");
    let runs = by_run(&records(&ctx));
    let phases: Vec<&str> = runs[run_id]
        .iter()
        .map(|x| x["phase"].as_str().unwrap())
        .collect();
    assert_eq!(phases, ["start", "end"]);
    assert_eq!(runs[run_id][1]["outcome"], "spawn_failed");
}

#[test]
fn large_records_written_from_many_threads_stay_whole() {
    let f = Fixture::new();
    let paths = f.paths.clone();
    // Well past the size a single append is atomic for on any platform.
    let big = "x".repeat(64 * 1024);
    std::thread::scope(|s| {
        for t in 0..8 {
            let (paths, big) = (&paths, &big);
            s.spawn(move || {
                for i in 0..25 {
                    runlog::append(paths, json!({"run_id": format!("{t}-{i}"), "pad": big}));
                }
            });
        }
    });
    let all = records(&f.ctx());
    assert_eq!(all.len(), 200);
    assert_eq!(by_run(&all).len(), 200);
}

#[test]
fn gc_rewriting_the_log_loses_no_record_appended_meanwhile() {
    let f = Fixture::new();
    let ctx = f.ctx();
    let old: String = (0..2000)
        .map(|i| {
            format!(
                "{}\n",
                json!({"run_id": format!("old-{i}"), "at": "2000-01-01T00:00:00.000Z"})
            )
        })
        .collect();
    std::fs::create_dir_all(&ctx.paths.home).unwrap();
    std::fs::write(runlog::path(&ctx.paths), old).unwrap();

    std::thread::scope(|s| {
        let paths = &ctx.paths;
        s.spawn(move || {
            for i in 0..300 {
                runlog::append(paths, json!({"run_id": format!("new-{i}")}));
            }
        });
        let report = trovador::gc::gc(&ctx, Duration::from_secs(86_400), true).unwrap();
        assert_eq!(report.runs.removed, 2000);
    });
    let runs = by_run(&records(&ctx));
    assert_eq!(runs.len(), 300, "only the new records, all of them");
    assert!(runs.keys().all(|k| k.starts_with("new-")));
}

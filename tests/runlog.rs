//! `runs.jsonl` stays valid JSONL under concurrent runs, and every record names its run.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use common::*;
use serde_json::{Value, json};
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

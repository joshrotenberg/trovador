//! `runs.jsonl`: one line when a run starts and one when it ends.
//!
//! Several runs can end at once, in one server or across processes, so each record is
//! serialized whole and written with a single call while holding `runs.lock`. gc takes
//! the same lock while it rewrites the file, so no record lands in a file being replaced.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use serde_json::Value;

use crate::paths::Paths;

pub fn path(paths: &Paths) -> PathBuf {
    paths.home.join("runs.jsonl")
}

/// An exclusive lock on the log, held until the returned file is dropped.
pub fn lock(paths: &Paths) -> std::io::Result<File> {
    crate::paths::private_dir(&paths.home)?;
    let f = crate::paths::private_file()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.home.join("runs.lock"))?;
    f.lock()?;
    Ok(f)
}

/// Appends one record, adding `at` and `pid`. Losing a line never affects the run, so
/// failures are ignored.
pub fn append(paths: &Paths, mut record: Value) {
    if let Some(obj) = record.as_object_mut() {
        obj.insert(
            "at".into(),
            humantime::format_rfc3339_millis(std::time::SystemTime::now())
                .to_string()
                .into(),
        );
        obj.insert("pid".into(), std::process::id().into());
    }
    let mut line = record.to_string();
    line.push('\n');
    let Ok(_lock) = lock(paths) else {
        return;
    };
    if let Ok(mut f) = crate::paths::private_file()
        .create(true)
        .append(true)
        .open(path(paths))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

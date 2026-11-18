//! The revision of a session file: what a transcript or text page was read from, and what
//! a continuation can check before reading the next one.
//!
//! Both kinds of page are addressed by offset into something derived from the whole file:
//! record indexes for a transcript, characters of the rendered and redacted text for
//! `text`. When the file grows or is rewritten between two pages, the second can skip or
//! repeat records and the caller cannot tell (trovador#83).
//!
//! The revision is an opaque string. It is built from what the scan already knows (size,
//! mtime, the projection version and the settings that shape the projection) plus the
//! hashes of the first and last 4 KB that the index uses to tell an append from a rewrite
//! (#25). Computing it reads two 4 KB regions, never the session.
//!
//! Any append changes it, including one that leaves every earlier record alone. The case
//! that needs this is a forked Codex child: its boundary between copied parent history and
//! its own records is found from a later line, so an appended line can turn records that
//! were already shown into `forked_history`. The other append effects are small (a final
//! `omitted` run's count grows, a duplicate-suppressed turn is dropped from the appended
//! part only). Telling the fork case apart needs a read of the session, so the rule stays
//! simple: a caller that is told to restart loses one page. A live session therefore fails
//! a continuation whenever it was written in between.
//!
//! The descriptor also holds `archived` and the file path. Whether a Codex file is in the
//! archived root changes how `fork_boundary` treats an unreached start ordinal, and a
//! rename keeps size, mtime and bytes, so without them a moved file kept its revision and
//! projected differently.
//!
//! A compressed rollout is revised by the compressed file's size and mtime only. Decoding
//! the tail to hash it would read the whole session, and Codex does not rewrite a `.zst`
//! in place. Compressing a plain rollout with no content change therefore changes the
//! revision: the stored file is a different one. A continuation that crosses it is told to
//! restart, which is the safe direction; the other would be to return a page labelled with
//! a revision it was not read under.

use std::path::Path;
use std::time::SystemTime;

use crate::archive::{self, ArchiveEntry};
use crate::index::{bytes_hash, region_hashes};
use crate::limits::Limits;
use crate::transcript::PROJECTION_VERSION;

/// A revision, and what is needed to check later that the bytes it names are still there.
#[derive(Clone, Debug)]
pub struct Revision {
    pub id: String,
    /// Hashes of the first and last 4 KB of a plain file, taken at `entry.size_bytes`.
    regions: Option<(i64, i64)>,
}

fn mtime_ns(t: SystemTime) -> i128 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

/// The revision of `e` as the scan found it. Page reads are cut at `e.size_bytes`, so this
/// names exactly the bytes a read will cover.
pub fn of(e: &ArchiveEntry, limits: &Limits) -> std::io::Result<Revision> {
    let compressed = archive::is_compressed(&e.file);
    let regions =
        if compressed {
            None
        } else {
            Some(region_hashes(&e.file, e.size_bytes).ok_or_else(|| {
                std::io::Error::other(format!("cannot sample {}", e.file.display()))
            })?)
        };
    // The settings are in because text offsets count characters of the rendered, redacted
    // text, which changes when any of them does. Another server with different limits
    // would otherwise accept a continuation and cut the text somewhere else.
    let descriptor = format!(
        "{}/{}|file={}|archived={}|projection={PROJECTION_VERSION}|record_chars={}|tool_chars={}|redact={}|{}|size={}|mtime_ns={}|regions={regions:?}",
        e.backend,
        e.session_id,
        e.file.display(),
        e.archived,
        limits.record_chars,
        limits.tool_chars,
        limits.redact,
        if compressed { "zst" } else { "jsonl" },
        e.size_bytes,
        mtime_ns(e.mtime),
    );
    Ok(Revision {
        id: format!("{:016x}", bytes_hash(descriptor.as_bytes())),
        regions,
    })
}

impl Revision {
    /// Whether the bytes this revision names are still in the file. Run after a read.
    /// Growth is allowed: the read was cut at the size the revision names, so appended
    /// bytes were never in it. A rewrite or a truncation is not allowed, and neither is a
    /// replaced compressed file, because the page may then mix two versions under one label.
    pub fn still_holds(&self, e: &ArchiveEntry) -> std::io::Result<bool> {
        let md = std::fs::metadata(&e.file)?;
        match self.regions {
            None => Ok(md.len() == e.size_bytes && md.modified().ok() == Some(e.mtime)),
            Some(regions) => Ok(md.len() >= e.size_bytes
                && region_hashes(Path::new(&e.file), e.size_bytes) == Some(regions)),
        }
    }
}

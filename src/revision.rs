//! The revision of a session file: what a transcript or text page was read from, and what
//! a continuation can check before reading the next one.
//!
//! Both kinds of page are addressed by offset into something derived from the whole file:
//! record indexes for a transcript, characters of the rendered and redacted text for
//! `text`. When the file is rewritten between two pages, the second can skip or repeat
//! records and the caller cannot tell (trovador#83).
//!
//! # The token
//!
//! The revision is one opaque string that carries what a later call needs to recover the
//! state it names: the size `S` the read was cut at, the file's mtime, hashes of the first
//! and last 4 KB of the first `S` bytes (the samples the index uses to tell an append from
//! a rewrite, #25), a kind (plain, plain with an unresolved fork, compressed) and a hash
//! of everything else that shapes the projection. That hash covers the backend, session id,
//! file path, the archived flag, `PROJECTION_VERSION`, the record and tool character caps
//! and the redact setting. The redaction rules are not versioned separately: they are
//! covered only through `PROJECTION_VERSION`, so a change to them must bump it. Producing a
//! token reads two 4 KB regions, plus the first line of a Codex file, never the session.
//! The format is not a promise. Callers pass the string back unchanged, and anything that
//! does not parse back to exactly the same string is refused like a stale revision.
//!
//! # Continuing a session that has grown
//!
//! A caller holding a token for size `S` that finds the file at `N > S` is served the
//! session as it was at `S`: the read is cut at `S` (the fork-boundary scan too), the first
//! `S` bytes are verified by the same samples, and the response says the session has grown
//! and gives the current revision. Appended content is simply not part of that read.
//! This keeps a caller paging through a live session from being refused and restarting
//! for ever. It works because, for Claude and for a Codex session that is not a fork with
//! an unresolved boundary, appended bytes never change how earlier bytes project: a final
//! `omitted` run's count can grow, which a read cut at `S` still renders as it did, and a
//! turn dropped as a duplicate is dropped from the appended part only.
//!
//! A forked Codex child whose start ordinal is not reached within `S` bytes is the
//! exception: a later line can resolve the boundary and turn records already shown into
//! `forked_history`. Its token is marked, and any append refuses the continuation.
//!
//! # What still refuses
//!
//! A rewrite or truncation of the first `S` bytes, a move between the active and archived
//! roots or any other change of path, compression, and different settings. The path and
//! archived flag are in the hash because `fork_boundary` treats an unreached start ordinal
//! differently in an archived file, and a rename keeps size, mtime and bytes.
//!
//! A compressed rollout is revised by its stored size and mtime only: decoding the tail
//! to sample it would read the whole session, and Codex does not rewrite a `.zst` in
//! place. Compressing a plain rollout with no content change therefore changes the
//! revision, because the stored file is a different one. A compressed token never matches
//! a grown file.
//!
//! # Limits of detection
//!
//! The mtime is in the token but is compared only when the file is still exactly `S` bytes,
//! because an append moves it. A same-size rewrite with a new mtime is caught then. Once
//! the file has grown, a rewrite of the first `S` bytes that leaves both sampled 4 KB
//! regions as they were cannot be told from an append. A same-size rewrite that keeps the
//! mtime and the samples is not detected either, and neither is a compressed rollout
//! replaced in place with the same size and mtime.

use std::path::Path;
use std::time::SystemTime;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::archive::{self, ArchiveEntry, Backend};
use crate::index::{bytes_hash, region_hashes};
use crate::limits::Limits;
use crate::transcript::PROJECTION_VERSION;

/// Set on a page read under a revision the session has since grown past.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Grown {
    /// The revision to pass as `expected_revision`, or to start a fresh read with, to see
    /// the session as it is now.
    pub current_revision: String,
    /// Bytes written to the session file since the revision the page was read under.
    pub bytes_added: u64,
}

/// A revision, and what is needed to check later that the bytes it names are still there.
#[derive(Clone, Debug)]
pub struct Revision {
    pub id: String,
    /// The size the read is cut at.
    pub size: u64,
    mtime_ns: u128,
    /// Hashes of the first and last 4 KB of a plain file, taken at `size`.
    regions: Option<(i64, i64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Plain,
    /// A Codex fork whose boundary is unresolved within `size`: appends change records.
    Fork,
    Compressed,
}

impl Kind {
    fn code(self) -> char {
        match self {
            Kind::Plain => 'p',
            Kind::Fork => 'f',
            Kind::Compressed => 'z',
        }
    }
}

struct Parsed {
    size: u64,
    mtime_ns: u128,
    head: i64,
    tail: i64,
    kind: Kind,
    hash: u64,
}

fn token(p: &Parsed) -> String {
    format!(
        "{:x}.{:x}.{:x}.{:x}.{}.{:016x}",
        p.size,
        p.mtime_ns,
        p.head as u64,
        p.tail as u64,
        p.kind.code(),
        p.hash
    )
}

/// Strict: the string must be exactly what [`token`] would print for what it parses to, so
/// padding, case, signs and extra fields are all refused.
fn parse(s: &str) -> Option<Parsed> {
    let parts: Vec<&str> = s.split('.').collect();
    let [size, mtime, head, tail, kind, hash] = parts[..] else {
        return None;
    };
    let kind = match kind {
        "p" => Kind::Plain,
        "f" => Kind::Fork,
        "z" => Kind::Compressed,
        _ => return None,
    };
    let parsed = Parsed {
        size: u64::from_str_radix(size, 16).ok()?,
        mtime_ns: u128::from_str_radix(mtime, 16).ok()?,
        head: u64::from_str_radix(head, 16).ok()? as i64,
        tail: u64::from_str_radix(tail, 16).ok()? as i64,
        kind,
        hash: u64::from_str_radix(hash, 16).ok()?,
    };
    (token(&parsed) == s).then_some(parsed)
}

fn mtime_ns(t: SystemTime) -> u128 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Everything besides the bytes that decides what a read of them returns.
fn settings_hash(e: &ArchiveEntry, limits: &Limits, compressed: bool) -> u64 {
    // The settings are in because text offsets count characters of the rendered, redacted
    // text, which changes when any of them does. Another server with different limits
    // would otherwise accept a continuation and cut the text somewhere else.
    bytes_hash(
        format!(
            "{}/{}|file={}|archived={}|projection={PROJECTION_VERSION}|record_chars={}|tool_chars={}|redact={}|compressed={compressed}",
            e.backend,
            e.session_id,
            e.file.display(),
            e.archived,
            limits.record_chars,
            limits.tool_chars,
            limits.redact,
        )
        .as_bytes(),
    )
}

/// Whether the first `e.size_bytes` bytes are a Codex fork whose boundary a later line
/// could still resolve. Reads the header line, and for a fork the lines up to the boundary.
fn fork_unresolved(e: &ArchiveEntry) -> std::io::Result<bool> {
    if e.backend != Backend::Codex {
        return Ok(false);
    }
    let mut reader = std::io::Read::take(archive::open_archive(&e.file, 0)?, e.size_bytes);
    Ok(crate::codex::fork_boundary(&mut reader, e.archived, true)?.unresolved)
}

fn io_error(e: &ArchiveEntry, what: &str) -> std::io::Error {
    std::io::Error::other(format!("cannot {what} {}", e.file.display()))
}

/// The revision of `e` as the scan found it. Page reads are cut at `e.size_bytes`, so this
/// names exactly the bytes a read will cover.
pub fn of(e: &ArchiveEntry, limits: &Limits) -> std::io::Result<Revision> {
    let compressed = archive::is_compressed(&e.file);
    let (regions, kind) = if compressed {
        (None, Kind::Compressed)
    } else {
        let regions = region_hashes(&e.file, e.size_bytes).ok_or_else(|| io_error(e, "sample"))?;
        let kind = if fork_unresolved(e)? {
            Kind::Fork
        } else {
            Kind::Plain
        };
        (Some(regions), kind)
    };
    let (head, tail) = regions.unwrap_or((0, 0));
    let parsed = Parsed {
        size: e.size_bytes,
        mtime_ns: mtime_ns(e.mtime),
        head,
        tail,
        kind,
        hash: settings_hash(e, limits, compressed),
    };
    Ok(Revision {
        id: token(&parsed),
        size: parsed.size,
        mtime_ns: parsed.mtime_ns,
        regions,
    })
}

/// What a call's `expected_revision` came to.
pub enum Check {
    /// Read the file as it is now: no expectation, or the expectation is the current state.
    Current(Revision),
    /// The session has grown past the expected revision with its first bytes intact: read
    /// it as it was, cut at `rev.size`.
    Pinned {
        rev: Revision,
        current: Revision,
        bytes_added: u64,
    },
    /// The expectation names a state this session is not in.
    Stale { current: Revision },
}

pub fn check(e: &ArchiveEntry, limits: &Limits, expected: Option<&str>) -> std::io::Result<Check> {
    let current = of(e, limits)?;
    let Some(expected) = expected else {
        return Ok(Check::Current(current));
    };
    if expected == current.id {
        return Ok(Check::Current(current));
    }
    let stale = |current| Ok(Check::Stale { current });
    let Some(p) = parse(expected) else {
        return stale(current);
    };
    // A mismatch at the same size, or any change to a compressed or forked token, is not a
    // plain append. A smaller file is a truncation.
    if p.kind != Kind::Plain
        || archive::is_compressed(&e.file)
        || p.hash != settings_hash(e, limits, false)
        || e.size_bytes <= p.size
        || region_hashes(&e.file, p.size) != Some((p.head, p.tail))
    {
        return stale(current);
    }
    let rev = Revision {
        id: expected.to_string(),
        size: p.size,
        mtime_ns: p.mtime_ns,
        regions: Some((p.head, p.tail)),
    };
    Ok(Check::Pinned {
        bytes_added: e.size_bytes - p.size,
        rev,
        current,
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
            None => Ok(
                md.len() == self.size && md.modified().ok().map(mtime_ns) == Some(self.mtime_ns)
            ),
            Some(regions) => Ok(md.len() >= self.size
                && region_hashes(Path::new(&e.file), self.size) == Some(regions)),
        }
    }
}

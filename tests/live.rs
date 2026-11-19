//! Properties that must hold for any machine's real archives.
//!
//! The fixture suite proves trovador agrees with fixtures. These run against the actual
//! `~/.claude` and `~/.codex` of whatever machine runs them, with whatever CLI versions wrote
//! them, and check properties rather than values. Every test is ignored by default so
//! `cargo test` and CI never read real data:
//!
//! ```text
//! cargo test --release --test live -- --ignored --nocapture
//! ```
//!
//! trovador's own state goes to a fresh home under the cargo target directory; the real
//! `~/.trovador` is never read or written. Sampling is seeded (`TROVADOR_LIVE_SEED`, printed)
//! and failures name the sessions involved.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use trovador::archive::{self, ArchiveEntry, Provider, SessionMeta};
use trovador::index::{Index, RefreshOpts, RefreshReport, SearchOpts};
use trovador::sessions::{self, Ctx, ListOpts};
use trovador::transcript::{Projection, Record, RecordKind};
use trovador::{Limits, Paths};

// --- shared setup ------------------------------------------------------------

struct Live {
    ctx: Ctx,
    seed: u64,
    started: SystemTime,
    /// Archive files with their size and mtime, taken before anything else ran.
    before: HashMap<PathBuf, (u64, SystemTime)>,
    /// A hash of the first 4 KB of every file idle for 10 minutes before the run.
    prefixes: HashMap<PathBuf, u64>,
    build: RefreshReport,
    /// Every session with its metadata, from the list with subagents.
    all: Vec<(ArchiveEntry, SessionMeta)>,
    sample: Vec<ArchiveEntry>,
}

fn archive_paths() -> Paths {
    let mut paths = Paths::under(Path::new(env!("CARGO_TARGET_TMPDIR")));
    let home = std::env::home_dir().unwrap();
    let archive = |name: &str, default: PathBuf| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or(default)
    };
    paths.claude_archive = archive("TROVADOR_CLAUDE_ARCHIVE", home.join(".claude/projects"));
    paths.codex_archive = archive("TROVADOR_CODEX_ARCHIVE", home.join(".codex/sessions"));
    paths.codex_archived = archive(
        "TROVADOR_CODEX_ARCHIVED",
        paths
            .codex_archive
            .parent()
            .unwrap()
            .join("archived_sessions"),
    );
    paths
}

fn live() -> &'static Live {
    static LIVE: OnceLock<Live> = OnceLock::new();
    LIVE.get_or_init(|| {
        let started = SystemTime::now();
        let before = snapshot_archives(&archive_paths());
        let idle_before = started - Duration::from_secs(600);
        let prefixes = before
            .iter()
            .filter(|(_, (_, m))| *m <= idle_before)
            .filter_map(|(p, _)| prefix_hash(p).map(|h| (p.clone(), h)))
            .collect();
        let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("live-home");
        let _ = std::fs::remove_dir_all(&home);
        let paths = Paths {
            home,
            ..archive_paths()
        };
        let ctx = Ctx {
            paths,
            limits: Limits::from_env(),
        };
        let build = sessions::reindex(&ctx, false).expect("build the index");
        let entries: HashMap<(Provider, String), ArchiveEntry> = archive::scan_all(&ctx.paths)
            .into_iter()
            .map(|e| ((e.provider, e.session_id.clone()), e))
            .collect();
        let listed = sessions::list(
            &ctx,
            &ListOpts {
                include_subagents: Some(true),
                include_automated: Some(true),
                limit: Some(usize::MAX),
                ..Default::default()
            },
        ).unwrap();
        let all: Vec<(ArchiveEntry, SessionMeta)> = listed
            .sessions
            .into_iter()
            .filter_map(|r| {
                let e = entries.get(&(r.provider, r.session_id.clone()))?.clone();
                Some((e, r.meta))
            })
            .collect();
        let seed = std::env::var("TROVADOR_LIVE_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            });
        let n = std::env::var("TROVADOR_LIVE_SAMPLE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        let sample = choose_sample(&all, seed, n);
        eprintln!(
            "live: seed {seed} (TROVADOR_LIVE_SEED={seed} to repeat); {} sessions, {} sampled; index built in {} ms at {}",
            all.len(),
            sample.len(),
            build.ms,
            ctx.paths.home.display()
        );
        Live {
            ctx,
            seed,
            started,
            before,
            prefixes,
            build,
            all,
            sample,
        }
    })
}

/// xorshift64: deterministic for a seed, and no dependency for one test file.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0.max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Format variety lives at the edges, so for every provider and origin the largest,
/// smallest, newest, and oldest session are always included, plus `n` at random.
fn choose_sample(all: &[(ArchiveEntry, SessionMeta)], seed: u64, n: usize) -> Vec<ArchiveEntry> {
    let mut groups: BTreeMap<(String, String), Vec<&ArchiveEntry>> = BTreeMap::new();
    for (e, m) in all {
        let origin = m.origin.clone().unwrap_or_else(|| "none".into());
        groups
            .entry((e.provider.to_string(), origin))
            .or_default()
            .push(e);
    }
    let mut picked: Vec<ArchiveEntry> = Vec::new();
    let mut seen = HashSet::new();
    let mut take = |e: &ArchiveEntry, picked: &mut Vec<ArchiveEntry>| {
        if seen.insert((e.provider, e.session_id.clone())) {
            picked.push(e.clone());
        }
    };
    for g in groups.values() {
        for e in [
            g.iter().max_by_key(|e| e.size_bytes),
            g.iter().min_by_key(|e| e.size_bytes),
            g.iter().max_by_key(|e| e.mtime),
            g.iter().min_by_key(|e| e.mtime),
        ]
        .into_iter()
        .flatten()
        {
            take(e, &mut picked);
        }
    }
    let mut rng = Rng(seed);
    for _ in 0..n.min(all.len()) {
        let (e, _) = &all[rng.below(all.len())];
        take(e, &mut picked);
    }
    picked
}

fn who(e: &ArchiveEntry) -> String {
    format!(
        "{}/{} ({} KB)",
        e.provider,
        e.session_id,
        e.size_bytes / 1024
    )
}

/// Whether a session's file changed after the test started, which a running client does
/// to the sessions it is in. Comparisons involving such a session are skipped, not failed.
fn written_during_run(e: &ArchiveEntry, since: SystemTime) -> bool {
    match std::fs::metadata(&e.file) {
        Ok(m) => m.len() != e.size_bytes || m.modified().is_ok_and(|t| t >= since),
        Err(_) => true,
    }
}

fn projection(e: &ArchiveEntry, limits: &Limits) -> Vec<Record> {
    Projection::open(e, limits)
        .unwrap_or_else(|err| panic!("open {}: {err}", who(e)))
        .collect()
}

fn snapshot_archives(paths: &Paths) -> HashMap<PathBuf, (u64, SystemTime)> {
    let mut out = HashMap::new();
    let mut stack = vec![
        paths.claude_archive.clone(),
        paths.codex_archive.clone(),
        paths.codex_archived.clone(),
    ];
    while let Some(dir) = stack.pop() {
        let Ok(items) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in items.flatten() {
            let p = item.path();
            match item.metadata() {
                Ok(m) if m.is_dir() => stack.push(p),
                Ok(m) => {
                    out.insert(p, (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
                }
                Err(_) => {}
            }
        }
    }
    out
}

/// Identifies a file's existing content, so an append by a running client can be told
/// apart from any other change.
fn prefix_hash(p: &Path) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    use std::io::Read;
    let mut buf = Vec::with_capacity(4096);
    std::fs::File::open(p)
        .ok()?
        .take(4096)
        .read_to_end(&mut buf)
        .ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    buf.hash(&mut h);
    Some(h.finish())
}

/// Independent of `archive::scan_*`: what a session file is, written out again.
fn walk_sessions(paths: &Paths) -> HashSet<(Provider, String)> {
    let mut out = HashSet::new();
    if let Ok(projects) = std::fs::read_dir(&paths.claude_archive) {
        for p in projects.flatten() {
            for f in std::fs::read_dir(p.path()).into_iter().flatten().flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                if f.path().is_file()
                    && let Some(id) = name.strip_suffix(".jsonl")
                {
                    out.insert((Provider::Claude, id.to_string()));
                }
            }
        }
    }
    let uuid = regex::Regex::new(
        // Codex 0.165 compresses finished rollouts to `.jsonl.zst` (#90).
        r"([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl(\.zst)?$",
    )
    .unwrap();
    let mut stack = vec![paths.codex_archive.clone(), paths.codex_archived.clone()];
    while let Some(dir) = stack.pop() {
        for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = f.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(c) = uuid.captures(&f.file_name().to_string_lossy()) {
                out.insert((Provider::Codex, c[1].to_string()));
            }
        }
    }
    out
}

// --- inventory and lookup ----------------------------------------------------

#[test]
#[ignore = "reads this machine's real archives"]
fn every_session_file_is_listed_exactly_once_newest_first() {
    let l = live();
    let walked = walk_sessions(&l.ctx.paths);
    let listed: Vec<(Provider, String)> = l
        .all
        .iter()
        .map(|(e, _)| (e.provider, e.session_id.clone()))
        .collect();
    let unique: HashSet<_> = listed.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        listed.len(),
        "a session id is listed more than once"
    );
    // Sessions created after the setup ran are in the walk but not the list.
    let missing: Vec<_> = walked
        .difference(&unique)
        .filter(|(b, id)| {
            archive::find_session(&l.ctx.paths, id, Some(*b))
                .map(|e| e.mtime < l.started)
                .unwrap_or(false)
        })
        .collect();
    assert!(missing.is_empty(), "on disk but not listed: {missing:?}");
    let extra: Vec<_> = unique.difference(&walked).collect();
    assert!(extra.is_empty(), "listed but not a session file: {extra:?}");

    for w in l.all.windows(2) {
        // A client appending to a session moves it between the list and the scan above.
        if written_during_run(&w[0].0, l.started) || written_during_run(&w[1].0, l.started) {
            continue;
        }
        assert!(
            w[0].0.mtime >= w[1].0.mtime,
            "not newest first: {} before {}",
            who(&w[0].0),
            who(&w[1].0)
        );
    }
    let top: HashSet<_> = sessions::list(
        &l.ctx,
        &ListOpts {
            limit: Some(usize::MAX),
            ..Default::default()
        },
    )
    .unwrap()
    .sessions
    .into_iter()
    .map(|r| (r.provider, r.session_id))
    .collect();
    assert!(top.is_subset(&unique));
    assert!(
        top.iter().all(|k| {
            l.all
                .iter()
                .find(|(e, _)| (e.provider, e.session_id.clone()) == *k)
                .is_some_and(|(_, m)| !m.subagent)
        }),
        "the default list contains a subagent"
    );
}

#[test]
#[ignore = "reads this machine's real archives"]
fn every_sampled_session_resolves_by_id_and_by_prefix() {
    let l = live();
    for e in &l.sample {
        let found = archive::find_session(&l.ctx.paths, &e.session_id, Some(e.provider))
            .unwrap_or_else(|err| panic!("{}: {err}", who(e)));
        assert_eq!(found.file, e.file);
        match archive::find_session(&l.ctx.paths, &e.session_id[..8], None) {
            Ok(f) => assert_eq!(f.session_id, e.session_id, "{}", who(e)),
            Err(archive::LookupError::Ambiguous { candidates, .. }) => assert!(
                candidates.contains(&format!("{}/{}", e.provider, e.session_id)),
                "{}: ambiguity does not name it",
                who(e)
            ),
            Err(err) => panic!("{}: prefix lookup failed: {err}", who(e)),
        }
    }
}

#[test]
#[ignore = "reads this machine's real archives"]
fn metadata_is_well_formed_for_every_session() {
    let l = live();
    let mut problems = Vec::new();
    for (e, m) in &l.all {
        let row = sessions::row(e, m.clone(), &l.ctx.limits);
        if let Some(p) = &row.project
            && !Path::new(p).is_absolute()
        {
            problems.push(format!("{}: project not absolute: {p}", who(e)));
        }
        if let Some(s) = &m.snippet {
            if s.chars().count() > 120 {
                problems.push(format!("{}: snippet over 120 chars", who(e)));
            }
            if s.starts_with('<') || s.starts_with("# AGENTS.md instructions") {
                problems.push(format!("{}: snippet is injected context: {s}", who(e)));
            }
        }
        // A compressed rollout is judged by its decompressed size, which the provider
        // reads on resume, not by its size on disk (#90).
        if row.resumable != (archive::transcript_bytes(e) < l.ctx.limits.resume_limit_bytes) {
            problems.push(format!("{}: resumable disagrees with size", who(e)));
        }
        if !row.open_with.contains(&e.session_id) {
            problems.push(format!("{}: open_with lacks the id", who(e)));
        }
        if m.subagent && m.origin.is_none() {
            problems.push(format!("{}: subagent with no origin", who(e)));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

// --- transcripts and the index ----------------------------------------------

#[test]
#[ignore = "reads this machine's real archives"]
fn sampled_transcripts_are_contiguous_capped_and_not_duplicated() {
    let l = live();
    let limits = &l.ctx.limits;
    let truncation = " ...[truncated]".chars().count();
    let mut rng = Rng(l.seed ^ 0x5eed);
    for e in &l.sample {
        let recs = projection(e, limits);
        for (i, r) in recs.iter().enumerate() {
            assert_eq!(r.index, i, "{}: index gap at {i}", who(e));
            if let Some(t) = &r.text {
                let cap = limits
                    .record_chars
                    .max(limits.tool_chars)
                    .max(limits.unrecognized_chars)
                    + truncation;
                assert!(
                    t.chars().count() <= cap,
                    "{}: record {i} is {} chars",
                    who(e),
                    t.chars().count()
                );
            }
        }
        for w in recs.windows(2) {
            if w[0].kind == RecordKind::Omitted && w[1].kind == RecordKind::Omitted {
                assert_ne!(
                    w[0].name,
                    w[1].name,
                    "{}: omitted run not collapsed at {}",
                    who(e),
                    w[0].index
                );
            }
            if w[0].kind == RecordKind::User && w[1].kind == RecordKind::User {
                assert!(
                    w[0].text != w[1].text || w[0].text.as_deref().is_none_or(|t| t.len() < 40),
                    "{}: the same user turn twice at {} (projected from two streams?)",
                    who(e),
                    w[0].index
                );
            }
        }
        if written_during_run(e, l.started) || recs.is_empty() {
            continue;
        }
        let offset = rng.below(recs.len());
        let page = trovador::transcript::page(e, offset, Some(25), limits).unwrap();
        let want: Vec<_> = recs.iter().skip(offset).take(page.records.len()).collect();
        assert_eq!(
            page.records.iter().collect::<Vec<_>>(),
            want,
            "{}: page at {offset} differs from the full projection",
            who(e)
        );
    }
}

#[test]
#[ignore = "reads this machine's real archives"]
fn indexed_records_equal_the_transcript_for_sampled_sessions() {
    let l = live();
    let idx = Index::open(&l.ctx.paths, &l.ctx.limits).unwrap();
    let mut compared = 0;
    for e in &l.sample {
        if written_during_run(e, l.started) {
            continue;
        }
        let meta = archive::read_meta(e, &l.ctx.limits);
        let indexed = idx.session_records(e.provider, &e.session_id).unwrap();
        if meta.origin.as_deref() == Some("guardian") {
            assert!(indexed.is_empty(), "{}: guardian text was indexed", who(e));
            continue;
        }
        let expected: Vec<(usize, String, String)> = projection(e, &l.ctx.limits.for_index())
            .into_iter()
            .filter(|r| r.kind != RecordKind::Omitted && r.text.is_some())
            .filter(|r| l.ctx.limits.index_reasoning || r.kind != RecordKind::Reasoning)
            .map(|r| {
                let kind = serde_json::to_value(r.kind)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string();
                (r.index, kind, r.text.unwrap())
            })
            .collect();
        assert_eq!(
            indexed.len(),
            expected.len(),
            "{}: record count differs",
            who(e)
        );
        if let Some(i) = indexed.iter().zip(&expected).position(|(a, b)| a != b) {
            panic!(
                "{}: first difference at {i}: index {:?} transcript {:?}",
                who(e),
                indexed[i],
                expected[i]
            );
        }
        compared += 1;
    }
    eprintln!("live: {compared} sessions compared index to transcript");
    assert!(compared > 0);
}

// --- search ------------------------------------------------------------------

/// A run of 4 to 6 plain words joined by single spaces, at least 25 characters long, so it
/// is one FTS token sequence and rare enough to keep the result set small.
fn phrase_from(text: &str, rng: &mut Rng) -> Option<(String, String)> {
    let runs: Vec<&str> = regex::Regex::new(r"[A-Za-z0-9]+(?: [A-Za-z0-9]+){3,5}")
        .unwrap()
        .find_iter(text)
        .map(|m| m.as_str())
        .filter(|s| s.len() >= 25)
        .collect();
    if runs.is_empty() {
        return None;
    }
    let span = runs[rng.below(runs.len())];
    Some((format!("\"{span}\""), span.to_string()))
}

#[test]
#[ignore = "reads this machine's real archives"]
fn a_phrase_from_any_indexed_record_finds_its_session() {
    let l = live();
    let idx = Index::open(&l.ctx.paths, &l.ctx.limits).unwrap();
    // Every matching session on one page, past the context ceilings a caller gets.
    let mut wide = l.ctx.clone();
    wide.limits.search_max = usize::MAX;
    wide.limits.response_max_chars = usize::MAX;
    let mut rng = Rng(l.seed ^ 0xfeed);
    let mut probes = 0;
    for e in &l.sample {
        if probes >= 20 || written_during_run(e, l.started) {
            continue;
        }
        let records = idx.session_records(e.provider, &e.session_id).unwrap();
        if records.is_empty() {
            continue;
        }
        let Some((terms, literal)) = (0..5).find_map(|_| {
            let (_, _, text) = &records[rng.below(records.len())];
            phrase_from(text, &mut rng)
        }) else {
            continue;
        };
        for (query, regex) in [(terms.clone(), false), (regex::escape(&literal), true)] {
            let r = sessions::search(
                &wide,
                &SearchOpts {
                    query: query.clone(),
                    regex,
                    include_subagents: Some(true),
                    // Recall, not the defaults: the probed session may be recent or automated.
                    include_active: Some(true),
                    include_automated: Some(true),
                    limit: Some(100_000),
                    per_session: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(
                r.hits.iter().any(|h| h.row.session_id == e.session_id),
                "{}: {} search for {query} did not find it ({} other sessions matched)",
                who(e),
                if regex { "regex" } else { "terms" },
                r.sessions_matched
            );
        }
        probes += 1;
    }
    eprintln!("live: {probes} search probes");
    assert!(probes >= 5, "too few records with a usable phrase");

    let nonce = format!("zq{}", uuid::Uuid::new_v4().simple());
    let r = sessions::search(
        &l.ctx,
        &SearchOpts {
            query: nonce,
            include_subagents: Some(true),
            include_active: Some(true),
            include_automated: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.sessions_matched, 0);
    assert_eq!(r.absence_is_proof, Some(true), "{:?}", r.note);
}

// --- refresh and caching -----------------------------------------------------

#[test]
#[ignore = "reads this machine's real archives"]
fn a_second_refresh_changes_only_what_was_written_since() {
    let l = live();
    let mut idx = Index::open(&l.ctx.paths, &l.ctx.limits).unwrap();
    let r = idx
        .refresh(&l.ctx.paths, &l.ctx.limits, &RefreshOpts::default())
        .unwrap();
    let written_since = archive::scan_all(&l.ctx.paths)
        .iter()
        .filter(|e| e.mtime >= l.started)
        .count();
    assert_eq!(r.failed, 0);
    assert!(
        r.appended + r.reindexed <= written_since,
        "{} appended and {} reindexed, but only {written_since} sessions were written since the build",
        r.appended,
        r.reindexed
    );
    assert_eq!(
        l.build.failed, 0,
        "the first build failed to read some sessions"
    );
}

#[test]
#[ignore = "reads this machine's real archives"]
fn a_rebuild_elsewhere_gives_the_same_records_per_session() {
    let l = live();
    let other = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("live-rebuild");
    let _ = std::fs::remove_dir_all(&other);
    let ctx2 = Ctx {
        paths: Paths {
            home: other.clone(),
            ..l.ctx.paths.clone()
        },
        limits: l.ctx.limits.clone(),
    };
    sessions::reindex(&ctx2, true).unwrap();
    let (a, b) = (
        Index::open(&l.ctx.paths, &l.ctx.limits).unwrap(),
        Index::open(&ctx2.paths, &ctx2.limits).unwrap(),
    );
    for e in &l.sample {
        if written_during_run(e, l.started) {
            continue;
        }
        assert_eq!(
            a.session_records(e.provider, &e.session_id).unwrap().len(),
            b.session_records(e.provider, &e.session_id).unwrap().len(),
            "{}",
            who(e)
        );
    }
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
#[ignore = "reads this machine's real archives"]
fn listing_is_the_same_with_and_without_the_index() {
    let l = live();
    let bare = Ctx {
        paths: Paths {
            home: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("live-no-index"),
            ..l.ctx.paths.clone()
        },
        limits: l.ctx.limits.clone(),
    };
    let opts = ListOpts {
        include_subagents: Some(true),
        include_automated: Some(true),
        limit: Some(usize::MAX),
        ..Default::default()
    };
    let rows = |c: &Ctx| -> HashMap<String, Value> {
        sessions::list(c, &opts)
            .unwrap()
            .sessions
            .into_iter()
            .map(|r| {
                (
                    format!("{}/{}", r.provider, r.session_id),
                    serde_json::to_value(r).unwrap(),
                )
            })
            .collect()
    };
    let (with, without) = (rows(&l.ctx), rows(&bare));
    let mut differ = Vec::new();
    for (k, v) in &with {
        if let Some(w) = without.get(k)
            && v != w
            && v["modified"] == w["modified"]
            && v["size_kb"] == w["size_kb"]
        {
            differ.push(k.clone());
        }
    }
    assert!(differ.is_empty(), "cached metadata differs for {differ:?}");
}

// --- doors -------------------------------------------------------------------

fn cli(ctx: &Ctx, args: &[&str]) -> Value {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_trovador"))
        .args(args)
        .env("TROVADOR_CLAUDE_ARCHIVE", &ctx.paths.claude_archive)
        .env("TROVADOR_CODEX_ARCHIVE", &ctx.paths.codex_archive)
        .env("TROVADOR_CODEX_ARCHIVED", &ctx.paths.codex_archived)
        .env("TROVADOR_HOME", &ctx.paths.home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// What the doors serve: the library's answer through the same redaction.
fn served(v: impl serde::Serialize) -> Value {
    let mut v = serde_json::to_value(v).unwrap();
    trovador::redact::redact_value(&mut v);
    v
}

/// Drops what legitimately varies between two calls a moment apart: rows for sessions
/// being written, and refresh timings.
fn stable(mut v: Value, since: SystemTime) -> Value {
    let cutoff = humantime::format_rfc3339_millis(since).to_string();
    for key in ["sessions", "hits"] {
        if let Some(rows) = v.get_mut(key).and_then(Value::as_array_mut) {
            rows.retain(|r| r["modified"].as_str().is_some_and(|m| m < cutoff.as_str()));
        }
    }
    if let Some(obj) = v.as_object_mut() {
        // Counts of what the defaults hid move as sessions cross the active window
        // between two calls a moment apart, and the note quotes those counts.
        for k in [
            "coverage",
            "total_sessions",
            "examined",
            "returned",
            "offset",
            "next_offset",
            "truncated",
            "sessions_matched",
            "hidden",
            "hidden_automated",
            "note",
            "absence_is_proof",
        ] {
            obj.remove(k);
        }
    }
    v
}

#[tokio::test]
#[ignore = "reads this machine's real archives"]
async fn the_cli_and_mcp_return_the_library_answer_on_real_data() {
    let l = live();
    let ctx = l.ctx.clone();
    let mut client = tower_mcp::TestClient::from_router(trovador::mcp::router(ctx.clone()));
    client.initialize().await;

    let lib = served(
        sessions::list(
            &ctx,
            &ListOpts {
                limit: Some(50),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let since = l.started;
    assert_eq!(
        stable(cli(&ctx, &["sessions", "list", "--limit", "50"]), since),
        stable(lib.clone(), since)
    );
    assert_eq!(
        stable(
            client
                .call_tool_json("sessions", json!({"action": "list", "limit": 50}))
                .await,
            since
        ),
        stable(lib, since)
    );

    for e in l
        .sample
        .iter()
        .filter(|e| !written_during_run(e, since))
        .take(5)
    {
        let lib = served(sessions::show(&ctx, &e.session_id, Some(e.provider)).unwrap());
        let args = [
            "sessions",
            "show",
            &e.session_id,
            "--provider",
            e.provider.as_str(),
        ];
        assert_eq!(cli(&ctx, &args), lib, "{}", who(e));
        assert_eq!(
            client
                .call_tool_json(
                    "sessions",
                    json!({"action": "show", "session_id": e.session_id, "provider": e.provider})
                )
                .await,
            lib,
            "{}",
            who(e)
        );
    }

    let q = SearchOpts {
        query: "the".into(),
        limit: Some(10),
        ..Default::default()
    };
    let lib = stable(served(sessions::search(&ctx, &q).unwrap()), since);
    assert_eq!(
        stable(cli(&ctx, &["search", "the", "--limit", "10"]), since),
        lib
    );
    assert_eq!(
        stable(
            client
                .call_tool_json("search", json!({"query": "the", "limit": 10}))
                .await,
            since
        ),
        lib
    );
}

// --- read-only ---------------------------------------------------------------

#[test]
#[ignore = "reads this machine's real archives"]
fn archive_files_idle_before_the_run_are_untouched() {
    let l = live();
    // Exercise the reading paths over the whole sample first.
    for e in &l.sample {
        let _ = projection(e, &l.ctx.limits);
        let _ = sessions::show(&l.ctx, &e.session_id, Some(e.provider));
    }
    // A client can resume an old session during the run and append to it; that is not
    // trovador. Anything else, a file shrinking or its existing bytes changing, would be.
    let after = snapshot_archives(&l.ctx.paths);
    let mut changed = Vec::new();
    let mut appended = 0;
    for (path, prefix) in &l.prefixes {
        let (size, mtime) = l.before[path];
        let Some((s, m)) = after.get(path) else {
            continue;
        };
        if (*s, *m) == (size, mtime) {
            continue;
        }
        if *s >= size && prefix_hash(path) == Some(*prefix) {
            appended += 1;
        } else {
            changed.push(path.display().to_string());
        }
    }
    eprintln!(
        "live: {} idle files checked, {appended} appended to by a running client",
        l.prefixes.len()
    );
    assert!(
        changed.is_empty(),
        "idle archive files rewritten: {changed:?}"
    );
}

// --- drift report ------------------------------------------------------------

#[test]
#[ignore = "reads this machine's real archives"]
fn report_record_categories_and_snippet_coverage() {
    let l = live();
    let mut omitted: BTreeMap<String, usize> = BTreeMap::new();
    let mut unrecognized: BTreeMap<String, usize> = BTreeMap::new();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for e in &l.sample {
        for r in projection(e, &l.ctx.limits) {
            let kind = serde_json::to_value(r.kind)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string();
            if r.kind == RecordKind::Omitted {
                *omitted
                    .entry(format!(
                        "{}:{}",
                        e.provider,
                        r.name.clone().unwrap_or_default()
                    ))
                    .or_default() += r.count.unwrap_or(1);
            }
            if r.kind == RecordKind::Unrecognized {
                *unrecognized
                    .entry(format!(
                        "{}:{}",
                        e.provider,
                        r.name.clone().unwrap_or_default()
                    ))
                    .or_default() += 1;
            }
            *kinds.entry(format!("{}:{kind}", e.provider)).or_default() += 1;
        }
    }
    let mut coverage: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (e, m) in &l.all {
        let c = coverage
            .entry(format!(
                "{}:{}",
                e.provider,
                m.origin.as_deref().unwrap_or("none")
            ))
            .or_default();
        c.0 += 1;
        c.1 += m.snippet.is_some() as usize;
    }
    eprintln!("live drift report (seed {}):", l.seed);
    eprintln!("  record kinds over the sample: {kinds:?}");
    eprintln!("  omitted categories over the sample, by source records:");
    for (k, n) in &omitted {
        eprintln!("    {k}: {n}");
    }
    // A type a newer CLI introduced shows up here: the projection has not been taught it.
    eprintln!("  unrecognized types over the sample:");
    for (k, n) in &unrecognized {
        eprintln!("    {k}: {n}");
    }
    eprintln!("  sessions with a snippet, by provider and origin:");
    for (k, (n, with)) in &coverage {
        eprintln!("    {k}: {with}/{n}");
    }
}

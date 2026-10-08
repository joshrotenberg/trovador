//! The SQLite index at `~/.trovador/index.db`: session metadata plus an FTS5 table of
//! projected record text, kept current incrementally.
//!
//! Records are keyed by rowid `session_row << 32 | record_index`, so one session's records
//! are a contiguous rowid range. FTS5 can delete and filter a rowid range directly, where a
//! filter on an UNINDEXED column would scan the whole table.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::archive::{self, ArchiveEntry, Backend, SessionMeta};
use crate::limits::Limits;
use crate::paths::Paths;
use crate::transcript::{CodexMode, PROJECTION_VERSION, Projection, ProjectionState, RecordKind};

const SCHEMA_VERSION: i64 = 2;

/// Origins whose text is not indexed. A Codex `guardian` session is an approval reviewer
/// whose input is a copy of its parent session's history: on 2026-10-07 that was 67 MB of
/// the 227 MB of projected text, and indexing it puts a duplicate hit on a reviewer session
/// for most queries that match a real one.
const TEXT_EXCLUDED_ORIGINS: &[&str] = &["guardian"];

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("index database: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("index directory {path}: {source}")]
    Dir {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, IndexError>;

pub struct Index {
    conn: Connection,
    path: PathBuf,
}

pub fn db_path(paths: &Paths) -> PathBuf {
    paths.home.join("index.db")
}

fn mtime_ns(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn key(backend: Backend, id: &str) -> (Backend, String) {
    (backend, id.to_string())
}

impl Index {
    /// Opens the index, creating it and its directory if needed.
    pub fn open(paths: &Paths) -> Result<Self> {
        std::fs::create_dir_all(&paths.home).map_err(|source| IndexError::Dir {
            path: paths.home.display().to_string(),
            source,
        })?;
        let path = db_path(paths);
        let conn = Connection::open(&path)?;
        let index = Self { conn, path };
        index.configure()?;
        index.migrate()?;
        Ok(index)
    }

    /// Opens the index only if it already exists. Reading a list must not create state.
    pub fn open_existing(paths: &Paths) -> Option<Self> {
        let path = db_path(paths);
        if !path.is_file() {
            return None;
        }
        let conn = Connection::open(&path).ok()?;
        let index = Self { conn, path };
        index.configure().ok()?;
        let version: Option<i64> = index
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .ok();
        (version == Some(SCHEMA_VERSION)).then_some(index)
    }

    fn configure(&self) -> Result<()> {
        // WAL lets the CLI read while a running server writes, and the timeout makes two
        // writers wait for each other instead of failing.
        self.conn.busy_timeout(Duration::from_secs(5))?;
        self.conn
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        register_regexp(&self.conn)?;
        Ok(())
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);",
        )?;
        let version: Option<i64> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if version == Some(SCHEMA_VERSION) {
            return Ok(());
        }
        // Everything here is derived from the archives, so an unknown schema is rebuilt
        // rather than migrated.
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS records;
             DROP TABLE IF EXISTS sessions;
             CREATE TABLE sessions (
                 id INTEGER PRIMARY KEY,
                 backend TEXT NOT NULL,
                 session_id TEXT NOT NULL,
                 file TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL,
                 meta TEXT NOT NULL,
                 paths TEXT NOT NULL,
                 origin TEXT,
                 subagent INTEGER NOT NULL,
                 text_indexed INTEGER NOT NULL,
                 projection_version INTEGER NOT NULL,
                 offset INTEGER NOT NULL,
                 next_index INTEGER NOT NULL,
                 codex_mode INTEGER NOT NULL,
                 mode_final INTEGER NOT NULL,
                 run_category TEXT,
                 run_count INTEGER,
                 last_kind TEXT,
                 last_hash INTEGER,
                 UNIQUE (backend, session_id)
             );
             CREATE VIRTUAL TABLE records USING fts5(
                 text, kind UNINDEXED, name UNINDEXED,
                 tokenize = 'unicode61 remove_diacritics 2'
             );",
        )?;
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('schema', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [SCHEMA_VERSION],
        )?;
        Ok(())
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Stored metadata for every session, for callers that want it without reading files.
    /// An entry is only trustworthy while the file's size and mtime still match.
    pub fn meta_snapshot(&self) -> HashMap<(Backend, String), (u64, i64, SessionMeta)> {
        let mut out = HashMap::new();
        let Ok(mut stmt) = self
            .conn
            .prepare("SELECT backend, session_id, size, mtime_ns, meta FROM sessions")
        else {
            return out;
        };
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        });
        for (b, id, size, mtime, meta) in rows.into_iter().flatten().flatten() {
            if let (Ok(b), Ok(meta)) = (b.parse::<Backend>(), serde_json::from_str(&meta)) {
                out.insert((b, id), (size as u64, mtime, meta));
            }
        }
        out
    }

    pub fn status(&self) -> Result<IndexStatus> {
        let (sessions, text_indexed): (i64, i64) = self.conn.query_row(
            "SELECT count(*), coalesce(sum(text_indexed), 0) FROM sessions",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let records: i64 = self
            .conn
            .query_row("SELECT count(*) FROM records", [], |r| r.get(0))?;
        let size_bytes = ["", "-wal"]
            .iter()
            .filter_map(|s| {
                let mut p = self.path.clone().into_os_string();
                p.push(s);
                std::fs::metadata(PathBuf::from(p)).ok()
            })
            .map(|m| m.len())
            .sum();
        Ok(IndexStatus {
            path: self.path.display().to_string(),
            size_bytes,
            sessions: sessions as usize,
            text_indexed: text_indexed as usize,
            records: records as usize,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct IndexStatus {
    pub path: String,
    pub size_bytes: u64,
    pub sessions: usize,
    pub text_indexed: usize,
    pub records: usize,
}

// --- refresh ---------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct RefreshOpts {
    /// Stop starting new files after this long. Files already started finish.
    pub budget: Option<Duration>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RefreshReport {
    /// Session files in the archives.
    pub sessions: usize,
    /// Files whose size and mtime matched the index, so nothing was read.
    pub unchanged: usize,
    /// Files continued from where the last pass stopped.
    pub appended: usize,
    /// Files indexed from the start: new, rewritten, moved, or from an older projection.
    pub reindexed: usize,
    /// Files the budget ran out before reaching. The index is stale for these.
    pub pending: usize,
    /// Index rows dropped because their file is gone.
    pub removed: usize,
    /// Files that could not be read.
    pub failed: usize,
    pub ms: u64,
}

struct StoredRow {
    id: i64,
    file: String,
    size: i64,
    mtime_ns: i64,
    text_indexed: bool,
    projection_version: i64,
    state: ProjectionState,
}

enum Work {
    Full,
    Resume(i64, ProjectionState),
}

struct Job {
    entry: ArchiveEntry,
    work: Work,
}

struct Done {
    entry: ArchiveEntry,
    meta: SessionMeta,
    text: bool,
    /// Where this pass started, to detect another process indexing the same file meanwhile.
    resumed_from: Option<(i64, u64)>,
    records: Vec<(usize, RecordKind, Option<String>, String)>,
    state: ProjectionState,
}

fn text_eligible(meta: &SessionMeta) -> bool {
    !meta
        .origin
        .as_deref()
        .is_some_and(|o| TEXT_EXCLUDED_ORIGINS.contains(&o))
}

fn kind_str(k: RecordKind) -> &'static str {
    match k {
        RecordKind::User => "user",
        RecordKind::Assistant => "assistant",
        RecordKind::ToolCall => "tool_call",
        RecordKind::ToolResult => "tool_result",
        RecordKind::Attachment => "attachment",
        RecordKind::System => "system",
        RecordKind::Reasoning => "reasoning",
        RecordKind::Omitted => "omitted",
    }
}

fn project(job: Job, limits: &Limits) -> Option<Done> {
    let meta = archive::read_meta(&job.entry, limits);
    let text = text_eligible(&meta);
    let (resume, resumed_from) = match job.work {
        Work::Resume(id, s) => {
            let from = (id, s.offset);
            (Some(s), Some(from))
        }
        Work::Full => (None, None),
    };
    let mut records = Vec::new();
    let state = if text {
        let mut proj = Projection::for_index(&job.entry, limits, resume).ok()?;
        for r in proj.by_ref() {
            if let Some(t) = r.text {
                records.push((r.index, r.kind, r.name, t));
            }
        }
        proj.state().clone()
    } else {
        // Not projected at all: the row only carries metadata.
        ProjectionState {
            offset: job.entry.size_bytes,
            mode_final: true,
            ..Default::default()
        }
    };
    Some(Done {
        entry: job.entry,
        meta,
        text,
        resumed_from,
        records,
        state,
    })
}

impl Index {
    fn stored_rows(&self) -> Result<HashMap<(Backend, String), StoredRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, backend, session_id, file, size, mtime_ns, text_indexed,
                    projection_version, offset, next_index, codex_mode, mode_final,
                    run_category, run_count, last_kind, last_hash
             FROM sessions",
        )?;
        let rows = stmt.query_map([], |r| {
            let run = match (
                r.get::<_, Option<String>>(12)?,
                r.get::<_, Option<i64>>(13)?,
            ) {
                (Some(c), Some(n)) => Some((c, n as usize)),
                _ => None,
            };
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                StoredRow {
                    id: r.get(0)?,
                    file: r.get(3)?,
                    size: r.get(4)?,
                    mtime_ns: r.get(5)?,
                    text_indexed: r.get::<_, i64>(6)? != 0,
                    projection_version: r.get(7)?,
                    state: ProjectionState {
                        offset: r.get::<_, i64>(8)? as u64,
                        next_index: r.get::<_, i64>(9)? as usize,
                        mode: CodexMode::from_i64(r.get(10)?),
                        mode_final: r.get::<_, i64>(11)? != 0,
                        run,
                        last_turn: match (
                            r.get::<_, Option<String>>(14)?,
                            r.get::<_, Option<i64>>(15)?,
                        ) {
                            (Some(k), Some(h)) => kind_from_str(&k).map(|k| (k, h as u64)),
                            _ => None,
                        },
                    },
                },
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (b, id, stored) = row?;
            if let Ok(b) = b.parse::<Backend>() {
                out.insert((b, id), stored);
            }
        }
        Ok(out)
    }

    /// Brings the index up to date with the archives, newest sessions first.
    pub fn refresh(
        &mut self,
        paths: &Paths,
        limits: &Limits,
        opts: &RefreshOpts,
    ) -> Result<RefreshReport> {
        let started = Instant::now();
        // A session id can appear in two project directories after a move; the newest
        // file is the live one, and scan_all is newest first.
        let mut seen = std::collections::HashSet::new();
        let entries: Vec<ArchiveEntry> = archive::scan_all(paths)
            .into_iter()
            .filter(|e| seen.insert(key(e.backend, &e.session_id)))
            .collect();
        let stored = self.stored_rows()?;
        let mut report = RefreshReport {
            sessions: entries.len(),
            ..Default::default()
        };

        let mut jobs = Vec::new();
        for e in &entries {
            let file = e.file.display().to_string();
            let work = match stored.get(&key(e.backend, &e.session_id)) {
                Some(s)
                    if s.file == file
                        && s.size == e.size_bytes as i64
                        && s.mtime_ns == mtime_ns(e.mtime)
                        && s.projection_version == PROJECTION_VERSION =>
                {
                    report.unchanged += 1;
                    continue;
                }
                // An append-only file that only grew continues where the last pass stopped.
                Some(s)
                    if s.file == file
                        && s.text_indexed
                        && s.projection_version == PROJECTION_VERSION
                        && s.state.mode_final
                        && e.size_bytes >= s.state.offset
                        && e.size_bytes as i64 >= s.size =>
                {
                    Work::Resume(s.id, s.state.clone())
                }
                _ => Work::Full,
            };
            jobs.push(Job {
                entry: e.clone(),
                work,
            });
        }

        // Rows whose file is gone, but only for an archive root that could be read: an
        // unmounted or unreadable root says nothing about whether its sessions still exist.
        let live: std::collections::HashSet<(Backend, String)> = entries
            .iter()
            .map(|e| key(e.backend, &e.session_id))
            .collect();
        let roots: Vec<(&std::path::Path, bool)> = [
            &paths.claude_archive,
            &paths.codex_archive,
            &paths.codex_archived,
        ]
        .into_iter()
        .map(|r| (r.as_path(), std::fs::read_dir(r).is_ok()))
        .collect();
        let root_readable = |file: &str| {
            roots
                .iter()
                .find(|(root, _)| std::path::Path::new(file).starts_with(root))
                .is_some_and(|(_, ok)| *ok)
        };
        let gone: Vec<i64> = stored
            .iter()
            .filter(|(k, _)| !live.contains(*k))
            .filter(|(_, s)| root_readable(&s.file))
            .map(|(_, s)| s.id)
            .collect();
        if !gone.is_empty() {
            let tx = self.conn.transaction()?;
            for id in &gone {
                delete_records(&tx, *id)?;
                tx.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
            }
            tx.commit()?;
            report.removed = gone.len();
        }

        let deadline = opts.budget.map(|b| started + b);
        let total = jobs.len();
        let queue = Mutex::new(jobs.into_iter());
        let started_jobs = AtomicUsize::new(0);
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 8);
        let (tx, rx) = mpsc::sync_channel::<Option<Done>>(workers * 2);

        let mut write_error = None;
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let tx = tx.clone();
                let queue = &queue;
                let started_jobs = &started_jobs;
                scope.spawn(move || {
                    loop {
                        if deadline.is_some_and(|d| Instant::now() >= d) {
                            break;
                        }
                        let Some(job) = queue.lock().expect("queue").next() else {
                            break;
                        };
                        started_jobs.fetch_add(1, Ordering::Relaxed);
                        if tx.send(project(job, limits)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tx);
            for done in rx {
                if write_error.is_some() {
                    continue;
                }
                match done {
                    None => report.failed += 1,
                    Some(d) => {
                        let resumed = d.resumed_from.is_some();
                        match self.write(d) {
                            Ok(true) if resumed => report.appended += 1,
                            Ok(true) => report.reindexed += 1,
                            Ok(false) => {}
                            Err(e) => write_error = Some(e),
                        }
                    }
                }
            }
        });
        if let Some(e) = write_error {
            return Err(e);
        }
        report.pending = total - started_jobs.load(Ordering::Relaxed);
        report.ms = started.elapsed().as_millis() as u64;
        Ok(report)
    }

    /// One file in one transaction. Returns false when another process indexed the same
    /// file first, in which case this pass's work is discarded.
    fn write(&mut self, d: Done) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let k = key(d.entry.backend, &d.entry.session_id);
        let existing: Option<(i64, i64)> = tx
            .query_row(
                "SELECT id, offset FROM sessions WHERE backend = ?1 AND session_id = ?2",
                params![k.0.as_str(), k.1],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, from)) = d.resumed_from {
            if existing != Some((id, from as i64)) {
                return Ok(false);
            }
        } else if let Some((id, _)) = existing {
            delete_records(&tx, id)?;
        }
        let paths_text = [d.meta.cwd.as_deref(), d.meta.relocated_cwd.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n");
        let meta_json = serde_json::to_string(&d.meta).unwrap_or_else(|_| "{}".into());
        let s = &d.state;
        let id: i64 = tx.query_row(
            "INSERT INTO sessions (backend, session_id, file, size, mtime_ns, meta, paths, origin,
                 subagent, text_indexed, projection_version, offset, next_index, codex_mode,
                 mode_final, run_category, run_count, last_kind, last_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT (backend, session_id) DO UPDATE SET
                 file = excluded.file, size = excluded.size, mtime_ns = excluded.mtime_ns,
                 meta = excluded.meta, paths = excluded.paths, origin = excluded.origin,
                 subagent = excluded.subagent, text_indexed = excluded.text_indexed,
                 projection_version = excluded.projection_version, offset = excluded.offset,
                 next_index = excluded.next_index, codex_mode = excluded.codex_mode,
                 mode_final = excluded.mode_final, run_category = excluded.run_category,
                 run_count = excluded.run_count, last_kind = excluded.last_kind,
                 last_hash = excluded.last_hash
             RETURNING id",
            params![
                k.0.as_str(),
                k.1,
                d.entry.file.display().to_string(),
                d.entry.size_bytes as i64,
                mtime_ns(d.entry.mtime),
                meta_json,
                paths_text,
                d.meta.origin,
                d.meta.subagent as i64,
                d.text as i64,
                PROJECTION_VERSION,
                s.offset as i64,
                s.next_index as i64,
                s.mode.as_i64(),
                s.mode_final as i64,
                s.run.as_ref().map(|(c, _)| c.clone()),
                s.run.as_ref().map(|(_, n)| *n as i64),
                s.last_turn.map(|(k, _)| kind_str(k)),
                s.last_turn.map(|(_, h)| h as i64),
            ],
            |r| r.get(0),
        )?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO records (rowid, text, kind, name) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (idx, kind, name, text) in &d.records {
                insert.execute(params![
                    (id << 32) | (*idx as i64),
                    text,
                    kind_str(*kind),
                    name
                ])?;
            }
        }
        tx.commit()?;
        Ok(true)
    }
}

fn kind_from_str(s: &str) -> Option<RecordKind> {
    match s {
        "user" => Some(RecordKind::User),
        "assistant" => Some(RecordKind::Assistant),
        _ => None,
    }
}

fn delete_records(tx: &rusqlite::Transaction, id: i64) -> rusqlite::Result<usize> {
    tx.execute(
        "DELETE FROM records WHERE rowid BETWEEN ?1 AND ?2",
        params![id << 32, (id << 32) | 0xffff_ffff],
    )
}

fn register_regexp(conn: &Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        "regexp",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            // Compiled once per statement rather than once per row.
            let re = ctx.get_or_create_aux(0, |v| -> std::result::Result<_, regex::Error> {
                regex::RegexBuilder::new(v.as_str().unwrap_or(""))
                    .case_insensitive(true)
                    .size_limit(1 << 22)
                    .build()
            })?;
            let text = ctx.get_raw(1).as_str().unwrap_or("");
            Ok(re.is_match(text))
        },
    )
}

// --- search ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// Sessions whose best match ranks highest first.
    #[default]
    Relevance,
    /// Most recently modified sessions first.
    Recent,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct SearchOpts {
    /// Words and "quoted phrases", all required. A word or phrase matches as a sequence of
    /// tokens, so a UUID or a snake_case name matches as written.
    pub query: String,
    /// Treat `query` as an FTS5 expression (AND, OR, NOT, NEAR, prefix*, column filters).
    #[serde(default)]
    pub fts: bool,
    /// Treat `query` as a case-insensitive regular expression over the indexed text.
    #[serde(default)]
    pub regex: bool,
    pub backend: Option<Backend>,
    /// Only sessions whose project path contains this text.
    pub project: Option<String>,
    /// Only sessions with this origin.
    pub origin: Option<String>,
    /// Include sessions started by another session.
    #[serde(default)]
    pub include_subagents: bool,
    /// Sessions to return.
    pub limit: Option<usize>,
    /// Matching records to show per session.
    pub per_session: Option<usize>,
    #[serde(default)]
    pub order: Order,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Match {
    /// Record index; pass it as the transcript offset to read around the match.
    pub index: usize,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub excerpt: String,
}

pub struct SessionHits {
    pub backend: Backend,
    pub session_id: String,
    pub meta: SessionMeta,
    pub match_count: usize,
    pub matches: Vec<Match>,
}

pub struct RawSearch {
    /// The FTS5 expression actually run, or the regex.
    pub expression: String,
    pub sessions_matched: usize,
    pub hits: Vec<SessionHits>,
}

/// Turns words and "quoted phrases" into an FTS5 expression where each is a quoted phrase
/// and all are required. Quoting every term keeps punctuation in identifiers from being
/// read as FTS5 operators.
pub fn terms_to_fts(query: &str) -> Option<String> {
    let mut terms = Vec::new();
    let mut rest = query.trim();
    while !rest.is_empty() {
        let (term, tail) = if let Some(r) = rest.strip_prefix('"') {
            match r.find('"') {
                Some(end) => (&r[..end], &r[end + 1..]),
                None => (r, ""),
            }
        } else {
            match rest.find(char::is_whitespace) {
                Some(end) => (&rest[..end], &rest[end..]),
                None => (rest, ""),
            }
        };
        if !term.trim().is_empty() {
            terms.push(format!("\"{}\"", term.trim().replace('"', "\"\"")));
        }
        rest = tail.trim_start();
    }
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

impl Index {
    pub fn search(&self, opts: &SearchOpts, limit: usize, per_session: usize) -> Result<RawSearch> {
        let expression = if opts.regex || opts.fts {
            opts.query.clone()
        } else {
            terms_to_fts(&opts.query).unwrap_or_default()
        };
        if expression.trim().is_empty() {
            return Ok(RawSearch {
                expression,
                sessions_matched: 0,
                hits: Vec::new(),
            });
        }

        let mut filters = vec!["s.text_indexed = 1".to_string()];
        let mut args: Vec<rusqlite::types::Value> = vec![expression.clone().into()];
        if let Some(b) = opts.backend {
            args.push(b.as_str().to_string().into());
            filters.push(format!("s.backend = ?{}", args.len()));
        }
        if let Some(p) = opts.project.as_deref().filter(|p| !p.is_empty()) {
            args.push(format!("%{}%", like_escape(p)).into());
            filters.push(format!("s.paths LIKE ?{} ESCAPE '\\'", args.len()));
        }
        if let Some(o) = opts.origin.as_deref() {
            args.push(o.to_string().into());
            filters.push(format!("s.origin = ?{}", args.len()));
        }
        if !opts.include_subagents {
            filters.push("s.subagent = 0".to_string());
        }
        let where_s = filters.join(" AND ");
        let matcher = if opts.regex {
            "regexp(?1, records.text)"
        } else {
            "records MATCH ?1"
        };
        // bm25() is only allowed in a query whose sole table is the FTS table, so records
        // are scored in a materialized CTE and joined and aggregated outside it. A plain
        // subquery is flattened back into the join and fails the same way.
        let score = if opts.regex { "0" } else { "bm25(records)" };
        let cte = format!(
            "WITH m AS MATERIALIZED (
                 SELECT records.rowid >> 32 AS sid, {score} AS score FROM records WHERE {matcher})"
        );
        let matched = "m JOIN sessions s ON s.id = m.sid";
        let order = match (opts.order, opts.regex) {
            (Order::Relevance, false) => "best ASC, s.mtime_ns DESC",
            _ => "s.mtime_ns DESC",
        };

        let sessions_matched: i64 = self.conn.query_row(
            &format!("{cte} SELECT count(DISTINCT m.sid) FROM {matched} WHERE {where_s}"),
            rusqlite::params_from_iter(args.iter()),
            |r| r.get(0),
        )?;

        let mut top = self.conn.prepare(&format!(
            "{cte} SELECT s.id, s.backend, s.session_id, s.meta, count(*) AS n, min(m.score) AS best
             FROM {matched} WHERE {where_s}
             GROUP BY s.id ORDER BY {order} LIMIT {limit}"
        ))?;
        let rows: Vec<(i64, String, String, String, i64)> = top
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;

        let excerpt_sql = if opts.regex {
            format!(
                "SELECT rowid & 4294967295, kind, name, text FROM records
                 WHERE {matcher} AND rowid BETWEEN ?2 AND ?3
                 ORDER BY rowid LIMIT {per_session}"
            )
        } else {
            format!(
                "SELECT rowid & 4294967295, kind, name,
                        snippet(records, 0, '[', ']', '...', 24) FROM records
                 WHERE {matcher} AND rowid BETWEEN ?2 AND ?3
                 ORDER BY rank LIMIT {per_session}"
            )
        };
        let mut excerpts = self.conn.prepare(&excerpt_sql)?;
        let re = if opts.regex {
            regex::RegexBuilder::new(&opts.query)
                .case_insensitive(true)
                .build()
                .ok()
        } else {
            None
        };

        let mut hits = Vec::new();
        for (id, backend, session_id, meta, n) in rows {
            let Ok(backend) = backend.parse::<Backend>() else {
                continue;
            };
            let matches = excerpts
                .query_map(
                    params![expression, id << 32, (id << 32) | 0xffff_ffff],
                    |r| {
                        let text: String = r.get(3)?;
                        Ok(Match {
                            index: r.get::<_, i64>(0)? as usize,
                            kind: r.get(1)?,
                            name: r.get(2)?,
                            excerpt: match &re {
                                Some(re) => regex_excerpt(re, &text),
                                None => text.split_whitespace().collect::<Vec<_>>().join(" "),
                            },
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            hits.push(SessionHits {
                backend,
                session_id,
                meta: serde_json::from_str(&meta).unwrap_or_default(),
                match_count: n as usize,
                matches,
            });
        }
        Ok(RawSearch {
            expression,
            sessions_matched: sessions_matched as usize,
            hits,
        })
    }

    /// One session's indexed records as (index, type, text), in order. What search can
    /// find for that session, for checking it against the transcript projection.
    pub fn session_records(
        &self,
        backend: Backend,
        id: &str,
    ) -> Result<Vec<(usize, String, String)>> {
        let sid: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE backend = ?1 AND session_id = ?2",
                params![backend.as_str(), id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(sid) = sid else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare(
            "SELECT rowid & 4294967295, kind, text FROM records
             WHERE rowid BETWEEN ?1 AND ?2 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![sid << 32, (sid << 32) | 0xffff_ffff], |r| {
            Ok((r.get::<_, i64>(0)? as usize, r.get(1)?, r.get(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Sessions the index has metadata for but no text, by origin.
    pub fn text_excluded(&self) -> Result<HashMap<String, usize>> {
        let mut stmt = self.conn.prepare(
            "SELECT coalesce(origin, ''), count(*) FROM sessions WHERE text_indexed = 0 GROUP BY 1",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.flatten().map(|(o, n)| (o, n as usize)).collect())
    }
}

/// About 90 characters either side of the first match, whitespace collapsed.
fn regex_excerpt(re: &regex::Regex, text: &str) -> String {
    let Some(m) = re.find(text) else {
        return text.chars().take(180).collect();
    };
    let start = text[..m.start()]
        .char_indices()
        .rev()
        .nth(89)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end = text[m.end()..]
        .char_indices()
        .nth(90)
        .map(|(i, _)| m.end() + i)
        .unwrap_or(text.len());
    let body = format!(
        "{}[{}]{}",
        &text[start..m.start()],
        m.as_str(),
        &text[m.end()..end]
    );
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "{}{}{}",
        if start > 0 { "..." } else { "" },
        body,
        if end < text.len() { "..." } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_term_is_quoted_so_punctuation_is_not_an_operator() {
        assert_eq!(
            terms_to_fts(r#"idempotency_key "exact phrase" NOT-an-op"#).as_deref(),
            Some(r#""idempotency_key" "exact phrase" "NOT-an-op""#)
        );
        assert_eq!(terms_to_fts(r#"say "hi"#).as_deref(), Some(r#""say" "hi""#));
        assert_eq!(terms_to_fts(r#"a"b"#).as_deref(), Some(r#""a""b""#));
        assert_eq!(terms_to_fts("   "), None);
    }

    #[test]
    fn a_regex_excerpt_brackets_the_match() {
        let re = regex::Regex::new("needle").unwrap();
        assert_eq!(regex_excerpt(&re, "a needle here"), "a [needle] here");
    }
}

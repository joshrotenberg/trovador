//! Shared task history. A lifetime lease distinguishes server instances even when PIDs repeat.
use crate::manager::{Status, TaskSnapshot};
use bottega_core::private::{private_dir, private_file, write_private};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Owner {
    pub pid: u32,
    pub instance_id: String,
    pub started_at_ms: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Record {
    pub version: u32,
    pub owner: Owner,
    pub provider_pid: Option<u32>,
    #[serde(flatten)]
    pub snapshot: TaskSnapshot,
}
struct Version {
    version: Option<serde_json::Value>,
}
impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct VersionVisitor;
        impl<'de> serde::de::Visitor<'de> for VersionVisitor {
            type Value = Version;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a task record object with a version field")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Version, M::Error> {
                let mut version = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "version" {
                        version = Some(map.next_value()?);
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(Version { version })
            }
        }
        deserializer.deserialize_map(VersionVisitor)
    }
}
// Streaming headers let scans skip allocating result text and structured output.
#[derive(Deserialize)]
struct Header {
    owner: Owner,
    task_id: String,
    conversation_id: String,
    parent_task_id: Option<String>,
    status: Status,
    started_at_ms: u64,
    finished_at_ms: Option<u64>,
}
impl Record {
    pub fn owner_error(&self) -> String {
        format!(
            "task is running under owner pid {} instance {} started at {}; only that server may cancel or continue it",
            self.owner.pid, self.owner.instance_id, self.owner.started_at_ms
        )
    }
}
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTasks {
    pub status: Option<Status>,
    /// Inclusive Unix timestamp in milliseconds.
    pub since: Option<u64>,
    /// Defaults to 50; maximum 10000.
    pub limit: Option<usize>,
    pub full_result: Option<bool>,
}
pub fn filter(mut tasks: Vec<TaskSnapshot>, input: ListTasks) -> Result<Vec<TaskSnapshot>, String> {
    let limit = input.limit.unwrap_or(50);
    if !(1..=10000).contains(&limit) {
        return Err("limit must be between 1 and 10000".into());
    }
    tasks.retain(|t| {
        input.status.is_none_or(|s| s == t.status)
            && input.since.is_none_or(|s| t.started_at_ms >= s)
    });
    tasks.sort_by(|a, b| {
        b.started_at_ms
            .cmp(&a.started_at_ms)
            .then_with(|| b.task_id.cmp(&a.task_id))
    });
    tasks.truncate(limit);
    if !input.full_result.unwrap_or(false) {
        for task in &mut tasks {
            task.result = None;
            task.structured_result = None;
        }
    }
    Ok(tasks)
}
pub struct Registry {
    dir: PathBuf,
    pub owner: Owner,
    _lease: Arc<Lease>,
    retention: usize,
    days: u64,
    maintenance: Mutex<Instant>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn setting(name: &str, default: u64) -> Result<u64, String> {
    match crate::config::var(name) {
        Ok(v) => v
            .parse()
            .map_err(|_| format!("TOSI_{name} must be a nonnegative integer")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(format!("TOSI_{name}: {e}")),
    }
}
impl Registry {
    pub fn open(state: &bottega_core::state::State) -> Result<Self, String> {
        Self::open_with_retention(
            state,
            setting("TASK_RETENTION", 500)? as usize,
            setting("TASK_RETENTION_DAYS", 30)?,
        )
    }
    pub fn open_with_retention(
        state: &bottega_core::state::State,
        retention: usize,
        days: u64,
    ) -> Result<Self, String> {
        if retention == 0 || days == 0 {
            return Err(
                "TOSI_TASK_RETENTION and TOSI_TASK_RETENTION_DAYS must be greater than zero".into(),
            );
        }
        let dir = state.tosi.clone();
        private_dir(&dir.join("tasks")).map_err(|e| e.to_string())?;
        private_dir(&dir.join("owners")).map_err(|e| e.to_string())?;
        let registration = private_file()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("registry.lock"))
            .map_err(|e| e.to_string())?;
        registration.lock().map_err(|e| e.to_string())?;
        static OWNER: OnceLock<Owner> = OnceLock::new();
        static LEASES: OnceLock<Mutex<HashMap<PathBuf, Weak<Lease>>>> = OnceLock::new();
        let owner = OWNER
            .get_or_init(|| Owner {
                pid: std::process::id(),
                instance_id: uuid::Uuid::new_v4().to_string(),
                started_at_ms: now(),
            })
            .clone();
        let mut leases = LEASES
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|e| e.to_string())?;
        let lease = if let Some(lease) = leases.get(&dir).and_then(Weak::upgrade) {
            lease
        } else {
            let path = dir.join("owners").join(&owner.instance_id);
            let file = private_file()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|e| e.to_string())?;
            file.lock().map_err(|e| e.to_string())?;
            write_private(
                &path.with_extension("json"),
                &serde_json::to_vec(&owner).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let lease = Arc::new(Lease { _file: file });
            leases.insert(dir.clone(), Arc::downgrade(&lease));
            lease
        };
        drop(leases);
        let store = Self {
            dir,
            owner,
            _lease: lease,
            retention,
            days,
            maintenance: Mutex::new(Instant::now()),
        };
        drop(registration);
        store.prune_best_effort();
        Ok(store)
    }
    fn lock(&self) -> Result<File, String> {
        self.lock_mode(false)
    }
    fn lock_mode(&self, shared: bool) -> Result<File, String> {
        let f = private_file()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join("registry.lock"))
            .map_err(|e| e.to_string())?;
        if shared { f.lock_shared() } else { f.lock() }.map_err(|e| e.to_string())?;
        Ok(f)
    }
    fn path(&self, id: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(id).map_err(|_| "invalid task_id".to_string())?;
        Ok(self.dir.join("tasks").join(format!("{id}.json")))
    }
    fn load(&self, id: &str) -> Result<Record, String> {
        let path = self.path(id)?;
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Ok(error) = fs::read_to_string(
                    self.dir
                        .join("tasks/quarantine")
                        .join(format!("{id}.error")),
                ) {
                    return Err(error);
                }
                let quarantined = self.dir.join("tasks/quarantine").join(format!("{id}.json"));
                match fs::read(quarantined) {
                    Ok(data) => data,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(format!("unknown task_id: {id} (pruned or never existed)"));
                    }
                    Err(e) => return Err(format!("cannot read quarantined task {id}: {e}")),
                }
            }
            Err(e) => return Err(format!("cannot read task {id}: {e}")),
        };
        let value: serde_json::Value = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
        if value["version"].as_u64() != Some(1) {
            return Err(format!(
                "unsupported task record version {} for {id}",
                value["version"]
            ));
        }
        serde_json::from_value(value).map_err(|e| format!("invalid task record {id}: {e}"))
    }
    fn save(&self, record: &Record) -> Result<(), String> {
        let mut value = serde_json::to_value(record).map_err(|e| e.to_string())?;
        // Disk never inherits the opt-out used for in-memory result presentation.
        for key in ["result", "structured_result", "error", "usage"] {
            if let Some(v) = value.get_mut(key) {
                bottega_core::redact::redact_value(v);
            }
        }
        write_private(
            &self.path(&record.snapshot.task_id)?,
            &serde_json::to_vec(&value).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }
    fn alive(&self, owner: &Owner) -> Result<bool, String> {
        if owner == &self.owner {
            return Ok(true);
        }
        uuid::Uuid::parse_str(&owner.instance_id)
            .map_err(|_| "invalid owner instance id".to_string())?;
        let marker = self
            .dir
            .join("owners")
            .join(format!("{}.json", owner.instance_id));
        match fs::read(marker) {
            Ok(data) => match serde_json::from_slice::<Owner>(&data) {
                Ok(identity) if &identity != owner => return Ok(false),
                Ok(_) => {}
                Err(e) => eprintln!("tosi: cannot parse owner identity; using lease: {e}"),
            },
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                eprintln!("tosi: cannot read owner identity; using lease: {e}");
            }
            Err(e) => return Err(format!("cannot read owner identity: {e}")),
        }
        let file = match File::open(self.dir.join("owners").join(&owner.instance_id)) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(format!("cannot read owner lease: {e}")),
        };
        match file.try_lock() {
            Ok(()) => Ok(false),
            Err(std::fs::TryLockError::WouldBlock) => Ok(true),
            Err(e) => Err(format!("cannot check owner lease: {e}")),
        }
    }
    fn owner_alive(&self, owner: &Owner) -> bool {
        match self.alive(owner) {
            Ok(alive) => alive,
            Err(e) => {
                eprintln!("tosi: owner liveness unknown; leaving task unchanged: {e}");
                true
            }
        }
    }
    fn recover(&self, mut record: Record) -> Result<Record, String> {
        if !record.snapshot.status.is_terminal() && !self.owner_alive(&record.owner) {
            record.snapshot.status = Status::Interrupted;
            record.snapshot.finished_at_ms = Some(now());
            let child = record.provider_pid.filter(|pid| process_alive(*pid));
            record.snapshot.error = Some(format!(
                "owning server pid {} instance {} stopped before the task finished{}",
                record.owner.pid, record.owner.instance_id,
                child.map(|p| format!("; pid {p} was the provider when the server stopped; check whether it is still running")).unwrap_or_default()
            ));
            self.save(&record)?;
        }
        Ok(record)
    }
    pub fn read(&self, id: &str) -> Result<Record, String> {
        let lock = self.lock_mode(true)?;
        let record = self.load(id)?;
        if record.snapshot.status.is_terminal() || self.owner_alive(&record.owner) {
            return Ok(record);
        }
        drop(lock);
        let _lock = self.lock()?;
        self.recover(self.load(id)?)
    }
    pub fn create(&self, snapshot: &TaskSnapshot) -> Result<(), String> {
        let _lock = self.lock()?;
        if let Some(parent) = &snapshot.parent_task_id {
            let records = self.records()?;
            if records.iter().any(|r| {
                r.conversation_id == snapshot.conversation_id
                    && (!r.status.is_terminal() || r.parent_task_id.as_ref() == Some(parent))
            }) {
                return Err("conversation already continued or has an active task; continue its latest completed task".into());
            }
        }
        self.save(&Record {
            version: 1,
            owner: self.owner.clone(),
            provider_pid: None,
            snapshot: snapshot.clone(),
        })
    }
    pub fn update(&self, snapshot: &TaskSnapshot) -> Result<(), String> {
        {
            let _lock = self.lock()?;
            let mut record = self.load(&snapshot.task_id)?;
            if record.owner != self.owner {
                return Err(record.owner_error());
            }
            record.snapshot = snapshot.clone();
            self.save(&record)?;
        }
        self.maintain();
        Ok(())
    }
    pub fn provider_started(&self, id: &str, pid: u32) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut record = self.load(id)?;
        if record.owner != self.owner {
            return Err(record.owner_error());
        }
        record.provider_pid = Some(pid);
        self.save(&record)
    }
    fn records(&self) -> Result<Vec<Header>, String> {
        let mut out = Vec::new();
        for entry in fs::read_dir(self.dir.join("tasks")).map_err(|e| e.to_string())? {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(e) => {
                    eprintln!("tosi: cannot scan task: {e}");
                    continue;
                }
            };
            if path.extension().is_none_or(|v| v != "json") {
                continue;
            }
            let version = match File::open(&path) {
                Ok(file) => serde_json::from_reader::<_, Version>(std::io::BufReader::new(file)),
                Err(e) => {
                    self.log_skipped(&path, &e.to_string());
                    continue;
                }
            };
            match version {
                Ok(Version { version: None }) => {
                    self.quarantine(&path, "task record has no version field");
                    continue;
                }
                Ok(Version {
                    version: Some(version),
                }) if version != serde_json::json!(1) => {
                    self.log_skipped(&path, &format!("unsupported task record version {version}"));
                    continue;
                }
                Ok(_) => {}
                Err(e) if e.is_io() => {
                    self.log_skipped(&path, &e.to_string());
                    continue;
                }
                Err(e) => {
                    self.quarantine(&path, &e.to_string());
                    continue;
                }
            }
            let parsed = (|| -> Result<Header, String> {
                let file = File::open(&path).map_err(|e| e.to_string())?;
                let mut header: Header = serde_json::from_reader(std::io::BufReader::new(file))
                    .map_err(|e| e.to_string())?;
                if path.file_stem().unwrap().to_string_lossy() != header.task_id {
                    return Err("task id does not match record filename".into());
                }
                uuid::Uuid::parse_str(&header.task_id)
                    .map_err(|_| "invalid task_id".to_string())?;
                uuid::Uuid::parse_str(&header.owner.instance_id)
                    .map_err(|_| "invalid owner instance id".to_string())?;
                if !header.status.is_terminal() && !self.owner_alive(&header.owner) {
                    let record = self.recover(self.load(&header.task_id)?)?;
                    header.status = record.snapshot.status;
                    header.finished_at_ms = record.snapshot.finished_at_ms;
                }
                Ok(header)
            })();
            match parsed {
                Ok(header) => out.push(header),
                Err(_) if !path.exists() => {}
                Err(e) => self.log_skipped(&path, &e),
            }
        }
        Ok(out)
    }
    pub fn list(&self, input: ListTasks) -> Result<Vec<TaskSnapshot>, String> {
        let limit = input.limit.unwrap_or(50);
        if !(1..=10000).contains(&limit) {
            return Err("limit must be between 1 and 10000".into());
        }
        self.maintain();
        let _lock = self.lock()?;
        let mut records = self.records()?;
        records.retain(|r| {
            input.status.is_none_or(|s| s == r.status)
                && input.since.is_none_or(|s| r.started_at_ms >= s)
        });
        records.sort_by(|a, b| {
            b.started_at_ms
                .cmp(&a.started_at_ms)
                .then_with(|| b.task_id.cmp(&a.task_id))
        });
        let mut tasks = Vec::new();
        for header in records {
            match self.load(&header.task_id) {
                Ok(record) => tasks.push(record.snapshot),
                Err(e) => self.log_skipped(&self.path(&header.task_id)?, &e),
            }
            if tasks.len() >= limit {
                break;
            }
        }
        filter(tasks, input)
    }
    fn log_skipped(&self, path: &std::path::Path, error: &str) {
        static WARNED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
        let mut warned = WARNED
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if warned.insert(path.to_path_buf()) {
            eprintln!(
                "tosi: skipping {}: {}",
                path.display(),
                bottega_core::redact::redact_str(error)
            );
        }
    }
    fn quarantine(&self, path: &std::path::Path, error: &str) {
        let error = bottega_core::redact::redact_str(error);
        eprintln!("tosi: quarantining {}: {error}", path.display());
        let dir = self.dir.join("tasks/quarantine");
        if let Err(e) = private_dir(&dir).and_then(|()| {
            fs::rename(path, dir.join(path.file_name().unwrap()))?;
            write_private(
                &dir.join(path.file_name().unwrap()).with_extension("error"),
                error.as_bytes(),
            )
        }) {
            eprintln!("tosi: cannot quarantine {}: {e}", path.display());
        }
    }
    fn prune_best_effort(&self) {
        if let Err(e) = self.prune() {
            eprintln!("tosi: task maintenance failed: {e}");
        }
    }
    fn maintain(&self) {
        let mut last = self.maintenance.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed() >= Duration::from_secs(60) {
            *last = Instant::now();
            drop(last);
            self.prune_best_effort();
        }
    }
    fn clean_temps(&self) -> Result<(), String> {
        for dir in [self.dir.join("tasks"), self.dir.join("owners")] {
            for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path.extension().is_some_and(|v| v == "tmp")
                    && fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|age| age >= Duration::from_secs(300))
                    && let Err(e) = fs::remove_file(&path)
                {
                    eprintln!("tosi: cannot remove stale temp {}: {e}", path.display());
                }
            }
        }
        Ok(())
    }
    pub fn prune(&self) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut records = self.records()?;
        records.sort_by_key(|r| std::cmp::Reverse(r.started_at_ms));
        let cutoff = now().saturating_sub(self.days.saturating_mul(86_400_000));
        let active = records.iter().filter(|r| !r.status.is_terminal()).count();
        let budget = self.retention.saturating_sub(active);
        let mut retained = 0;
        for record in &records {
            if !record.status.is_terminal() {
                continue;
            }
            if retained >= budget || record.finished_at_ms.unwrap_or(record.started_at_ms) < cutoff
            {
                fs::remove_file(self.path(&record.task_id)?).map_err(|e| e.to_string())?;
            } else {
                retained += 1;
            }
        }
        self.clean_temps()?;
        for entry in fs::read_dir(self.dir.join("owners")).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some() {
                continue;
            }
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(e) => {
                    eprintln!("tosi: cannot read owner lease {}: {e}", path.display());
                    continue;
                }
            };
            if file.try_lock().is_ok() {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
                let marker = path.with_extension("json");
                match fs::remove_file(marker) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.to_string()),
                }
            }
        }
        Ok(())
    }
}

struct Lease {
    _file: File,
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // Signal zero only checks existence and never sends a signal to the process.
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}
#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

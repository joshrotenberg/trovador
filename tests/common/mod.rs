//! Fixture archives built inside a temp directory, laid out like a real home directory.
//! Every test gets its own, so the suite can run concurrently and never sees real state.

#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tempfile::TempDir;
use trovador::{Ctx, Limits, Paths};

pub struct Fixture {
    pub dir: TempDir,
    pub paths: Paths,
}

impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::under(dir.path());
        Self { dir, paths }
    }

    pub fn ctx(&self) -> Ctx {
        Ctx {
            paths: self.paths.clone(),
            limits: Limits::default(),
        }
    }

    pub fn ctx_with(&self, limits: Limits) -> Ctx {
        Ctx {
            paths: self.paths.clone(),
            limits,
        }
    }

    /// Writes `<claude>/<project>/<id>.jsonl`. `age` sets the mtime that far in the past,
    /// so tests control list order without sleeping.
    pub fn claude(&self, project: &str, id: &str, records: &[Value], age: u64) -> PathBuf {
        let dir = self.paths.claude_archive.join(project);
        write(dir.join(format!("{id}.jsonl")), records, age)
    }

    /// Writes `<codex archived_sessions>/rollout-...-<id>.jsonl`, where Codex moves a thread
    /// it has archived.
    pub fn codex_archived(&self, id: &str, records: &[Value], age: u64) -> PathBuf {
        write(
            self.paths
                .codex_archived
                .join(format!("rollout-2026-09-01T12-00-00-{id}.jsonl")),
            records,
            age,
        )
    }

    /// Writes `<codex>/2026/10/07/rollout-...-<id>.jsonl`.
    pub fn codex(&self, id: &str, records: &[Value], age: u64) -> PathBuf {
        let dir = self.paths.codex_archive.join("2026/10/07");
        write(
            dir.join(format!("rollout-2026-10-07T12-00-00-{id}.jsonl")),
            records,
            age,
        )
    }
}

fn write(path: PathBuf, records: &[Value], age: u64) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body: String = records.iter().map(|r| format!("{r}\n")).collect();
    fs::write(&path, body).unwrap();
    let t = SystemTime::now() - Duration::from_secs(age);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(t)
        .unwrap();
    path
}

// --- record builders, in each provider's current shape ---------------------

pub fn claude_user(cwd: &str, text: &str) -> Value {
    json!({"type": "user", "cwd": cwd, "entrypoint": "cli",
           "message": {"role": "user", "content": text}})
}

pub fn claude_assistant(text: &str) -> Value {
    json!({"type": "assistant", "message": {"role": "assistant",
           "content": [{"type": "text", "text": text}]}})
}

pub fn claude_tool_use(name: &str, input: Value) -> Value {
    json!({"type": "assistant", "message": {"role": "assistant",
           "content": [{"type": "tool_use", "name": name, "input": input}]}})
}

pub fn claude_attachment(kind: &str) -> Value {
    json!({"type": "attachment", "attachment": {"type": kind}})
}

pub fn codex_meta(cwd: &str, source: Value) -> Value {
    json!({"type": "session_meta", "payload": {"cwd": cwd, "originator": "codex_exec",
           "source": source}})
}

pub fn codex_response(role: &str, text: &str) -> Value {
    json!({"type": "response_item", "payload": {"type": "message", "role": role,
           "content": [{"type": "input_text", "text": text}]}})
}

pub fn codex_item(item: Value) -> Value {
    json!({"type": "event_msg", "payload": {"type": "item_completed", "item": item}})
}

pub fn codex_user_item(text: &str) -> Value {
    codex_item(json!({"type": "UserMessage", "content": [{"type": "text", "text": text}]}))
}

pub fn codex_agent_item(text: &str) -> Value {
    codex_item(json!({"type": "AgentMessage", "content": [{"type": "Text", "text": text}]}))
}

pub fn codex_event(kind: &str, message: &str) -> Value {
    json!({"type": "event_msg", "payload": {"type": kind, "message": message}})
}

pub const ID_A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
pub const ID_B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
pub const ID_C: &str = "cccccccc-0000-4000-8000-000000000003";
pub const ID_D: &str = "abbbbbbb-0000-4000-8000-000000000004";

//! Fixture archives built inside a temp directory, laid out like a real home directory.
//! Every test gets its own, so the suite can run concurrently and never sees real state.

#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tempfile::TempDir;
use trovador::{Ctx, Limits, Paths};

/// Defaults for tests. Fixture files are written moments before they are read, so the
/// active-session default would hide every one; tests of that default turn it back on.
pub fn limits() -> Limits {
    Limits {
        active_window_s: 0,
        ..Limits::default()
    }
}

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
            limits: limits(),
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

// --- fake providers --------------------------------------------------------

/// The id the fake claude gives a forked session, and the fake codex a new one.
pub const FORK_ID: &str = "f0f0f0f0-0000-4000-8000-00000000000f";
pub const CODEX_NEW_ID: &str = "c0dec0de-0000-4000-8000-00000000000c";

const FAKE_CLAUDE: &str = r#"#!/bin/bash
# Speaks claude -p in stream-json or json. A prompt whose first line contains "slow" sleeps
# past any test timeout; "fail" ends with an error result. --no-session-persistence writes
# no transcript. Full prompts are appended to the .prompts log.
# One write per invocation, so concurrent runs cannot interleave their lines.
rec="$(printf '%s\n' "---" "cwd=$PWD" "depth=$TROVADOR_DEPTH"; printf 'arg=%s\n' "$@")"
printf '%s\n' "$rec" >> "__LOG__"
sid=""; resume=""; fork=0; fmt=""; persist=1
while [ $# -gt 0 ]; do
  case "$1" in
    --session-id) sid="$2"; shift;;
    --resume) resume="$2"; shift;;
    --fork-session) fork=1;;
    --output-format) fmt="$2"; shift;;
    --no-session-persistence) persist=0;;
  esac
  shift
done
prompt="$(cat)"
# Each prompt in its own file: summary chunks run in parallel.
mkdir -p "__LOG__.prompts.d"
printf '%s' "$prompt" > "$(mktemp "__LOG__.prompts.d/p.XXXXXX")"
first="$(printf '%s' "$prompt" | head -n1)"
safe="$(printf '%s' "$first" | tr -d '"\\' | cut -c1-200)"
if [ -n "$resume" ]; then
  if [ $fork = 1 ]; then id="__FORK__"; else id="$resume"; fi
elif [ -n "$sid" ]; then
  id="$sid"
else
  id="00000000-0000-4000-8000-000000000000"
fi
# "vanish" dies right after naming the session, before any transcript exists.
case "$first" in *vanish*)
  printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$id"
  exit 1;;
esac
if [ $persist = 1 ] && { [ -z "$resume" ] || [ $fork = 1 ]; }; then
  mkdir -p "__ARCHIVE__/-fake"
  printf '{"type":"user","cwd":"%s","message":{"role":"user","content":"started"}}\n' "$PWD" > "__ARCHIVE__/-fake/$id.jsonl"
fi
case "$first" in
  "This is part"*) reply="partial summary";;
  "These are sequential"*) reply="merged summary";;
  "Summarize this"*) reply="single summary";;
  *) reply="echo: $safe";;
esac
if [ "$fmt" = "json" ]; then
  printf '{"type":"result","subtype":"success","is_error":false,"result":"%s","session_id":"%s","total_cost_usd":0.001,"num_turns":1}\n' "$reply" "$id"
  exit 0
fi
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$id"
case "$first" in *slow*) sleep 30;; esac
case "$first" in *fail*)
  printf '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"it failed","session_id":"%s"}\n' "$id"
  exit 1;;
esac
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"partial"}]}}\n'
printf '{"type":"result","subtype":"success","is_error":false,"result":"%s","session_id":"%s","total_cost_usd":0.01,"num_turns":1,"usage":{"input_tokens":3}}\n' "$reply" "$id"
"#;

const FAKE_CODEX: &str = r#"#!/bin/bash
# Speaks codex exec --json, for exec and exec resume.
# One write per invocation, so concurrent runs cannot interleave their lines.
rec="$(printf '%s\n' "---" "cwd=$PWD" "depth=$TROVADOR_DEPTH"; printf 'arg=%s\n' "$@")"
printf '%s\n' "$rec" >> "__LOG__"
resume=0; id="__NEW__"; pos=(); skip=0
for a in "$@"; do
  if [ $skip = 1 ]; then skip=0; continue; fi
  case "$a" in
    exec|-) ;;
    resume) resume=1;;
    -c|--config|-m|--model|-s|--sandbox|--add-dir|-C|--cd|--output-schema|-p|--profile) skip=1;;
    -*) ;;
    *) pos+=("$a");;
  esac
done
if [ $resume = 1 ] && [ ${#pos[@]} -gt 0 ]; then id="${pos[0]}"; fi
prompt="$(cat)"
# Each prompt in its own file: summary chunks run in parallel.
mkdir -p "__LOG__.prompts.d"
printf '%s' "$prompt" > "$(mktemp "__LOG__.prompts.d/p.XXXXXX")"
first="$(printf '%s' "$prompt" | head -n1)"
safe="$(printf '%s' "$first" | tr -d '"\\' | cut -c1-200)"
printf '{"type":"thread.started","thread_id":"%s"}\n' "$id"
if [ $resume = 0 ]; then
  mkdir -p "__ARCHIVE__/2026/10/07"
  printf '{"type":"session_meta","payload":{"cwd":"%s","source":"exec"}}\n' "$PWD" > "__ARCHIVE__/2026/10/07/rollout-2026-10-07T00-00-00-$id.jsonl"
fi
case "$first" in *slow*) sleep 30;; esac
case "$first" in *fail*) echo '{"type":"turn.failed","error":{"message":"it failed"}}'; exit 1;; esac
printf '{"type":"item.completed","item":{"type":"agent_message","text":"echo: %s"}}\n' "$safe"
echo '{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":2}}'
"#;

impl Fixture {
    /// Points this fixture at fake claude and codex binaries that write into its archives.
    pub fn with_fakes(mut self) -> Self {
        let bin = self.dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let claude = FAKE_CLAUDE
            .replace("__LOG__", &bin.join("claude.args").display().to_string())
            .replace(
                "__ARCHIVE__",
                &self.paths.claude_archive.display().to_string(),
            )
            .replace("__FORK__", FORK_ID);
        let codex = FAKE_CODEX
            .replace("__LOG__", &bin.join("codex.args").display().to_string())
            .replace(
                "__ARCHIVE__",
                &self.paths.codex_archive.display().to_string(),
            )
            .replace("__NEW__", CODEX_NEW_ID);
        for (name, body) in [("claude", claude), ("codex", codex)] {
            let p = bin.join(name);
            fs::write(&p, body).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        self.paths.claude_bin = Some(bin.join("claude"));
        self.paths.codex_bin = Some(bin.join("codex"));
        self
    }

    /// Every prompt a fake binary received, oldest first. Calls that ran in parallel
    /// have no meaningful order among themselves.
    pub fn prompts(&self, name: &str) -> Vec<String> {
        let dir = self
            .dir
            .path()
            .join("bin")
            .join(format!("{name}.args.prompts.d"));
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut files: Vec<_> = entries
            .flatten()
            .map(|e| {
                let t = e.metadata().and_then(|m| m.modified()).ok();
                (t, e.path())
            })
            .collect();
        files.sort();
        files
            .into_iter()
            .map(|(_, p)| fs::read_to_string(p).unwrap_or_default())
            .collect()
    }

    /// Every invocation of a fake binary, as the lines it logged: cwd=, depth=, arg=.
    pub fn calls(&self, name: &str) -> Vec<Vec<String>> {
        let log = self.dir.path().join("bin").join(format!("{name}.args"));
        let Ok(text) = fs::read_to_string(log) else {
            return Vec::new();
        };
        text.split("---\n")
            .filter(|c| !c.trim().is_empty())
            .map(|c| c.lines().map(str::to_string).collect())
            .collect()
    }
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

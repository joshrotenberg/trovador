# Tosi

A personal local plugin that lets either Codex or Claude Code delegate tasks to
Claude and Codex through the same five MCP tools. One Rust executable owns the
task records and CLI processes; a small skill describes how to delegate and
collect results. It uses `claude-wrapper`, `codex-wrapper`, and `tower-mcp`.

The source checkout's `ROADMAP.md` tracks routing,
delegation guidance, dogfood reviews, distribution work, and a second-round
orchestration candidate covering workflows, debate, consensus, and pipelines.
`NAMING.md` records the choice of Tosi and the earlier naming research.

Tosi is intended for personal use on a home machine and laptop. It is one crate in
the bottega workspace, next to trovador, which it links to record the sessions it
starts, so trovador's list and search include them; bottega's marketplace installs
both as plugins. Workers always start fresh: past sessions are history, never
continued or forked.

## Build and install

tosi lives in the [bottega](../../README.md) workspace, which also packages it as a
plugin for Claude Code and Codex (`plugins/tosi`). Install the binary, then the
plugin from the bottega marketplace:

```sh
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked --git https://github.com/joshrotenberg/bottega tosi
```

On each machine, install the provider CLIs you want to use and authenticate them
normally. The plugin does not bundle CLIs, credentials, or model access. You only need
Claude installed if you run Claude tasks, and Codex installed if you run Codex tasks.
Restart the host or start a new session after installing or updating.

For ordinary delegation, call the installed tools through the host, using its
tool discovery if they are deferred. If they are unavailable, check the plugin
connection and refresh the host session. Do not substitute a shell or Python
stdio bridge: it creates a separate server with its own environment, sandbox,
and task registry.

### Migrating from Task Runners

The source directory is now `tosi` instead of `task-runners`, and the executable,
plugin, and generated package use `tosi`. Rebuild from the renamed checkout and
update explicit launch paths. First remove the old plugin and its catalog
registration: the old catalog points at the moved directory, and its stale path
can prevent host catalog operations. For the previous user-scoped installations:

```sh
codex plugin remove task-runners@task-runners-local
codex plugin marketplace remove task-runners-local
claude plugin uninstall task-runners@task-runners-local --scope user --keep-data
claude plugin marketplace remove task-runners-local --scope user
```

Then install `tosi@bottega` as above. Removing the old plugin first avoids loading
both servers and preserves Claude's plugin data. Refresh the host session and rediscover the tools;
existing host caches do not rename themselves. MCP resource URIs now start with
`skill://tosi/`.

Finish or cancel active work before replacing a server: task IDs remain local to
that process. Existing `TASK_RUNNERS_*` configuration variables remain accepted
as legacy fallbacks, with the corresponding `TOSI_*` value taking precedence.
Use the new names in new configurations. Historical review records retain the
old identity and paths as evidence of the versions they reviewed.

## Tools

| Tool | Arguments | Behavior |
| --- | --- | --- |
| `check_environment` | Optional `backend`: `claude` or `codex`; omitted checks both | Report executable, environment, and authentication status without a model call. |
| `spawn_task` | `task`, absolute `cwd`; optional `backend`, `model`, `effort`, `access`, `timeout_secs`, `output_schema` | Start one bounded turn in a new session and return its ID. |
| `wait_tasks` | `task_ids`; optional `timeout_ms` | Return when any task finishes or the wait expires; `0` reads a snapshot. Default 30 seconds, maximum 55. |
| `continue_task` | `task_id`, `message` | Continue the newest successful turn in the same provider conversation; return a new task ID. |
| `cancel_task` | `task_id` | Cancel and wait up to 55 seconds for cleanup; return `cancelling` if cleanup is still running. |

Example `spawn_task` input:

```json
{
  "backend": "claude",
  "task": "Review lib/parser.ex for correctness. Return only actionable findings with file and line references. Do not edit files.",
  "cwd": "/absolute/path/to/project",
  "access": "read_only"
}
```

The omitted backend defaults to Claude. `timeout_secs` defaults to 600 and has a
maximum of 3600. A follow-up inherits the original backend, model, effort, directory,
access, per-turn timeout, and output schema. Only the newest successful turn can
be continued, and only one turn runs per provider conversation. Failed or cancelled conversations do
not automatically retry. Remove finished IDs before the next `wait_tasks`
call, because any already finished task causes an immediate return. Ending a
wait does not cancel its workers.

## Model and effort selection

Pass `model` and optional `effort` to `spawn_task` when you want explicit execution
settings. Honor the user's selections. If omitted, these options use the provider
CLI's behavior, including applicable defaults and inherited environment in the
server's isolated launch context. They do not inherit the calling chat's model
or effort. The explicit effort value `none` is different from omission.

| Backend | Exposed effort values | Provider option |
| --- | --- | --- |
| Claude | `low`, `medium`, `high`, `xhigh`, `max` | `--effort` |
| Codex | `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, `ultra` | `model_reasoning_effort` |

Support depends on the model. Invalid enum values, unsupported backend/value
combinations, and known incompatible exact-model combinations are rejected before
launch. The source checkout's `resources/model-capabilities.json` supplies the
exact-model checks, including recognized dated/context forms; aliases and unknown
model IDs are passed to the provider with compatibility unverified. Validation
makes no CLI or API model request.
Identical effort labels do not promise equal compute, cost, or quality across
providers. Codex advertises automatic delegation for `ultra`; the delegation
guidance reserves that choice for an explicit user request.

Every task snapshot retains `backend`, `model`, and `effort`, plus this selection
record. For example, a request omitting backend/model and asking for high effort
with the default Claude backend produces:

```json
{
  "selection": {
    "requested": { "backend": null, "model": null, "effort": "high" },
    "selected": { "backend": "claude", "model": null, "effort": "high" },
    "reported": { "model": null, "effort": null }
  }
}
```

`requested` preserves caller omissions; `selected` records the resolved backend
and model/effort options sent to the adapter. `reported` remains `null` because
the wrappers do not provide reliable per-turn model/effort evidence. Usage totals
and `modelUsage` do not establish those fields. The server does not downgrade,
substitute, fall back, or retry, but provider CLIs can resolve aliases, apply caps,
or use defaults; Tosi cannot attest the effective settings.

`continue_task` keeps the original backend/model/effort and schema without
overrides. Start a fresh task to change them. Reusing an alias or an omitted
setting does not freeze the provider's underlying model or defaults across turns.
For selection guidance, read the model and effort reference at
`resources/skills/delegate/references/model-effort.md` in the source checkout or
`skills/delegate/references/model-effort.md` in a generated plugin. MCP clients
can read `skill://tosi/delegate/references/model-effort.md`.

## Structured results

Pass `output_schema` to `spawn_task` when the result should be JSON with a known
shape. It must be a JSON Schema object with an explicit root `"type": "object"`
and fit within 64000 bytes when serialized as UTF-8 JSON. Schemas are validated
offline: local `$ref` references are supported, while external file and URL
references are rejected. Each provider supports its own JSON Schema subset;
prefer explicit object `properties`, list every property in `required`, and set
`additionalProperties` to `false`.

Example `spawn_task` input:

```json
{
  "backend": "codex",
  "cwd": "/absolute/path/to/project",
  "task": "Read context.txt and return its content in the marker field.",
  "output_schema": {
    "type": "object",
    "properties": { "marker": { "type": "string" } },
    "required": ["marker"],
    "additionalProperties": false
  }
}
```

Both providers receive the schema through their native structured-output
options. The server then validates the returned JSON locally before marking the
task `completed`. Read the JSON value from `structured_result`; it is separate
from the prose `result` and is `null` when no schema was requested. Claude may
return an empty prose result when it supplies only structured output.

For a task requesting a schema, missing output, invalid JSON, a schema mismatch,
or output exceeding 64000 serialized UTF-8 JSON bytes makes the task `failed`.
Structured JSON is never truncated. `continue_task` inherits the same schema
and cannot override it; start a new task when you need a different output shape.

## Configuration

Set these variables in the environment of the host that launches the MCP server,
or in its MCP server configuration. A GUI launched independently of your shell
may not inherit shell exports. Tosi starts provider processes directly;
it does not load a login shell, aliases, or shell initialization files.

| Variable | Default | Purpose |
| --- | --- | --- |
| `TOSI_DEFAULT_BACKEND` | `claude` | `claude` or `codex` when a task omits `backend`. |
| `TOSI_MAX_CONCURRENCY` | `4` | Maximum active tasks in this server process, from 1 to 32. Starts at capacity are rejected, not queued. |
| `TOSI_CLAUDE_BIN` | `claude` on `PATH` | Claude executable path. |
| `TOSI_CODEX_BIN` | `codex` on `PATH` | Codex executable path. |
| `TOSI_NO_REDACT` | unset | `1` returns worker results, structured results, and errors without replacing secrets (API keys, tokens, private keys, `NAME=secret` assignments) with `[redacted:<kind>]`. No tool argument can turn redaction off. |

Legacy `TASK_RUNNERS_*` counterparts are accepted when the corresponding `TOSI_*`
variable is absent. Prefer `TOSI_*`; the new name takes precedence when both exist.
`TOSI_NO_REDACT` has no legacy counterpart: a second, undocumented name for a switch
that turns off redaction is one nobody auditing the environment would look for.

If executable discovery differs between hosts, configure absolute
`TOSI_CLAUDE_BIN` and `TOSI_CODEX_BIN` paths. Keep `HOME`,
`CLAUDE_CONFIG_DIR`, and `CODEX_HOME` consistent with the intended CLI account in
the host environment when needed. Do not copy or export credentials to repair a
launch-context mismatch.

For intentional standalone MCP client setup, configure that client's approved
launch path to run the executable. It starts in stdio mode without arguments.
Use the host's normal approval process; do not bypass a denied action. Stdout is
reserved for MCP messages, and diagnostics go to stderr.

## Environment diagnostics

Call `check_environment` through the installed plugin before diagnosing a missing
CLI or authentication failure. Select a `backend` to check only that provider;
omitting it checks both. The same diagnostic is available as JSON from the CLI:

```sh
tosi doctor
tosi doctor --backend claude
```

The report includes executable path and version, nonsecret environment and
configuration paths, presence booleans for credential variables, and authentication
status (`authenticated`, `unavailable`, or `unknown`) with hints. It makes no
model call and returns no credential values. `doctor` exits with status 1 if any
selected provider is not ready; select one backend if only that CLI is needed.
Recognized versions are numeric `X.Y.Z` releases, optionally followed by
`-alpha.N`, `-beta.N`, or `-rc.N` with a numeric ordinal. Other version formats
remain unrecognized; raw probe output is withheld rather than returned as a
version. Readiness also requires an authenticated status and does not prove
network access or model availability.

An authentication error describes the checked process's launch context, not
necessarily the user's login. Compare the executable, configuration paths, and
host environment before recommending another login. A sandboxed process can
report unavailable authentication while the same CLI works in an approved host
context. Claude uses macOS Keychain for credential storage, making access to that
store one possible boundary; the status alone does not prove the cause. See
[Claude credential management](https://code.claude.com/docs/en/authentication#credential-management).

An inherited sandbox cannot be repaired inside the child process. Use the
configured host launch path and its normal approvals; do not disable protections,
read credential files, or extract Keychain secrets. Recommend signing in only
when checks in the intended host context support that diagnosis. A successful
status check does not test model access or account limits.

## Instructions from one source

The short core skill in `resources/skills/delegate/SKILL.md` covers the tool
contract and links to deeper references in its `references/` directory: parallel
tasks, implementation followed by review, continued conversations, and model/effort
selection. Load a reference only when it fits the task. The server exposes five
Markdown resources in total.

The MCP server embeds that same text as readable resources, starting at
`skill://tosi/delegate/SKILL.md`, so a client using only the executable
can discover the instructions. Plugin generation copies the same canonical
tree into `skills/`; it does not maintain a second version of the guidance.
Instruction changes require rebuilding the binary to refresh the embedded
resources, as well as regenerating any plugin package.

Longer CLI help and MCP instructions/tool descriptions live in `resources/cli/`
and `resources/mcp/`. Rust embeds them with `include_str!` at compile time, so
the executable does not need these source files at runtime.

## Access and task lifetime

`read_only` is the default. Claude workers receive only Read, Glob, and Grep;
`workspace_write` also enables Edit, Write, and NotebookEdit. These are Claude
tool restrictions, not an operating-system sandbox or a directory allowlist.
Claude workers cannot run Bash commands in this version. Use Codex or the parent
agent for shell commands and tests. Codex uses its read-only or workspace-write
sandbox with approvals disabled.

The supplied `cwd` is the working directory, not a promise of identical file
confinement across both providers. Use separate worktrees for concurrent edits;
the plugin does not create worktrees, merge changes, or review its own results.
Delegates receive the supplied brief and can inspect repository files, but do
not inherit the parent conversation or connected tools. Claude's isolated
configuration disables inherited custom hooks, installed plugins, MCP servers,
and skills, and skips automatic CLAUDE.md loading; include applicable repository
instructions explicitly in its brief. Claude's built-in plugins may still load,
subject to the explicit tool list. Codex keeps native AGENTS.md handling while ignoring
user configuration and disabling plugins. Project or managed configuration may
still apply to Codex.

Task records and results are held in memory for the current MCP server lifetime.
The server retains at most 1024 tasks, then rejects starts until a new server
session is created. Each prose result is limited to 64000 characters and marks
`result_truncated` if shortened. Requested structured results have a separate
64000-byte limit and fail instead of being truncated. Each provider stdout/stderr
capture is capped at 4 MiB; Claude turns are also limited to 30 CLI turns per task.
Provider CLIs may retain conversation history on disk, but task IDs cannot be
recovered after the server restarts. Each host connection creates its own server
and task registry; installing in both hosts or on both machines does not share
running tasks. There is no daemon, persistence service, scheduler, automatic
retry, or cross-machine resume.

Worker processes carry both `TOSI_CHILD=1` and the legacy `TASK_RUNNERS_CHILD=1`;
the server recognizes either marker to prevent recursive delegation across old
and new installations. Successful cancellation means
the adapter's cleanup returned, not that an arbitrary remote service rolled back
work. A `cancelling` task should be checked until it reaches a terminal state.

## Package layout

The plugin is `plugins/tosi` in the bottega repository: both hosts' manifests, the
`delegate` skill copied from `resources/skills/` by `scripts/sync-skills.sh`, and a
`bin/tosi` shim that runs the installed binary.

## Verification

Development review follows the source checkout's `AGENTS.md`: use both Task
Runners backends for substantive change batches, with bounded, independent
reviews and targeted follow-ups. Runtime tests remain separate from model review.

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked
python3 scripts/smoke.py
```

The default tests use fake provider executables and check parallel execution,
continuation ordering, failures, cancellation, timeouts, process cleanup, and shutdown during
an outstanding wait. Resource tests read the embedded instructions through MCP.
The Python 3 smoke script exercises the actual stdio MCP protocol.

Rust live integration tests are disabled by default. Enable them explicitly:

```sh
TOSI_LIVE_TESTS=1 cargo test --locked --test live -- --nocapture
```

These tests launch the compiled server, communicate through MCP stdio, and run
the actual provider CLIs. They check structured read-only results, a resumed
conversation preserving its schema and provider session, and workspace writes.
Both providers run by default. To select one, set
`TOSI_LIVE_BACKENDS=claude` or `TOSI_LIVE_BACKENDS=codex`; the
explicit value `claude,codex` selects both. A selected provider that is missing
or unauthenticated fails the test rather than being skipped.

Live checks use your CLI accounts and consume model usage. They create
disposable test directories and leave provider conversation history on disk.
The existing Python live smoke checks are also available:

```sh
python3 scripts/smoke.py --live
python3 scripts/smoke.py --live --edit-only
```

The first checks parallel read-only tasks and resumed conversations on both
providers. The second verifies each provider can write one requested test file.
Use `--backend claude` or `--backend codex` to check one provider.

# trovador

Local Claude Code and Codex sessions as one MCP server and CLI.

Both CLIs write their sessions to disk as JSONL. trovador reads those archives, lists, pages
through, and searches them, and serves the same answers to a terminal and to an agent over
MCP.

What is on disk is what the providers kept. Claude Code removes old transcripts according
to its `cleanupPeriodDays` setting, and Codex moves archived threads to
`~/.codex/archived_sessions`, which trovador also reads. A transcript the provider deleted is
gone from trovador's list and index on the next refresh.

It can also run turns: start a Claude or Codex session, continue one it started, or fork
one, with a consent policy deciding which existing sessions it may add a turn to.

It is the Rust successor to [spike](https://github.com/joshrotenberg/spike); the v0.1 scope
is tracked in [#1](https://github.com/joshrotenberg/trovador/issues/1).

## Status

| Slice | State |
|---|---|
| Archive reading: list, show, transcript | Done |
| Full-text index and search | Done |
| Execution: ask, continue, fork, with consent | Done |
| Summaries, consult, import from spike | Done |

## Install

From crates.io once published, or from a checkout:

```sh
cargo install trovador --locked        # or: cargo install --path . --locked
trovador import --dry-run      # optional: see what would come over from spike
trovador import                # optional: spike's consent records and summaries
trovador index                 # build the search index once; searches keep it current
claude mcp add --scope user trovador -- ~/.cargo/bin/trovador serve
codex mcp add trovador -- ~/.cargo/bin/trovador serve
```

`trovador doctor` checks the setup: it finds `claude` and `codex` and their versions,
reads the archives, and reports the index and consent state, with a fix for each problem.

Supported on macOS and Linux. Windows is untested: running a provider relies on Unix
process groups to stop it on timeout.

Register the absolute path: a client launched from the desktop may not have `~/.cargo/bin`
on its `PATH`. A stdio server is started once per client session and never reloads, so
restart the client after reinstalling.

## Coming from spike

trovador covers the parts of spike that were used, through fewer tools. On 2026-10-07 the
local transcripts held 28 calls to spike's MCP tools across 6 sessions, all to `ask`,
`search`, `list_sessions`, `whoami`, `digest`, `route`, `standing_list`, `surface`,
`operations`, `board`, and `pad_set`.

| spike | trovador |
|---|---|
| `list_sessions`, `dump` | `sessions` with `action: list` or `show` |
| `session://{backend}/{id}/transcript` | `sessions` with `action: transcript`, or the same resource |
| `search` | `search`, over a full-text index rather than the newest 400 files |
| `ask` | `ask` |
| `consult` | `ask` with `about` |
| `summarize`, `session://.../summary` | `summarize`, and the same resource |
| `agent-sessions consent ...` | `trovador consent ...` |
| `diagnose` | `trovador doctor` (CLI) |
| `canvass`, `digest`, `board`, `activity`, `windows`, `pin`, `pad`, `pad_set`, `standing_*`, `route`, `runners`, `usage`, `whoami`, `surface`, `operations`, `gc` | not ported |

`docs/parity-2026-10-07.md` compares the two on this machine's archive, produced by
`scripts/parity.py`.

## Use

```sh
trovador sessions list                       # newest first, top-level sessions only
trovador sessions list --project trovador    # project path contains this text
trovador sessions list --origin cli          # how it was started
trovador sessions list --include-subagents   # also sessions started by other sessions
trovador sessions show 27d1f434              # id or unambiguous prefix
trovador sessions transcript 27d1f434 --offset 200 --limit 50
trovador search idempotency_key              # words and "quoted phrases", all required
trovador search 'consent NOT spawned' --fts  # FTS5 syntax
trovador search 'retry_after:\s*\d+' --regex # regular expression, newest first
trovador index                               # bring the index fully up to date
trovador index --rebuild                     # discard it and build again
trovador ask "review the diff" --cwd ~/code/x # start a claude session (costs tokens)
trovador ask "and the tests?" --session 3e22  # continue it
trovador ask "try another way" --session 3e22 --fork
trovador ask - --backend codex < prompt.md    # prompt from stdin
trovador ask "what was decided?" --about 27d1 # consult: summary of 27d1, asked of codex
trovador summarize 27d1f434                   # cached until the session changes
trovador import --dry-run                     # what would come over from spike
trovador consent status                       # who may be continued
trovador consent approve 27d1f434             # allow one session trovador did not start
trovador consent policy approved              # ...and honour approvals
trovador doctor                              # paths, limits, archive counts
trovador serve                               # MCP over stdio
```

Every command prints one JSON document. The MCP `sessions` tool takes the same inputs as an
`action` of `list`, `show`, or `transcript`, and the `search` tool takes the same inputs as
`trovador search`; both return the same JSON as the CLI. Resources
`session://{backend}/{id}` and `session://{backend}/{id}/transcript` return `show` and the
first transcript page.

## What a session row carries

| Field | Source |
|---|---|
| `project` | Where a resume runs from: Claude's latest `relocated` cwd, else the first `cwd` |
| `snippet` | The first block the user typed, skipping injected `<...>` context and AGENTS.md |
| `title`, `title_source` | Claude `custom-title` (typed) over `ai-title` (generated); latest wins |
| `origin` | Claude `entrypoint` (`cli`, `claude-desktop`, `sdk-cli`); Codex `source` (`cli`, `vscode`, `exec`, `guardian`, `thread_spawn`) |
| `subagent`, `parent_session_id` | Codex sessions started by another session |
| `artifacts`, `pull_requests` | Claude `frame-link` and `pr-link` records |
| `archived` | Codex moved the thread to `archived_sessions` |
| `resumable` | Transcript size under the resume ceiling (900,000 bytes) |
| `open_with` | The command to paste in a terminal |

Lists hide subagents by default. On the machine this was developed on, 2,950 of 5,292
sessions were subagents, 1,694 of them Codex `guardian` approval reviewers.

## Transcripts

Records are `user`, `assistant`, `tool_call`, `tool_result`, `attachment`, `system`, or
`omitted` with a category. Consecutive omitted records of one category collapse into one
with a `count`. Text is capped per record (4,000 chars, 600 for tool calls and results) and
per page (100,000 chars); a page says when it stopped early and where to continue.

Codex writes each turn to more than one stream. trovador projects the one the user saw:
`item_completed` events in current versions, `user_message` and `agent_message` events in
older ones, and `response_item` records only in files that have nothing else.

Files are streamed. A page from a 377 MB Codex session took 0.30 s and 6.8 MB of resident
memory (release build, `/usr/bin/time -l`, 2026-10-07).

## Search

The index is SQLite with FTS5 at `~/.trovador/index.db`: one row of metadata per session and
one full-text row per projected record that carries text, keyed so a hit's `index` is the
transcript offset to open it at.

- **Queries.** By default every word and "quoted phrase" is required and each matches as a
  sequence of tokens, so a UUID or a `snake_case` name matches as written. `--fts` passes
  FTS5 syntax through (`OR`, `NOT`, `NEAR`, `prefix*`). `--regex` runs a case-insensitive
  regular expression over the indexed text.
- **Ranking.** Sessions are ordered by their best-matching record (bm25), or by recency with
  `--recent`. Regex results are always newest first.
- **Freshness.** A search first brings the index up to date, for at most
  `TROVADOR_INDEX_BUDGET_MS` (15,000). Files only appended to are read from where the last
  pass stopped; anything else changed is read again. `exhaustive` is true only when nothing
  was left out, and `absence_is_proof` says whether an empty result means the text is not
  there.
- **Not indexed.** Codex `guardian` approval reviewers keep their metadata but not their
  text, which is a copy of their parent session's history. `coverage.text_excluded` counts
  them.

Measured on 5,366 local sessions (12.4 GB of JSONL) on 2026-10-07, release build:

| Operation | Time |
|---|---|
| `trovador index --rebuild` | 8.65 s, 285 MB database, 511,568 records |
| A search with nothing new to index | 94 to 107 ms |
| A search that picks up two appended sessions | 234 ms |
| `sessions list` over every session, index present | 0.09 s (8.8 s without) |

## Running turns

`ask` runs one turn through the provider's own CLI (`claude -p` with stream-json, `codex exec
--json`). It starts a new session unless given `session`, an id or unambiguous prefix, in
which case it continues that one from the directory the session lives in. `fork` (Claude
only) resumes a session's context into a new session. The result carries the reply, the
`session_id`, cost or usage as the provider reports them, and whether the session can take
another turn.

- **The session id is recorded as soon as it exists.** For a new Claude session trovador
  chooses the id and records it before the process starts; for Codex and for forks it is
  recorded from the first streamed event that names it. A run that times out or fails still
  returns its `session_id`, and can be continued like any session trovador started.
- **Timeouts** default to 600 s for Claude and 900 s for Codex (`TROVADOR_CLAUDE_TIMEOUT_S`,
  `TROVADOR_CODEX_TIMEOUT_S`), or `timeout_s` per call. A timeout kills the provider's whole
  process group and comes back as a result with `timed_out: true`.
- **Options a backend cannot honour** (for example `max_turns` or `allowed_tools` on Codex)
  are reported in `warnings` rather than failing the run, so one set of options can be sent
  to both backends. `bypassPermissions` and Codex's `danger-full-access` are refused.
- **Depth.** Every session trovador starts can reach trovador again. `TROVADOR_DEPTH` is
  passed down and a run at `TROVADOR_MAX_DEPTH` (2) is refused.
- **Log.** Every run's start and end is a line in `~/.trovador/runs.jsonl`.

## Summaries and consulting

`summarize` renders a session's transcript, cuts it into 250,000-character chunks,
summarizes each with `haiku`, and merges them into one summary: goal, what happened, key
decisions, current state, open items. Past 8 chunks the first and last halves are kept and
the summary says which parts were left out, which bounds the cost of a very large session.

Summary runs use `--no-session-persistence`, so they leave no transcript in the archive and
are never offered for continuation. The result is cached in `~/.trovador/summaries/` against
the source file's size and mtime; a cache hit costs nothing and says `cached: true`. The
`session://{backend}/{id}/summary` resource returns only a cached summary and never makes one.

`ask` with `about: <session>` consults instead of continuing: it summarizes that session and
starts a new session, on the other backend unless `backend` says otherwise, seeded with the
summary and the question, in the source session's project. That works for sessions too
large to resume, and the new session can be continued like any trovador started.

## Importing from spike

`trovador import` reads spike's `~/.agent-sessions` and never writes to it. Spawned records
and approvals or denials are merged into trovador's consent state, keeping trovador's own
decision where both have one; spike's level is adopted only if trovador has none yet.
Summaries come over when the session has not changed since spike made them. Standing
sessions, pads, blobs, windows, runners, and reports are counted and not imported.
`--dry-run` reports without changing anything, and a second import changes nothing.

## Consent

Starting a session is always allowed. Continuing one is decided by `~/.trovador/consent.json`
(mode 0600):

| Level | May continue |
|---|---|
| `deny` | nothing |
| `spawned` (default) | sessions trovador started |
| `approved` | those, plus sessions approved with `trovador consent approve` |
| `all` | everything |

An explicit `trovador consent deny` wins at every level. Consent is checked against the
canonical session id, which is the only id the provider is given, so a prefix cannot get
around a denial. A state file that cannot be read denies everything and is never
overwritten. The policy changes only through the `trovador consent` CLI; the MCP server can
report a check (`sessions show` includes one) but has no tool that changes it.

## Where things live

trovador reads `~/.claude/projects`, `~/.codex/sessions`, and `~/.codex/archived_sessions`
and never writes to any of them itself; the providers write there when `ask` runs them. Its
own state goes under `~/.trovador`: `index.db`, `consent.json`, `runs.jsonl`, `summaries/`.
`sessions list` reads the index when it exists but never creates it. Override the paths with
`TROVADOR_CLAUDE_ARCHIVE`, `TROVADOR_CODEX_ARCHIVE`, `TROVADOR_CODEX_ARCHIVED`,
`TROVADOR_HOME`, and choose the provider binaries with `TROVADOR_CLAUDE_BIN` and
`TROVADOR_CODEX_BIN`. Limits are listed by `trovador doctor` and
overridable with `TROVADOR_*` variables (see `src/limits.rs`).

## Testing on a new machine

`cargo test` runs fixture archives only. `tests/live.rs` checks properties that must hold for
any machine's real archives, whatever CLI versions wrote them: every session file listed
once, transcripts contiguous and capped, the index equal to the transcript projection,
search finding a phrase taken from any indexed record, CLI and MCP agreeing, and archive
files left untouched. Its state goes to the cargo target directory, never `~/.trovador`.

```sh
cargo test --release --test live -- --ignored --nocapture
TROVADOR_LIVE_SPEND=1 cargo test --release --test live -- --ignored --nocapture
```

The second form also runs turns on both backends, a timeout, a summary, and a consult, on
sessions the tests create themselves; no existing transcript is sent to a provider.
Sampling is seeded and the seed is printed (`TROVADOR_LIVE_SEED` repeats a run). The
output ends with a drift report of record kinds and omitted categories, where a record type
a newer CLI introduced shows up as a new category.

## License

MIT or Apache-2.0, at your option.

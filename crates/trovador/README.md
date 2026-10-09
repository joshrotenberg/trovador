# trovador

Local Claude Code and Codex sessions as one MCP server and CLI.

Both CLIs write their sessions to disk as JSONL. trovador reads those archives, lists, pages
through, and searches them, and serves the same answers to a terminal and to an agent over
MCP.

What is on disk is what the providers kept. Claude Code removes old transcripts according
to its `cleanupPeriodDays` setting, and Codex moves archived threads to
`~/.codex/archived_sessions`, which trovador also reads. A transcript the provider deleted is
gone from trovador's list and index on the next refresh.

It only reads: past sessions are history. New work is tosi's (`crates/tosi`), and going
back into a session is a person's `claude --resume` or `codex resume`, which `sessions
show` prints as `open_with`.

It is the Rust successor to [spike](https://github.com/joshrotenberg/spike); the v0.1 scope
is tracked in [#1](https://github.com/joshrotenberg/trovador/issues/1).

## Status

| Slice | State |
|---|---|
| Archive reading: list, show, transcript | Done |
| Full-text index and search | Done |
| Running turns | Removed: new work is tosi's; past sessions are not continued |
| Summaries, import from spike | Done; consulting moved to tosi's delegate skill |

## Install

From crates.io once published, or from a checkout:

```sh
cargo install trovador --locked        # or: cargo install --path . --locked
trovador import --dry-run      # optional: see what would come over from spike
trovador import                # optional: spike's spawned-session records and summaries
trovador index                 # build the search index once; searches keep it current
claude mcp add --scope user trovador -- ~/.cargo/bin/trovador serve
codex mcp add trovador -- ~/.cargo/bin/trovador serve
```

`trovador doctor` checks the setup: it finds `claude` and `codex` and their versions,
reads the archives, and reports the index and the record of sessions the toolkit started,
with a fix for each problem.

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
| `ask` | tosi's `spawn_task` for new work; past sessions are not continued |
| `consult` | `summarize`, then tosi's `spawn_task` (tosi's consult reference) |
| `summarize`, `session://.../summary` | `summarize`, and the same resource |
| `agent-sessions consent ...` | removed: nothing continues past sessions |
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
trovador sessions list --since 1d            # last written in the past day (also 36h, 1w, 2026-10-01)
trovador sessions show 27d1f434              # id or unambiguous prefix
trovador sessions transcript 27d1f434 --offset 200 --limit 50
trovador search idempotency_key              # words and "quoted phrases", all required
trovador search "consent" --since 1w         # only sessions written in the last week
trovador search 'consent NOT spawned' --fts  # FTS5 syntax
trovador search 'retry_after:\s*\d+' --regex # regular expression, newest first
trovador index                               # bring the index fully up to date
trovador index --rebuild                     # discard it and build again
trovador summarize 27d1f434                   # cached until the session changes
trovador import --dry-run                     # what would come over from spike
trovador gc                                  # what cached summaries, saved replies, and run-log lines could go
trovador gc --apply                          # remove them
trovador doctor                              # providers, archives, index, provenance
trovador serve                               # MCP over stdio
```

Every command prints one JSON document. The MCP `sessions` tool takes the same inputs as an
`action` of `list`, `show`, `transcript`, or `output`, and the `search` tool takes the same inputs as
`trovador search`; both return the same JSON as the CLI. Resources
`session://{backend}/{id}` and `session://{backend}/{id}/transcript` return `show` and the
first transcript page.

## Examples

Real output from the development machine on 2026-10-08, over 5,452 sessions, while working on
trovador itself. Fields are trimmed (`…`), and home paths are shortened to `~`. Costs are as
the provider reported them.

**Where did that come from?** One search covers both agents. The session running the search
had just mentioned the term, so it is left out and counted in `hidden`.

```console
$ trovador search stdout-check --per-session 1
{
  "query": "stdout-check",
  "sessions_matched": 3,
  "returned": 3,
  "hits": [
    {
      "backend": "claude",
      "session_id": "baad058e-684e-417c-9c59-dbf351e2dc7b",
      "title": "spike-dev",
      "matches": [{"index": 6074, "type": "tool_result",
        "excerpt": "14: \"verify\": \"npm run fmt:check && npm run lint && npm run check && npm run [stdout-check] && npm test\","}],
      …
    },
    { "backend": "claude", "session_id": "79892eda-…", "title": "spike-dev2", … },
    {
      "backend": "codex",
      "session_id": "01a007a0-134e-70e2-84e5-a19ae6996317",
      "matches": [{"index": 2987, "type": "system",
        "excerpt": "...cal blocker: `npm run fmt:check` fails on `README.md`. Tests pass 449/0; typecheck, lint, [stdout-check], and `git diff --check` pass."}],
      …
    }
  ],
  "hidden": {"active": 1, "automated": 0, "excluded": 0, "unavailable": 0},
  "exhaustive": true,
  "note": "matching sessions left out by default: 1 written in the last 600 s (include_active)"
}
```

**Open it where the match is.** `index` is the record to start a transcript page at.

```console
$ trovador sessions transcript 79892eda --limit 4
{
  "backend": "claude",
  "session_id": "79892eda-e7f0-4ff6-a162-623ae8bc2718",
  "offset": 0,
  "records": [
    {"index": 0, "type": "omitted", "name": "ai-title", "count": 1},
    {"index": 1, "type": "omitted", "name": "queue-operation", "count": 2},
    {"index": 2, "type": "user", "text": "```ini\n[wrap] main f528143 · 2 PRs open · 52 issues · working tree clean\n…"},
    {"index": 3, "type": "omitted", "name": "attachment", "count": 4}
  ],
  "has_more": true,
  "next_offset": 4
}
```

**What happened in it?** A 1.1 MB session, summarized with haiku for about a cent. Asking
again returns the cache at no cost until the session changes.

```console
$ trovador summarize 79892eda
{
  "session_id": "79892eda-e7f0-4ff6-a162-623ae8bc2718",
  "summary": "**Goal:** Fix issue #63 in the `spike` repo: `list --project` took about 1.16s because every session's metadata was read to apply the filter.\n\n**What happened:** Measurement showed the issue's framing was incomplete. …\n\n**Open items:**\n- Review and merge PR #90.\n- #66: file mtime is processing time, but records carry event time. …\n- #89: use tool-use count as liveness, …",
  "cached": false,
  "cost_usd": 0.00971224,
  "model": "haiku",
  "chunks": 1,
  "transcript_chars": 88727,
  …
}
```

**Ask about it, then keep going.** That is tosi's job now: `summarize` the session, then
`spawn_task` with the summary in the brief (see `crates/tosi`).

**Check a new machine.**

```console
$ trovador doctor
{
  "version": "0.1.0",
  "ok": true,
  "problems": [],
  "providers": [
    {"backend": "claude", "path": "~/.local/bin/claude", "version": "2.1.293 (Claude Code)"},
    {"backend": "codex", "path": "~/.local/bin/codex", "version": "codex-cli 0.157.1"}
  ],
  "archives": [
    {"name": "claude", "path": "~/.claude/projects", "readable": true, "sessions": 1594},
    {"name": "codex", "path": "~/.codex/sessions", "readable": true, "sessions": 3786},
    {"name": "codex archived", "path": "~/.codex/archived_sessions", "readable": true, "sessions": 73}
  ],
  "index": {"size_bytes": 418643968, "sessions": 5452, "text_indexed": 3694, "records": 671135, …},
  "provenance": {"spawned": 4, …},
  …
}
```

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

trovador only reads sessions, and nothing in the toolkit continues or forks one. New work
is tosi's `spawn_task` (`crates/tosi`), which records each session it starts (see
Provenance). The only model calls trovador makes are summaries:

- **Depth.** A summary runs `claude -p` with `TROVADOR_DEPTH` passed down, and a run at
  `TROVADOR_MAX_DEPTH` (2) is refused.
- **Log.** Every summary run's start and end is a line in `~/.trovador/runs.jsonl`, sharing a
  `run_id`.
- **Tasks.** Over MCP, `summarize` accepts task execution. A client that negotiates tasks
  gets a task id at once and polls `tasks/get`; any other client gets the result as before.
  A task without a TTL keeps its run and result for the Claude timeout plus one hour.
  Cancelling a task, or letting it expire, kills the run, and the run log records it as
  `cancelled`.

## Summaries

`summarize` renders a session's transcript, cuts it into 250,000-character chunks,
summarizes each with `haiku`, and merges them into one summary: goal, what happened, key
decisions, current state, open items. Past 8 chunks the first and last halves are kept and
the summary says which parts were left out, which bounds the cost of a very large session.

Summary runs use `--no-session-persistence`, so they leave no transcript in the archive and
are never offered for continuation. The result is cached in `~/.trovador/summaries/` against
the source file's size and mtime; a cache hit costs nothing and says `cached: true`. The
`session://{backend}/{id}/summary` resource returns only a cached summary and never makes one.
A summary longer than `TROVADOR_RESPONSE_MAX_CHARS` is returned in parts, each marked
`truncated`; `offset` (`--offset` in the CLI) reads the next part from the cache.

To ask a fresh session about an earlier one, which also works for sessions too large to
resume, use tosi's consult reference: `summarize`, then `spawn_task` with the summary.

## Importing from spike

`trovador import` reads spike's `~/.agent-sessions` and never writes to it. Its spawned
records are added to the provenance record; its consent level and decisions have no
counterpart here and are counted, not imported.
Summaries come over when the session has not changed since spike made them. Standing
sessions, pads, blobs, windows, runners, and reports are counted and not imported.
`--dry-run` reports without changing anything, and a second import changes nothing.

## Provenance

`~/.trovador/spawned.json` (mode 0600) records the sessions the toolkit started; tosi adds
each session it starts. list and search leave automated sessions (`claude -p`, `codex exec`)
out by default, except these. `sessions show` reports `spawned`. It is a record, not a
permission: nothing decides whether a past session may be continued, because nothing
continues one. An installation from before this record existed carries its spawned list over
from `consent.json` once. A record that cannot be read is never overwritten; until it is
repaired, the toolkit's sessions are hidden like any other automated session.

## Where things live

trovador reads `~/.claude/projects`, `~/.codex/sessions`, and `~/.codex/archived_sessions`
and never writes to any of them itself; the providers write there when tosi runs them. Its
own state goes under `~/.trovador`: `index.db`, `spawned.json`, `runs.jsonl`, `summaries/`.
`outputs/`, left by `ask` before it moved, is removed by `trovador gc` once old enough.
`sessions list` reads the index when it exists but never creates it. Override the paths with
`TROVADOR_CLAUDE_ARCHIVE`, `TROVADOR_CODEX_ARCHIVE`, `TROVADOR_CODEX_ARCHIVED`,
`TROVADOR_HOME`, and choose the provider binaries with `TROVADOR_CLAUDE_BIN` and
`TROVADOR_CODEX_BIN`. Limits are listed by `trovador doctor` and
overridable with `TROVADOR_*` variables (see `src/limits.rs`). `TROVADOR_INDEX_REASONING=0`
leaves reasoning records out of the index (416 MB to 290 MB on the development machine);
transcripts still show them.

## Testing on a new machine

`cargo test` runs fixture archives only. `tests/live.rs` checks properties that must hold for
any machine's real archives, whatever CLI versions wrote them: every session file listed
once, transcripts contiguous and capped, the index equal to the transcript projection,
search finding a phrase taken from any indexed record, CLI and MCP agreeing, and archive
files left untouched. Its state goes to the cargo target directory, never `~/.trovador`.

```sh
cargo test --release --test live -- --ignored --nocapture
```

No live test sends a transcript to a provider; the tests that ran model turns moved out with
`ask`. Sampling is seeded and the seed is printed (`TROVADOR_LIVE_SEED` repeats a run). The
output ends with a drift report of record kinds and omitted categories, where a record type
a newer CLI introduced shows up as a new category.

## License

MIT or Apache-2.0, at your option.

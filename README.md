# trovador

Local Claude Code and Codex sessions as one MCP server and CLI.

Both CLIs write their sessions to disk as JSONL. trovador reads those archives, lists, pages
through, and searches them, and serves the same answers to a terminal and to an agent over
MCP.

What is on disk is what the providers kept. Claude Code removes old transcripts according
to its `cleanupPeriodDays` setting, and Codex moves archived threads to
`~/.codex/archived_sessions`, which trovador also reads. A transcript the provider deleted is
gone from trovador's list and index on the next refresh.
It is the Rust successor to [spike](https://github.com/joshrotenberg/spike); the v0.1 scope
is tracked in [#1](https://github.com/joshrotenberg/trovador/issues/1).

## Status

| Slice | State |
|---|---|
| Archive reading: list, show, transcript | Done |
| Full-text index and search | Done |
| Execution: ask, continue, consult, with consent | Not started |
| Summaries, import from spike | Not started |

## Install

```sh
cargo install --path .
claude mcp add --scope user trovador -- trovador serve
```

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

## Where things live

trovador reads `~/.claude/projects`, `~/.codex/sessions`, and `~/.codex/archived_sessions`
and never writes to any of them. Its own state, the index, goes under `~/.trovador`.
`sessions list` reads the index when it exists but never creates it. Override the paths with
`TROVADOR_CLAUDE_ARCHIVE`, `TROVADOR_CODEX_ARCHIVE`, `TROVADOR_CODEX_ARCHIVED`,
`TROVADOR_HOME`. Limits are listed by `trovador doctor` and
overridable with `TROVADOR_*` variables (see `src/limits.rs`).

## License

MIT or Apache-2.0, at your option.

# trovador

Search and read local Claude Code and Codex session history, from a CLI or an MCP server.

Both CLIs write their sessions to disk as JSONL. trovador reads those archives, lists, pages
through, and searches them, and serves the same answers to a terminal and to an agent over
MCP. It runs no model and costs no tokens.

Codex 0.165 compresses finished rollouts to `rollout-*.jsonl.zst`; trovador reads those the
same as plain `.jsonl` files, and a session keeps its index row when Codex compresses it. On
the development machine on 2026-11-02, 5,307 of 5,804 Codex rollouts were compressed.

What is on disk is what the providers kept. Claude Code removes old transcripts according
to its `cleanupPeriodDays` setting, and Codex moves archived threads to
`~/.codex/archived_sessions`, which trovador also reads. A transcript the provider deleted is
gone from trovador's list; its index rows are removed on the next refresh that can read
its configured root.

Search only considers rows under its currently configured archive roots. It keeps rows
outside those roots in the shared index, so CLI and MCP processes with different archive
settings can search without deleting each other's rows. `trovador index` removes rows
outside the configured roots only when every configured root is readable or does not
exist, and reports the number removed. Otherwise its report includes
`cleanup_skipped_reason`, naming the roots that blocked cleanup and their read errors.
Refresh removes missing sessions under readable configured roots;
rows under an unavailable configured root are retained and search reports that root as
unavailable. If a configured root cannot be read and the index contains out-of-root rows
that could use another spelling of that root, search reports the root as unavailable
and does not claim absence proof. Missing roots without indexed rows under their spelling
do not prevent absence proof.

It only reads: past sessions are history. Going back into a session is a person's
`claude --resume` or `codex resume`, which `sessions show` prints as `open_with`. Running
new agent work is a separate tool's job.

It is the Rust successor to [spike](https://github.com/joshrotenberg/spike). It lived for a
while in a workspace called bottega as `crates/trovador`; that history is on the
`archive/bottega` branch.

## Status

| Slice | State |
|---|---|
| Archive reading: list, show, transcript, text | Done |
| Full-text index and search | Done |
| Running turns, summaries, consent | Removed: trovador runs no model |

## Install

From crates.io once published, or from a checkout:

```sh
cargo install trovador --locked        # or: cargo install --path . --locked
trovador index                 # build the search index once; searches keep it current
claude mcp add --scope user trovador -- ~/.cargo/bin/trovador serve
codex mcp add trovador -- ~/.cargo/bin/trovador serve
```

`trovador doctor` checks the setup: it reads the archives and reports the index, with a
fix for each problem.

Supported on macOS and Linux. Windows is untested.

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
| `summarize`, `session://.../summary` | `sessions` with `action: text`; the caller summarizes |
| `ask`, `consult`, `agent-sessions consent ...` | removed: trovador runs no model and continues nothing |
| `diagnose` | `trovador doctor` (CLI) |
| `gc`, `canvass`, `digest`, `board`, `activity`, `windows`, `pin`, `pad`, `pad_set`, `standing_*`, `route`, `runners`, `usage`, `whoami`, `surface`, `operations` | not ported |

`docs/parity-2026-10-07.md` compares the two on this machine's archive, produced by
`scripts/parity.py`.

## Use

The `--project` filter for listing and searching matches path substrings ignoring ASCII
case. Non-ASCII case remains significant. The MCP `project` parameters behave identically.

```sh
trovador sessions list                       # newest first, top-level sessions only
trovador sessions list --project trovador    # project path contains this text (ASCII case-insensitive)
trovador sessions list --origin cli          # how it was started
trovador sessions list --include-subagents   # also sessions started by other sessions
trovador sessions list --since 1d            # last written in the past day (also 36h, 1w, 2026-10-01)
trovador sessions show 27d1f434              # id or unambiguous prefix
trovador sessions transcript 27d1f434 --offset 200 --limit 50
trovador sessions text 27d1f434              # the whole session as plain text, in parts
trovador search idempotency_key              # words and "quoted phrases", all required
trovador search "redact" --since 1w          # only sessions written in the last week
trovador search 'redact NOT summary' --fts   # FTS5 syntax
trovador search 'retry_after:\s*\d+' --regex # regular expression, newest first
trovador index                               # bring the index fully up to date
trovador index --rebuild                     # discard it and build again
trovador doctor                              # archives and index
trovador serve                               # MCP over stdio
```

The `--since` and `--until` filters accept RFC 3339 times with `Z` or numeric offsets
(for example, `2026-10-01T09:00:00-07:00`, equivalent to `2026-10-01T16:00:00Z`),
dates at midnight UTC, and durations back from now. Times without a zone use UTC.

Every command prints one JSON document. The MCP `sessions` tool takes the same inputs as an
`action` of `list`, `show`, `transcript`, or `text`, and the `search` tool takes the same inputs as
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
  "hidden": {"active": 1, "automated": 0, "subagents": 0, "excluded": 0, "unavailable": 0},
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

**What happened in it?** `sessions text` returns the session as plain text, one block per
record, in parts of `TROVADOR_RESPONSE_MAX_CHARS`. An agent reads as many parts as it needs
and summarizes them itself; `truncated.next_offset` says where the next part starts.

**Check a new machine.**

```console
$ trovador doctor
{
  "version": "0.1.0",
  "ok": true,
  "problems": [],
  "archives": [
    {"name": "claude", "path": "~/.claude/projects", "readable": true, "sessions": 1594},
    {"name": "codex", "path": "~/.codex/sessions", "readable": true, "sessions": 3786},
    {"name": "codex archived", "path": "~/.codex/archived_sessions", "readable": true, "sessions": 73}
  ],
  "index": {"size_bytes": 418643968, "sessions": 5452, "text_indexed": 3694, "records": 671135, …},
  …
}
```

## What a session row carries

| Field | Source |
|---|---|
| `project` | Where a resume runs from: Claude's latest `relocated` cwd, else the first `cwd` |
| `snippet` | The first block the user typed, skipping injected `<...>` context and AGENTS.md; a forked Codex child uses its own first message, including an inter-agent assignment |
| `title`, `title_source` | Claude `custom-title` (typed) over `ai-title` (generated); latest wins |
| `origin` | Claude `entrypoint` (`cli`, `claude-desktop`, `sdk-cli`); Codex `source` (`cli`, `vscode`, `exec`, `guardian`, `thread_spawn`) |
| `subagent`, `parent_session_id` | Codex sessions started by another session |
| `artifacts`, `pull_requests` | Claude `frame-link` and `pr-link` records |
| `archived` | Codex moved the thread to `archived_sessions` |
| `resumable` | Transcript size under the resume ceiling (900,000 bytes) |
| `open_with` | The command to paste in a terminal |

Indexed Claude files retain the latest custom title across appends. Without an index,
files up to `TROVADOR_TITLE_SCAN_MAX_BYTES` (4,194,304 bytes by default) are scanned for
custom titles. Larger unindexed files use the head and tail windows, so a custom title
between those windows can be missed. Run `trovador index` to recover it. Setting the limit
to `0` disables the fallback scan. Between index refreshes, list and show scan appended
bytes only when `size - stored offset` fits the same budget. Above it, they apply the
stored custom title over the head/tail metadata until `trovador index` or search refreshes
the file. Complete final JSON title records without a newline can be displayed within
the budget, but enter the persisted title state only after their newline is written.

Lists hide subagents by default. On the machine this was developed on, 2,950 of 5,292
sessions were subagents, 1,694 of them Codex `guardian` approval reviewers.

## Transcripts

Records are `user`, `assistant`, `tool_call`, `tool_result`, `attachment`, `system`, `reasoning`,
or `omitted` with a category. `reasoning` appears only when the provider kept readable text.
Forked Codex children collapse copied parent history into an `omitted` record named
`forked_history`; it does not contribute turns, indexed text, or session text. An
impossible boundary in an archived file preserves the original transcript. A terminal-event
fallback in an unarchived file is for display only; indexing waits for a known boundary.
Consecutive omitted records of one category collapse into one
with a `count`. Text is capped per record (4,000 chars, 600 for tool calls and results) and
per page (100,000 chars); a page says when it stopped early and where to continue.

Offsets into a compressed rollout count decompressed bytes. A zstd stream has no random
access, so a page or index pass that starts partway through decodes from the beginning and
discards what it skips. A rollout that stops decoding partway is a read error for that
session, never a shorter transcript. `resumable` for a compressed rollout uses the
decompressed size from the frame header: one measured at 749 KB on disk held 2,947 KB of
JSONL.

Codex writes each turn to more than one stream. trovador projects the one the user saw:
`item_completed` events in current versions, `user_message` and `agent_message` events in
older ones, and `response_item` records in the oldest files. In Items mode, user turns
that exist only as response items are also retained, including guardian review turns.
Copies across response, user-message event, and UserMessage item streams are projected
once even when their text blocks were flattened. Response user blocks concatenate
without adding separators. Separate identical turns remain separate. Injected response
blocks stay excluded from the displayed user text.

Files are streamed. A page from a 377 MB Codex session took 0.30 s and 6.8 MB of resident
memory (release build, `/usr/bin/time -l`, 2026-10-07).

## Search

The index is SQLite with FTS5 at `~/.trovador/index.db`: one row of metadata per session and
one full-text row per projected record that carries text, keyed so a hit's `index` is the
transcript offset to open it at.

- **Queries.** By default every word and "quoted phrase" is required and each matches as a
  sequence of tokens, so a UUID or a `snake_case` name matches as written. `--fts` passes
  FTS5 syntax through (`OR`, `NOT`, `NEAR`, `prefix*`). `--regex` runs a case-insensitive
  regular expression over the indexed text. Terms are checked with the index's SQLite
  tokenizer before opening or refreshing the index. A non-empty term that produces no
  tokens is rejected because the AND query cannot match; use `--regex` to search
  punctuation. Empty quoted phrases and stray quotes are ignored, but a query with no
  remaining searchable terms is rejected. Valid FTS5 syntax runs unchanged, including
  negated filters and OR branches. When an FTS query returns no hits and references the
  unindexed `kind` or `name` columns or contains a phrase or word with no tokens,
  `absence_is_proof` is false and a note explains why.
- **Ranking.** Sessions are ordered by their best-matching record (bm25), or by recency with
  `--recent`. Regex results are always newest first.
- **Hidden matches.** `hidden` counts matching sessions left out: `active` for recent
  writes, `automated` for automated sessions not started by trovador, `subagents` for
  sessions started by another session, `excluded` for `exclude_session`, and `unavailable`
  for unreadable archive roots. Counts can overlap. Use `--include-active`,
  `--include-automated`, or `--include-subagents` to include the respective defaults.
  An empty result with hidden matches has `absence_is_proof: false`; its note names
  the options that can reveal those matches.
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

## Session text

`sessions text` (MCP: `sessions` with `action: text`) renders a session the way a summary
needs it: user and assistant turns, tool calls and results, attachments, and system
records, without omitted records or reasoning headings. The text is redacted as a whole
and then cut into parts of `TROVADOR_RESPONSE_MAX_CHARS` (100,000) characters, so no part
can begin or end inside a secret. `offset` counts characters of the redacted text. A
forked Codex child's text leaves out the parent history it copied.

trovador once made these summaries itself with haiku and cached them. The agent asking is
already a model, with its own judgment of what matters for the question, so it is given the
text instead. Each part renders the whole session again; there is no cache of rendered
text, because it would be a second copy of transcript content, secrets included.

## Where things live

trovador reads `~/.claude/projects`, `~/.codex/sessions`, and `~/.codex/archived_sessions`
and never writes to any of them. Its own state is the search index, `~/.trovador/index.db`,
which is derived and can be deleted at any time; the directory is 0700 and its files 0600.
`sessions list` reads the index when it exists but never creates it. Override the paths with
`TROVADOR_CLAUDE_ARCHIVE`, `TROVADOR_CODEX_ARCHIVE`, `TROVADOR_CODEX_ARCHIVED`,
and `TROVADOR_HOME`. Limits are listed by `trovador doctor` and
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

No live test sends anything to a provider. Sampling is seeded and the seed is printed (`TROVADOR_LIVE_SEED` repeats a run). The
output ends with a drift report of record kinds and omitted categories, where a record type
a newer CLI introduced shows up as a new category.

## License

MIT or Apache-2.0, at your option.

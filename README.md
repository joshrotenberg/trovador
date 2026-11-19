# trovador

Search and read your local Claude Code and Codex session history, from the command line or
from an agent over MCP.

Both CLIs save every session as JSONL. trovador indexes those files so you can list
sessions, search all of them at once, page through a transcript, or read a session as plain
text. The CLI and the MCP server call the same library and return the same JSON.

- **Read-only.** Never writes to `~/.claude` or `~/.codex`. Its only state is under
  `~/.trovador`; delete that and it is gone.
- **No model.** Costs no tokens. An agent that wants a summary reads the text and writes
  one itself.
- **History only.** Does not start or resume sessions. It prints the resume command
  (`claude --resume`, `codex resume`) for you to run.

macOS and Linux.

## Install

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/joshrotenberg/trovador/releases/latest/download/trovador-installer.sh | sh
# or
cargo install trovador --locked
# or, from a checkout
cargo install --path . --locked
```

Each puts `trovador` in `~/.cargo/bin`. Then:

```sh
trovador index        # build the search index (searches keep it current after this)
trovador doctor       # check archives, index, and settings
trovador init         # install the plugin into Claude Code and Codex
```

### The agent plugin

`trovador init` installs:

- the MCP server (`trovador serve`) with the `search` and `sessions` tools;
- a `recall` skill: when to look in past sessions and how to read the results;
- on Claude Code, a SessionStart hook that tells each session its own id, so search can
  leave out the current conversation.

It writes a local plugin marketplace to `~/.trovador/plugin` and registers it with each
host's own `plugin` commands. It edits no host configuration file.

```sh
trovador init --dry-run                    # show files and commands, change nothing
trovador init --host claude                # one host (default: both; a host not on PATH is skipped)
trovador init --binary /path/to/trovador   # binary the plugin runs (default: this one)
```

After upgrading, run `trovador init` again and restart the host; a stdio MCP server is never
reloaded.

**Codex hook.** Codex does not load hooks from plugins. Add the entry `init` prints to
`hooks.SessionStart` in `~/.codex/hooks.json`:

```json
{"hooks": [{"type": "command", "command": "/path/to/trovador hook session-start --provider codex || true"}]}
```

**MCP server only**, without the plugin:

```sh
claude mcp add --scope user trovador -- ~/.cargo/bin/trovador serve
codex mcp add trovador -- ~/.cargo/bin/trovador serve
```

**Uninstall:** `claude plugin uninstall trovador@trovador-local` and `codex plugin remove
trovador@trovador-local`, then remove the `trovador-local` marketplace from each host.

The plugin uses an MCP server rather than a skill that shells out, because agent sandboxes
usually cannot write `~/.trovador`. The host starts MCP servers outside the sandbox, so the
server can keep the index current. Inside a sandbox, `trovador search` falls back to
searching the existing index read-only and reports the result as not exhaustive.

## Use cases

```sh
# "We fixed this already." Find the error, then open the transcript at the match.
trovador search "connection reset by peer" --since 2w
trovador sessions transcript 79892eda --offset 6070 --limit 20

# "What did Codex say about the migration?"
trovador search migration --provider codex --project payments

# "What was I doing yesterday?"
trovador sessions list --since 1d

# "Summarize that session." Plain text, in parts, for an agent to read.
trovador sessions text 27d1f434

# "Where did that PR come from?" Title, first prompt, linked PRs, resume command.
trovador sessions show 27d1f434
```

From an agent with the plugin installed, ask in plain words ("didn't we decide this
earlier?"). The `recall` skill has it search, open the matching transcript, and cite the
session.

A search result, trimmed. The session running the search had just used the term, so it is
counted in `hidden` rather than returned:

```console
$ trovador search rate-limit --per-session 1
{
  "query": "rate-limit",
  "sessions_matched": 2,
  "hits": [
    {
      "provider": "claude",
      "session_id": "3f2a9c1e-…",
      "title": "api client retries",
      "matches": [{"index": 412, "type": "tool_result",
        "excerpt": "...the server returned 429 with a [rate-limit] header of 30 seconds..."}]
    },
    {
      "provider": "codex",
      "session_id": "019f4b7d-…",
      "matches": [{"index": 88, "type": "assistant",
        "excerpt": "...backing off on [rate-limit] responses before retrying..."}]
    }
  ],
  "hidden": {"active": 1, "automated": 0, "subagents": 0, "excluded": 0, "unavailable": 0},
  "exhaustive": true,
  "note": "matching sessions left out by default: 1 written in the last 600 s (include_active)"
}
```

## Usage

Every command prints one JSON document on stdout. Diagnostics and progress go to stderr.
At a terminal, `init` reports each host command, and an index refresh that takes over a
second reports its progress:

```text
trovador: indexing 9017 session files (9017 new)
trovador: indexed 3462 of 9017 (38%)
trovador: indexed 9017 session files in 17.0 s
```

### CLI

| Command | What it does |
|---|---|
| `sessions list` | Sessions newest first, one page at a time |
| `sessions show <id>` | Metadata, resumability, and resume command |
| `sessions transcript <id>` | One page of the transcript |
| `sessions text <id>` | The whole session as plain text, in parts |
| `search <query>` | Full-text search across every session |
| `index [--rebuild]` | Bring the index fully up to date, or rebuild it |
| `doctor` | Archive paths and counts, the index, every setting in effect |
| `init` | Install the plugin |
| `serve` | MCP server over stdio |
| `hook session-start` | SessionStart hook handler, run by a host |

Session ids can be shortened to any unambiguous prefix.

```sh
trovador sessions list --project trovador     # project path contains this (ASCII case-insensitive)
trovador sessions list --origin cli           # how it was started
trovador sessions list --archived true        # only archived Codex threads (false: only the rest)
trovador sessions list --offset 20            # next page, from next_offset
trovador sessions transcript 27d1f434 --offset 200 --limit 50
trovador sessions text 27d1f434 --offset 100000 --expected-revision <revision>
trovador search idempotency_key               # words and "quoted phrases", all required
trovador search 'redact NOT summary' --fts    # FTS5 syntax
trovador search 'retry_after:\s*\d+' --regex  # regular expression
trovador search parser --recent --per-session 5
```

Filters for `sessions list` and `search`:

| Option | Meaning |
|---|---|
| `--provider claude\|codex` | One provider only |
| `--project <text>` | Project path contains this text |
| `--origin <origin>` | `cli`, `claude-desktop`, `sdk-cli`, `exec`, `vscode`, ... |
| `--since`, `--until` | Last written at/after or before: RFC 3339 time, date (midnight UTC), or duration back from now (`36h`, `2d`, `1w`) |
| `--archived true\|false` | Only archived Codex threads, or only the rest |
| `--include-subagents[=false]` | Sessions started by another session |
| `--include-automated[=false]` | `claude -p` and `codex exec` sessions |
| `--limit`, `--offset` | Page size; sessions to skip (follow `next_offset`) |

`search` also takes `--include-active[=false]` (written in the last 10 minutes),
`--exclude-session <id>`, `--per-session <n>`, `--recent` (newest first instead of best
match), `--fts`, and `--regex`.

`--no-redact` shows secrets instead of `[redacted:<kind>]`.

### MCP

`trovador serve` offers two read-only tools and two resources, returning the same JSON as
the CLI:

| Tool or resource | Inputs |
|---|---|
| `search` | As `trovador search`: `query`, `fts`, `regex`, `provider`, `project`, `origin`, `include_active`, `include_automated`, `include_subagents`, `exclude_session`, `since`, `until`, `archived`, `limit`, `offset`, `per_session`, `order` |
| `sessions` | `action` (`list`, `show`, `transcript`, `text`) plus that command's inputs: `session_id`, `offset`, `limit`, `expected_revision`, list filters |
| `session://{provider}/{id}` | Same as `sessions` `show` |
| `session://{provider}/{id}/transcript` | First transcript page |

The server tells clients that transcript text is historical data, not instructions. Only
`trovador serve --no-redact` turns redaction off; no tool argument can.

A client that knows its session id (from the SessionStart hook) should pass it as
`exclude_session` with `include_active: true`, to leave out only the current conversation
rather than everything from the last 10 minutes.

### Reading results

**Session rows** (`sessions list`, `sessions show`, search hits):

| Field | Source |
|---|---|
| `project` | Claude's latest `relocated` cwd, else the first `cwd`; where a resume runs |
| `snippet` | First block the user typed, skipping injected `<...>` context and AGENTS.md |
| `title`, `title_source` | Claude `custom-title` over `ai-title`; latest wins |
| `origin` | Claude `entrypoint`; Codex `source` (`cli`, `vscode`, `exec`, `guardian`, `thread_spawn`) |
| `subagent`, `parent_session_id` | Codex sessions started by another session |
| `artifacts`, `pull_requests` | Claude `frame-link` and `pr-link` records |
| `archived` | In Codex `archived_sessions` |
| `resumable` | Under the resume ceiling (900,000 bytes, decompressed) |
| `open_with` | Command to resume it |

**Transcript records** are `user`, `assistant`, `tool_call`, `tool_result`, `attachment`,
`system`, `reasoning`, `unrecognized`, or `omitted`.

- `omitted`: a known type with nothing to read (bookkeeping, injected context, binary,
  encrypted). `name` is the category; consecutive ones collapse with a `count`.
- `unrecognized`: a type trovador does not know yet. `name` is the provider's type, `text` a
  capped copy of its JSON. It is indexed, so text in a new format is still searchable.
- `reasoning`: only where the provider kept readable text.
- A Claude prompt typed while a turn was running is a `user` record named `queued`.

Text is capped per record (4,000 characters; 600 for tool calls and results) and per page
(100,000).

**Paging.** `sessions list`, `search`, and `sessions transcript` return one page. Pass
`next_offset` as `offset` for the next; no `next_offset` means the last page. Search pages
count sessions, and its totals (`sessions_matched`, `hidden`, `exhaustive`) cover the whole
result. Each page is a fresh query; pass the same `--until` on every page for a stable walk.

**Revisions.** Transcript pages and text parts report a `revision`. Pass it back as
`expected_revision` when continuing. If the session was only appended to, it is read as it
was and the result carries `grown`. Any other change (rewritten, moved, compressed,
different settings) is refused with the current revision; start again at offset 0.

**Search hits** list matching records with an `index` (the transcript offset to open) and
an excerpt with the match in brackets. `hidden` counts matching sessions left out by a
default, by `exclude_session`, or because their archive was unreadable. `exhaustive: false`
means the index was not fully current; `absence_is_proof` says whether an empty result
proves absence. Read `note` when either is false.

**Session text** has user and assistant turns, tool calls and results, attachments, and
system records; not omitted, reasoning, or unrecognized. It is redacted as a whole, then
split into 100,000-character parts, so no part starts or ends inside a secret. Continue from
`truncated.next_offset`.

## Configuration

All settings are environment variables; `trovador doctor` shows the values in effect. For
the MCP server, set them where the host starts it.

**Paths**

| Variable | Default |
|---|---|
| `TROVADOR_CLAUDE_ARCHIVE` | `~/.claude/projects` |
| `TROVADOR_CODEX_ARCHIVE` | `~/.codex/sessions` |
| `TROVADOR_CODEX_ARCHIVED` | `archived_sessions` beside the Codex archive |
| `TROVADOR_HOME` | `~/.trovador` (index and plugin marketplace) |

**stderr**

| Variable | Default | Effect |
|---|---|---|
| `TROVADOR_LOG` | `warn` | `tracing` filter. `debug`: scans, refresh plan and summary, command inputs and counts, MCP call times, init host commands. `trace`: one line per file indexed |
| `TROVADOR_PROGRESS` | on at a terminal | Progress lines; `1`/`0` forces it. Never under `serve` |
| `TROVADOR_PROGRESS_AFTER_MS` | 1000 | How long a refresh runs before reporting progress |

**Visibility.** Booleans take `1`/`true`/`yes`/`on` or `0`/`false`/`no`/`off`. A call's
`include_*` input overrides them either way.

| Variable | Default | Effect |
|---|---|---|
| `TROVADOR_INCLUDE_ACTIVE` | off | Sessions written within `TROVADOR_ACTIVE_WINDOW_S` (search only) |
| `TROVADOR_INCLUDE_AUTOMATED` | off | `claude -p` and `codex exec` sessions |
| `TROVADOR_INCLUDE_SUBAGENTS` | off | Sessions started by another session |
| `TROVADOR_ACTIVE_WINDOW_S` | 600 | What counts as active; `0` hides nothing for being recent |

**Index**

| Variable | Default | Effect |
|---|---|---|
| `TROVADOR_INDEX_REDACT` | off | Redact secrets before writing the index |
| `TROVADOR_INDEX_REASONING` | on | Index reasoning records (`0` cut 416 MB to 290 MB on the development machine) |
| `TROVADOR_INDEX_BUDGET_MS` | 15000 | How long a search may spend updating the index |

**Sizes.** Most of what trovador returns lands in an agent's context window.

| Variable | Default | Effect |
|---|---|---|
| `TROVADOR_RESPONSE_MAX_CHARS` | 100000 | Characters per transcript page or text part |
| `TROVADOR_RECORD_CHARS` | 4000 | Per-record cap |
| `TROVADOR_TOOL_CHARS` | 600 | Tool call and result cap |
| `TROVADOR_UNRECOGNIZED_CHARS` | 1000 | `unrecognized` cap; `0` shows them as `omitted`, unindexed |
| `TROVADOR_PAGE_SIZE` | 200 | Records per transcript page |
| `TROVADOR_LIST_DEFAULT` | 20 | Sessions per list page |
| `TROVADOR_SEARCH_DEFAULT` | 20 | Sessions per search page |
| `TROVADOR_PER_SESSION` | 3 | Matching records per session |
| `TROVADOR_LIST_LOOKAHEAD` | 500 | Sessions a list reads past a full page to find out if there is another |
| `TROVADOR_OFFSET_MAX` | 1000000 | Largest offset accepted |
| `TROVADOR_RESUME_LIMIT` | 900000 | Bytes above which a session is not resumable |
| `TROVADOR_HEAD_BYTES`, `TROVADOR_HEAD_MAX_BYTES` | 65536, 4194304 | How far into a file to look for metadata |
| `TROVADOR_TITLE_SCAN_MAX_BYTES` | 4194304 | Largest unindexed Claude file scanned for its custom title; `0` disables |

Changing a setting that affects what the index stores (caps, reasoning, redaction) rebuilds
the index on the next search or `trovador index`.

## Privacy and secrets

trovador reads `~/.claude/projects`, `~/.codex/sessions`, and `~/.codex/archived_sessions`
and never writes to them. `~/.trovador` is 0700 and its files 0600. The index is derived and
safe to delete. `sessions list` uses the index if it exists but never creates it.

Output is redacted on both doors: API keys, tokens, private keys, and `NAME=value` secrets
become `[redacted:<kind>]`, in excerpts and session text too. Only `--no-redact` at startup
turns it off.

The index stores text as written by default, so searching for a token finds its session.
`TROVADOR_INDEX_REDACT=1` redacts before writing, so nothing under `~/.trovador` holds a
recognized secret. Searching for a secret then finds nothing, and excerpts stay redacted even
with `--no-redact`; transcripts and session text still read the provider files. On the
development machine (2026-11-18) it redacted about 2,300 of 1,078,470 records and slowed a
full build from 15.4 s to 16.2 s.

## How it works

**Sessions on disk.** Claude Code deletes transcripts older than `cleanupPeriodDays`; Codex
moves archived threads to `archived_sessions`, which trovador also reads. A deleted
transcript leaves the index on the next refresh. Compressed Codex rollouts (`.jsonl.zst`,
0.165 and later) read like plain files.

**Index.** SQLite FTS5, one row per session and one per transcript record with text, keyed
so a match's `index` is its transcript offset. Each search first updates it for up to
`TROVADOR_INDEX_BUDGET_MS`: appended files are read from where the last pass stopped, other
changes are reread. If the index is not writable (sandbox, read-only filesystem), search uses
it as is and reports the result as not exhaustive. Codex `guardian` approval reviewers keep
metadata but not text, which duplicates their parent.

| Operation (development machine, release build) | Result |
|---|---|
| Full index build, 9,008 sessions (2026-11-18) | 15.4 s, 670 MB, 1,078,470 records |
| Search, nothing new to index (2026-10-07) | 94 to 107 ms |
| Search picking up two appended sessions (2026-10-07) | 234 ms |
| `sessions list`, all sessions, index present (2026-10-07) | 0.09 s (8.8 s without) |
| One page from a 377 MB Codex session (2026-10-07) | 0.30 s, 6.8 MB resident |

**Codex streams.** Codex writes each turn more than once. trovador shows the copy the user
saw (`item_completed` events now; `user_message`/`agent_message` events or `response_item`
records in older versions) and shows each turn once.

**Codex forks.** A forked subagent starts with a copy of its parent's history. That copy
becomes one `omitted` record named `forked_history` and is not indexed. If the file never
reaches the boundary its header names, a finished file (archived or compressed) is shown and
indexed whole; a file Codex may still append to waits.

**Revisions** combine the read size, mtime, hashes of the first and last 4 KB, and a hash of
the settings that shape the text; the format may change. Not detected: a rewrite that keeps
size, mtime, and both sampled regions; a rewrite of earlier bytes in a grown file that leaves
both samples alone (it looks like an append); a compressed rollout replaced with the same
size and mtime.

**Titles.** Indexed Claude files keep their latest custom title across appends. Without the
index, files up to `TROVADOR_TITLE_SCAN_MAX_BYTES` are scanned for it; larger ones use head
and tail and can miss it until `trovador index` runs.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`cargo test` builds fixture archives in temp directories and never touches real state.

`tests/live.rs` checks invariants against your real archives, read-only, with state in the
target directory:

- every session listed once;
- transcripts contiguous and capped;
- the index matching the transcripts;
- search finding any indexed phrase;
- CLI and MCP agreeing.

```sh
cargo test --release --test live -- --ignored --nocapture
```

Sampling is seeded (`TROVADOR_LIVE_SEED` repeats a run). The output ends with a drift report
of record kinds and unrecognized types, where formats from a newer CLI show up.

Releases: release-plz publishes to crates.io and tags; dist builds binaries and the
installer from the tag. Conventions are in `AGENTS.md`.

## License

MIT or Apache-2.0, at your option.

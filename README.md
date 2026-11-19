# trovador

Search and read your local Claude Code and Codex session history, from the command line or
from an agent over MCP.

Both CLIs keep every session on disk as JSONL. trovador indexes those files and lets you list
sessions, search across all of them at once, page through a transcript, and read a whole
session as plain text. The same library answers both the CLI and the MCP server, so a person
and an agent get identical results.

- **Read-only.** It never writes to `~/.claude` or `~/.codex`. Its only state is a search
  index under `~/.trovador`; delete that directory and trovador is gone.
- **No model.** It runs no model and costs no tokens. An agent that wants a summary reads
  the session text and summarizes it itself.
- **History, not control.** It does not continue, resume, or start sessions. It prints the
  command to resume one (`claude --resume`, `codex resume`) for a person to run.

Supported on macOS and Linux. Windows is untested.

## Install

Install the binary with the release installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/joshrotenberg/trovador/releases/latest/download/trovador-installer.sh | sh
```

or from crates.io, or from a checkout:

```sh
cargo install trovador --locked
cargo install --path . --locked
```

All three put `trovador` in `~/.cargo/bin`. Then:

```sh
trovador index        # build the search index once; searches keep it current after that
trovador doctor       # check the archives, the index, and the settings in effect
trovador init         # install the plugin into Claude Code and Codex
```

### The agent plugin

`trovador init` gives Claude Code and Codex three things:

- the MCP server (`trovador serve`), with the `search` and `sessions` tools;
- a `recall` skill that tells the agent when to look in past sessions and how to read what
  it finds;
- on Claude Code, a SessionStart hook that tells each session its own id, so a search can
  leave out exactly the current conversation.

It writes a local plugin marketplace to `~/.trovador/plugin` and registers it through each
host's own commands (`claude plugin marketplace add` and `claude plugin install`, `codex
plugin marketplace add` and `codex plugin add`). It edits no host configuration file.

```sh
trovador init --dry-run          # show what it would write and which commands it would run
trovador init --host claude      # one host only (default: both; a host not on PATH is skipped)
trovador init --binary /path/to/trovador   # the binary the plugin starts (default: this one)
```

Run `trovador init` again after upgrading, then restart the host: an MCP server started over
stdio is never reloaded. The plugin starts the binary by absolute path, because a host
launched from the desktop may not have `~/.cargo/bin` on its `PATH`.

Codex does not load hooks from plugins, so on Codex the session-id hook is an entry in
`~/.codex/hooks.json` that you add yourself. `init` reports whether it is there and prints
the entry to add under `hooks.SessionStart`:

```json
{"hooks": [{"type": "command", "command": "/path/to/trovador hook session-start --backend codex || true"}]}
```

To register only the MCP server, without the plugin:

```sh
claude mcp add --scope user trovador -- ~/.cargo/bin/trovador serve
codex mcp add trovador -- ~/.cargo/bin/trovador serve
```

`init` warns if a `trovador` server is registered by hand as well, since its tools would be
offered twice. To uninstall the plugin, run `claude plugin uninstall trovador@trovador-local`
and `codex plugin remove trovador@trovador-local`, then remove the `trovador-local`
marketplace from each host.

The plugin carries an MCP server, not only a skill that runs the CLI, because an agent's
sandbox usually cannot write `~/.trovador`. Inside a Codex `workspace-write` sandbox,
`trovador search` cannot update the index and searches the existing one read-only, saying
the result is not exhaustive. A host starts its MCP servers outside the sandbox, so the
server keeps the index current.

## Use cases

**"We fixed this already."** Search both agents' history for the error message, function
name, or issue number, then open the transcript where it matched:

```sh
trovador search "connection reset by peer" --since 2w
trovador sessions transcript 79892eda --offset 6070 --limit 20
```

**"What did Codex say about the migration?"** Narrow by backend and project:

```sh
trovador search migration --backend codex --project payments
```

**"What was I doing yesterday?"** List recent sessions, newest first:

```sh
trovador sessions list --since 1d
```

**"Summarize that session."** An agent reads the session as plain text, in parts, and
summarizes it with its own judgment of what matters:

```sh
trovador sessions text 27d1f434
```

**"Where did that PR come from?"** A session row carries the pull requests and artifacts it
linked, its title and first prompt, and the command to resume it:

```sh
trovador sessions show 27d1f434
```

**From an agent.** With the plugin installed, ask in plain words ("didn't we decide this in
an earlier session?"). The `recall` skill has the agent call `search`, open the matching
transcript with `sessions`, and cite the session it found.

The shape of a search result, with illustrative values and most fields trimmed. Here the
session doing the search had just mentioned the term, so it is left out and counted in
`hidden`:

```console
$ trovador search rate-limit --per-session 1
{
  "query": "rate-limit",
  "sessions_matched": 2,
  "returned": 2,
  "hits": [
    {
      "backend": "claude",
      "session_id": "3f2a9c1e-…",
      "title": "api client retries",
      "matches": [{"index": 412, "type": "tool_result",
        "excerpt": "...the server returned 429 with a [rate-limit] header of 30 seconds..."}],
      …
    },
    {
      "backend": "codex",
      "session_id": "019f4b7d-…",
      "matches": [{"index": 88, "type": "assistant",
        "excerpt": "...backing off on [rate-limit] responses before retrying..."}],
      …
    }
  ],
  "hidden": {"active": 1, "automated": 0, "subagents": 0, "excluded": 0, "unavailable": 0},
  "exhaustive": true,
  "note": "matching sessions left out by default: 1 written in the last 600 s (include_active)"
}
```

## Usage

Every command prints one JSON document on stdout. Diagnostics go to stderr.

### CLI

| Command | What it does |
|---|---|
| `trovador sessions list` | Sessions newest first, one page at a time |
| `trovador sessions show <id>` | One session's metadata, resumability, and resume command |
| `trovador sessions transcript <id>` | One page of the session's projected transcript |
| `trovador sessions text <id>` | The whole session as plain text, in parts |
| `trovador search <query>` | Full-text search across every session |
| `trovador index [--rebuild]` | Bring the index fully up to date, or discard and rebuild it |
| `trovador doctor` | Archive paths and counts, the index, and every setting in effect |
| `trovador init` | Install the plugin into Claude Code and Codex |
| `trovador serve` | Run the MCP server over stdio |
| `trovador hook session-start` | The SessionStart hook handler, run by a host, not by hand |

A session id can be given in full or as any unambiguous prefix.

```sh
trovador sessions list --project trovador     # project path contains this text (ASCII case-insensitive)
trovador sessions list --origin cli           # how the session was started
trovador sessions list --include-subagents    # also sessions started by other sessions
trovador sessions list --archived true        # only Codex threads in archived_sessions (false: only the rest)
trovador sessions list --offset 20            # the next page, from next_offset of the last one
trovador sessions transcript 27d1f434 --offset 200 --limit 50
trovador sessions text 27d1f434 --offset 100000 --expected-revision <revision>
trovador search idempotency_key               # words and "quoted phrases", all required
trovador search 'redact NOT summary' --fts    # FTS5 syntax
trovador search 'retry_after:\s*\d+' --regex  # regular expression, newest first
trovador search parser --recent --per-session 5
```

Filters shared by `sessions list` and `search`:

| Option | Meaning |
|---|---|
| `--backend claude\|codex` | One provider only |
| `--project <text>` | Project path contains this text, ignoring ASCII case |
| `--origin <origin>` | How it was started: `cli`, `claude-desktop`, `sdk-cli`, `exec`, `vscode`, ... |
| `--since`, `--until` | Last written at or after, or before: an RFC 3339 time (`Z` or a numeric offset), a date at midnight UTC, or a duration back from now (`36h`, `2d`, `1w`) |
| `--archived true\|false` | Only archived Codex threads, or only everything else |
| `--include-subagents[=false]` | Include, or with `=false` hide, sessions started by another session |
| `--include-automated[=false]` | Include, or hide, `claude -p` and `codex exec` sessions |
| `--limit`, `--offset` | Page size, and sessions to skip (follow `next_offset`) |

`search` also takes `--include-active[=false]` (sessions written in the last 10 minutes),
`--exclude-session <id>`, `--per-session <n>` (matching records shown per session), `--recent`
(newest first instead of best match), `--fts`, and `--regex`.

`--no-redact`, before or after the command, shows secrets instead of `[redacted:<kind>]`.

### MCP

`trovador serve` offers two tools and two resources, all read-only, returning the same JSON
as the CLI:

| Tool or resource | Inputs |
|---|---|
| `search` | The same inputs as `trovador search`: `query`, `fts`, `regex`, `backend`, `project`, `origin`, `include_active`, `include_automated`, `include_subagents`, `exclude_session`, `since`, `until`, `archived`, `limit`, `offset`, `per_session`, `order` |
| `sessions` | `action`: `list`, `show`, `transcript`, or `text`, with the inputs of the matching CLI command (`session_id`, `offset`, `limit`, `expected_revision`, and the list filters) |
| `session://{backend}/{id}` | The same as `sessions` `show` |
| `session://{backend}/{id}/transcript` | The first transcript page |

The server tells clients that transcript text is historical data and not instructions.
Redaction can only be turned off with `trovador serve --no-redact`; no tool argument can.

A client that knows its own session id (the SessionStart hook provides it) should pass it as
`exclude_session` together with `include_active: true`. That leaves out only the current
conversation instead of every session written in the last 10 minutes.

### Reading what comes back

**Session rows.** `sessions list`, `sessions show`, and every search hit carry:

| Field | Source |
|---|---|
| `project` | Where a resume runs from: Claude's latest `relocated` cwd, else the first `cwd` |
| `snippet` | The first block the user typed, skipping injected `<...>` context and AGENTS.md |
| `title`, `title_source` | Claude `custom-title` (typed) over `ai-title` (generated); latest wins |
| `origin` | Claude `entrypoint` (`cli`, `claude-desktop`, `sdk-cli`); Codex `source` (`cli`, `vscode`, `exec`, `guardian`, `thread_spawn`) |
| `subagent`, `parent_session_id` | Codex sessions started by another session |
| `artifacts`, `pull_requests` | Claude `frame-link` and `pr-link` records |
| `archived` | Codex moved the thread to `archived_sessions` |
| `resumable` | The transcript is under the resume ceiling (900,000 bytes, decompressed) |
| `open_with` | The command to paste in a terminal to resume it |

**Transcript records** are `user`, `assistant`, `tool_call`, `tool_result`, `attachment`,
`system`, `reasoning`, `unrecognized`, or `omitted`:

- `omitted` stands for records of a known type with nothing to read: client bookkeeping,
  context injected for the model, images and other binary data, encrypted content. `name` is
  the category, and consecutive ones collapse into one record with a `count`.
- `unrecognized` is a record, content block, attachment subtype, or Codex item of a type
  trovador has not been taught. `name` is the provider's type and `text` a shortened copy of
  its raw JSON, with binary and encrypted values replaced by `"[binary]"`. Search indexes it,
  so text in a shape a newer CLI introduced can still be found.
- `reasoning` appears only where the provider kept readable text: Claude thinking, Codex
  reasoning summaries.
- A Claude prompt typed while a turn was running is a `user` record named `queued`.

Text is capped per record (4,000 characters, 600 for tool calls and results) and per page
(100,000 characters). A page that stopped early says so and gives `next_offset`.

**Paging.** `sessions list`, `search`, and `sessions transcript` return one page. When there
is more, the result carries `next_offset`; pass it as `offset` with the same other inputs.
No `next_offset` means that was the last page. A search counts sessions, not records, and its
totals (`sessions_matched`, `hidden`, `exhaustive`) describe the whole result on every page.
Each page is a fresh query, so sessions written while paging can shift later pages; pass the
same `--until` on every page for a steady walk.

**Revisions.** Every transcript page and text part reports a `revision` of the session it
was read from. Pass it back as `expected_revision` on each continuation. A session that has
only been appended to since is still read as it was, so a walk through a live session stays
consistent; the page then carries `grown` with the `current_revision`. A session changed in
any other way (rewritten, moved, compressed, read with different settings) is refused with
the current revision, and the caller starts again from offset 0.

**Search results.** Each hit lists matching records with an `index`, which is the transcript
offset to open the match at, and an excerpt with the match in brackets. `hidden` counts
matching sessions left out by a default (`active`, `automated`, `subagents`), by
`exclude_session`, or because their archive could not be read. `exhaustive` is false when
the index was not fully current, and `absence_is_proof` says whether an empty result means
the text is not there. Read `note` when either is false.

**Session text.** `sessions text` renders user and assistant turns, tool calls and results,
attachments, and system records, without omitted, reasoning, or unrecognized records. The
text is redacted as a whole and then cut into parts of 100,000 characters, so no part can
begin or end inside a secret. `truncated.next_offset` says where the next part starts.

## Configuration

Everything is set with environment variables. `trovador doctor` reports the value of each in
effect. For the MCP server, the variables must be set in the environment the host starts it
in.

**Where things are.**

| Variable | Default |
|---|---|
| `TROVADOR_CLAUDE_ARCHIVE` | `~/.claude/projects` |
| `TROVADOR_CODEX_ARCHIVE` | `~/.codex/sessions` |
| `TROVADOR_CODEX_ARCHIVED` | `archived_sessions` beside the Codex archive (`~/.codex/archived_sessions`) |
| `TROVADOR_HOME` | `~/.trovador`, which holds the index and the plugin marketplace |
| `TROVADOR_LOG` | `warn`; a `tracing` filter for diagnostics on stderr |

**What search and list show.** Each takes `1`, `true`, `yes`, `on`, or `0`, `false`, `no`,
`off`; any other value keeps the default. A call's own `include_*` input always wins, in
either direction.

| Variable | Default | Effect |
|---|---|---|
| `TROVADOR_INCLUDE_ACTIVE` | off | Include sessions written within `TROVADOR_ACTIVE_WINDOW_S` (search only) |
| `TROVADOR_INCLUDE_AUTOMATED` | off | Include `claude -p` and `codex exec` sessions |
| `TROVADOR_INCLUDE_SUBAGENTS` | off | Include sessions started by another session |
| `TROVADOR_ACTIVE_WINDOW_S` | 600 | What counts as active; `0` hides nothing for being recent |

**What the index holds.**

| Variable | Default | Effect |
|---|---|---|
| `TROVADOR_INDEX_REDACT` | off | Redact secrets before writing the index, not only on output |
| `TROVADOR_INDEX_REASONING` | on | Index reasoning records; `0` leaves them out (416 MB to 290 MB on the development machine) |
| `TROVADOR_INDEX_BUDGET_MS` | 15000 | How long a search may spend updating the index before it queries |

**Sizes**, which are context budgets as much as anything: most of what trovador returns ends
up in an agent's context window.

| Variable | Default | Effect |
|---|---|---|
| `TROVADOR_RESPONSE_MAX_CHARS` | 100000 | Characters of content in one transcript page or text part |
| `TROVADOR_RECORD_CHARS` | 4000 | Per-record cap |
| `TROVADOR_TOOL_CHARS` | 600 | Cap for tool calls and results |
| `TROVADOR_UNRECOGNIZED_CHARS` | 1000 | Cap for `unrecognized` records; `0` shows them as `omitted` and keeps them out of the index |
| `TROVADOR_PAGE_SIZE` | 200 | Records per transcript page, at most |
| `TROVADOR_LIST_DEFAULT` | 20 | Sessions per list page |
| `TROVADOR_SEARCH_DEFAULT` | 20 | Sessions per search page |
| `TROVADOR_PER_SESSION` | 3 | Matching records shown per session |
| `TROVADOR_LIST_LOOKAHEAD` | 500 | Sessions a list reads past a full page to learn whether another page exists |
| `TROVADOR_OFFSET_MAX` | 1000000 | Largest offset accepted |
| `TROVADOR_RESUME_LIMIT` | 900000 | Bytes above which a session is reported not resumable |
| `TROVADOR_HEAD_BYTES`, `TROVADOR_HEAD_MAX_BYTES` | 65536, 4194304 | How far into a file metadata is looked for |
| `TROVADOR_TITLE_SCAN_MAX_BYTES` | 4194304 | Size up to which an unindexed Claude file is scanned for its custom title; `0` disables it |

Changing a setting that shapes what the index stores (the character caps, reasoning,
redaction) starts a fresh index file on the next search or `trovador index`.

## Privacy and secrets

trovador reads `~/.claude/projects`, `~/.codex/sessions`, and `~/.codex/archived_sessions` and
never writes to them. Its index at `~/.trovador/index.db` is derived and can be deleted at any
time; the directory is 0700 and its files 0600. `sessions list` reads the index when it
exists but never creates it.

Output on both doors is redacted: API keys, tokens, private keys, and `NAME=value` secrets
become `[redacted:<kind>]`, including in search excerpts and session text. Only
`--no-redact` given when the process starts turns that off.

By default the index holds transcript text as written, so a search for a token still finds
its session. With `TROVADOR_INDEX_REDACT=1`, the same redaction is applied before anything is
written, so no file under `~/.trovador` holds a secret the rules recognise. A search for a
secret then finds nothing, and with `--no-redact` search excerpts and titles still show
`[redacted:<kind>]`; transcripts and session text read the provider files and are unaffected.
On the development machine on 2026-11-18 it redacted about 2,300 of 1,078,470 records, and a
full index build took 16.2 s against 15.4 s.

## How it works

**What is on disk is what the providers kept.** Claude Code deletes old transcripts after
its `cleanupPeriodDays`, and Codex moves archived threads to `~/.codex/archived_sessions`,
which trovador also reads. A transcript the provider deleted disappears from trovador on the
next index refresh that can read its archive. Codex 0.165 and later compress finished
rollouts to `.jsonl.zst`; trovador reads those the same as plain files.

**The index** is SQLite with FTS5: one metadata row per session and one full-text row per
transcript record that carries text, keyed so a match's `index` is its transcript offset.
Every search first brings it up to date for at most `TROVADOR_INDEX_BUDGET_MS`. Files that
were only appended to are read from where the last pass stopped; anything else that changed
is read again. When the index cannot be written (an agent sandbox, a read-only filesystem),
search reads it as it is and says the result is not exhaustive. Codex `guardian` approval
reviewers keep their metadata but not their text, which is a copy of their parent's history.

Measured on the development machine, release build:

| Operation | Result |
|---|---|
| Full index build, 9,008 sessions (2026-11-18) | 15.4 s, 670 MB, 1,078,470 records |
| A search with nothing new to index (2026-10-07) | 94 to 107 ms |
| A search that picks up two appended sessions (2026-10-07) | 234 ms |
| `sessions list` over every session, index present (2026-10-07) | 0.09 s (8.8 s without) |
| One page from a 377 MB Codex session (2026-10-07) | 0.30 s, 6.8 MB resident |

**Codex streams.** Codex writes each turn more than once. trovador shows the copy the user
saw: `item_completed` events in current versions, `user_message` and `agent_message` events
in older ones, and `response_item` records in the oldest, and shows a turn copied across
streams once.

**Codex forks.** A Codex subagent started as a fork begins with a copy of its parent's
history. That copy becomes one `omitted` record named `forked_history`, and contributes no
turns, indexed text, or session text. When the file never reaches the boundary its header
names, a finished file (archived, or compressed) keeps its whole transcript and is indexed
as shown, while a file Codex may still append to waits for the boundary.

**Revisions** carry the size the read was cut at, the file's mtime, hashes of the first and
last 4 KB at that size, and a hash of the settings that shape the text; the format is not a
promise. A rewrite that keeps the size, the mtime, and both sampled regions is not detected,
and once a file has grown, a rewrite of its earlier bytes that leaves both samples unchanged
looks like an append. A compressed rollout replaced in place with the same size and mtime is
not detected either.

**Titles.** Indexed Claude files keep their latest custom title across appends. Without an
index, files up to `TROVADOR_TITLE_SCAN_MAX_BYTES` are scanned for it, and larger ones use
their head and tail, so a title between them can be missed until `trovador index` runs.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`cargo test` uses fixture archives built in temporary directories and never touches real
state. `tests/live.rs` checks properties that must hold for any machine's real archives:
every session file listed once, transcripts contiguous and capped, the index equal to the
transcript projection, search finding a phrase from any indexed record, CLI and MCP agreeing,
and archive files left untouched. Its state goes to the cargo target directory, never
`~/.trovador`, and it sends nothing to a provider.

```sh
cargo test --release --test live -- --ignored --nocapture
```

Sampling is seeded and the seed is printed (`TROVADOR_LIVE_SEED` repeats a run). The output
ends with a drift report of record kinds, omitted categories, and unrecognized types; a type
a newer CLI introduced shows up there as unrecognized.

Releases are cut by release-plz, which publishes to crates.io and tags, and dist, which
builds the binaries and the installer from the tag. `AGENTS.md` has the conventions for
working in this repository.

## License

MIT or Apache-2.0, at your option.

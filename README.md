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
trovador init --dry-run        # what init would write and which host commands it would run
trovador init                  # install the plugin into Claude Code and Codex
```

`trovador doctor` checks the setup: it reads the archives and reports the index, with a
fix for each problem.

Supported on macOS and Linux. Windows is untested.

### The plugin

`trovador init` installs a plugin that gives an agent three things:

- the MCP server (`trovador serve`), with the `search` and `sessions` tools;
- a `recall` skill that says when to look in past sessions and how to read what it finds;
- on Claude Code, a SessionStart hook that tells the session its own id, so search can
  leave out exactly this conversation instead of every session active in the last 10
  minutes.

It writes a local marketplace to `~/.trovador/plugin` and registers it with each host's own
commands: `claude plugin marketplace add` and `claude plugin install` (or `update`) at user
scope, and `codex plugin marketplace add` and `codex plugin add`. It edits no host
configuration file itself. `--host claude` or `--host codex` limits it to one host; under
the default `both`, a host that is not on PATH is skipped. Run it again after upgrading
trovador to update the plugin, and restart the host: a stdio server is started once per
session and never reloads.

The plugin starts the binary by absolute path, the one `init` ran as unless `--binary`
says otherwise, because a host started from the desktop may not have `~/.cargo/bin` on its
`PATH`.

The plugin carries the MCP server, not only a skill that runs the CLI, because an agent's
sandbox usually cannot write `~/.trovador`. Under `codex sandbox -c
'sandbox_mode="workspace-write"'`, `trovador search` cannot update the index
(2026-11-02); it searches the existing one read-only and says the result is not exhaustive.
A host starts its MCP servers outside the agent's sandbox, so the server keeps the index
current.

Codex does not load hooks from plugins (`codex features list` shows `plugin_hooks` as
removed in 0.165), so on Codex the hook is an entry in `~/.codex/hooks.json`. `init`
reports whether one is there and, if not, the entry to add under `hooks.SessionStart`:

```json
{"hooks": [{"type": "command", "command": "/path/to/trovador hook session-start --backend codex || true"}]}
```

`init` reports a `trovador` MCP server configured by hand in either host, since its tools
would be offered twice; remove it with `claude mcp remove trovador` or `codex mcp remove
trovador`. To uninstall, `claude plugin uninstall trovador@trovador-local` and `codex plugin
remove trovador@trovador-local`, then remove the `trovador-local` marketplace from each.

Without the plugin, register the server by hand:

```sh
claude mcp add --scope user trovador -- ~/.cargo/bin/trovador serve
codex mcp add trovador -- ~/.cargo/bin/trovador serve
```

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
trovador sessions list --offset 20           # the next page, from next_offset of the last
trovador sessions list --archived true       # only Codex threads in archived_sessions (false: only the rest)
trovador sessions show 27d1f434              # id or unambiguous prefix
trovador sessions transcript 27d1f434 --offset 200 --limit 50
trovador sessions text 27d1f434              # the whole session as plain text, in parts
trovador sessions text 27d1f434 --offset 100000 --expected-revision 1c8a.17a3f2b4c5d6e7f8.9f1e2d3c4b5a6978.1a2b3c4d5e6f7081.p.3f9a1c0b7d2e4a58  # as it was at that revision; fails unless it only grew
trovador search idempotency_key              # words and "quoted phrases", all required
trovador search "redact" --since 1w          # only sessions written in the last week
trovador search redact --offset 20           # the next page of sessions, from next_offset
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

### Paging

`sessions list` and `search` return one page: `--limit` sessions (`TROVADOR_LIST_DEFAULT`,
`TROVADOR_SEARCH_DEFAULT`), starting `--offset` sessions in. A result that has more after it
carries `next_offset`; pass it as `offset`, with the same other inputs, for the next page.
The last page has no `next_offset`. The MCP `sessions` tool (for `action: list`) and `search`
take `offset` too, and the library computes both, so the doors return the same page.

- **The unit is a session.** A search groups its matching records by session, and `offset`
  counts sessions in that order: best match first, or newest first with `--recent` and for
  `--regex`. Ties on score and time break by the index's session number, so the order is
  total and two pages never split a tie differently. A list counts sessions that pass every
  filter, so `--offset 20` means the same thing whatever the filters hide.
- **Coverage is for the whole result.** `sessions_matched`, `exhaustive`, `coverage` and
  the `hidden` counts for the defaults (`active`, `automated`, `subagents`, `excluded`)
  describe the query, not the page, and are the same on every page. Two `hidden` counts
  are for the page only: `unavailable` (a match whose file or archive root could not be
  read) and `unrecognized` (shown only when above zero: a match indexed under a backend this version does not know;
  `trovador index --rebuild` fixes it). `absence_is_proof` appears only on an empty page
  and is false for any `offset` above zero, or when the page consumed matches it could not
  show. A page past the last match says how many matched; a page with matches but nothing
  showable says so and keeps `next_offset`. With zero matches and an offset above zero,
  the note says to retry at offset 0.
- **Look-ahead.** A list learns whether another page exists by reading past a full page
  until it finds one more matching session, at most `TROVADOR_LIST_LOOKAHEAD` sessions
  (500) so a selective `--project` or `--origin` filter does not read every remaining
  session. A list's `truncated` is true when it found one, or when it stopped at that bound
  with older sessions unread; `next_offset` is present exactly when `truncated` is, and the
  next page can then be empty. `truncated` false means the list reached its end.
- **No snapshot between pages.** Each page is a fresh query. Sessions written while paging
  can shift later pages, so an item can repeat or be skipped, most visibly in a list, which
  puts new sessions first. For a steady walk, pass the same `--until` (an RFC 3339 time
  taken before the first page) on every page; sessions written afterwards are then outside
  the window. A search that brings the index up to date between pages can also change bm25
  scores, so a relevance order can move between pages where `--recent` does not.
- **Bounds.** An offset above `TROVADOR_OFFSET_MAX` (1,000,000) is rejected with `invalid
  offset`; a negative or non-numeric offset is rejected as an invalid number by both doors.
  An offset past the last result is not an error: it returns an empty page.
- **Archived threads.** `--archived true` keeps only Codex threads read from
  `archived_sessions`, `--archived false` keeps only sessions not read from it (all Claude
  sessions and Codex threads that are not archived), and leaving it out includes both, as
  before. It is a filter you ask for, so what it excludes is not counted in `hidden`.

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

**Revisions.** A transcript offset is a record index and a text offset is a character of
the rendered, redacted text, so a session rewritten between two pages can make the second
skip or repeat content. Every transcript page and text part therefore reports a
`revision`, an opaque string naming the state of the session it was read from. Pass it
unchanged as `expected_revision` (`--expected-revision` on the CLI) on every continuation;
the first page and every continuation of one walk report the same string. The text
`truncated.continue_with` already carries it. Without `expected_revision` the call reads the
session as it is and reports the revision of that state.

The revision is one string that carries what a later call needs to recover the state it
names: the size the read was cut at, the file's mtime, hashes of the first and last 4 KB of
those bytes, a kind (plain, a Codex fork with an unresolved boundary, or compressed) and a
hash of the file path, the archived flag, the projection version and the record and tool
character limits and redact setting. Its format is not a promise. Producing it reads two
4 KB regions, and for a Codex file a classification read: the header line for an unforked
file, the lines up to the history boundary for a fork, and all the bytes up to the size for
a fork whose boundary is not reached. On a 208 MB unresolved fork (release build, three
runs) a refused continuation, which is classification only, took 0.10 to 0.11 s and a first
page with `--limit 1` 0.25 s; a 208 MB unforked file took 0.00 s for both. A string that
does not parse back to exactly itself is refused like a stale one.

**A session that has only grown.** If the file is larger than the revision's size and its
first bytes still match the samples, the call is served the session as it was at that
size: the read stops there, offsets and records are exactly what the revision described,
and `total` and the end of the session are those of that state: the end of a pinned walk is
the end of the session as it was at that revision, and `grown` says more exists. The response carries
`grown`, with `current_revision` and `bytes_added`, so the caller can start a fresh read
without `expected_revision` when it wants the new content. This is what lets an agent page
through a session that is still being written. A torn final line at the cut is projected
as it was on the first read, because the bytes are the same.

**What refuses.** Any other mismatch is an error on both doors, like a lookup that finds
nothing, naming the revision the caller passed and the current one, and telling the caller
to restart from offset 0 without `expected_revision`: a rewrite or truncation, a file
moved between the active and archived roots, a file compressed or replaced, different
`TROVADOR_*` limits or `--no-redact`, a touch that leaves the size unchanged, a malformed
string, and any append to a Codex fork
whose start ordinal has not been reached in the bytes read. For that fork a later line can
resolve the boundary and turn records already shown into `forked_history`, so the append
cannot be tolerated. The same root reached through a different path spelling or a symlink
gives a different revision, so two doors configured with different spellings of a root
never share one. A compressed rollout is revised by its stored size and mtime, so
compressing a plain rollout changes the revision even though the records do not, and a
compressed token never matches a grown file.

**Limits of the check.** The mtime is compared only while the file is still the size the
revision names, because an append moves it. At that size it catches a same-size rewrite
with a new mtime. Once the file has grown, a rewrite of the first bytes that leaves the
sampled first and last 4 KB as they were cannot be told from an append, and a rewrite that
keeps the size, the mtime and the samples is not detected at all. A compressed rollout
replaced in place with the same size and mtime is not detected. Reads of a plain file stop
at the size the revision names, for the fork-boundary scan as well as the records, so
bytes appended during a read are never returned under that revision. If a plain file is
rewritten during a read, the call fails with a message to retry.

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


When the index cannot be updated, because its directory is not writable (an agent sandbox,
a read-only filesystem), search reads the index as it is. `exhaustive` is false and `note`
says why, since sessions written after the last refresh are not covered. With no index, or
one built by another version, search fails and says to run `trovador index` where it can
write, or to use the MCP server. The read-only path takes no locks when there is no WAL, so
it checks after its last read that the file did not change under it and fails with a
request to retry if it did. On a 482 MB index it took 0.10 s and 17 MB of memory.

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

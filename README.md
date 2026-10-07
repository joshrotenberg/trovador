# trovador

Local Claude Code and Codex sessions as one MCP server and CLI.

Every session either CLI has run is already on disk. trovador reads those archives, lists
and pages through them, and serves the same answers to a terminal and to an agent over MCP.
It is the Rust successor to [spike](https://github.com/joshrotenberg/spike); the v0.1 scope
is tracked in [#1](https://github.com/joshrotenberg/trovador/issues/1).

## Status

| Slice | State |
|---|---|
| Archive reading: list, show, transcript | Done |
| Full-text index and search | Not started |
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
trovador doctor                              # paths, limits, archive counts
trovador serve                               # MCP over stdio
```

Every command prints one JSON document. The MCP `sessions` tool takes the same inputs as an
`action` of `list`, `show`, or `transcript` and returns the same JSON. Resources
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

## Where things live

trovador reads `~/.claude/projects` and `~/.codex/sessions` and never writes to either. Its
own state goes under `~/.trovador`. Override any of them with `TROVADOR_CLAUDE_ARCHIVE`,
`TROVADOR_CODEX_ARCHIVE`, `TROVADOR_HOME`. Limits are listed by `trovador doctor` and
overridable with `TROVADOR_*` variables (see `src/limits.rs`).

## License

MIT or Apache-2.0, at your option.

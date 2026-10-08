# What shipping means here

**Shipped** means the repository is public, `v0.1.0` is tagged and published to crates.io
as `trovador`, `cargo install trovador` followed by the README's setup works on a machine
that has never seen it, and the live suite passes on at least two machines with different
data and CLI versions.

The items below come from [use-cases.md](use-cases.md) and
[landscape-2026-10.md](landscape-2026-10.md). **Must** blocks shipping. **Should** is
expected soon after. **Later** waits for a use that needs it.

## Must

Decided 2026-10-07: both search defaults apply to both doors; automated sessions are
excluded by default except those trovador started.

Status: every item below is merged except release mechanics, which is #15. What remains
after #15 is in the person's hands: making the repository public, adding the
`COMMITTER_TOKEN` and `CARGO_REGISTRY_TOKEN` secrets, and setting `RELEASE_ENABLED`.

| Item | Done in |
|---|---|
| Search leaves out the calling session | #12 |
| Automated sessions out of default results | #12 |
| Transcripts marked untrusted | #13 |
| Secrets redacted by default | #13 |
| `serve --read-only` | #13 |
| `doctor` checks providers | #14 |
| Release mechanics | #15 |

| Item | Why | Use case |
|---|---|---|
| Search leaves out the calling session: an `exclude_session` parameter, and by default sessions written in the last 10 minutes, with `include_active` to opt back in. The same default for the CLI and MCP | An agent searching for earlier work finds its own conversation first; this happened when searching from the development session | 2 |
| Automated sessions (`sdk-cli`, `exec`, `sdk-ts`) out of default `list` and `search` results unless `include_automated`, except sessions trovador started, which are always included | 20 of 43 sessions matching one real query were automated; trovador's own fan-out workers are automated but are what a coordinator searches for | 1, 3 |
| Transcripts and excerpts marked as untrusted data, in the server instructions and the tool descriptions | Earlier sessions can contain instructions an agent would otherwise follow | 2 |
| Secrets redacted in MCP output by default: API keys, bearer tokens, private keys, and `KEY=value` lines for names that look secret. The opt-out is a `serve` flag, never a tool argument | Transcripts contain pasted credentials, and MCP hands them to whatever client is connected | 2 |
| `trovador serve --read-only`, which offers no `ask` and no `summarize` | Anyone who wants search should not have to give a connected client the ability to spend money and run agents | all |
| Release mechanics: public repository, `release-plz` and `cliff.toml` as in the other crates, `CHANGELOG.md`, an MSRV job in CI, supported platforms stated (macOS and Linux; Windows untested) | A published crate needs them | |
| `doctor` reports whether `claude` and `codex` are found and their versions, plus index and consent state | The first thing to check on a new machine, and spike's `diagnose` covered it | |

## Should

| Item | Why | Use case |
|---|---|---|
| A long `ask` returns a task handle at once and reports progress, through MCP's task extension, which tower-mcp supports | A 10-minute Codex turn otherwise holds one tool call open for 10 minutes | 3 |
| `since` and `until` on `list` and `search` (done, #16) | "What did I do last week" without a model call | 1, 7 |
| `resume_blocked_by` names the call that works: `ask` with `about` and the same backend (done, #16) | The way to continue past a full context is not discoverable | 5 |
| `trovador gc` for `summaries/` and `runs.jsonl` (done, #17) | Both grow without bound | |
| A way to leave reasoning out of the index (done, #17: `TROVADOR_INDEX_REASONING=0`, 416 MB to 290 MB) | It raised the index from 287 MB to 415 MB | |
| ~~Peak memory during a full build bounded by bytes queued for the writer~~ Dropped in #17 | Measured: one worker peaks no lower than eight (645 MB against 610 MB), Claude alone peaks at 77 MB, and three runs of the same build range from 539 to 651 MB. The writer queue is not the cause, and no single change moved the peak beyond that run-to-run spread. The build is a one-off of about 10 s | |

## Later

- Keeping index rows after the provider deletes the transcript, opt-in (use case 9).
- A `pr` filter on `list`, and a `path` filter that matches tool arguments (use case 8).
- A digest of what changed since a time, summarizing only those sessions (use case 7).
- Fan-out with synthesis (spike's `canvass`), once someone needs it (use case 3).
- More agents, semantic search, export, an HTTP transport.

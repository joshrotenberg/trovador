# Use cases

> **2026-11-02:** trovador is now read-only and runs no model. `ask`, `summarize`, consent,
> `import`, and `gc` were removed; `sessions text` hands the caller a session's text to
> summarize itself. The items below are kept as the record of what was decided at the time.

Each case is something that has been done, or tried, with spike or trovador, or that
comparable tools are used for (see [landscape-2026-10.md](landscape-2026-10.md)). For each:
who does it, the steps, whether trovador handles it today, and what is missing. Findings
marked *measured* were checked on the development machine on 2026-10-07.

The two callers are different. A **person** at the CLI wants a short answer and a command
to paste. An **agent** over MCP wants bounded JSON it can act on, and is the one most
likely to be misled by noise in the results.

## 1. Find where I did something

> "Which session did I work out the consent levels in?"

**Steps.** `search` with a few words, then `sessions show` on the best hit, then paste
`open_with`, or `sessions transcript --offset <match index>` to read around the hit.

**Today.** Works. Searches return in about 0.1 s, and each match carries the transcript
offset to read from.

**Missing.**
- **Automated sessions crowd the results.** 1,334 of 1,587 local Claude sessions are
  `claude -p` runs, mostly started by tools. For `idempotency_key`, 20 of 43 hit sessions
  were `sdk-cli` or `exec` (*measured*). The `origin` filter exists, but it is opt-in.
- **No time filter.** "Last week" could only be done by reading dates off the results.
  `since` and `until` were added in #16.

## 2. An agent recalls earlier work

> An agent, partway through a task: "have we solved this before?"

**Steps.** The agent calls `search`, then `sessions` with `action: transcript` at a
match's index.

**Today.** Works, with one trap.

**Missing.**
- **The searching session finds itself.** Its own transcript contains the words it is
  searching for. Searching "jazzyalex agent-sessions" from this session returned exactly
  one session: this one (*measured*). AgentsView excludes sessions active in the last 10
  minutes by default and takes a `current_session_id`.
- **Transcripts are untrusted input.** They are handed straight to an agent. Earlier
  sessions can contain instructions, pasted secrets, or prompt injection. Nothing marks
  them as data, and nothing redacts them.

## 3. Fan out workers across backends

> The bottega run on 2026-10-07: four sessions, two Claude and two Codex, each in its own
> worktree, each told to join a mailbox; then follow-up turns to each.

**Steps.** `ask` four times with `cwd`, `add_dirs`, `allowed_tools`, and `max_turns`;
keep each `session_id`; `ask` again with `session` for follow-ups.

**Today.** Works, including the cases that broke spike. A Codex turn has 900 s before the
timeout; a run killed by its timeout still returns its id; and that id can be continued
without approval. Options Codex cannot honour come back as warnings.

**Missing.**
- **A long turn blocks the MCP call.** A 10-minute Codex turn holds the client's tool call
  open for 10 minutes. Claude Code moved spike's call to the background after 120 s. MCP's
  task extension, which tower-mcp supports, would let `ask` return a handle at once and
  report progress.
- **One call per worker.** spike's `canvass` (fan out and synthesize) was never used, so
  this is not a gap yet.

## 4. A second opinion from the other backend

> "Have Codex look at what Claude decided in that session."

**Steps.** `ask` with `about: <session>`.

**Today.** Works. The 2026-10-07 check took 5.7 s with the summary already cached, and
the consulting session can be continued. No comparable tool summarizes a past session and
runs the other backend on it.

## 5. Continue past a full context

> A session is over the resume ceiling, but the work is not finished.

**Steps.** `ask` with `about: <session>`, the same `backend`, and a prompt that says to
continue the work.

**Today.** Works. Since #16, `resume_blocked_by` names the call: `ask` with `about` set to
the session and the same backend.

## 6. Pick up a run that was killed

**Steps.** Take the `session_id` from the timed-out result, then `ask` with `session`.

**Today.** Works. Fixed in #4 (spike#108), and in #7 for a run killed before the
transcript existed, which now reports that there is nothing to continue.

## 7. What did I work on

> "What did I do yesterday, across both agents?"

**Today.** Partly. `sessions list --since 1d` (#16) lists what was worked on, with titles
and snippets, at no model cost. Summarizing each of those costs one model call per
uncached session. spike's `digest` did this and was used twice.

**Missing.** A digest that summarizes only what changed can come later.

## 8. Find the session behind a PR or a file

> "Which session opened PR 42?" "Which sessions touched `src/index.rs`?"

**Today.** Partly. Claude records `pr-link`, and trovador lists it on the row
(`pull_requests`), but nothing filters by it. A path finds sessions through full-text
search, without telling a read of the path apart from an edit to it.

**Missing.** A `pr` filter on `list`. A `path` filter that matches tool-call arguments.

## 9. Find something older than the provider keeps

> "Where did I do X three months ago?"

**Today.** Not supported. Claude Code deletes old transcripts according to
`cleanupPeriodDays`, and trovador drops a session from the index once its file is gone.
ccrider and AgentsView keep their own copy for this reason.

**Missing.** An opt-in: keep the index rows of sessions whose file was deleted, and mark
them `archived_by_trovador`. A search can still find them. Nothing can resume them.

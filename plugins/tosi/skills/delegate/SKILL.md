---
name: delegate
description: Delegate a bounded local task, independent review, or follow-up to Claude or Codex through Tosi. Use when the user requests either provider or when a separate worker would help complete the authorized task.
---

# Delegate a task

Use Tosi for a bounded assignment to a local Claude or Codex worker.
Call this host's installed MCP tools, using tool discovery first if they are
deferred. If unavailable, diagnose the plugin connection or refresh the host
session; do not create a shell or Python stdio bridge for ordinary delegation.
Honor the user's backend, model, and effort choice. Otherwise omit `backend` to
use the configured default. Omitted model/effort use provider CLI behavior, not
the calling chat's settings. Read the model/effort reference when selecting them;
do not invent routing rules or automatically select Codex `ultra`.

For executable or authentication failures, call `check_environment` with the
affected `backend` (omit it to check both). It reports nonsecret launch context
and auth status without a model call. An auth error may reflect this process's
environment or credential-store access; do not tell the user to log in again
before checking the intended host context. Never read/export credentials or
bypass sandbox denials. See the implementation reference for diagnosis.

Give each worker an absolute `cwd` and a self-contained brief: outcome, relevant
files, constraints, repository instructions, and acceptance criteria. Workers do
not inherit this conversation or the parent's connected tools. Claude also skips
automatic CLAUDE.md loading; include its applicable instructions explicitly.

Start with `spawn_task`, retain its ID, and collect outcomes with `wait_tasks`.
A wait timeout is a progress snapshot, not a task failure or cancellation.
Use `continue_task` for a follow-up to the newest successful turn, or
`cancel_task` when work is no longer needed. Inspect returned states and verify
results and changed files before integrating them. Task IDs last only for this
server process; this is not a persistent autonomous team.
Follow-ups retain the original backend, model, and effort without overrides.
Selection metadata records the request and settings sent; `reported.model` and
`reported.effort` remain `null`. Do not infer them from tokens or usage totals.

For a JSON result, pass `output_schema` to `spawn_task`. Use an explicit root
`"type": "object"`, object properties, all properties required, and
`additionalProperties: false`; provider schema support varies. The schema must
be self-contained and at most 64000 serialized UTF-8 JSON bytes. Local `$ref`
references work; external file or URL references do not. Both providers apply
the schema natively and the server validates the returned JSON locally.

Read `structured_result` separately from prose `result`; it is `null` unless a
schema was requested. Claude may return structured output with empty prose.
Missing, invalid, mismatched, or oversized requested JSON makes a task fail;
structured results have a 64000-byte limit and are never truncated.
Follow-ups inherit the schema without an override.

Default to `read_only`; use `workspace_write` only for authorized edits. Access
modes differ by provider: Claude has a restricted tool list and no Bash tool,
while Codex uses its native sandbox. Delegation must not expand the user's scope
or bypass a denied action. Isolate parallel edits with worktrees or disjoint
file ownership.

Read only the detailed pattern relevant to the assignment. The plugin includes
these local references; a standalone MCP client can retrieve the identical text
with `resources/read` at the corresponding URI:

- [Parallel tasks](references/parallel-tasks.md): independent assignments,
  ownership, collecting results, and capacity.
  `skill://tosi/delegate/references/parallel-tasks.md`
- [Implementation and review](references/implementation-review.md): edit scope,
  provider access, validation, and an independent review.
  `skill://tosi/delegate/references/implementation-review.md`
- [Follow-up and cancellation](references/follow-up.md): conversation continuity,
  waiting, result handling, and settled cancellation.
  `skill://tosi/delegate/references/follow-up.md`
- [Model and effort](references/model-effort.md): user preferences, provider
  support, omitted settings, and requested versus reported selection.
  `skill://tosi/delegate/references/model-effort.md`

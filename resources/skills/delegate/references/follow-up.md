# Follow-up and cancellation

Use a follow-up when a successfully completed worker conversation contains
useful context for one more bounded assignment. For an independent opinion,
start a fresh task instead.

Call `continue_task` with the newest successful `task_id` and a self-contained
`message`. The server starts a new process with the same backend conversation,
working directory, model, effort, access mode, per-turn timeout, and output schema,
returning a new task ID. These settings cannot be overridden on a follow-up; use
`spawn_task` when you need different settings or an output shape. Retained aliases
or omitted settings do not guarantee unchanged provider defaults; see the
[model and effort reference](model-effort.md). Only one turn
may be active in a conversation. After it finishes, use the newest returned ID;
older turns cannot be forked or continued again. Failed or cancelled tasks must
be replaced with `spawn_task`, including the facts needed to restart the work.

Collect results with `wait_tasks`. `timeout_ms` is at most 55000, and `0` returns
an immediate snapshot. The wait ends when any requested task is terminal or its
own timeout expires. A wait timeout leaves the worker running. The worker has a
separate execution deadline: `timeout_secs` defaults to 600 and is at most 3600.
There are no automatic retries.

Use `cancel_task` when an assignment is no longer needed. It waits up to 55
seconds for provider cleanup. `cancelled` means that cleanup returned;
`cancelling` means cleanup is still in progress, so check with `wait_tasks`.
Cancellation of an already terminal task returns the existing result. Never
interpret a cancellation request, a wait timeout, or a disconnected client as
evidence that the task completed successfully.

Read the final status, result, and any changed files. Report failures accurately.
If `result_truncated` is true, the result exceeded the retained 64000-character
limit; request a shorter follow-up after a successful turn or inspect the
relevant workspace artifacts. Do not assume missing text was unimportant.

When `output_schema` was requested, read `structured_result` as the validated
JSON value. It is separate from the prose result, which may be empty for a
Claude structured-only response. Without a requested schema, `structured_result`
is `null`. Both providers apply their native schema option, and the server
validates the output locally. Missing JSON, invalid JSON, a schema mismatch, or
output above 64000 serialized UTF-8 JSON bytes makes the task fail; structured
JSON is never truncated. A schema must itself fit within 64000 serialized bytes,
explicitly declare root `"type": "object"`, and use only local `$ref` references.
For provider compatibility, prefer object properties with every property in
`required` and `additionalProperties: false`.

Task IDs are local to the current server process. Provider CLI history may
remain on disk, but a new server cannot continue an old Tosi task ID.
Retain useful outcomes in the parent conversation or workspace before ending
the server session.

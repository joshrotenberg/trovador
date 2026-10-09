Give the latest successfully completed task a follow-up assignment. Starts a new
short-lived process resuming the same provider conversation, workspace, model,
effort, access, per-turn timeout, and output_schema; returns a new task ID. These
settings cannot be overridden. An alias or omitted setting does not freeze the
provider's effective model or defaults; reported model/effort remain null.
One active turn per conversation. Failed/interrupted/cancelled turns
must be replaced by spawn_task.

If a schema was requested, read the locally validated JSON in structured_result;
prose result may be empty for Claude. Missing, invalid, mismatched, or oversized
requested JSON fails the task. Structured JSON is never truncated and is limited
to 64000 serialized UTF-8 JSON bytes. Without a schema, structured_result is null.

A completed latest turn can be continued after a restart. A live task owned by
another server is refused with its owner identity.

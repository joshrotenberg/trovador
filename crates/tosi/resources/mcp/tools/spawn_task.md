Delegate one bounded assignment to a short-lived Claude or Codex CLI worker.
Returns immediately with a task ID. Provide an absolute cwd and all relevant
context/instructions; this chat is not automatically inherited. Defaults to
read-only. Use separate worktrees for concurrent edits.

Optional backend, model, and effort select execution settings. Honor explicit user
choices. Claude effort accepts low, medium, high, xhigh, max; Codex exposes none,
minimal, low, medium, high, xhigh, max, ultra, depending on the model. Unsupported
backend/value and known exact-model combinations reject before start; aliases
and unknown models have unverified compatibility. Omitted model/effort use CLI
defaults and applicable environment, not this chat's settings. Do not automatically
select Codex ultra. Read skill://tosi/delegate/references/model-effort.md
for guidance. Task snapshots retain requested/selected settings; reported model
and effort are null, not inferred from usage. The server does not downgrade,
fall back, or retry; provider CLIs may resolve aliases or apply caps/defaults.

Optional output_schema requests a JSON result. Supply a self-contained JSON
Schema object with explicit root type "object", at most 64000 serialized UTF-8
JSON bytes. Local $ref references are supported; external references are not.
Provider schema support varies; prefer object properties, all properties
required, and additionalProperties false. Each provider applies the schema
natively, then the server validates the output locally. Read structured_result
separately from prose result; it is null without a schema, and Claude may return
empty prose with structured output. Missing, invalid, mismatched, or oversized
requested JSON fails the task. Structured JSON is never truncated and is limited
to 64000 serialized UTF-8 JSON bytes.

Task IDs survive restarts. Use list_tasks to find them and wait_tasks to retrieve
results. The owner serves memory results; other processes read the always-redacted
disk copy, which may differ when TOSI_NO_REDACT is enabled in the owner.

The owner caches at most 1024 task entries. At capacity, persisted terminal
entries are evicted oldest first and then read from the redacted disk copy.
Active and unsaved entries are protected. Dirty saves are retried on listing
and every 60 seconds, as well as when waiting on that ID. Unknown record versions
are left in place for newer servers; they produce an error only on direct reads.

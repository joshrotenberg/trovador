Collect task results. Returns when any requested task is terminal or the bounded
timeout expires. timeout_ms=0 reads status immediately. Remove completed IDs before
waiting again. Does not cancel workers when waiting ends.
Each snapshot includes the original effort and selection metadata: requested
versus selected settings, with reported model/effort null because actual per-turn
settings cannot be verified.

Tasks survive restarts. Terminal `interrupted` means the owning server stopped
before finishing; its error can report a surviving provider child PID. Waiting
for another live server's task polls durable history.

Known snapshots are returned in tasks. Unavailable IDs appear in errors as
{task_id, error}, including unknown IDs (pruned or never existed), without failing
other IDs in the same request. The owner serves its memory results and retries
failed saves; other servers read the always-redacted disk copy. With
TOSI_NO_REDACT enabled in the owner, that copy can differ from the validated
structured result and may not satisfy the original schema.

The owner caches at most 1024 task entries. At capacity, persisted terminal
entries are evicted oldest first and then read from the redacted disk copy.
Active and unsaved entries are protected. Dirty saves are retried on listing
and every 60 seconds, as well as when waiting on that ID. Unknown record versions
are left in place for newer servers; they produce an error only on direct reads.

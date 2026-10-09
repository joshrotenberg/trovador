# Parallel tasks

Use this pattern when assignments can make progress independently, such as
reviewing separate components or investigating two possible causes of a bug.
Choose a concrete question and expected result for each worker. Do not create a
team merely to keep workers busy.

1. Assign each worker a bounded brief with its relevant context and absolute
   `cwd`. Workers receive no parent conversation. Include repository instructions,
   files to inspect, constraints, and the evidence expected in the result.
2. Use `read_only` for investigation. For authorized parallel edits, create
   separate worktrees through the parent host's tools or assign disjoint file
   ownership. Tosi does not create worktrees or merge changes.
3. Call `spawn_task` once per assignment and retain every task ID. A task starts
   immediately or is rejected when the concurrency limit is reached; it is not
   queued. Wait for capacity before resubmitting a rejected assignment.
4. Call `wait_tasks` with active IDs and `timeout_ms` at most 55000. It returns
   when any requested task is terminal or the wait expires. Remove terminal IDs
   before waiting again. Use `timeout_ms: 0` for an immediate snapshot; do useful
   independent work between waits rather than polling continuously.
5. Inspect every outcome, including failures. Resolve conflicting evidence in
   the parent conversation and verify consequential claims. Cancel remaining
   work that no longer contributes to the objective.

For example, one worker can inspect a parser while another checks the relevant
call sites. Both should return file references and evidence; the parent decides
whether the findings explain the observed failure.

Each host connection has its own task registry and concurrency limit. Records
last only for that server process. There is no shared daemon, cross-machine
handoff, automatic retry, or nested Tosi server. A server retains at most
1024 task records; start a new server session after reaching that limit, once
the current work is settled and its results are retained elsewhere.

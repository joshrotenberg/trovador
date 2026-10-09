# Tosi development

## Review through Tosi

Use this project through its own MCP tools to review substantive changes with
both Claude and Codex. The owner has authorized bounded paid runs on both
backends for testing this project; individual runs within that scope do not
require another confirmation. This is a development workflow, not a scheduled
background job.

Start with one baseline review of the current implementation, then review
coherent change batches. Formatting, typo fixes, and similarly trivial edits
can skip dual paid review. Treat authentication, permissions, process cleanup,
tool contracts, launch configuration, and packaging behavior as substantive
even when their diffs are small.

1. Run relevant local checks and prepare a stable source snapshot, change list,
   requirements, applicable repository instructions, and test results. If Git
   is unavailable, use a bounded source copy and file hashes/change manifest;
   do not initialize a repository merely for review. Keep the reviewed source
   unchanged while reviewers work.
2. Use the host's installed Tosi tools, discovering deferred tools as
   needed. Check a new or failing launch context with `check_environment`.
   Missing tools or authentication must be diagnosed through the normal host
   connection and approval flow. Do not improvise a sandboxed shell bridge or
   extract credentials. Report a missing backend review explicitly.
3. Spawn one fresh `read_only` reviewer per backend, preferably in parallel.
   Give both the same review scope and evidence, without the other's findings.
   Reviewers must not edit files or delegate further. The parent supplies test
   evidence; Claude cannot run shell commands through this adapter.
   Allow Codex read-only inspection commands such as `rg`, `cat`, and `sed`;
   a blanket shell prohibition can prevent it from reading the source. Keep
   test/build execution with the parent unless the assignment says otherwise.
4. Request `output_schema` with structured findings containing priority,
   file/line, failure scenario, evidence, impact, and suggested correction.
   Include review limitations separately. An empty findings array is valid;
   task completion alone does not establish that the change is correct.
5. Deduplicate and verify findings before implementing fixes. Run the relevant
   checks again after changes. Use focused follow-ups for affected areas; ask
   both reviewers again when a correction changes shared behavior or creates
   substantial regression risk. Further rounds require new material evidence,
   not a ritual approval loop. There are no automatic paid retries.
6. Record the reviewed snapshot, backend/task IDs, explicitly selected model
   settings, findings and dispositions, checks performed, and remaining gaps.
   Do not guess the actual model or effort when a provider does not report it.

Default to a five-minute execution limit per routine review, extending it for
a justified larger scope. This bounds duration, not monetary cost. Honor user
model/effort choices and use only parameters the current server supports.

Source review and runtime verification are separate. Changes to server behavior
need relevant runtime tests. When validating through a packaged plugin, rebuild
and regenerate the candidate, load it in a fresh host connection, and verify
the executable being exercised matches the candidate. Do not count an older
cached binary as validation of new behavior.

The first baseline should cover task lifecycle and cancellation, both provider
adapters, structured results and continuation, environment/auth diagnostics,
plugin packaging, and the existing test coverage. The first baseline and its
verified corrections completed on 2026-10-08; see
[the review record](reviews/2026-10-08-baseline/REPORT.md) for snapshots, findings,
checks, and limitations. Subsequent substantive batches use this same workflow.

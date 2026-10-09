Tosi delegates bounded local assignments to Claude or Codex CLI workers.
Use the host's installed tools, discovering deferred tools when needed. If tools
are unavailable, diagnose the connection or refresh the host session; do not
launch a shell/Python stdio bridge for ordinary delegation.
Read skill://tosi/delegate/SKILL.md using resources/read for general
guidance, then retrieve only the relevant linked pattern: parallel tasks,
implementation and review, follow-up and cancellation, model and effort, or
consulting an earlier session.
resources/list discovers all six Markdown resources; they contain the same canonical skill and references
included in the plugin package.

check_environment checks executable, environment, and authentication status for
an optional backend (omitted checks both), without a model call or secret values.
Use it to diagnose launch-context failures before recommending login. An auth
error does not prove the user is logged out. Do not extract credentials or bypass
inherited sandbox restrictions; use the host's configured launch path and normal
approvals for intentional standalone clients.

spawn_task returns a task ID; wait_tasks collects status and results without
cancelling work. continue_task starts a follow-up from the latest successful task
and returns a new ID. cancel_task requests cancellation and reports whether
cleanup is complete.

Honor explicit backend/model/effort choices. Omitted model and effort use provider
CLI behavior, not the calling chat's settings. Effort values are provider/model
dependent; read the model-effort reference when selecting them. Do not automatically
select Codex ultra. Continuations retain the original settings and schema without
overrides. selection distinguishes requested and selected settings; reported model
and effort remain null because actual per-turn settings cannot be verified. Never
infer them from usage totals. No server-side downgrading, fallback, or retry occurs.

Supply an absolute cwd and a self-contained brief with applicable repository
instructions; the parent conversation is not inherited. Default to read-only and
isolate parallel edits. Task IDs exist only for this server process. Hosts decide
whether to load resources into model context; exposing them does not automatically
inject their contents.

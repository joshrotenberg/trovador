# Tosi roadmap

Status: completed milestones and proposed work, October 8, 2026. This records
direction and completion criteria, not a supported API or a release schedule.
Start with evidence from using the current implementation before expanding it.

Milestones 1–6 are the first-round priorities. A second planning round will use
the results of that work to choose the next bounded scope.

Keep the core small: one bounded assignment per worker turn, then exit. A saved
provider conversation can support another explicit turn. The calling agent owns
planning, context, and coordination; the server owns execution and deterministic
policy.

## Already implemented

- Claude and Codex behind `spawn_task`, `wait_tasks`, `continue_task`, and
  `cancel_task`, with bounded concurrency, timeouts, and process cleanup.
- Optional model and effort selection, access modes, and locally validated
  `output_schema` results. Continuations retain the original execution settings.
- Backend/effort validation and a dated exact-model capability table. Task
  snapshots distinguish requested and selected settings; reported model/effort
  remain unknown rather than being inferred from usage.
- `check_environment` and `doctor` for executable and authentication diagnostics
  in the actual launch context, without exposing credentials or calling a model.
- A canonical delegation skill and deeper references, including model/effort
  guidance, packaged as plugin files and embedded as five MCP resources.
- Clap CLI and `cargo xtask package` producing Codex, Claude, or combined plugin
  packages with a native executable.
- Deterministic tests, environment-gated real-provider tests, and successful
  real runs through both backends and plugin hosts.

There is no routing policy engine or release CI yet. Omitting `model` or `effort`
leaves those options to the provider CLI; MCP does not automatically
inherit the calling chat's model or effort. The first dual-backend baseline review
and verified fixes are complete; see the
[baseline review record](reviews/2026-10-08-baseline/REPORT.md). Explicit effort
controls and their bounded validation are also complete; see the
[effort review record](reviews/2026-10-08-effort/REPORT.md). Deterministic routing
is the next planned implementation milestone.

## 1. Establish the dogfood baseline

**Completed 2026-10-08.** Independent Claude/Codex reviews found and verified
prerelease-readiness and artifact-selection issues; focused reviews also caught
the packaging alias's relative-path behavior. All were corrected and tested,
with no further findings in the final focused reviews. The record above preserves
coverage limits, task IDs, source/binary identities, and the corrected Codex
inspection brief. This is the process for subsequent substantive batches.

Review the current implementation through the installed plugin with one fresh,
independent reviewer on each backend. Follow [AGENTS.md](AGENTS.md) for the
bounded paid review workflow and when subsequent changes warrant both reviewers.

- Cover lifecycle and cancellation, provider adapters, structured results and
  continuation, environment/auth diagnostics, packaging, and test gaps.
- Use the same stable source snapshot and test evidence, with a common findings
  schema. Verify findings before fixing them; record dispositions and limitations.
- Make the review brief, output schema, and evidence record reusable. Identify
  the reviewed source and the actual packaged binary used for runtime checks.
- Carry forward environment regressions: CLI versus GUI launch context, explicit
  binary paths, unavailable authentication, and stale host/plugin caches. Diagnose
  these through normal host connections and approvals rather than moving secrets
  or changing protections.

**Done when:** both reviews have recorded outcomes, accepted findings have fixes
or an explicit deferral, and relevant checks pass against the candidate. An
unavailable backend remains a documented gap, not a completed review.

## 2. Add explicit effort controls

**Completed 2026-10-08.** `spawn_task` accepts optional provider execution `effort`
alongside `backend` and `model`, with adapter support for fresh and continued turns.

- Omission preserves provider CLI defaults and applicable inherited environment;
  it does not inherit the calling chat's settings.
- Backend/value checks and the dated `resources/model-capabilities.json` table
  reject known incompatible combinations before launch. Aliases and unknown
  models pass backend-level validation with compatibility unverified. The server
  does not downgrade, substitute, fall back, or retry automatically.
- Continuations retain backend/model/effort and schema. Snapshots include
  `selection.requested`, `selection.selected`, and `selection.reported`; reported
  model/effort remain `null` because the wrappers lack reliable per-turn evidence.
  Retained aliases or omissions cannot freeze the provider's effective settings.
- Canonical guidance documents these limits and avoids equating effort labels
  with normalized compute or cost across providers.

**Validation:** 49 deterministic tests, the build, and fake MCP smoke passed.
Bounded low-effort structured fresh and follow-up turns passed on both providers
(four paid turns total). Independent Claude/Codex reviews reported no runtime
findings; the two documentation findings were corrected. See the effort review
record above for evidence and coverage limits; these runs do not verify every
model/effort combination or attest the provider's actual settings.

## 3. Add deterministic routing policies

Support an instance default policy and a per-spawn policy choice. Begin with
direct selection and named rule sets. Keep generic task classification separate
from concrete provider settings:

| Proposed input | Meaning |
| --- | --- |
| `task_effort` | Caller-assessed task difficulty, such as low, medium, or high. |
| `backend`, `model`, `effort` | Concrete execution choices. |
| `routing` | Use the instance policy, select a named policy, or request direct selection. |

These names and the configuration format are provisional. One motivating rule:
high `task_effort` selects Claude, model alias `opus`, and execution effort `high`;
unmatched requests pass their explicit selection arguments through unchanged.

- Proposed precedence: select the request's policy or instance default; apply
  the first matching rule to only the fields it sets; resolve remaining defaults;
  validate the resulting combination before admission. Document this precisely.
- A direct per-request choice bypasses policy rewriting, allowing the caller to
  honor an exact user selection. Rules do not modify task permissions, schema,
  timeout, or other unrelated arguments.
- Return requested and selected settings, policy identity, and matched rule so
  callers can explain the decision. Report an effective model only when known.
- Continuations reuse the original resolved selection and policy decision. They
  do not reroute when policy configuration changes or switch provider sessions.
  Pinning preserves the settings sent; omitted defaults and floating aliases
  cannot guarantee an unchanged effective model.
- Reject invalid configuration and invalid resolved combinations clearly. Do
  not silently retry, substitute a model, or fall back to another paid backend.

**Done when:** deterministic tests cover precedence, unmatched requests, direct
selection, invalid combinations, decision metadata, and pinned continuations.
Real plugin runs demonstrate both a matched rule and a direct request.

## 4. Add round robin within one server instance

Build on the routing contract with an explicitly configured rotation of backend
or backend/model/effort selections. Do not add a global scheduler.

- Advance once when a new task is admitted, atomically with capacity checks.
  Invalid or rejected requests do not consume a position; admitted tasks that
  later fail do. Continuations do not advance the rotation.
- Keep state local to the MCP server process. Separate instances have separate
  counters; restarting begins a new rotation.
- Preserve the same decision metadata and continuation guarantees as rule-based
  routing. Define how a rule set can choose a rotation before composing policies.

**Done when:** concurrent admission tests prove ordering and counter behavior,
and a bounded real sequence exercises both providers without implicit retries.

## 5. Teach callers how to choose

**Partially complete 2026-10-08.** The core skill links to a canonical model/effort
reference, available in plugin packages and as the fifth MCP resource. It covers
user preferences, task difficulty, latency/usage tradeoffs, omitted settings,
provider compatibility, selection metadata, and continuation. Codex `ultra` is
reserved for an explicit user choice in the guidance.

Keep the core skill general and extend the canonical references as routing
mechanisms become available, so plugin users and standalone MCP clients receive
the same instructions. Remaining work depends on milestones 3 and 4:

- Explain exact selection versus configured policy and how policy decisions
  interact with explicit user preferences and omitted settings.
- Show small examples for a routine task, a difficult review, a user's high-effort
  Opus policy, direct selection, and round robin. Treat model mappings as user
  configuration rather than universal claims about model quality.
- Keep the distinction between native subagent inheritance and Tosi
  defaults explicit in new routing examples; callers still need to pass settings
  when the MCP context does not supply them.

**Done when:** examples match supported tool schemas and configured behavior,
packaged and embedded instructions agree, and both hosts can follow the guidance.
Ship guidance with each supported mechanism rather than waiting for every phase.

## 6. Make distribution repeatable

The existing archive already bundles the server executable. Recipients on a
matching OS/architecture do not need Rust; source builders do. Provider CLIs,
authentication, and model access remain prerequisites.

- Separate source-build and prebuilt-install instructions clearly.
- Add CI-built, versioned archives for tested platforms, starting with the
  current macOS ARM64 path and expanding macOS/Linux coverage as verified.
- Include checksums and build identity; keep binary, manifests, and instructions
  tied to the same release. Test extraction, launch, and fresh host loading.
- Keep local/repository distribution distinct from any future public plugin
  directory submission. Publication is a separate decision.

**Done when:** a recipient can install a matching artifact without a Rust
toolchain, verify its identity, and run environment checks and a bounded task.
Do not compile or download dependencies during normal MCP startup. An external
package-manager install or a pinned, verified bootstrapper can be considered later.

### Bottega marketplace direction

Tosi is the selected tool name under Bottega. The existing `bottega` repository
is marketplace-only: Tosi and trovador remain separate tools in their own
repositories, with their own executables and plugin instructions. The rename
uses the transitional `tosi-local` catalog; integrating with Bottega is separate
work and is not part of the rename.

The marketplace may remain private/local for the two personal machines. A public
GitHub repository is optional and does not publish a crate or submit a plugin to
a public directory. A shared binary exposing separate MCP subcommands remains
a possible later design, not the current architecture or a commitment to merge
the tools. Continue tool functionality before taking on that packaging change.

## Second-round planning checkpoint

After the first milestones and baseline dogfood review, revisit actual usage,
unresolved findings, and practical limitations before selecting more features.

- Assess routing decisions, model/effort usability, structured-result reliability,
  launch-context failures, and installation on the two personal machines.
- Prioritize regressions and friction found in real tasks. Consider deferred
  capabilities only when a concrete use case justifies their added complexity.
- Choose a small next batch with rationale, dependencies, and completion criteria;
  carry forward unfinished first-round work explicitly.
- Review substantive implementation and fixes with both backends under
  `AGENTS.md`, using focused follow-ups as appropriate. Planning this checkpoint
  does not schedule work or trigger another paid review by itself.

**Done when:** the roadmap records the selected second-round scope and the
evidence behind it. Deferred ideas remain candidates, not commitments.

### Candidate: orchestration patterns (`spawn_workflow` / `spawn_*`)

Add reusable orchestration over the existing task runners. A workflow fixes the
dependencies, fan-out, and aggregation; each step remains a bounded Claude or
Codex task. This provides repeatable control flow, not deterministic model answers.

The proposed entry point accepts a pattern configuration, workflow definition,
or saved name, plus JSON arguments and run limits, then returns a run ID promptly.
Callers need to inspect progress, wait for completion, and cancel the run, with child task IDs
and step outcomes visible. Decide whether existing wait/cancel tools can support
workflow records cleanly or need separate counterparts before fixing the API.

- Support per-item pipelines, bounded parallel steps, and explicit joins. Wait
  for a whole stage only when the next operation needs all its results, such as
  deduplicating findings before verification.
- Validate dependencies, limits, and schemas before starting workers; reject
  cycles in the initial graph format. Pass only validated structured results to
  dependent steps, retaining partial results when another branch fails.
- Reuse task admission, backend/model/effort routing, access controls, and
  `output_schema` validation. Steps can select either backend; continuations
  retain their original selection. Workflow coordinators must not occupy worker
  slots while waiting for their children.
- Share the server's concurrency limit across direct tasks and all workflows.
  Bound total tasks and runtime per workflow, and define pending-work limits and
  admission behavior. Workflow cancellation stops further admissions, cancels
  active children, and waits for cleanup; host shutdown follows the same rules.
- Keep completed, failed, skipped, and cancelled outcomes distinct. Define how
  each affects dependent steps and the final result. Preserve schema failures
  and coverage gaps rather than turning them into empty successful results.
- Require explicit workflow invocation or an explicitly invoked skill that
  authorizes it. Make fan-out and limits inspectable before launch. Begin with
  small runs; retries count toward the limits and require an explicit policy.
  Token or monetary caps must not be advertised until they can be enforced.

Start by evaluating named/versioned workflows or a small declarative graph. A
JavaScript orchestration layer is a later option if reusable compositions need
it; it requires an execution sandbox, bounded evaluation, and a clear versioned
contract. Arbitrary scripts, nested workflows, and open-ended search loops are
outside the first experiment.

Use the two-backend review as the first workflow: independent readers, a join to
deduplicate findings, bounded verification of selected findings, then an aggregate
report with evidence and limitations. A verifier vote does not establish that a
finding is correct. Keep the first experiment read-only; concurrent edits need
explicit workspace/worktree ownership and a merge strategy.

Record a run journal and workflow version from the start. Treat cached resume as
a separate increment from provider conversation continuation: validity depends
on the source snapshot, instructions, schemas, resolved settings, dependencies,
and inputs, not just matching prompts. Resume must not silently repeat edits or
other side effects; replaying an output does not recreate filesystem changes.
Cross-restart recovery also depends on a deliberate durable-state design.

**Done when:** a small mixed-backend workflow produces schema-validated step
results and a traceable final report, and tests cover shared capacity, partial
failure, cancellation, and shutdown. Review the experiment before adding a
script runtime, persistent resume, or larger fan-out.

#### API shape and additional patterns

Design one shared server execution layer with parameterized patterns. The caller
supplies the assignment, participants or roles, relevant settings, and bounds;
the server handles dispatch, context passed between steps, rounds, and collection.
Compare these tool surfaces before committing to names or argument schemas:

| Surface | Design question |
| --- | --- |
| `spawn_workflow` with a typed `pattern` configuration | Can one discoverable tool express the patterns without a large, confusing schema? |
| `spawn_debate`, `spawn_consensus`, `spawn_pipeline` | Do focused tool descriptions and schemas make correct invocation easier? |
| Existing task primitives, with additional granular controls only as needed | Which interventions actually require caller control between stages? |

Dedicated tools can be thin adapters to the same executor. They must share
validation, routing, admission limits, run records, and cancellation semantics.
Keep pattern-specific fields typed; avoid a universal bag of optional arguments.
Preserve ordinary `spawn_task` for callers coordinating work themselves.

Candidate patterns and their output contracts:

| Pattern | Behavior and result |
| --- | --- |
| Debate | Assign perspectives, collect initial positions, and run bounded critique/rebuttal rounds. Make synthesis or judging optional with stated criteria; preserve evidence, concessions, objections, and unresolved disagreements. |
| Consensus | Collect independent initial answers, then apply an explicit agreement/quorum rule, with optional bounded reconciliation. Report agreement, dissent, valid participation, and `no_consensus` when appropriate; failed workers are not votes. |
| Pipeline | Connect schema-validated stage outputs to declared inputs, with per-item progression, optional fan-out, and explicit joins. Return final outputs alongside stage failures and coverage gaps. |

Allow backend/model/effort choices per role or stage through the shared routing
contract. Define maximum participants, rounds, total tasks, and duration before
launch, including any synthesis or judging tasks. A workflow skill explains which
pattern fits the task and how to interpret its result; agreement alone is not
evidence of correctness.

**Design checkpoint:** express one small debate, consensus task, and pipeline
through both a unified tool and dedicated-tool schemas. Compare clarity, schema
size, validation errors, and caller control before choosing what to expose. Keep
JavaScript and arbitrary composition optional later extensions. These are
second-round candidates, not additional first-round implementation commitments.

## Deferred until evidence calls for them

Long-lived specialized workers, durable server state, cross-machine scheduling,
provider/session migration, automatic retries or failover, randomized or learned
routing, and normalized monetary budgets. Timeouts already bound duration; they
are not spending caps. Windows support needs an explicit portability and testing
effort before being advertised.

Keep future shared spawn options tied to concrete tasks and both adapters'
capabilities. This roadmap does not introduce arbitrary CLI argument passthrough
or expand worker access. Substantive implementation batches follow the two-backend
review workflow; planning, formatting, and typo-only changes need not incur paid
runs.

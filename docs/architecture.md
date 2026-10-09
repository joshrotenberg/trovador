# Architecture

This document describes what bottega is, how it is put together, and where it is going. It is
written so the design can be understood, or rebuilt, without the current implementation: the
rules and contracts here are the architecture, and the stack is a detail. Where the
implementation matters, it is noted as such.

`README.md` remains canonical for what exists and how to install it. Issues are the backlog;
this document links to them rather than repeating them.

## The problem

Two agent CLIs, Claude Code and Codex, run side by side on the same machine. Each keeps its own
sessions, its own memory, and its own tools, so work done in one is invisible to the other, and
neither can hand a task to the other. bottega is a set of small local services that bridge them:

- find and read earlier work in either CLI's history;
- delegate a bounded task to either CLI and collect a structured result;
- later, keep shared memory that both can read and write.

It is personal tooling for one person on their own machines. Nothing is published or released.

## Principles

1. **The noun is a session, not an agent.** An agent is stateless between invocations; a session
   is what accumulates. Both CLIs already persist sessions, so they are addressable whether or
   not anything was built to address them.
2. **Read what already exists; add nothing that must be undone.** bottega reads the CLIs' own
   files and never writes to them. Everything bottega writes lives under one directory, and
   removing that directory removes bottega and nothing of the user's.
3. **Past sessions are read-only history.** Agents search, read, and summarize them. Nothing in
   bottega continues or forks a past session; going back into one is the user's own act, with
   the CLI's resume command. Earlier context is carried forward as a summary in a new brief.
4. **Workers are single-use.** A delegated worker gets a self-contained assignment, completes it,
   returns a result, and is discarded. A follow-up to the same worker is allowed while the
   delegation is live; nothing depends on a worker persisting.
5. **Contracts, not prose.** Anything a caller acts on comes back in a declared shape that is
   validated before it is returned. Every field is required, so a worker cannot quietly omit
   "did you run the tests". Structured output was the single biggest enabler in practice.
6. **Small, independent services.** Each service is useful on its own, is built one at a time,
   and is dropped if it does not earn its place in daily use. An earlier, larger attempt at the
   same idea failed on complexity; this one keeps the bar low on purpose.
7. **Honest about completeness and cost.** A search says whether its absence of results is proof.
   A tool that spends model tokens says so before and after. An error says what kind it is and
   what the user should do.
8. **Packaging is a build output.** Each service is a library first. The plugin that installs it
   into a host is generated from it, not maintained by hand.

## Services

| Service | Role | Surface | Status |
|---|---|---|---|
| **trovador** | History: index, search, read, and summarize every session of both CLIs | `search`, `sessions` (list, show, transcript), `summarize`; `session://` resources | in daily use |
| **tosi** | Delegation: run a bounded task on a Claude or Codex worker | `spawn_task`, `wait_tasks`, `continue_task`, `cancel_task`, `check_environment` | in daily use |
| **facts** | Shared memory across both CLIs: preferences, decisions, notes | to be designed from an existing spike | planned (#87) |

Considered and not built as services:

| Idea | Decision |
|---|---|
| Mail between agents | Dropped. Workers are single-use, so there is no one waiting to read it. Cross-CLI handoff is a delegated task. |
| Work management | Folded into tosi: a unit of work is an input contract plus an output contract, tracked with task labels and a durable task registry (#84, #82). |
| Scheduling | Not a service. A one-shot `tosi run` lets any existing scheduler start a task (#86). |
| Manager | The calling agent is the manager. |
| Central skill registry | Starts as tosi profiles (#85), since tosi workers are the consumers that do not load skills themselves. |

### What each service owns

- trovador owns its index, its summary cache, and its run log. It reads the CLIs' archives and
  writes nothing to them.
- tosi owns its task registry (planned, #82). Workers it starts are recorded in shared
  provenance so trovador can show them.
- State touched by more than one service lives in a shared area (see State).
- Services never depend on each other. Shared code lives in a core library.

## Layers

```
              host app (Claude Code, Codex)
                        │  MCP (stdio today; HTTP for the shared server)
       ┌────────────────┼─────────────────┐
       │                │                 │
   plugin: trovador  plugin: tosi    plugin: facts        generated from the crates
       │                │                 │
   binary trovador   binary tosi     binary facts         thin: CLI subcommands + `serve`
       │                │                 │
   trovador lib      tosi lib        facts lib            domain logic + toolset
       └────────────────┼─────────────────┘
                   bottega-core                           state paths, redaction,
                                                          provenance, shared types
```

- **Core library.** State locations, redaction, provenance, the backend enum, private file
  helpers, and the shared error and status shape. No MCP, no database. Code moves into core
  only when a second service needs it.
- **Service library.** The domain logic plus a *toolset*: one function that registers the
  service's tools, resources, and skills on an MCP router.
- **Two doors over one library.** Every service has a CLI and an MCP server, and both call the
  same library functions, so they cannot disagree. Bugs where the two doors computed the same
  answer differently were frequent enough to make this a rule.
- **Thin binaries, one per service.** `serve` mounts the toolset; other subcommands are the CLI.
- **A shared server later (#80).** One long-lived local process mounts several toolsets, one
  endpoint per toolset. Each plugin points at its own endpoint, so every host still sees
  separate servers with separate context, while the process, the task pool, and the index
  writer are shared. Delegated work then survives a host restart, and a task started from one
  CLI can be collected from the other.

## MCP surface

### Design rules

- **Small verb sets.** tosi is four verbs and a diagnostic. A smaller surface is easier for an
  agent to use correctly than one that exposes every option.
- **Tool names are unique across the suite**, checked in CI, so toolsets can be combined in one
  server without renaming.
- **Typed inputs, JSON outputs.** Inputs are declared schemas. Outputs are JSON documents.
- **Contracts.** A caller can ask for a result in a declared schema; the provider applies it
  natively and the service validates it locally before returning it. Invalid or oversized
  structured output fails the task rather than being passed on. Named, versioned contracts
  (`review`, `implementation`, `investigation`) and input contracts are planned (#84).
- **Errors are data.** A failure carries a kind (for example an expired login versus a missing
  one) and the action the user should take, not only prose (#67).
- **Historical text is untrusted.** Transcripts, excerpts, and summaries can contain
  instructions. Every tool and resource that returns them says they are data, not commands.
- **Redaction at the output boundary.** Secrets are replaced with `[redacted:<kind>]` before any
  text leaves a service, including worker results. One implementation, in core.
- **Bounded responses.** Long results page (`offset`, `next_offset`); work that could run long has
  a time budget and reports what it did not finish.
- **Completeness is reported.** A search returns whether it was exhaustive and whether the
  absence of a match is proof, with counts of what was left out and why. Saying "not proof" is
  always allowed; claiming proof falsely is a bug.
- **Cost is disclosed.** Tools that spend model tokens say so in their description and report
  cost and usage in the result.
- **stdout is the protocol channel** for stdio servers. Diagnostics go to stderr, enforced by a
  check.

### Instructions and resources

Each server publishes short instructions that say what exists and the rules for using it
(untrusted text, cost, completeness). Longer guidance is published as resources with the same
text the plugin ships as skills, so a client without plugin support gets identical guidance.

### Transports and security

Today each host starts each server over stdio. The shared server (#80) will use HTTP on the
loopback interface only, check the request origin, and require a bearer token read from a file
private to the user, because a local endpoint that can start writing workers must not be open to
other local processes or web pages.

## Skill surface

Three layers, each loaded only when needed, because context is the scarce resource:

| Layer | Purpose | When it enters context |
|---|---|---|
| Kickoff skill (`recall`, `delegate`) | When and how to use the service, in a few hundred words | when the user invokes it, or when the agent loads it because the task matches its description; once per session is enough |
| Server instructions | What exists and the rules (untrusted text, cost, completeness) | every session, briefly |
| Reference resources | Detailed patterns (parallel tasks, implementation and review, follow-ups, model and effort, consulting an earlier session) | only the one a task needs |

Skills are written to work in either CLI: they name tools by function, not by host-specific
prefix.

Delegated workers do not load skills or project instructions themselves, so whatever they need
must be in their brief. Profiles (#85) package that: a named role (reviewer, developer,
researcher) that bundles instructions, default backend, model, and effort, access mode, and
contracts. With several orchestrators sharing a backlog, a profile also carries the claim
protocol: a draft pull request linked to the issue and an in-progress label, checked again
immediately before claiming.

## Plugin design

### One marketplace, one plugin per service

A single repository holds the services, the plugins, and the catalog that both hosts install
from. Each service is its own plugin, so a user installs only what they want and each one adds
only its own context.

### One directory serves both hosts

A plugin directory carries both hosts' files side by side:

| | Claude Code | Codex |
|---|---|---|
| Manifest | `.claude-plugin/plugin.json` | `plugin.json` (Agent Plugins 1.0) |
| MCP servers | `.mcp.json` | `mcp.json`, with a plugin-relative `./bin/<tool>` command |
| Skills | `skills/<name>/SKILL.md` | the same files |
| Hooks | a file named in the Claude manifest | not loaded from plugins; a user-level hooks file instead |

The portable files (`plugin.json`, `mcp.json`, `skills/`) conform to Agent Plugins 1.0 and are
validated against its schemas. Two deliberate deviations: the Claude-specific files sit at the
plugin root, because Claude Code requires them there, and the binary shim depends on the
environment to find the installed binary, because the plugin does not bundle one.

Plugins carry no version field. Claude Code then tracks the repository commit, and Codex refreshes
its copy when the plugin is added again.

### The plugin is a build output

Each service is a regular library and binary. Today a `bin/` shim in the plugin runs the binary
that `cargo install` put on the machine. The plan (#61) is one command that builds the binaries,
generates every plugin directory from crate metadata and checked-in templates (so host quirks
stay reviewable as files), writes the catalog, and refreshes each installed host. The same
generator is meant to package other MCP servers the same way. Shipping to people without a build
toolchain would add prebuilt binaries and is deliberately out of scope for now.

### Configuration

Settings are environment variables today. Claude Code's per-plugin user configuration can expose
them where available (#60); Codex has no equivalent, so its defaults stay environment-driven.

## State

Every service keeps its state under one root:

```
~/.bottega/                 BOTTEGA_HOME overrides the root
├── shared/                 state more than one service touches
│   ├── spawned.json        provenance: written by tosi, read by trovador
│   ├── contracts/          named input and output contracts (planned)
│   └── profiles/           user profiles (planned)
├── trovador/               index, summary cache, run log
├── tosi/                   task registry (planned)
└── facts/                  shared memory (planned)
```

- One function in the core library resolves every location, and resolving a path never touches
  the filesystem. Each binary prepares its state explicitly at startup.
- Each service owns its directory; anything two services touch lives in `shared/`. There is no
  shared database.
- Directories are private to the user (0700) and files 0600, because they hold transcript text.
- Removing `~/.bottega` removes everything bottega wrote.
- Tests build the entire tree under one temporary root, and a guard fails the run if any
  resolved path escapes it.
- Moving from an older layout is a one-time migration with a single commit point, safe against
  concurrent processes and resumable from a crash at any step. It never overwrites, never merges,
  and reports anything it cannot resolve through `doctor`.

## How delegation is used

The pattern that emerged from running real batches through tosi:

- **An orchestrator and single-use workers.** The calling session plans, claims, and integrates;
  workers do one bounded task each.
- **Write in isolation.** Each writing worker gets its own git worktree; parallel writers never
  share one. Making that the default is planned (#68).
- **Author, review, follow up.** One backend implements, the other reviews read-only against a
  findings contract, and findings go back to the author as a follow-up. The orchestrator reads
  the final change and runs the project's gates itself before merging. In practice the reviews
  caught defects that tests did not, including data-loss paths.
- **Read-only fan-out.** Stage large inputs as files in the worker's working directory, have the
  worker return a structured result, and let the orchestrator perform side effects (posting a
  review, merging). Read-only workers cannot and should not act on the world.
- **Durable, labeled tasks.** Task state outlives the process that started it, and tasks carry
  labels so a batch can be recovered after a crash (#82, #84).

## Status and direction

| Area | Done | Next | Later |
|---|---|---|---|
| History (trovador) | search, read, summarize; provenance; 22 bug fixes from review and dogfooding | concurrent index access (#14, #16); mid-session PR links (#17); secret handling in search and summaries (#12, #24) | context-fullness report (#77); new session from summaries (#78) |
| Delegation (tosi) | four verbs, schemas, redaction, diagnostics | durable registry (#82); named contracts and labels (#84); profiles (#85); structured login failures (#67); worktrees by default (#68); `tosi run` (#86) | workflows over contracts (#56, #57); routing (#54, #55) |
| Platform | one repo, plugins for both hosts, shared core, shared state directory | one-command install (#61); settings via host config (#60) | shared server for both hosts (#80) |
| Memory | | | facts from the existing spike (#87) |

The rule for anything new: one piece at a time, each in daily use before the next starts.

## Implementation notes

Today's implementation, for orientation only: Rust in one Cargo workspace (`bottega-core`,
`trovador`, `tosi`), MCP through `tower-mcp`, CLI wrappers for both providers, SQLite with FTS5
for trovador's index, and plugins in the Agent Plugins 1.0 and Claude Code formats. Any of these
can change without changing the architecture above.

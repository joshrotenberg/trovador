# Working in this repository

Read this before changing anything. It is the whole convention set.

`CLAUDE.md` is a symlink to this file, so Claude Code and Codex read the same thing.

## What this is

A local CLI and MCP server for searching and reading every Claude Code and Codex session
on the machine. The noun is a session, not an agent: an agent is stateless between
invocations, a session is what accumulates, and both CLIs already persist them on disk.

It only reads, and it runs no model. A caller that wants a summary reads `sessions text`
and summarizes it itself. Starting or continuing agent work belongs to another tool; do
not add it here.

This is the Rust successor to [spike](https://github.com/joshrotenberg/spike), published
there as `agent-sessions`. It spent a while as `crates/trovador` in a workspace called
bottega; that history, with the fixes made there, is on the `archive/bottega` branch. spike's test suite and README are the behavioural reference for
anything ported from it; where trovador deliberately differs, say so in the PR.

`README.md` is canonical for what exists. Keep it current in the same change that alters
behaviour, not afterwards.

## Two properties worth defending

**Seamless in and out.** trovador reads what the CLIs already write and never writes to
`~/.claude` or `~/.codex`. Everything it owns lives under `~/.trovador`, so removing that
directory removes trovador and leaves the user's sessions untouched.

**Both doors agree.** `src/lib.rs` is the library. `trovador <command>` and
`trovador serve` (MCP over stdio) are thin adapters over it. spike had four separate bugs
from the MCP tool and the CLI computing the same answer differently. If a projection is
needed by both doors, it lives in the library and both call it.

## Ground rules

- **Branch, always.** Never commit to `main`.
- **Open a draft PR when you create the branch**, before implementing. The body is the plan.
  The commit stream is the work. Mark it ready when it is done and green.
- **Before every push:** `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`. If you cannot run them, say so plainly in the PR.
- **Never change git author or committer**, and never add a `Co-Authored-By` or any other
  attribution trailer.
- **No AI attribution** in commits, PRs, issues, or code comments.

## Writing

Applies to commits, PR bodies, issues, README, docs, and code comments alike.

- **No em dashes.** Anywhere.
- **State facts.** No editorialising, no marketing tone. Describe what changed and why.
- **Conventional-commit prefixes** on commit messages and PR titles.
- **Claims must be measured.** A performance claim carries the number and how it was obtained.

## Comments

Comments explain **why**, not what. A comment earns its place by recording a decision, a
constraint, or a failure that motivated the shape. Where a bug prompted the code, name the
bug, including spike issue numbers where the lesson came from there.

## Code

- **stdout is the JSON-RPC stream** under `trovador serve`. Nothing in the library prints to
  stdout. Diagnostics go through `tracing`, which writes to stderr.
- **A stdio server is spawned once per client and never reloads.** After changing the code,
  the CLI is the only door running it until the client restarts.
- **Every tunable lives in `src/limits.rs`**, is overridable by a `TROVADOR_*` environment
  variable, and is reported by `trovador doctor`. These numbers are context budgets: most of
  what trovador returns ends up in some agent's context window.
- **Unknown records are kept as `omitted` with a category**, never silently dropped. Provider
  formats change without notice.

## Tests

- `cargo test` is deterministic: no network, no model spend, fixture archives built in temp
  directories by `tests/common/mod.rs`, including zstd-compressed Codex rollouts.
- **The suite never touches real state.** Every path comes from a `Paths` value built from a
  temp directory. Nothing in the library falls back to the real home directory except
  `Paths::from_env`, which tests never call.
- **The suite must survive being run concurrently.** Scratch paths come from `tempfile`,
  never from timestamps.
- A test name says what would break. Adding a feature without assertions is not done, and
  deleting assertions to make a build green is never the fix.

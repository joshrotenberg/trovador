# bottega

Personal tools for Claude Code and Codex, and the plugin marketplace that installs them
in both. One repository holds the tools' source (`crates/`), the plugins (`plugins/`),
and the catalog (`.claude-plugin/marketplace.json`).

| Plugin | What it gives an agent | Source |
|---|---|---|
| `trovador` | Search, read, and summarize every past Claude Code and Codex session on the machine | [`crates/trovador`](crates/trovador) |
| `tosi` | Delegate bounded tasks to local Claude and Codex workers: spawn, wait, continue, cancel | [`crates/tosi`](crates/tosi) |

`crates/` is one Cargo workspace. Both services use bottega-core for the record of sessions tosi starts,
so trovador's list and search include them. Past sessions are read-only history: agents
read, search, and summarize them, and nothing in the toolkit continues or forks one. Both
tools came from repositories of their own, now archived; their history is kept here.

[`docs/architecture.md`](docs/architecture.md) describes the design behind all of this, done and planned:
the principles, the services, the MCP and skill surfaces, the plugin design, and the state
layout, independent of the implementation.

## How it fits together

Nothing is released. A plugin directory holds manifests, skills, hooks, and a `bin/`
shim. The shim runs the tool that `cargo install` put in `~/.cargo/bin`
(or `$<TOOL>_BIN`, or `PATH`), and prints the install command if it is missing.

A tool that also serves its skills as MCP resources keeps them in its crate's
`resources/skills/`, and `scripts/sync-skills.sh <plugin>` copies them into the plugin.
`check.sh` fails when a copy has drifted. trovador's skill is written in its plugin.

Each plugin directory carries both hosts' files side by side:

| | Claude Code | Codex |
|---|---|---|
| Manifest | `.claude-plugin/plugin.json` | `plugin.json` (Agent Plugins 1.0) |
| MCP servers | `.mcp.json`, paths from `${CLAUDE_PLUGIN_ROOT}` | `mcp.json`, a `./bin/<tool>` command relative to the plugin |
| Hooks | `hooks/claude.json`, named in the manifest | not loaded from plugins; see [Codex hook](#codex-hook) |
| Skills | `skills/<name>/SKILL.md` | the same files |

Hooks stay out of the default `hooks/hooks.json` path because Codex also reads that
file; a shared file would run the Claude hook in Codex sessions.

Plugins carry no `version`. Claude Code then uses the git commit and reads the plugin
straight from this checkout, so every commit is an update. Codex copies each plugin into
`~/.codex/plugins/cache/` as version 1.0.0; adding it again copies the current files.

Codex accepts `${PLUGIN_ROOT}` in an MCP server's `command` without error but does not
start the server; a relative `./bin/<tool>` command works.

Codex 0.162 loads no hooks from plugins. Its app server lists none for any plugin,
including OpenAI's bundled ones, and `codex features list` reports `plugin_hooks` as
removed. trovador's SessionStart hook therefore runs only in Claude Code unless you add
it to Codex yourself (below). Without it, search in a Codex session leaves out every
recently written session instead of only the current one.

## Set up a machine

`rm -rf ~/.bottega` removes suite state and leaves provider sessions untouched.

The repository is private, so cargo fetches with the git CLI, which uses your GitHub
credentials. From a clone, `cargo install --path crates/<tool>` works too.

```sh
export CARGO_NET_GIT_FETCH_WITH_CLI=true
cargo install --locked --git https://github.com/joshrotenberg/bottega trovador
cargo install --locked --git https://github.com/joshrotenberg/bottega tosi
gh repo clone joshrotenberg/bottega ~/Code/github.com/joshrotenberg/bottega
```

Claude Code:

```sh
claude plugin marketplace add ~/Code/github.com/joshrotenberg/bottega
claude plugin install trovador@bottega
claude plugin install tosi@bottega
```

Codex:

```sh
codex plugin marketplace add ~/Code/github.com/joshrotenberg/bottega
codex plugin add trovador@bottega
codex plugin add tosi@bottega
```

### Codex hook

Optional. The hook tells a Codex session its own id, so trovador's search can leave out
only that session. Codex does not load it from the plugin, so it goes in
`~/.codex/hooks.json`, with the absolute path to the installed binary:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/Users/you/.cargo/bin/trovador hook session-start --backend codex || true"
          }
        ]
      }
    ]
  }
}
```

Then open `codex`, run `/hooks`, and trust it under SessionStart. This file is outside
the plugin: removing the plugin does not remove it, so delete the entry yourself.

### Earlier registrations

If a tool was registered some other way before (`claude mcp add`, an
`[mcp_servers.<name>]` table in `~/.codex/config.toml`, or another local marketplace),
remove that registration so its tools are not listed twice.

## Update

```sh
git -C ~/Code/github.com/joshrotenberg/bottega pull
cargo install --locked --path ~/Code/github.com/joshrotenberg/bottega/crates/<tool>
```

Then update the plugin in each host (`claude plugin update <tool>@bottega`; in Codex,
add it again) and restart both host apps. A running MCP server never reloads.

## Check

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
scripts/check.sh
```

`check.sh` parses every JSON file, runs `claude plugin validate` on the marketplace and
each plugin when `claude` is installed, compares synced skills with their crates, and
starts each shim with `--version`. CI runs all four on Linux, and the Rust checks on
macOS.

`crates/bottega-core` is the shared library for redaction, provenance, backend names,
private state files, and state locations. Both services depend on it.

State lives under `BOTTEGA_HOME` or `~/.bottega`: `shared/` holds provenance and its lock,
`trovador/` holds the index, summaries, and run log. `tosi/` is reserved and is not
created yet. Non-empty `TROVADOR_HOME` overrides only trovador's directory; empty
overrides are ignored. Provenance always goes to `<root>/shared/`, including when
only `TROVADOR_HOME` is set; preparation moves its legacy provenance there without
overwriting existing files. Directories are 0700 and files are 0600.

The first run after upgrading migrates `~/.trovador` when neither override is set
and the new trovador directory does not exist. Restart both host apps after upgrading
so no old server keeps writing to the old location. Conflicts and migration failures
are reported by `trovador doctor`; existing directories are never merged.

Path resolution is pure: `state::resolve`, `state::from_env`, and `Paths` construction
compute paths without creating or migrating state. Only binary entry points call
`state::prepare`; tests call it explicitly with temporary roots.

Migration commits by renaming the old directory to the new service directory.
Across filesystems it verifies a partial copy against the source path, size, and
mtime manifest before committing, then retains `~/.trovador.migrated` for manual
removal. An incomplete partial copy is retried on startup. Shared-file moves resume
after a crash. Recursive permission tightening is best effort and runs once on the
newly committed migration tree. Ordinary starts tighten only the state directories
themselves and preserve modes of their existing contents, including override roots.
A source changing during copy fails the command: retry shortly or stop the old
server before retrying. Stable migration failures can still use the old location.
Doctor inspects conflicts, partial copies, and retired sources on every call.

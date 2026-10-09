# bottega

Personal tools for Claude Code and Codex, and the plugin marketplace that installs them
in both. One repository holds the tools' source (`crates/`), the plugins (`plugins/`),
and the catalog (`.claude-plugin/marketplace.json`).

| Plugin | What it gives an agent | Source |
|---|---|---|
| `trovador` | Search, read, and summarize every past Claude Code and Codex session on the machine | [`crates/trovador`](crates/trovador) |
| `tosi` | Delegate bounded tasks to local Claude and Codex workers: spawn, wait, continue, cancel | [`crates/tosi`](crates/tosi) |

`crates/` is one Cargo workspace. tosi links trovador to record each session it starts,
so trovador's list and search include them. Past sessions are read-only history: agents
read, search, and summarize them, and nothing in the toolkit continues or forks one. Both
tools came from repositories of their own, now archived; their history is kept here.

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
| Hooks | `hooks/claude.json`, named in the manifest | `extensions["com.openai"].hooks` in `plugin.json` |
| Skills | `skills/<name>/SKILL.md` | the same files |

Hooks stay out of the default `hooks/hooks.json` path because Codex also reads that
file; a shared file would run the Claude hook in Codex sessions.

Plugins carry no `version`. Claude Code then uses the git commit and reads the plugin
straight from this checkout, so every commit is an update. Codex copies each plugin into
`~/.codex/plugins/cache/` as version 1.0.0; adding it again copies the current files.

Codex accepts `${PLUGIN_ROOT}` in an MCP server's `command` without error but does not
start the server; a relative `./bin/<tool>` command works.

Codex runs a plugin's hooks only after you trust them: open `codex`, run `/hooks`, and
trust the plugin's hooks. Until then trovador's SessionStart hook is skipped and search
falls back to leaving out every recently written session.

## Set up a machine

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

If a tool was registered some other way before (`claude mcp add`, an
`[mcp_servers.<name>]` table in `~/.codex/config.toml`, or another local marketplace),
remove that registration so its tools are not listed twice.

## Update

```sh
git -C ~/Code/github.com/joshrotenberg/bottega pull
cargo install --locked --path ~/Code/github.com/joshrotenberg/bottega/crates/<tool>
```

Then update the plugin in each host (`claude plugin update <tool>@bottega`; in Codex,
add it again) and start a new session. A running MCP server never reloads.

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

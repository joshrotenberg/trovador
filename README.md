# bottega

A personal marketplace of Claude Code and Codex plugins. This repository is the
marketplace: a catalog in `.claude-plugin/marketplace.json` and one directory per
plugin under `plugins/`. Both hosts read the same checkout.

| Plugin | What it gives an agent | Source |
|---|---|---|
| `trovador` | Search, read, and summarize every past Claude Code and Codex session on the machine | [joshrotenberg/trovador](https://github.com/joshrotenberg/trovador) |
| `tosi` | Delegate bounded tasks to local Claude and Codex workers: spawn, wait, continue, cancel | [joshrotenberg/tosi](https://github.com/joshrotenberg/tosi) |

## How it fits together

Nothing here is built or released. A plugin directory holds manifests, skills, hooks,
and a `bin/` shim. The shim runs the tool that `cargo install` put in `~/.cargo/bin`
(or `$<TOOL>_BIN`, or `PATH`), and prints the install command if it is missing.
Updating a tool is `cargo install`; updating a plugin is a commit here.

A tool that also serves its skills as MCP resources keeps them in its own
`resources/skills/`, and `scripts/sync-skills.sh <plugin>` copies them here. `check.sh`
fails when a copy has drifted from a local checkout of the tool. trovador's skill is
written here directly.

Each plugin directory carries both hosts' files side by side:

| | Claude Code | Codex |
|---|---|---|
| Manifest | `.claude-plugin/plugin.json` | `plugin.json` (Agent Plugins 1.0) |
| MCP servers | `.mcp.json`, paths from `${CLAUDE_PLUGIN_ROOT}` | `mcp.json`, paths from `${PLUGIN_ROOT}` |
| Hooks | `hooks/claude.json`, named in the manifest | `extensions["com.openai"].hooks` in `plugin.json` |
| Skills | `skills/<name>/SKILL.md` | the same files |

Hooks stay out of the default `hooks/hooks.json` path because Codex also reads that
file; a shared file would run the Claude hook in Codex sessions.

Plugins carry no `version`. Claude Code then uses the git commit, so every commit is
an update.

## Set up a machine

Both repositories are private, so cargo fetches with the git CLI, which uses your
GitHub credentials.

```sh
export CARGO_NET_GIT_FETCH_WITH_CLI=true
cargo install --locked --git https://github.com/joshrotenberg/trovador
cargo install --locked --git https://github.com/joshrotenberg/tosi
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
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked --git https://github.com/joshrotenberg/<tool>
```

Then update the plugin in each host (`claude plugin update <tool>@bottega`; in Codex,
add it again) and start a new session. A running MCP server never reloads.

## Check

```sh
scripts/check.sh
```

Parses every JSON file, runs `claude plugin validate` on the marketplace and each
plugin, compares synced skills with the tool checkouts, and starts each shim with
`--version`.

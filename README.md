# bottega

A personal marketplace of Claude Code and Codex plugins. This repository is the
marketplace: a catalog in `.claude-plugin/marketplace.json` and one directory per
plugin under `plugins/`. Both hosts read the same checkout.

| Plugin | What it gives an agent | Source |
|---|---|---|
| `trovador` | Search, read, and summarize every past Claude Code and Codex session on the machine | [joshrotenberg/trovador](https://github.com/joshrotenberg/trovador) |

## How it fits together

Nothing here is built or released. A plugin directory holds manifests, skills, hooks,
and a `bin/` shim. The shim runs the tool that `cargo install` put in `~/.cargo/bin`
(or `$TROVADOR_BIN`, or `PATH`), and prints the install command if it is missing.
Updating a tool is `cargo install`; updating a plugin is a commit here.

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
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked --git https://github.com/joshrotenberg/trovador
gh repo clone joshrotenberg/bottega ~/Code/github.com/joshrotenberg/bottega
```

Claude Code:

```sh
claude plugin marketplace add ~/Code/github.com/joshrotenberg/bottega
claude plugin install trovador@bottega
```

Codex:

```sh
codex plugin marketplace add ~/Code/github.com/joshrotenberg/bottega
codex plugin add trovador@bottega
```

If trovador was registered directly as an MCP server before (`claude mcp add`,
`[mcp_servers.trovador]` in `~/.codex/config.toml`), remove that entry so the tools
are not listed twice.

## Update

```sh
git -C ~/Code/github.com/joshrotenberg/bottega pull
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked --git https://github.com/joshrotenberg/trovador
```

Then update the plugin in each host (`claude plugin update trovador@bottega`; in Codex,
add it again) and start a new session. A running MCP server never reloads.

## Check

```sh
scripts/check.sh
```

Parses every JSON file, runs `claude plugin validate` on the marketplace and each
plugin, and starts each shim with `--version`.

## Planned

- The subagent toolset now called task-runners (spawn, wait, continue, cancel), under
  a new name when it moves here.

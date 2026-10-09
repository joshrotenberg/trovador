# Working in bottega

`CLAUDE.md` is a symlink to this file.

This repository holds personal tools for Claude Code and Codex (`crates/`, one Cargo
workspace) and the plugin marketplace that installs them (`plugins/`,
`.claude-plugin/marketplace.json`). Each crate has its own AGENTS.md with rules for
working in it; read it before changing that crate.

## Rules

- Branch and open a draft PR before changing anything. Never commit to `main`.
- Before every push: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace`, and `scripts/check.sh`.
- Every plugin directory has both hosts' files: `.claude-plugin/plugin.json` and
  `.mcp.json` for Claude Code, `plugin.json` and `mcp.json` (Agent Plugins 1.0) for
  Codex. A change to one host's manifest needs the matching change to the other.
- In Codex `mcp.json`, use a relative `./bin/<tool>` command. With `${PLUGIN_ROOT}` in
  `command`, Codex installs the plugin but never starts the server.
- Keep hooks out of `hooks/hooks.json`: Codex reads that path too. Claude hooks go in a
  file the Claude manifest names; Codex hooks go in `extensions["com.openai"].hooks`.
- Skills a tool serves itself (its crate's `resources/skills/`) are copied into its
  plugin with `scripts/sync-skills.sh`; edit them in the crate, never in `plugins/`.
- No `version` fields. Claude Code uses the git commit instead.
- Skills are shared by both hosts. Write them for either agent: name MCP tools by their
  tool names, never by a host-specific prefix.
- No em dashes, no AI attribution, conventional-commit prefixes.

# Working in bottega

`CLAUDE.md` is a symlink to this file.

This repository is a plugin marketplace for Claude Code and Codex. It holds manifests,
skills, hooks, and shims; the tools themselves live in their own repositories and are
installed with `cargo install`.

## Rules

- Branch and open a draft PR before changing anything. Never commit to `main`.
- Run `scripts/check.sh` before every push.
- Every plugin directory has both hosts' files: `.claude-plugin/plugin.json` and
  `.mcp.json` for Claude Code, `plugin.json` and `mcp.json` (Agent Plugins 1.0) for
  Codex. A change to one host's manifest needs the matching change to the other.
- Keep hooks out of `hooks/hooks.json`: Codex reads that path too. Claude hooks go in a
  file the Claude manifest names; Codex hooks go in `extensions["com.openai"].hooks`.
- No `version` fields. Claude Code uses the git commit instead.
- Skills are shared by both hosts. Write them for either agent: name MCP tools by their
  tool names, never by a host-specific prefix.
- No em dashes, no AI attribution, conventional-commit prefixes.

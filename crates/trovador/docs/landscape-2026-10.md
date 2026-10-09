# Landscape, October 2026

Tools that work with local coding-agent session history. Checked on 2026-10-07 against
each repository's README, documentation, and GitHub metadata. Star counts are from that
date. Anything that could not be checked against the repository is marked *unverified*.

## Viewers and search

| Tool | Form | Agents | Search | Runs or resumes | API | Notes |
|---|---|---|---|---|---|---|
| [Agent Sessions](https://github.com/jazzyalex/agent-sessions), 893 stars | macOS app (Swift) | 16 | SQLite FTS5 | copies a resume command or opens a terminal | none (its own competitive doc lists an MCP server as a candidate) | quota and cost meters; per-format fixtures, stewards, and "Session-Bench" format gates |
| [AgentsView](https://github.com/kenn-io/agentsview), 6,072 stars | Go daemon, web UI, Tauri app, CLI | 40+ | FTS5, opt-in semantic and hybrid | builds a resume command and launches a terminal | read-only MCP (stdio and HTTP) | LLM insights, "Recall" lessons, cost, sync to Postgres, DuckDB, ClickHouse, S3 |
| [ccrider](https://github.com/neilberkman/ccrider), 132 stars | Go CLI, TUI, MCP | 7 | incremental FTS5 | TUI runs resume or fork | MCP | keeps content after Claude's cleanup deletes it; `generate_session_anchor` |
| [showagent](https://github.com/aytzey/showagent), 51 stars | Go TUI, CLI, MCP | 6 | fuzzy, first and latest prompts only | converts a session into another agent's format | MCP with `--read-only` | secrets redacted by default; transcripts marked as untrusted |
| [claude-code-log](https://github.com/daaain/claude-code-log), 1,239 stars | Python CLI, TUI | Claude, Codex (beta) | full text in `serve` mode | no | no | HTML and Markdown export, watch mode |
| [Claude Code History Viewer](https://github.com/jhlee0409/claude-code-history-viewer), 2,225 stars | Tauri app, web server | 31 claimed | yes | no | no | analytics |
| [ai-sessions-mcp](https://github.com/yoavf/ai-sessions-mcp), 30 stars | Go MCP | 6 | yes | no | MCP | optional upload to a sharing site |
| [claude-historian-mcp](https://github.com/Vvkmnn/claude-historian-mcp), 179 stars | TypeScript MCP, skill | Claude | scans files on demand | no | MCP | a skill tells Claude to check history before searching the web |

## Usage meters

[ccusage](https://github.com/ccusage/ccusage) (18.9k stars) reports tokens and cost for 17
agents. [CodexBar](https://github.com/steipete/CodexBar) (22.3k stars) is a menu-bar quota
meter. Both are outside trovador's scope.

## Handoff and consultation

- [continues](https://github.com/yigitkonur/cli-continues) (1,561 stars) hands a session
  from one of 16 agents to another. It writes recent messages, file changes, and tool
  activity into a handoff document and launches the target CLI with it. `--debug-prompt`
  prints the prompt without launching.
- [PAL MCP](https://github.com/BeehiveInnovations/pal-mcp-server) (11.8k stars, last push
  2025-12) consults other models over their APIs. Its `clink` tool runs Codex, Gemini, or
  Claude CLIs as subagents. It works on live conversations, not on the archive.
- Smaller bridges wrap one CLI for another: `tuannvm/codex-mcp-server`,
  `Chao-Shiun/claude-consult-mcp`. Both CLIs also ship their own MCP servers (`claude mcp
  serve`, `codex mcp-server`). One write-up reports that `codex mcp-server` did not return
  conversation ids. *Unverified.*

## Orchestrators

[claude-squad](https://github.com/smtg-ai/claude-squad),
[ccmanager](https://github.com/kbwo/ccmanager),
[vibe-kanban](https://github.com/BloopAI/vibe-kanban), and
[opcode](https://github.com/winfunc/opcode) (dormant since 2025-10) run live sessions in
terminals or worktrees. None of them indexes or searches history.

## Where trovador stands

**Shared with others:** an FTS5 index over Claude and Codex archives, and a paged
transcript over MCP. AgentsView covers most of the read side, for 40 agents.

**Not found elsewhere:**
- Running a turn headlessly against an archived session, from MCP or the CLI: starting,
  continuing, or forking it, and getting the reply back as data. The viewers open a
  terminal instead, and the consult bridges have no archive.
- A consent policy deciding which existing sessions may be continued.
- `about`: summarizing a past session and asking the other backend about it.
- Exhaustiveness reported with every search. AgentsView's `coverage` object is the
  nearest equivalent.

**What others have that trovador lacks:** more agents, terminal launch, cost reporting,
export, semantic search, and retention past the provider's cleanup.

**Worth borrowing:**
- AgentsView: exclude the current session and recently active sessions from search by
  default. Exclude one-shot and automated sessions by default. Offer a slim tool profile.
- showagent: redact secrets by default, with the opt-out set only when the server starts.
  Mark transcripts as untrusted historical data.
- ccrider: keep content after the provider deletes it.
- continues: a dry run that prints the prompt a handoff or consult would send.

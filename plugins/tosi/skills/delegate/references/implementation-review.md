# Implementation and review

Use this pattern for an authorized change that benefits from a separate review.
An independent reviewer should receive the problem, intended behavior, scope,
and validation criteria, rather than being told to agree with the implementer.

1. Read the repository's instructions and prepare an explicit implementation
   brief. Include the allowed files, desired behavior, constraints, and tests or
   other evidence that would demonstrate success. Claude workers skip automatic
   CLAUDE.md loading, so include applicable instructions in the brief. Codex keeps
   its native AGENTS.md handling. Neither inherits the parent conversation.
2. Run the implementer with `workspace_write` in the chosen directory. Do not
   have multiple writers modify the same files simultaneously. A worktree must
   already exist if one is requested; this server does not create it.
3. Inspect the returned result and the actual diff. Run appropriate validation
   through a provider that supports it or through the parent host's tools.
4. Start a separate `read_only` review with the relevant diff/files and original
   requirements. Request actionable findings with file references and an
   explanation of impact. Use a user-specified backend/model/effort when given;
   do not silently substitute one. Synthesize the review and make necessary corrections
   within the authorized scope.

Provider access is deliberately different:

- Claude `read_only` offers Read, Glob, and Grep. `workspace_write` additionally
  offers Edit, Write, and NotebookEdit with `acceptEdits`. Neither mode exposes
  Bash, so Claude cannot run tests or shell commands through this adapter. These
  are tool restrictions, not an operating-system filesystem sandbox. The working
  directory does not establish a filesystem security boundary.
- Codex uses its native read-only or workspace-write sandbox with approval
  requests disabled. User config and plugins are disabled; project or managed
  MCP configuration may still apply. Tosi refuses nested server startup
  in worker processes.

Both use authentication available to the installed CLI in the server's launch
context. Claude disables installed customizations, uses an explicit tool list,
and loads an empty strict MCP configuration.

If a worker reports an executable or authentication error, call
`check_environment` for its backend through the installed tools. Check its
executable/version, environment and configuration paths, and auth status before
recommending login. `unavailable` or `unknown` describes that process, not a
confirmed account-wide logout. The diagnostic makes no model call and exposes
credential-variable presence only, never their values.

Tosi starts plain processes without loading a login shell. If necessary,
configure absolute `TOSI_CLAUDE_BIN` / `TOSI_CODEX_BIN` paths and
consistent `HOME`, `CLAUDE_CONFIG_DIR`, and `CODEX_HOME` in the host environment.
On macOS, Claude's credential storage uses Keychain, so access to that store is
one possible cause of different auth status across launch contexts. See
[Claude credential management](https://code.claude.com/docs/en/authentication#credential-management).

A child cannot repair an inherited sandbox. For intentional standalone clients,
use the host's configured launch path and normal approvals. Do not replace
unavailable installed tools with a shell bridge, extract credentials, restore
bypass flags, or broaden permissions to overcome a denial. Report the evidence
and any uncertainty; recommend login only when checks in the intended host
context support that diagnosis.

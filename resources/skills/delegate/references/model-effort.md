# Model and effort

Use this reference when choosing `backend`, `model`, or optional `effort` for
`spawn_task`. Honor explicit user choices. If a choice is unsupported, report
the incompatibility instead of silently substituting a provider or setting.

Omit `backend` to use the server's configured default. Omit `model` or `effort`
to leave that option to the provider CLI, including applicable defaults and
inherited environment. These omissions do not inherit the calling chat's model
or effort. Worker configuration is isolated as described in the implementation
reference; do not assume every saved CLI setting is loaded.

| Backend | Exposed effort values | Forwarding |
| --- | --- | --- |
| Claude | `low`, `medium`, `high`, `xhigh`, `max` | Claude Code `--effort` |
| Codex | `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, `ultra` | Codex `model_reasoning_effort` configuration |

Values are model-dependent. Unknown enum values, unsupported backend/value
combinations, and incompatible combinations for exact models in the maintained
capability table are rejected before a worker starts, including recognized dated
and context forms. Aliases and unknown model
IDs are forwarded with compatibility unverified; provider acceptance still
depends on the installed CLI, selected model, and account. The table is a dated
compatibility check, not a universal catalog or guarantee of model availability.
It lives at `resources/model-capabilities.json` in the source checkout and is
embedded in the executable. Validation makes no CLI or API model request.

When the user leaves the choice to you, prefer lower effort for bounded mechanical
work with clear acceptance criteria. Consider higher effort for ambiguous
investigations or difficult reviews where additional reasoning may help. Balance
latency and usage against the task; identical labels do not represent equal
compute, cost, or quality across providers. Task difficulty does not imply a
particular model family. No task classification, routing rules, or round robin
are implemented yet.

Do not automatically select Codex `ultra`: the provider advertises it with
automatic delegation behavior. Honor an explicit user choice when compatible,
while retaining the authorized task scope and access restrictions. Effort is not
a token or spending cap; `timeout_secs` bounds duration only.

Task snapshots expose `backend`, `model`, and `effort`, plus `selection`:

- `requested`: the caller's backend/model/effort; omissions remain `null`.
- `selected`: the resolved backend and model/effort options sent to the adapter;
  omitted model/effort remain `null`.
- `reported`: model and effort remain `null`. The wrappers do not supply reliable
  per-turn evidence for these fields; do not reconstruct them from `modelUsage`,
  token counts, or result prose.

The provider CLI may resolve aliases, apply defaults or caps, or otherwise change
effective settings. Tosi does not downgrade, substitute, fall back, or
retry on its own, and cannot attest the effective settings. `continue_task`
retains the original backend/model/effort and output schema, with no override.
Start a fresh task to change them. Reusing an alias or an omitted setting does
not freeze the provider's underlying model or defaults across turns.

Compatibility evidence was checked on 2026-10-08 against Claude Code 2.1.293,
`claude-wrapper` 0.14.4, and Codex CLI 0.157.1's
`codex debug models --bundled` output. See
[Claude Code model configuration](https://code.claude.com/docs/en/model-config)
for provider-specific effort and model behavior. Refresh the maintained
capability table when updating providers; do not guess support for a new model.

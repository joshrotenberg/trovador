Check provider CLI discovery, versions, and authentication visible to this MCP
server. Optional backend selects Claude or Codex; omitted checks both. Returns
non-secret environment paths, credential-variable presence, and diagnostic hints.
Makes no model request and does not change authentication or permissions.

Use before delegation from a new host or after an authentication/startup failure.
An unavailable login describes this launch context, not necessarily the user's
terminal login. Inherited sandbox restrictions can hide credential-store access.
Status checks do not prove network access or successful model execution. Use
normal host approvals for any separate execution; do not extract credentials or
change sandbox settings to work around a denial.

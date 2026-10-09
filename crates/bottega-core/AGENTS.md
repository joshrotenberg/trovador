# Working in bottega-core

Follow the root AGENTS.md. This library holds redaction, provenance, backend names,
private-file helpers, and state location resolution shared by the services.

Keep it independent of MCP, archive parsing, and SQLite. Tests use explicit temporary
state directories and never resolve or write real user state.

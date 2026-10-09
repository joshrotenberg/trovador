# Working in bottega-core

Follow the root AGENTS.md. This library holds redaction, provenance, backend names,
private-file helpers, and state location resolution shared by the services.

Keep it independent of MCP, archive parsing, and SQLite. Tests use explicit temporary
state directories and never resolve or write real user state.

Resolve durable state only through `bottega-core::state`: `~/.bottega/shared` and
`~/.bottega/trovador`. Tests isolate the root with `BOTTEGA_HOME` or explicit
temporary inputs and assert that durable paths stay under their temporary root.

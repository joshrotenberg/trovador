# bottega-core

Shared library for trovador and tosi: redaction, provenance, backend names, private
files, and state locations. State stays in `TROVADOR_HOME` or `~/.trovador`.

Provenance accepts an explicit state directory so callers and tests control every
file it reads or writes. Trovador keeps its archive paths and provider binaries.

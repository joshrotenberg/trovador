# bottega-core

Shared library for trovador and tosi: redaction, provenance, backend names, private
files, and state locations.

Provenance accepts an explicit state directory so callers and tests control every
file it reads or writes. Trovador keeps its archive paths and provider binaries.

State lives under `BOTTEGA_HOME` or `~/.bottega`: `shared/` holds provenance and its lock,
`trovador/` holds the index, summaries, and run log. `tosi/` is reserved and is not
created yet. Non-empty `TROVADOR_HOME` overrides only trovador's directory; empty
overrides are ignored. Provenance always goes to `<root>/shared/`, including when
only `TROVADOR_HOME` is set; preparation moves its legacy provenance there without
overwriting existing files. Directories are 0700 and files are 0600.

The first run after upgrading migrates `~/.trovador` when neither override is set
and the new trovador directory does not exist. Restart both host apps after upgrading
so no old server keeps writing to the old location. Conflicts and migration failures
are reported by `trovador doctor`; existing directories are never merged.

Path resolution is pure: `state::resolve`, `state::from_env`, and `Paths` construction
compute paths without creating or migrating state. Only binary entry points call
`state::prepare`; tests call it explicitly with temporary roots.

Migration commits by renaming the old directory to the new service directory.
Across filesystems it verifies a partial copy against the source path, size, and
mtime manifest before committing, then retains `~/.trovador.migrated` for manual
removal. An incomplete partial copy is retried on startup. Shared-file moves resume
after a crash. Recursive permission tightening is best effort and runs once on the
newly committed migration tree. Ordinary starts tighten only the state directories
themselves and preserve modes of their existing contents, including override roots.
A source changing during copy fails the command: retry shortly or stop the old
server before retrying. Stable migration failures can still use the old location.
Doctor inspects conflicts, partial copies, and retired sources on every call.

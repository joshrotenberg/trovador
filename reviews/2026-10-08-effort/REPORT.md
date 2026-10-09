# Explicit effort controls: dual-backend review

Completed 2026-10-08. Optional provider effort, retained continuation settings,
requested/selected/reported metadata, and canonical model/effort guidance are
implemented. Both paid reviewers completed independently through the installed
Task Runners MCP tools. No runtime finding was reported; two documentation
findings were verified and corrected. This is bounded evidence, not a guarantee
of correctness.

## Scope and identities

The reviewed source snapshot is
`65b1c3fb29f8b6edb267c17bf3a314430e6f7dadafa04bd796b84f9082fac707`.
See [source manifest](source-manifest.json), [changes](changes.diff), and
[review brief](brief.md). The project has no Git repository; bounded source
copies and SHA-256 manifests identify the work without inventing a commit.

| Backend | Task ID | Outcome |
| --- | --- | --- |
| Claude | `93a427ce-5ae3-4307-956e-7f6ca58fe035` | Two documentation findings; no runtime findings |
| Codex | `602fa10e-aaeb-4b29-94de-686e1664aa5f` | No findings |

Both tasks were fresh, read-only, limited to 300 seconds, and given the same
snapshot, evidence, and structured findings schema. Model and effort were
omitted: the installed review-runner schema did not yet expose effort. Actual
model/effort cannot be inferred from those omissions or provider usage. Reviewers
did not edit, delegate, run tests, or use network. Codex was permitted read-only
shell inspection. No automatic paid retries or fallbacks were used.

Complete results, coverage, limitations, and provider usage are in
[review-results.json](review-results.json); launch records and the schema are in
[review-spawns.json](review-spawns.json) and [review-schema.json](review-schema.json).
Usage fields are preserved as reported; no combined monetary total is inferred.

## Findings and disposition

| Finding | Verified correction |
| --- | --- |
| Claude P3: roadmap still describes effort as unimplemented | Updated current status and milestone 2; milestone 5 records completed selection guidance and remaining routing guidance. |
| Claude P3: README link uses a source-only path in the generated package | Replaced the link with explicit source/package paths and the stable MCP resource URI. |

Only README.md and ROADMAP.md changed after the reviewed source snapshot. Their
diff and identities are in [post-review-docs.diff](post-review-docs.diff) and
[post-review-changes.json](post-review-changes.json). These prose corrections did
not warrant another paid review. Executable source and embedded resources stayed
unchanged after review. The shared binary/private marketplace idea with trovador
remains future packaging direction; it was not implemented in this batch.

## Validation

- Formatting, Clippy with warnings denied, debug/release builds, and 49
  deterministic tests passed. The ordinary suite's four gated live cases
  returned early and are not counted as paid verification.
- Fake-provider MCP smoke passed, covering discovery, both backends, structured
  output, continuation, cancellation, and host-disconnect cleanup.
- Explicit low effort with model omitted passed real structured-output and
  follow-up tests on both backends: four paid CLI turns total. The first turn read
  a temporary context token, the file was removed, and the continued provider
  session returned the remembered token under the retained schema.
- Fresh packaged and installed MCP processes expose the optional effort enum,
  preserve effort/selection/schema on both fake-provider continuations, and serve
  all five resources exactly matching canonical and packaged text.
- The release binary, generated package, archive member, and refreshed Codex and
  Claude installed copies all match SHA-256
  `37a07d15ace406af9c3f843ee87ec954a0d16685938f686602098e8da58c625a`.

Evidence: [checks](checks.json), [live configuration](live.json),
[live results](live.log), [package probe](package-probe.json),
[installed probe](installed-probe.json), [installed smoke](installed-smoke.log),
and [binary identities](binary-identities.json). The bounded fake-provider probe
is saved in [verify-package.py](verify-package.py).

Packaging used the explicit reviewed release executable with
`cargo xtask package --host both --skip-build --binary target/release/task-runners`.
Codex's normal plugin-add command refreshed its cache. Claude was reinstalled
through normal plugin commands with `--keep-data`, because the previous batch
established that same-version update retained the stale executable. No cached
files were edited directly; this was local installation, not publication.

## Limits and follow-up

The live tests prove provider acceptance and successful schema/continuation at
low effort. They do not attest actual effective model/effort or cover every
combination. Current wrapper results provide no authoritative per-turn settings,
so `selection.reported` remains null. The small dated capability table rejects
known incompatibilities; aliases and unknown IDs remain provider-validated.

The installed connection's authentication probes returned unknown for both
backends before the reviews and still do afterward, while both real MCP reviews
succeeded. Approved host execution of doctor reported both authenticated and
the paid integration tests succeeded. The shell and MCP probes describe different
launch contexts; the exact reason for this discrepancy remains unresolved. The
[installed diagnostic result](installed-environment.json) is retained as a
follow-up. No credentials were extracted or restrictions bypassed, and an unknown
probe was not treated as proof that the user needed to log in.

Review runs used the prior installed server as a source-review runner; they do
not validate the new server runtime. New runtime evidence comes from the built
candidate tests and fresh installed-binary probes. Running host connections can
retain old processes/tool schemas: this chat still advertised the pre-effort
schema. Refresh the connection or use a new host session to expose the new field.
A native host-loader paid task with the new schema was not repeated in this batch.

Next planned functionality is deterministic routing, building on the explicit
settings and selection metadata. Revisit the diagnostic discrepancy and actual
fresh host loading as part of the next host-level test.

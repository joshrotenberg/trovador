# Tosi rename and local installation migration

Completed 2026-10-08. Task Runners is now Tosi. The source directory moved from
`/Users/joshrotenberg/Code/github.com/genagent/task-runners` to
`/Users/joshrotenberg/Code/github.com/genagent/tosi`. The Rust package/imports,
executable, plugin manifests, server identity, resource URIs, build output, and
active instructions use the new name. The five tool names remain unchanged.

Bottega's existing repository is marketplace-only and was not modified. Tosi
uses the transitional local `tosi-local` catalog pending a separate Bottega entry.
No shared binary, repository publication, crate publication, or public plugin
submission was performed. Historical review records retain their original names,
paths, and hashes. Previous generated artifacts were preserved at the locations
in [legacy-artifacts.json](legacy-artifacts.json).

## Compatibility

`TOSI_*` configuration names take precedence over their `TASK_RUNNERS_*`
counterparts. Fallback occurs only when the new variable is absent; empty,
invalid, and non-Unicode primary values are not replaced by the old value.
Binary overrides retain their `OsString` paths. Paid-test opt-in and backend
selection support the same migration rule.

Recursion markers deliberately have different semantics: either
`TOSI_CHILD=1` or `TASK_RUNNERS_CHILD=1` blocks MCP startup, even when the other
marker disagrees. Both adapters set both markers for old/new interoperability.
Status-only doctor remains allowed inside a worker. No provider permissions,
authentication storage, task settings, or tool contracts were broadened.

## Independent reviews and findings

Both reviews used the host's installed original Task Runners tools, with a stable
snapshot, read-only access, 300-second limits, and a shared structured findings
schema. Model and effort were omitted; actual settings are not inferred. These
were source reviews, not runtime validation of the renamed executable.

| Backend / stage | Task ID | Outcome |
| --- | --- | --- |
| Claude rename review | `d5c8e27c-1a7a-4427-a9e8-8e4ce9793c46` | Three P3 migration/error/test findings |
| Codex rename review | `148a1bee-6888-40d3-b746-feed60e21034` | No actionable findings |
| Claude focused follow-up | `d1656eb7-7111-4fc3-8756-72d34d1a5d9f` | Corrections confirmed; final documentation ordering concern |

Verified corrections:

- Configuration errors now name both current and legacy variables, including
  numeric parse failures. Acceptance rules are unchanged.
- Migration instructions explicitly remove the old plugin and stale catalog
  before registering/installing Tosi. Actual Codex catalog listing reproduced
  the stale-directory error; the documented removal-first sequence then succeeded
  on both hosts. The final ordering edit was prose-only and received no further
  paid review.
- Invalid-primary subprocess tests assert the intended failure message. Doctor
  tests prove readiness with either child marker on both backends. The optional
  end-to-end live-gating test remains a coverage gap; precedence is tested through
  the helper and the gate was inspected in both reviews.
- Parent inspection caught a 32-character plugin subtitle exceeding the format's
  30-character limit. It is now 26 characters, with an assertion in the existing
  package test.

No confirmed runtime finding remains open. One focused Claude continuation was
used for these limited corrections; no automatic retries or fallback providers
were used. Reviewers did not edit, delegate, run tests, or use network.

Evidence: [initial brief](brief.md), [schema](review-schema.json),
[launches](review-spawns.json), [initial results](review-results.json),
[focused brief](fix-brief.md), [focused launch](fix-spawn.json), and
[focused results](fix-review-results.json). Usage is preserved as provider data;
no aggregate cost or effective model/effort is inferred.

## Source and binary identity

- Rename snapshot: `c695f8f2ed18897918dc8b3c9efd24a1a0a1a9214678fff12c7c467166a2ff8b`
  ([manifest](source-manifest.json), [diff](changes.diff)).
- Corrected snapshot: `7e0135ce4f9c078d166cf0413c05eefc3d8d3e2ec59b45df8b943ba9888d5559`
  ([manifest](fix-source-manifest.json), [diff](fix-changes.diff)).
- Only README migration ordering changed after the focused snapshot
  ([diff](final-docs.diff), [hashes](post-review-changes.json)).
- Release, generated plugin, archive member, and both installed copies match
  SHA-256 `b6d4353be3f46bcf1134dc5f1d138a5e839efa92770114867fdb5a27192b5c16`
  ([identities](binary-identities.json)).

There is no Git repository for this tool; bounded copies and file manifests
identify the source. Cargo reordered the renamed root package entry in Cargo.lock;
parsed semantic equality confirmed no dependency change from that reordering.

## Validation and installation

Formatting, Clippy with warnings denied, all 56 deterministic tests, debug/release
builds, and MCP smoke passed. Four gated live cases were explicitly disabled in
the ordinary suite and are not counted as paid integration tests. Following the
corrections, 11 diagnostics tests, 6 shutdown tests, all 4 package tests, formatting,
Clippy, and the release build passed again. See [checks](checks.json) and
[focused checks](fix-checks.json).

The regenerated package passed Claude's plugin-manifest validator. Fresh packaged
and installed processes passed fake-provider checks for server/tool/resource
discovery, structured output, explicit effort, selection metadata, continuation,
cancellation, and disconnect cleanup. All five embedded resources matched the
canonical and packaged text. See [package smoke](package-smoke.log),
[package probe](package-probe.json), [manifest validation](claude-plugin-validation.log),
[installed smoke](installed-smoke.log), and [installed probe](installed-probe.json).

After collecting all review results, normal host CLI commands removed
`task-runners@task-runners-local` and its stale catalog, registered the renamed
source directory, and installed `tosi@tosi-local` in both Codex and Claude. Claude
uninstall used `--keep-data`. Both catalog-list commands now succeed, show
`tosi-local`, and omit `task-runners-local` ([catalog checks](catalog-checks.json)).
Cache files were not manually edited. No active review was interrupted.

Fresh native host sessions still need to load the renamed tool namespace; existing
chat connections may retain old metadata. Verification used fresh processes from
the actual installed binaries, not a new paid native-host task. Real provider
integration cases were not repeated for this identity/configuration migration;
the two paid reviews and one focused continuation ran through the old installed
review server. The earlier authentication-probe discrepancy is not resolved by
this rename and remains recorded in the effort review.

# First dual-backend dogfood review

Date: 2026-10-08. The baseline review and verified corrections are complete.
Claude and Codex reviewed the implementation independently through the installed
Task Runners MCP tools. Both final focused reviews returned no further findings.
This records bounded review evidence, not a guarantee of correctness.

## Findings and disposition

| Finding | Verification and disposition |
| --- | --- |
| Claude P3: authenticated prerelease CLIs report not ready | Confirmed in the strict version parser. Fixed by accepting only numeric `X.Y.Z` and the explicit `alpha.N`, `beta.N`, and `rc.N` prerelease forms. Arbitrary suffixes and extra lines remain withheld. Both-provider diagnostics and doctor regression tests pass. |
| Codex P2: `--skip-build` guesses the wrong artifact path with an explicit Cargo target | Confirmed that normal builds use Cargo's artifact message but skip-build guessed the default release directory. Removed path guessing: `--skip-build` and `--binary PATH` now require each other. Tests prove the selected target-specific bytes reach both plugin and archive even when an older default-path binary exists. |
| Both follow-ups: packaging alias changes cwd before resolving a relative binary (Claude P3, Codex P2) | Deduplicated into one finding and confirmed in `scripts/package.sh`. Fixed by preserving caller cwd and invoking `cargo run` with an absolute xtask manifest. A regression covers paths with spaces; a real invocation from an external directory packaged the intended caller binary instead of the distinct checkout binary. Both final reviews confirm resolution. |

The initial Codex assignment could not inspect files because the parent's brief
incorrectly prohibited all shell commands. Its empty findings are not a source
review. After diagnosing this instruction mismatch, the parent explicitly
continued the task with permission for read-only inspection commands. It then
completed the baseline. No Claude findings were supplied to it. The correction
and exact prompt are in [codex-correction.md](codex-correction.md); AGENTS.md now
prevents repeating the mistake. There were no automatic retries or fallbacks.

## Review identities

| Stage | Backend | Task ID |
| --- | --- | --- |
| Baseline | Claude | `25940bf4-1cfc-4d86-b878-356a8d8c2991` |
| Initial inspection blocked by brief | Codex | `3140030b-e74f-481c-a8f4-af3030e5e1d4` |
| Corrected baseline assignment | Codex | `a9b82d92-8efe-46de-a0f6-f645a1fd5ced` |
| Diagnostics and explicit-binary correction | Claude | `4bf9e653-f3dc-4129-8b1a-db33bf96c432` |
| Diagnostics and explicit-binary correction | Codex | `b93bbf2c-a6eb-4032-be08-aa9519993226` |
| Alias correction | Claude | `e1a2fd38-3dd9-40cd-af1c-223a8999acfb` |
| Alias correction | Codex | `d55874fe-d6cc-4526-bcb3-11f213ef7325` |

All turns used `read_only`, a 300-second per-turn timeout, and the same structured
findings schema. `model` was omitted; this delegates selection to each CLI and
does not inherit the calling chat's settings. No effort parameter exists yet.
Actual model/effort is not reported by these task snapshots. Raw provider usage
is preserved in the result JSON; no aggregate monetary total is inferred from
potentially cumulative continuation usage or missing provider cost fields.

Both conversations continued successfully for the focused corrections, preserving
their schema and settings. Their original cwd remained the baseline directory;
follow-up prompts explicitly named each new immutable snapshot by absolute path.
No reviewers edited files, ran tests, or delegated. The parent and native helpers
verified findings, implemented the fixes, and ran checks.

## Source and executable identity

There is no Git repository, so immutable bounded copies, per-file SHA-256
manifests, and diffs identify the work without inventing a commit.

- Baseline: `9433d2501142d830f21c13be2ea21d2a48625560faaece050bf88bfe47512de7`
  ([manifest](source-manifest.json), [brief](brief.md)).
- Diagnostics and packaging candidate:
  `ddce5a5cc64e7b0483f67c6bee719fed72c14cfbbedbdbc5fed1d7353a48bc3d`
  ([manifest](fix-source-manifest.json), [changes](changes.diff), [brief](fix-brief.md)).
- Final alias candidate:
  `f9268181b70d268d200d98007a9b45a5058e47dbbcf6dce3946af883d0d1fde9`
  ([manifest](final-source-manifest.json), [changes](alias-changes.diff), [brief](alias-brief.md)).

Completion notes in AGENTS.md/ROADMAP.md and this evidence record were updated
after the final snapshot; executable source was not changed after review.
NAMING.md is a separate research note, not part of these code snapshots. No rename
was selected.

Baseline reviews used the installed server binary
`3b25a2413e2049d4b4e3edbaabb52a844c599ba632dabf33243f5cbe458747fb`.
The corrected release, generated plugin, archive executable, and refreshed
Codex/Claude caches match
`5605a9d62c0cbe2de6a4df3073f72c92f2a81bf60321b72365172367a0a6d219`.
The installed server was a review runner; its old binary was not counted as
runtime validation of changed source.

## Validation

- Formatting and workspace Clippy with warnings denied pass.
- The workspace suite passed with 40 deterministic tests after the diagnostics
  fix; all four xtask tests passed after adding the alias regression, bringing
  deterministic coverage to 41 tests. Four gated live cases returned early and
  are not counted as real-provider tests.
- Debug/release builds and fake-provider MCP smoke checks pass. Smoke coverage
  includes tool/resource discovery, both providers, structured output,
  continuation, cancellation, and host-disconnect cleanup.
- Fresh generated-package probes verify prerelease readiness, rejection of
  unknown version suffixes, doctor exit codes, and withholding fixture secrets.
- The real packaging alias was exercised from an external caller directory with
  competing binary paths; intended bytes were selected. The normal release
  package was regenerated afterward, removing the test marker from final artifacts.
- A fresh MCP stdio process from the refreshed installed binary passes the smoke
  suite. Installed hashes are checked against the built release.

Evidence: [initial checks](checks.json), [correction checks](fix-checks.json),
[alias checks](alias-checks.json), [real alias check](alias-real-check.json),
[package checks](package-checks.json), [installed checks](installed-checks.json).
Complete review outputs, coverage, limitations, and provider usage are in
[initial-results.json](initial-results.json), [codex-review.json](codex-review.json),
[fix-results.json](fix-results.json), and [alias-results.json](alias-results.json).

## Remaining limits and next work

No confirmed review finding remains open. Wrapper internals, actual provider
process-tree guarantees, and possible raw error text on successful-exit provider
failures were not fully audited. Final runtime checks use fake provider fixtures;
the prior paid live integration suite and actual host-loader checks were not
repeated for these localized changes. Final caches are refreshed, but running
host processes can retain the old executable; fresh host sessions pick up the
new copy. Claude's same-version update initially retained the old binary, so its
plugin was reinstalled through the CLI with `--keep-data` and hashes rechecked.

The alias regression test may be sensitive to symlinked checkout paths because
its expected manifest is canonicalized; this did not affect the actual checkout
or external-directory test. Caller-side Cargo/toolchain configuration can affect
building xtask; the internal release build still runs from the checkout root.
Explicit `--binary` freshness and architecture remain caller responsibilities.
Unrecognized version formats still report not ready by design and are documented.

Next planned implementation milestone: explicit provider effort controls and
transparent requested/selected settings, before routing or workflow expansion.

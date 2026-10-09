# Codex inspection correction

The original brief incorrectly prohibited all shell commands for both providers.
Claude has native Read/Glob/Grep tools; this Codex worker had no available local
reader outside its shell tool. Its first completed result therefore contains no
source review and is not evidence of correctness.

After diagnosing the instruction mismatch, the parent explicitly continued the
Codex conversation with task `a9b82d92-8efe-46de-a0f6-f645a1fd5ced` (parent task
`3140030b-e74f-481c-a8f4-af3030e5e1d4`). This was an intentional corrected
assignment, not an automatic retry. It retained the original snapshot, schema,
read-only access, provider defaults, and 300-second execution timeout. No Claude
findings were supplied. The original snapshot and brief were not modified.

Follow-up message:

> Correction to my review instructions: you may use read-only shell commands
> (for example pwd, rg, cat, sed, nl, ls) to inspect the fixed local source
> snapshot, REVIEW_BRIEF.md, and PARENT_CHECKS.json. My blanket prohibition on
> shell commands accidentally blocked your normal file-reading tools; this
> permission supersedes that sentence in the initial task and brief. Do not
> edit files, run tests/builds, execute project scripts, delegate, or access the
> network. Complete the original independent baseline review with the same
> scope and evidence, reporting only concrete actionable bugs and explicit
> coverage/limitations under the existing output schema. No other reviewer
> findings are supplied.

AGENTS.md now makes the provider-specific inspection allowance explicit for
future reviews. Test/build execution remains with the parent.

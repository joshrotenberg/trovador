# Cutover from spike, 2026-10-07

What was done on the development machine to start using trovador alongside spike
(`agent-sessions`), and what each step measured. spike was left installed and registered.

## Steps

| Step | Command | Result |
|---|---|---|
| Install | `cargo install --path . --locked` | `~/.cargo/bin/trovador`, 0.1.0 |
| Preview the import | `trovador import --dry-run` | would adopt level `approved`, add 3 spawned and 2 decisions; 11 summaries stale; 31 other spike entries not imported |
| Import | `trovador import` | as previewed; checksum of every file under `~/.agent-sessions` identical before and after |
| Consent state | `trovador consent status` | level `approved` (from spike), 3 spawned, 2 approved; `~/.trovador/consent.json` mode 0600 |
| Index | `trovador index` | 5,378 sessions, 0 failed, 287 MB, 512,780 records, 8.88 s |
| Parity | `python3 scripts/parity.py --markdown docs/parity-2026-10-07.md` | see [parity-2026-10-07.md](parity-2026-10-07.md) and the summary below |
| Register, Claude Code | `claude mcp add --scope user trovador -- ~/.cargo/bin/trovador serve` | connected |
| Register, Codex | `codex mcp add trovador -- ~/.cargo/bin/trovador serve` | listed, enabled |
| Handshake | initialize, tools/list, resources/templates/list, one search over stdio | tools `ask`, `search`, `sessions`, `summarize`; 3 resource templates; the search refreshed 2 appended sessions in 42 ms; nothing on stderr |

All 11 of spike's summaries were stale: each session had been written to after spike
summarized it, so importing them would have served outdated summaries as current.

## Registration found on the machine

| Client | `agent-sessions` entry | State |
|---|---|---|
| Claude Code (user scope) | `node .../spike/src/server.ts` | connected |
| Codex | `node .../spike/server.ts` | that file does not exist; spike's server is `src/server.ts`, so Codex sessions could not reach spike |

Both entries were left as found. trovador was registered alongside them under its own name.

## Parity summary

From [parity-2026-10-07.md](parity-2026-10-07.md):

- **Inventory.** All 5,305 sessions spike lists are in trovador. trovador lists 73 more,
  all Codex threads in `archived_sessions`, which spike does not scan. Nothing is spike-only.
- **Metadata**, 60 sessions sampled. Project and resumability agree for all 60. Snippets
  agree for 50; in 8 spike shows injected AGENTS.md or attachment text where trovador
  shows what the user typed or nothing; 2 others differ in a subagent's input and in one
  session where only trovador found a snippet.
- **Search**, the four queries real sessions sent to spike. Every session spike found and
  trovador did not is a Codex `guardian` approval reviewer, whose text trovador leaves out
  of the index on purpose. trovador found up to 69 sessions spike did not, because spike
  skips files over 24 MB and stops scanning at its limit. A full spike scan took 17.7 to
  23.2 s per query; the same query as a trovador regex took 1.0 to 1.1 s.
- **Consent.** The 5 sessions spike recorded as spawned or approved get the same
  continuation answer from trovador after the import.

## Left for the person

- Removing spike's `agent-sessions` registrations (`claude mcp remove agent-sessions -s user`,
  `codex mcp remove agent-sessions`), once trovador has been used for real work.
- Client sessions already running keep the servers they started with; trovador's tools
  appear in sessions started after the registration.

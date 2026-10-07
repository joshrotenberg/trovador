#!/usr/bin/env python3
"""Compare trovador with spike (agent-sessions) on the same real archive.

Runs both CLIs read-only and reports where they agree and where they differ, so a cutover
from spike can be judged on what each actually returns rather than on what each claims.
Nothing here runs a model or changes either tool's state.

    python3 scripts/parity.py --markdown docs/parity-latest.md

Each difference is classified where the cause is known (for example, spike reports the
directory a Claude session started in, trovador the one it was relocated to). Anything left
unclassified is a gap to investigate.
"""

import argparse
import json
import random
import re
import subprocess
import sys
import time

# Queries real sessions sent to spike's search tool, from local transcripts.
REAL_QUERIES = [
    "550e8400-e29b-41d4-a716-446655440000",
    "idempotency_key",
    "GitHub Project",
    "continuation consent",
]


def run(cmd, timeout=900):
    started = time.time()
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    if out.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)} failed: {out.stderr.strip()[:400]}")
    return json.loads(out.stdout), time.time() - started


def inventory(spike, trov):
    s, s_t = run([spike, "list", "--limit", "1000000", "--json"])
    t, t_t = run([trov, "sessions", "list", "--include-subagents", "--limit", "1000000"])
    s_ids = {(r["backend"], r["session_id"]) for r in s["sessions"]}
    t_rows = {(r["backend"], r["session_id"]): r for r in t["sessions"]}
    t_ids = set(t_rows)
    only_t = t_ids - s_ids
    only_s = s_ids - t_ids
    explained = {"archived": 0, "newer_than_spike_list": 0}
    started = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(time.time() - s_t - t_t - 5))
    unexplained = []
    for k in only_t:
        r = t_rows[k]
        if r.get("archived"):
            explained["archived"] += 1
        elif r["modified"] >= started:
            explained["newer_than_spike_list"] += 1
        else:
            unexplained.append(k)
    return {
        "spike_sessions": len(s_ids),
        "trovador_sessions": len(t_ids),
        "both": len(s_ids & t_ids),
        "trovador_only": len(only_t),
        "trovador_only_explained": explained,
        "trovador_only_unexplained": [f"{b}/{i}" for b, i in sorted(unexplained)[:20]],
        "spike_only": [f"{b}/{i}" for b, i in sorted(only_s)[:20]],
        "spike_only_count": len(only_s),
        "seconds": {"spike": round(s_t, 2), "trovador": round(t_t, 2)},
        "_common": sorted(s_ids & t_ids),
        "_trov_rows": t_rows,
    }


def classify_snippet(s_snip, t_snip, t_row):
    if s_snip == t_snip:
        return "same"
    # Injected context first: a session whose only first-turn text is an injected AGENTS.md
    # has a spike snippet and, correctly, no trovador one.
    if s_snip and (s_snip.startswith("# AGENTS.md") or s_snip.startswith("# Files mentioned")):
        return "spike_shows_injected_context"
    if not s_snip and t_snip:
        return "trovador_found_spike_missed"
    if s_snip and not t_snip:
        return "spike_found_trovador_missed"
    if t_snip.startswith("scheduled task:"):
        return "trovador_names_scheduled_task"
    if t_row.get("origin") in ("guardian", "thread_spawn") or t_row.get("subagent"):
        return "subagent_input_differs"
    return "different"


def metadata(spike, trov, common, t_rows, sample):
    rng = random.Random(7)
    picks = rng.sample(common, min(sample, len(common)))
    tally = {"project": {}, "snippet": {}, "resumable": {}}
    examples = {}
    for backend, sid in picks:
        s, _ = run([spike, "show", sid, "--backend", backend, "--json"], timeout=120)
        t = t_rows[(backend, sid)]
        if s.get("project") == t.get("project"):
            p = "same"
        elif t.get("relocated_cwd") and s.get("project") == t.get("cwd"):
            p = "trovador_follows_relocation"
        else:
            p = "different"
        sn = classify_snippet(s.get("snippet"), t.get("snippet"), t)
        r = "same" if bool(s.get("continuation", {}).get("resumable", s.get("resumable"))) == t["resumable"] else "different"
        for field, v in (("project", p), ("snippet", sn), ("resumable", r)):
            tally[field][v] = tally[field].get(v, 0) + 1
            if v in ("different", "spike_found_trovador_missed") and len(examples.setdefault(f"{field}:{v}", [])) < 3:
                examples[f"{field}:{v}"].append(
                    {"session": f"{backend}/{sid}", "spike": s.get(field if field != "resumable" else "continuation"), "trovador": t.get(field)}
                )
    return {"sampled": len(picks), "tally": tally, "examples": examples}


def search(spike, trov, t_rows):
    results = []
    for q in REAL_QUERIES:
        s, s_t = run([spike, "search", q, "--limit", "100000", "--json"], timeout=1800)
        t_terms, t_t = run([trov, "search", q, "--include-subagents", "--limit", "100000", "--per-session", "1"])
        t_lit, t_lt = run([trov, "search", re.escape(q), "--regex", "--include-subagents", "--limit", "100000", "--per-session", "1"])
        s_ids = {(h["backend"], h["session_id"]) for h in s["hits"]}
        lit_ids = {(h["backend"], h["session_id"]) for h in t_lit["hits"]}
        term_ids = {(h["backend"], h["session_id"]) for h in t_terms["hits"]}
        spike_only = s_ids - lit_ids
        reasons = {}
        for k in spike_only:
            row = t_rows.get(k)
            why = (
                "not_in_trovador_list" if row is None
                else "guardian_text_not_indexed" if row.get("origin") == "guardian"
                else "unexplained"
            )
            reasons[why] = reasons.get(why, 0) + 1
        results.append({
            "query": q,
            "spike": {"sessions": len(s_ids), "scanned": s.get("sessions_scanned"), "available": s.get("sessions_available"),
                      "skipped_too_large": s.get("skipped_too_large"), "exhaustive": s.get("exhaustive"), "seconds": round(s_t, 2)},
            "trovador_regex_literal": {"sessions": len(lit_ids), "exhaustive": t_lit["exhaustive"], "seconds": round(t_lt, 2)},
            "trovador_terms": {"sessions": len(term_ids), "seconds": round(t_t, 2)},
            "both": len(s_ids & lit_ids),
            "trovador_only": len(lit_ids - s_ids),
            "spike_only": len(spike_only),
            "spike_only_reasons": reasons,
            "spike_only_unexplained": [f"{b}/{i}" for b, i in sorted(k for k in spike_only if t_rows.get(k) and t_rows[k].get("origin") != "guardian")][:10],
        })
    return results


def consent(spike, trov):
    raw = json.load(open(subprocess.run(["sh", "-c", "echo $HOME"], capture_output=True, text=True).stdout.strip() + "/.agent-sessions/consent.json"))
    rows = []
    for rec in raw.get("spawned", []) + raw.get("decisions", []):
        b, sid = rec["backend"], rec["session_id"]
        try:
            s, _ = run([spike, "consent", "check", sid, "--backend", b, "--json"], timeout=60)
            t, _ = run([trov, "consent", "check", sid, "--backend", b], timeout=60)
            rows.append({"session": f"{b}/{sid}", "spike_allowed": s.get("allowed"), "trovador_allowed": t.get("allowed")})
        except RuntimeError as e:
            rows.append({"session": f"{b}/{sid}", "error": str(e)[:200]})
    return {"checked": len(rows), "agree": sum(1 for r in rows if r.get("spike_allowed") == r.get("trovador_allowed")), "rows": rows}


def markdown(report):
    inv, meta = report["inventory"], report["metadata"]
    lines = [f"# Parity with spike, {report['date']}", "", "Generated by `scripts/parity.py`.", "", "## Inventory", "",
             "| | Count |", "|---|---|",
             f"| spike sessions | {inv['spike_sessions']} |", f"| trovador sessions | {inv['trovador_sessions']} |",
             f"| in both | {inv['both']} |",
             f"| trovador only: Codex archived threads | {inv['trovador_only_explained']['archived']} |",
             f"| trovador only: written after spike's list ran | {inv['trovador_only_explained']['newer_than_spike_list']} |",
             f"| trovador only: unexplained | {len(inv['trovador_only_unexplained'])} |",
             f"| spike only | {inv['spike_only_count']} |", "",
             f"Full list: spike {inv['seconds']['spike']} s, trovador {inv['seconds']['trovador']} s.", "",
             f"## Metadata, {meta['sampled']} sessions sampled", ""]
    for field, t in meta["tally"].items():
        lines.append(f"- **{field}**: " + ", ".join(f"{k} {v}" for k, v in sorted(t.items())))
    lines += ["", "## Search", "",
              "spike matches a case-insensitive substring; the closest trovador equivalent is a regex of the escaped query, shown as `regex`. `terms` is trovador's default mode.", "",
              "| Query | spike | trovador regex | trovador terms | both | trovador only | spike only | spike only, why | spike s | trovador regex s |",
              "|---|---|---|---|---|---|---|---|---|---|"]
    for r in report["search"]:
        why = ", ".join(f"{k} {v}" for k, v in r["spike_only_reasons"].items()) or "none"
        lines.append(f"| `{r['query']}` | {r['spike']['sessions']} | {r['trovador_regex_literal']['sessions']} | {r['trovador_terms']['sessions']} | {r['both']} | {r['trovador_only']} | {r['spike_only']} | {why} | {r['spike']['seconds']} | {r['trovador_regex_literal']['seconds']} |")
    if report.get("consent"):
        c = report["consent"]
        lines += ["", "## Consent", "", f"{c['agree']} of {c['checked']} sessions that spike recorded as spawned or decided get the same continuation answer from both."]
    return "\n".join(lines) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--spike", default="agent-sessions")
    ap.add_argument("--trovador", default="trovador")
    ap.add_argument("--sample", type=int, default=60)
    ap.add_argument("--skip-consent", action="store_true")
    ap.add_argument("--markdown")
    a = ap.parse_args()
    report = {"date": time.strftime("%Y-%m-%d")}
    inv = inventory(a.spike, a.trovador)
    common, t_rows = inv.pop("_common"), inv.pop("_trov_rows")
    report["inventory"] = inv
    report["metadata"] = metadata(a.spike, a.trovador, common, t_rows, a.sample)
    report["search"] = search(a.spike, a.trovador, t_rows)
    if not a.skip_consent:
        report["consent"] = consent(a.spike, a.trovador)
    if a.markdown:
        open(a.markdown, "w").write(markdown(report))
    json.dump(report, sys.stdout, indent=2)
    print()


if __name__ == "__main__":
    main()

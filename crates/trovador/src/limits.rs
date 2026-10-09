//! Every tunable in one place, each overridable by a `TROVADOR_*` environment variable and
//! reported by `trovador doctor`.
//!
//! Almost everything read here ends up pasted into some agent's context window, so these
//! are context budgets first and I/O tuning second. A page of 200 records at 4,000 chars
//! each is about 800,000 characters, enough to exhaust the context of whoever asked.
//! Payloads are therefore capped by size, and every truncation says so in the response.

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Limits {
    /// Ceiling on the content of one response: the characters of transcript records in a
    /// page, or of a summary's text. The fields around them are extra.
    /// Model text past it is cut, and the response says how to read the rest.
    pub response_max_chars: usize,
    /// Head read steps for session metadata, smallest first. Each step is used only when
    /// the previous one left cwd or the first user message missing. Codex writes a large
    /// developer preamble first (first user message seen at byte 119,106), and a Claude
    /// session that opens with attachments was seen with its first user message at byte
    /// 757,435, past spike's 512 KB ceiling.
    pub head_steps: Vec<usize>,
    /// Per-record cap in a transcript projection, so one huge message cannot dominate.
    pub record_chars: usize,
    /// Cap for tool results and tool call arguments, which are rarely worth more.
    pub tool_chars: usize,
    /// Records per transcript page.
    pub page_size: usize,
    /// Bytes above which a session is reported unresumable. The provider rejects the turn
    /// with "Prompt is too long" somewhere past this size.
    pub resume_limit_bytes: u64,
    /// Default number of sessions a list returns.
    pub list_default: usize,
    /// How long a search may spend bringing the index up to date before it queries what is
    /// there. The first build of a large archive takes longer; `trovador index` has no
    /// budget.
    pub index_budget_ms: usize,
    /// Default number of sessions a search returns.
    pub search_default: usize,
    /// Default number of matching records shown per session.
    pub per_session_default: usize,
    /// Seconds a Claude turn may run before it is killed.
    pub claude_timeout_s: usize,
    /// Seconds a Codex turn may run. Longer than Claude's: spike's 120 s default killed a
    /// Codex turn between two MCP writes it was making (spike#107).
    pub codex_timeout_s: usize,
    /// Nesting depth at which a run is refused. Every session trovador starts can reach
    /// trovador again, so without a ceiling one prompt could recurse indefinitely.
    pub max_depth: usize,
    /// Model for summaries. They read a lot and write little, so a small model.
    pub summary_model: String,
    /// Characters of rendered transcript per summary chunk.
    pub summary_chunk_chars: usize,
    /// Chunks summarized at most. Past this the first and last halves are kept, which
    /// bounds the cost of summarizing a very large session.
    pub summary_max_chunks: usize,
    /// Provider runs started at once by one operation. Each is a full CLI start.
    pub fanout: usize,
    /// Seconds within which a session counts as active and is left out of search by
    /// default. The session doing the search is always active, and its own transcript
    /// contains the words it searches for. Zero turns the default off.
    pub active_window_s: usize,
    /// Index reasoning records (Claude thinking text, Codex reasoning summaries) for
    /// search. They raised the index from 287 MB to 415 MB on the development machine.
    /// Transcripts show them either way.
    pub index_reasoning: bool,
    /// Redact secrets in what is returned, including search excerpts, which are built
    /// after redaction. Turned off only by `--no-redact` when the process starts.
    pub redact: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            response_max_chars: 100_000,
            head_steps: vec![64 * 1024, 512 * 1024, 4 * 1024 * 1024],
            record_chars: 4_000,
            tool_chars: 600,
            page_size: 200,
            resume_limit_bytes: 900_000,
            list_default: 20,
            index_budget_ms: 15_000,
            search_default: 20,
            per_session_default: 3,
            claude_timeout_s: 600,
            codex_timeout_s: 900,
            max_depth: 2,
            summary_model: "haiku".into(),
            summary_chunk_chars: 250_000,
            summary_max_chunks: 8,
            fanout: 3,
            active_window_s: 600,
            index_reasoning: true,
            redact: true,
        }
    }
}

impl Limits {
    /// Defaults overridden by any `TROVADOR_*` variable that parses. A value that does not
    /// parse is ignored rather than silently becoming zero.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let num = |k: &str, fallback: usize| {
            get(k)
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(fallback)
        };
        let head_max = num(
            "TROVADOR_HEAD_MAX_BYTES",
            *d.head_steps.last().unwrap_or(&0),
        );
        let head_first = num("TROVADOR_HEAD_BYTES", d.head_steps[0]).min(head_max);
        let mut head_steps: Vec<usize> = d
            .head_steps
            .iter()
            .copied()
            .filter(|s| *s > head_first && *s < head_max)
            .collect();
        head_steps.insert(0, head_first);
        if head_max > head_first {
            head_steps.push(head_max);
        }
        Self {
            response_max_chars: num("TROVADOR_RESPONSE_MAX_CHARS", d.response_max_chars),
            head_steps,
            record_chars: num("TROVADOR_RECORD_CHARS", d.record_chars),
            tool_chars: num("TROVADOR_TOOL_CHARS", d.tool_chars),
            page_size: num("TROVADOR_PAGE_SIZE", d.page_size).max(1),
            resume_limit_bytes: num("TROVADOR_RESUME_LIMIT", d.resume_limit_bytes as usize) as u64,
            list_default: num("TROVADOR_LIST_DEFAULT", d.list_default).max(1),
            index_budget_ms: num("TROVADOR_INDEX_BUDGET_MS", d.index_budget_ms),
            search_default: num("TROVADOR_SEARCH_DEFAULT", d.search_default).max(1),
            per_session_default: num("TROVADOR_PER_SESSION", d.per_session_default).max(1),
            claude_timeout_s: num("TROVADOR_CLAUDE_TIMEOUT_S", d.claude_timeout_s).max(1),
            codex_timeout_s: num("TROVADOR_CODEX_TIMEOUT_S", d.codex_timeout_s).max(1),
            max_depth: num("TROVADOR_MAX_DEPTH", d.max_depth),
            summary_model: get("TROVADOR_SUMMARY_MODEL")
                .filter(|m| !m.trim().is_empty())
                .unwrap_or(d.summary_model),
            summary_chunk_chars: num("TROVADOR_SUMMARY_CHUNK_CHARS", d.summary_chunk_chars).max(1),
            summary_max_chunks: num("TROVADOR_SUMMARY_MAX_CHUNKS", d.summary_max_chunks).max(1),
            fanout: num("TROVADOR_FANOUT", d.fanout).max(1),
            active_window_s: num("TROVADOR_ACTIVE_WINDOW_S", d.active_window_s),
            index_reasoning: get("TROVADOR_INDEX_REASONING")
                .map(|v| !matches!(v.trim(), "0" | "false" | "no" | "off"))
                .unwrap_or(d.index_reasoning),
            // Deliberately not read from the environment: only the command line turns it off.
            redact: d.redact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn an_unparseable_override_keeps_the_default_instead_of_becoming_zero() {
        let l = Limits::from_lookup(lookup(&[("TROVADOR_PAGE_SIZE", "lots")]));
        assert_eq!(l.page_size, 200);
    }

    #[test]
    fn head_steps_stay_ascending_and_end_at_the_configured_maximum() {
        let l = Limits::from_lookup(lookup(&[
            ("TROVADOR_HEAD_BYTES", "1000"),
            ("TROVADOR_HEAD_MAX_BYTES", "600000"),
        ]));
        assert_eq!(l.head_steps, vec![1000, 65536, 524288, 600000]);
    }

    #[test]
    fn a_first_step_above_the_maximum_collapses_to_one_read() {
        let l = Limits::from_lookup(lookup(&[
            ("TROVADOR_HEAD_BYTES", "9000000"),
            ("TROVADOR_HEAD_MAX_BYTES", "1000"),
        ]));
        assert_eq!(l.head_steps, vec![1000]);
    }
}

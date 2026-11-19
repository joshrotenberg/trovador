//! Every tunable in one place, each overridable by a `TROVADOR_*` environment variable and
//! reported by `trovador doctor`.
//!
//! Almost everything read here ends up pasted into some agent's context window, so these
//! are context budgets first and I/O tuning second. A page of 200 records at 4,000 chars
//! each is about 800,000 characters, enough to exhaust the context of whoever asked.
//! Payloads are therefore capped by size, and every truncation says so in the response.

use serde::Serialize;

/// Default for `index_tool_chars`. Index size and full build time on the development
/// machine (9,100 sessions, 2026-11-19), by this cap: 600, 643 MB in 86 s; 4,000, 1.60 GB
/// in 114 s; 16,000, 2.18 GB in 115 s; 64,000, 2.72 GB in 170 s. 4,000 matches
/// `record_chars` and reached two of the three words that motivated #101 (at 2,278 and
/// 3,309 characters; the third was at 6,200).
const INDEX_TOOL_CHARS: usize = 4_000;

#[derive(Clone, Debug, Serialize)]
pub struct Limits {
    /// Ceiling on the items of one response, in characters: sessions in a list, hits in a
    /// search and records in a transcript page, each measured as its JSON, or the text of a
    /// session part. The fields around them are extra, a few hundred characters. A page
    /// stops at the item that would pass it, keeps at least one, and says how to read the
    /// rest. Claude Code saves an MCP result over 50,000 characters to a file and shows the
    /// model only its path, so the default leaves room under that.
    pub response_max_chars: usize,
    /// Head read steps for session metadata, smallest first. Each step is used only when
    /// the previous one left cwd or the first user message missing. Codex writes a large
    /// developer preamble first (first user message seen at byte 119,106), and a Claude
    /// session that opens with attachments was seen with its first user message at byte
    /// 757,435, past spike's 512 KB ceiling.
    pub head_steps: Vec<usize>,
    /// Unindexed Claude files above this size keep head/tail title metadata. Indexing
    /// tracks titles incrementally regardless of this fallback scan budget (issue #36).
    pub title_scan_max_bytes: usize,
    /// Per-record cap in a transcript projection, so one huge message cannot dominate.
    pub record_chars: usize,
    /// Cap for tool results and tool call arguments in transcripts and session text,
    /// which are rarely worth more to read.
    pub tool_chars: usize,
    /// Cap for tool results and tool call arguments in the index, so search finds what
    /// they say past `tool_chars`. File reads, grep output and test logs put error
    /// messages and code thousands of characters in; with the index at `tool_chars` a
    /// search for a word 2,278 characters into a tool result found nothing (#101).
    pub index_tool_chars: usize,
    /// Cap for the JSON kept from a record, block or item of a type the projection does not
    /// know (an `unrecognized` record). Enough to show its type, field names and the start
    /// of its text, and to make that text searchable. Zero shows such records as `omitted`
    /// with no text, as before they existed.
    pub unrecognized_chars: usize,
    /// Records per transcript page.
    pub page_size: usize,
    /// Bytes above which a session is reported unresumable. The provider rejects the turn
    /// with "Prompt is too long" somewhere past this size.
    pub resume_limit_bytes: u64,
    /// Default number of sessions a list returns.
    pub list_default: usize,
    /// Most sessions a list returns, whatever `limit` asks for. A larger `limit` is
    /// lowered to this rather than rejected.
    pub list_max: usize,
    /// How long a search may spend bringing the index up to date before it queries what is
    /// there. The first build of a large archive takes longer; `trovador index` has no
    /// budget.
    pub index_budget_ms: usize,
    /// How long an index refresh runs before it prints progress (when progress is on), so
    /// the few grown files a search usually finds stay silent and a long build says what it
    /// is doing.
    pub progress_after_ms: usize,
    /// Default number of sessions a search returns.
    pub search_default: usize,
    /// Most sessions a search returns, whatever `limit` asks for.
    pub search_max: usize,
    /// Default number of matching records shown per session.
    pub per_session_default: usize,
    /// Most matching records shown per session, whatever `per_session` asks for.
    /// `match_count` still counts them all, and a transcript reads the rest.
    pub per_session_max: usize,
    /// Sessions a list reads past a full page while it looks for one more match. It decides
    /// whether `next_offset` is present, and with a selective `project` or `origin` filter
    /// each one read costs a metadata read. When it runs out first, the list reports that
    /// more may follow and `next_offset` continues from there.
    pub list_lookahead: usize,
    /// Largest `offset` a list or search accepts. Paging past the end is an empty page,
    /// not an error, so this only rejects values that cannot be a position in any real
    /// archive and keeps a mistyped number from turning into a scan of everything.
    pub offset_max: usize,
    /// Seconds within which a session counts as active and is left out of search by
    /// default. The session doing the search is always active, and its own transcript
    /// contains the words it searches for. Zero turns the default off.
    pub active_window_s: usize,
    /// Index reasoning records (Claude thinking text, Codex reasoning summaries) for
    /// search. They raised the index from 287 MB to 415 MB on the development machine.
    /// Transcripts show them either way.
    pub index_reasoning: bool,
    /// Whether search and list include, when a call does not say, sessions written within
    /// `active_window_s`. Hidden by default because the session doing the search is among
    /// them; a caller that passes its own id as `exclude_session` can include the rest.
    pub include_active: bool,
    /// Whether search and list include, when a call does not say, automated sessions
    /// (`claude -p`, `codex exec`). Hidden by default because scripted runs outnumber the
    /// sessions a person started.
    pub include_automated: bool,
    /// Whether search and list include, when a call does not say, sessions started by
    /// another session. Hidden by default: on the development machine 2,950 of 5,292
    /// sessions were subagents, 1,694 of them Codex approval reviewers.
    pub include_subagents: bool,
    /// Redact secrets before they are written to the index, as well as on output. Off by
    /// default: the index is private to the user (0700 directory, 0600 files) and output
    /// is redacted either way, and an unredacted index lets a search for a token still find
    /// the session. On, `~/.trovador` holds no secret the redaction rules recognise, at the
    /// cost of those searches, and of `--no-redact` excerpts and indexed titles staying
    /// redacted. Changing it starts a fresh index file.
    pub index_redact: bool,
    /// Redact secrets in what is returned, including search excerpts, which are built
    /// after redaction. Turned off only by `--no-redact` when the process starts.
    pub redact: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            response_max_chars: 40_000,
            head_steps: vec![64 * 1024, 512 * 1024, 4 * 1024 * 1024],
            title_scan_max_bytes: 4 * 1024 * 1024,
            record_chars: 4_000,
            tool_chars: 600,
            index_tool_chars: INDEX_TOOL_CHARS,
            unrecognized_chars: 1_000,
            page_size: 200,
            resume_limit_bytes: 900_000,
            list_default: 20,
            list_max: 100,
            index_budget_ms: 15_000,
            progress_after_ms: 1_000,
            search_default: 20,
            search_max: 50,
            per_session_default: 3,
            per_session_max: 10,
            list_lookahead: 500,
            offset_max: 1_000_000,
            active_window_s: 600,
            index_reasoning: true,
            include_active: false,
            include_automated: false,
            include_subagents: false,
            index_redact: false,
            redact: true,
        }
    }
}

impl Limits {
    /// These limits as the index applies them: tool records capped at `index_tool_chars`
    /// instead of `tool_chars`. Only text changes, never which records exist, so a match
    /// index still opens the same record in a transcript.
    pub fn for_index(&self) -> Self {
        Self {
            tool_chars: self.index_tool_chars,
            ..self.clone()
        }
    }

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
        // Unlike TROVADOR_INDEX_REASONING, which treats anything but an off word as on, these
        // accept only clear words, so a typo keeps the default instead of widening results.
        let flag = |k: &str, fallback: bool| match get(k).as_deref().map(str::trim) {
            Some("1" | "true" | "yes" | "on") => true,
            Some("0" | "false" | "no" | "off") => false,
            _ => fallback,
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
            title_scan_max_bytes: num("TROVADOR_TITLE_SCAN_MAX_BYTES", d.title_scan_max_bytes),
            record_chars: num("TROVADOR_RECORD_CHARS", d.record_chars),
            tool_chars: num("TROVADOR_TOOL_CHARS", d.tool_chars),
            index_tool_chars: num("TROVADOR_INDEX_TOOL_CHARS", d.index_tool_chars),
            unrecognized_chars: num("TROVADOR_UNRECOGNIZED_CHARS", d.unrecognized_chars),
            page_size: num("TROVADOR_PAGE_SIZE", d.page_size).max(1),
            resume_limit_bytes: num("TROVADOR_RESUME_LIMIT", d.resume_limit_bytes as usize) as u64,
            list_default: num("TROVADOR_LIST_DEFAULT", d.list_default).max(1),
            list_max: num("TROVADOR_LIST_MAX", d.list_max).max(1),
            index_budget_ms: num("TROVADOR_INDEX_BUDGET_MS", d.index_budget_ms),
            progress_after_ms: num("TROVADOR_PROGRESS_AFTER_MS", d.progress_after_ms),
            search_default: num("TROVADOR_SEARCH_DEFAULT", d.search_default).max(1),
            search_max: num("TROVADOR_SEARCH_MAX", d.search_max).max(1),
            per_session_default: num("TROVADOR_PER_SESSION", d.per_session_default).max(1),
            per_session_max: num("TROVADOR_PER_SESSION_MAX", d.per_session_max).max(1),
            list_lookahead: num("TROVADOR_LIST_LOOKAHEAD", d.list_lookahead),
            offset_max: num("TROVADOR_OFFSET_MAX", d.offset_max),
            active_window_s: num("TROVADOR_ACTIVE_WINDOW_S", d.active_window_s),
            index_reasoning: get("TROVADOR_INDEX_REASONING")
                .map(|v| !matches!(v.trim(), "0" | "false" | "no" | "off"))
                .unwrap_or(d.index_reasoning),
            include_active: flag("TROVADOR_INCLUDE_ACTIVE", d.include_active),
            include_automated: flag("TROVADOR_INCLUDE_AUTOMATED", d.include_automated),
            include_subagents: flag("TROVADOR_INCLUDE_SUBAGENTS", d.include_subagents),
            index_redact: flag("TROVADOR_INDEX_REDACT", d.index_redact),
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
    fn request_ceilings_default_to_values_that_fit_a_context_and_can_be_raised() {
        let d = Limits::default();
        assert_eq!(
            (
                d.response_max_chars,
                d.list_max,
                d.search_max,
                d.per_session_max
            ),
            (40_000, 100, 50, 10)
        );
        let l = Limits::from_lookup(lookup(&[
            ("TROVADOR_LIST_MAX", "500"),
            ("TROVADOR_SEARCH_MAX", "200"),
            ("TROVADOR_PER_SESSION_MAX", "0"),
        ]));
        assert_eq!((l.list_max, l.search_max, l.per_session_max), (500, 200, 1));
    }

    #[test]
    fn the_offset_ceiling_can_be_overridden() {
        assert_eq!(Limits::default().offset_max, 1_000_000);
        let l = Limits::from_lookup(lookup(&[("TROVADOR_OFFSET_MAX", "50")]));
        assert_eq!(l.offset_max, 50);
        let l = Limits::from_lookup(lookup(&[("TROVADOR_LIST_LOOKAHEAD", "7")]));
        assert_eq!(l.list_lookahead, 7);
    }

    #[test]
    fn the_unrecognized_cap_can_be_overridden_or_turned_off() {
        assert_eq!(Limits::default().unrecognized_chars, 1_000);
        let l = Limits::from_lookup(lookup(&[("TROVADOR_UNRECOGNIZED_CHARS", "250")]));
        assert_eq!(l.unrecognized_chars, 250);
        let l = Limits::from_lookup(lookup(&[("TROVADOR_UNRECOGNIZED_CHARS", "0")]));
        assert_eq!(l.unrecognized_chars, 0);
    }

    #[test]
    fn visibility_defaults_read_clear_words_and_ignore_anything_else() {
        let d = Limits::default();
        assert!(!d.include_active && !d.include_automated && !d.include_subagents);
        let l = Limits::from_lookup(lookup(&[
            ("TROVADOR_INCLUDE_ACTIVE", "yes"),
            ("TROVADOR_INCLUDE_AUTOMATED", " 1 "),
            ("TROVADOR_INCLUDE_SUBAGENTS", "true"),
        ]));
        assert!(l.include_active && l.include_automated && l.include_subagents);
        let l = Limits::from_lookup(lookup(&[("TROVADOR_INCLUDE_SUBAGENTS", "sure")]));
        assert!(!l.include_subagents, "a typo must not widen results");
    }

    #[test]
    fn index_redaction_is_off_unless_clearly_turned_on() {
        assert!(!Limits::default().index_redact);
        assert!(Limits::from_lookup(lookup(&[("TROVADOR_INDEX_REDACT", "on")])).index_redact);
        assert!(!Limits::from_lookup(lookup(&[("TROVADOR_INDEX_REDACT", "please")])).index_redact);
    }

    #[test]
    fn the_title_scan_budget_can_be_overridden_or_disabled() {
        let l = Limits::from_lookup(lookup(&[("TROVADOR_TITLE_SCAN_MAX_BYTES", "12345")]));
        assert_eq!(l.title_scan_max_bytes, 12345);
        let l = Limits::from_lookup(lookup(&[("TROVADOR_TITLE_SCAN_MAX_BYTES", "0")]));
        assert_eq!(l.title_scan_max_bytes, 0);
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

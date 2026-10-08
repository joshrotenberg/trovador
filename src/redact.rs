//! Secrets taken out of what trovador returns.
//!
//! Transcripts hold whatever was pasted into a session or printed by a tool: API keys,
//! tokens, private keys. Both doors pass their output through [`redact_value`] by default,
//! so the CLI and the MCP server still return identical JSON. The index itself is not
//! redacted, so a search for a token still finds the session; the excerpt it returns says
//! `[redacted:<kind>]`. The only way to turn this off is a flag given when the process
//! starts, never an argument a connected client can pass.

use std::borrow::Cow;
use std::sync::OnceLock;

use regex::{Captures, Regex};
use serde_json::Value;

struct Rule {
    kind: &'static str,
    re: Regex,
    /// Capture groups kept in front of the redaction, such as `Bearer ` or `NAME=`.
    keep: usize,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let rule = |kind, pattern: &str, keep| Rule {
            kind,
            re: Regex::new(pattern).expect("redaction pattern"),
            keep,
        };
        // Order matters: the more specific shapes run first, so an Anthropic key is not
        // reported as an OpenAI one.
        vec![
            rule(
                "private_key",
                r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(?:-----END [A-Z ]*PRIVATE KEY-----|\z)",
                0,
            ),
            rule("anthropic", r"sk-ant-[A-Za-z0-9_-]{20,}", 0),
            rule("openai", r"\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{20,}", 0),
            rule(
                "github",
                r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})",
                0,
            ),
            rule("aws", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", 0),
            rule("slack", r"\bxox[abposr]-[A-Za-z0-9-]{10,}", 0),
            rule("google", r"\bAIza[0-9A-Za-z_-]{35}", 0),
            rule(
                "jwt",
                r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
                0,
            ),
            rule(
                "bearer",
                r"(?i)(\bbearer\s+)([A-Za-z0-9._~+/=-]{16,})",
                1,
            ),
            rule(
                "assignment",
                // The name may be quoted, and the quotes escaped, as in JSON tool arguments
                // and JSON inside a string; unquoted names alone let `"password": "..."`
                // through (#23). The value stops at a backslash for the escaped form.
                r#"(?i)(\b[A-Z0-9_.-]*(?:SECRET|TOKEN|PASSWORD|PASSWD|API[_-]?KEY|ACCESS[_-]?KEY|PRIVATE[_-]?KEY)[A-Z0-9_.-]*(?:\\?["'])?\s*[=:]\s*(?:\\?["'])?)([^\s"'`,;\\]{6,})"#,
                1,
            ),
        ]
    })
}

/// The text with every secret replaced by `[redacted:<kind>]`. Borrowed when nothing
/// matched, which is the common case.
pub fn redact_str(text: &str) -> Cow<'_, str> {
    let mut out: Cow<str> = Cow::Borrowed(text);
    for rule in rules() {
        if !rule.re.is_match(&out) {
            continue;
        }
        let replaced = rule
            .re
            .replace_all(&out, |c: &Captures| {
                let whole = c.get(0).map(|m| m.as_str()).unwrap_or("");
                let value = c.get(rule.keep + 1).map(|m| m.as_str()).unwrap_or(whole);
                // `max_tokens: 4000` and `TOKEN_COUNT=123456` name a token but are not one,
                // and a value an earlier rule already replaced keeps its more specific kind.
                if rule.kind == "assignment"
                    && (value.chars().all(|ch| ch.is_ascii_digit())
                        || value.starts_with("[redacted:"))
                {
                    return whole.to_string();
                }
                let prefix: String = (1..=rule.keep)
                    .filter_map(|i| c.get(i).map(|m| m.as_str()))
                    .collect();
                format!("{prefix}[redacted:{}]", rule.kind)
            })
            .into_owned();
        out = Cow::Owned(replaced);
    }
    out
}

/// Redacts every string in a JSON value in place. Object keys are left alone: they are
/// trovador's field names, not session content.
pub fn redact_value(v: &mut Value) {
    match v {
        Value::String(s) => {
            if let Cow::Owned(r) = redact_str(s) {
                *s = r;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        Value::Object(map) => map.values_mut().for_each(redact_value),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String {
        redact_str(s).into_owned()
    }

    #[test]
    fn each_kind_of_secret_is_replaced_and_named() {
        let cases = [
            (
                "key sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123",
                "key [redacted:anthropic]",
            ),
            ("sk-proj-abcdefghijklmnopqrstuvwx1234", "[redacted:openai]"),
            (
                "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
                "[redacted:github]",
            ),
            (
                "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEFGH",
                "[redacted:github]",
            ),
            ("AKIAIOSFODNN7EXAMPLE", "[redacted:aws]"),
            ("xoxb-1234567890-abcdefghij", "[redacted:slack]"),
            (
                "AIzaSyA1234567890abcdefghijklmnopqrstuv",
                "[redacted:google]",
            ),
            (
                "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N",
                "[redacted:jwt]",
            ),
            (
                "Authorization: Bearer abcdef0123456789xyz",
                "Authorization: Bearer [redacted:bearer]",
            ),
            (
                "export DB_PASSWORD=hunter2hunter2",
                "export DB_PASSWORD=[redacted:assignment]",
            ),
            (
                "api_key: \"s3cr3t-value-here\"",
                "api_key: \"[redacted:assignment]\"",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(r(input), want, "{input}");
        }
    }

    #[test]
    fn quoted_and_escaped_json_fields_are_redacted() {
        let cases = [
            (
                r#"{"password":"synthetic-passphrase"}"#,
                r#"{"password":"[redacted:assignment]"}"#,
            ),
            (
                r#"{"api_key": "synthetic-generic-key"}"#,
                r#"{"api_key": "[redacted:assignment]"}"#,
            ),
            (
                r#"'token': 'synthetic-access-token'"#,
                r#"'token': '[redacted:assignment]'"#,
            ),
            (
                r#"{\"token\":\"synthetic-access-token\",\"n\":1}"#,
                r#"{\"token\":\"[redacted:assignment]\",\"n\":1}"#,
            ),
        ];
        for (input, want) in cases {
            assert_eq!(r(input), want, "{input}");
        }
        for fine in [
            r#""max_tokens": 4000"#,
            r#""input_tokens":12"#,
            r#""token_count": "123456789""#,
        ] {
            assert_eq!(r(fine), fine);
        }
    }

    #[test]
    fn a_private_key_block_is_replaced_whole_even_when_cut_short() {
        let pem = "before\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\nAAAA\n-----END OPENSSH PRIVATE KEY-----\nafter";
        assert_eq!(r(pem), "before\n[redacted:private_key]\nafter");
        let cut = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow ...[truncated]";
        assert_eq!(r(cut), "[redacted:private_key]");
    }

    #[test]
    fn ids_hashes_paths_and_token_counts_are_left_alone() {
        for s in [
            "27d1f434-b559-4583-8026-7c2ba30ac060",
            "commit 3214f33a9c0e1b2d4f5a6b7c8d9e0f1a2b3c4d5e",
            "/Users/me/Code/github.com/joshrotenberg/trovador/src/redact.rs",
            "max_tokens: 4000",
            "TOKEN_COUNT=123456",
            "\"input_tokens\":12345678",
            "the password field is required",
            "sk-short",
        ] {
            assert_eq!(r(s), s);
        }
    }

    #[test]
    fn redacting_a_value_changes_strings_but_never_keys() {
        let mut v = serde_json::json!({
            "OPENAI_API_KEY": "fine as a key",
            "records": [{"text": "OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyz"}],
            "n": 3
        });
        redact_value(&mut v);
        assert_eq!(v["OPENAI_API_KEY"], "fine as a key");
        assert_eq!(v["records"][0]["text"], "OPENAI_API_KEY=[redacted:openai]");
        assert_eq!(v["n"], 3);
    }
}

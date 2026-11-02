//! Long text bounded by `response_max_chars`, with the rest read by offset.
//!
//! A session's text can be any length, and whatever a tool returns lands in the caller's
//! context. A fake provider's 120,000-character reply came back whole with the budget set
//! to 1,000 (trovador#32). Text over the budget is cut, the response says so, and the rest
//! is read by offset.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Present when the text returned is not the whole text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Truncated {
    /// Characters in the whole text.
    pub total_chars: usize,
    /// The character the returned text starts at.
    pub offset: usize,
    pub returned_chars: usize,
    /// Where the next part starts. Absent on the last part.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    /// How to read the next part, from the MCP server and from the CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continue_with: Option<String>,
}

/// The text that parts are cut from: redacted as a whole when redaction is on. Cutting
/// first let a part start or end inside a secret, and a fragment such as `k-ant-api03-...`
/// matches no rule, so the rest of the key came back in the clear (pre-release review).
/// Offsets therefore count characters of the redacted text.
pub fn source<'a>(text: &'a str, limits: &crate::limits::Limits) -> std::borrow::Cow<'a, str> {
    if limits.redact {
        crate::redact::redact_str(text)
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// Up to `max` characters of `text` from character `offset`, and what was left out.
pub fn part(
    text: &str,
    offset: usize,
    max: usize,
    continue_with: impl Fn(usize) -> String,
) -> (String, Option<Truncated>) {
    let total = text.chars().count();
    let offset = offset.min(total);
    let returned: String = text.chars().skip(offset).take(max.max(1)).collect();
    let n = returned.chars().count();
    if offset == 0 && n == total {
        return (returned, None);
    }
    let next = (offset + n < total).then_some(offset + n);
    (
        returned,
        Some(Truncated {
            total_chars: total,
            offset,
            returned_chars: n,
            next_offset: next,
            continue_with: next.map(continue_with),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_cover_the_text_exactly_once_on_character_boundaries() {
        let text = "héllo wörld, ünïcode";
        let mut offset = 0;
        let mut joined = String::new();
        loop {
            let (t, tr) = part(text, offset, 3, |n| n.to_string());
            joined.push_str(&t);
            match tr.and_then(|t| t.next_offset) {
                Some(n) => offset = n,
                None => break,
            }
        }
        assert_eq!(joined, text);
        assert_eq!(part(text, 0, 100, |_| String::new()), (text.into(), None));
        let (t, tr) = part(text, 999, 3, |_| String::new());
        assert_eq!(t, "");
        assert_eq!(tr.unwrap().next_offset, None);
    }
}

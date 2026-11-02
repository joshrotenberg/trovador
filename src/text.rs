//! A whole session as plain text, for the caller to read or summarize.
//!
//! trovador runs no model. It once summarized sessions with haiku and cached the result;
//! the agent asking is already a model, so it is handed the same rendering that summary
//! was made from and summarizes it in its own context, with its own judgment of what
//! matters. A session can be any size, so the text comes in parts of
//! `response_max_chars`, and the caller decides how many to read.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::archive::{ArchiveEntry, Backend};
use crate::limits::Limits;
use crate::output::{self, Truncated};
use crate::transcript::{self, Projection, RecordKind};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SessionText {
    pub backend: Backend,
    pub session_id: String,
    /// One part of the session's text: one line block per record, oldest first.
    pub text: String,
    /// Set when `text` is not the whole session. Read the rest from `next_offset`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<Truncated>,
}

/// The transcript as text, without omitted records, which carry nothing to read, or
/// reasoning headings, which say little about what happened and would make every part
/// longer.
pub fn render(e: &ArchiveEntry, limits: &Limits) -> std::io::Result<String> {
    let mut out = String::new();
    let mut projection = Projection::open(e, limits)?;
    for r in projection.by_ref() {
        if matches!(r.kind, RecordKind::Omitted | RecordKind::Reasoning) {
            continue;
        }
        out.push_str(&transcript::render(&r));
        out.push('\n');
    }
    // A compressed rollout that stops decoding partway would otherwise come back as a
    // shorter session, and a caller summarizing it could not tell.
    match projection.take_read_error() {
        Some(error) => Err(error),
        None => Ok(out),
    }
}

/// At most `response_max_chars` of the session's text from character `offset`. Every part
/// renders the whole session again, so a caller that reads them all pays for the file once
/// per part; the alternative was a cache of rendered text that would hold transcript
/// content, secrets included, outside the provider's archive.
pub fn part(e: &ArchiveEntry, offset: usize, limits: &Limits) -> std::io::Result<SessionText> {
    let whole = render(e, limits)?;
    let source = output::source(&whole, limits);
    let id = e.session_id.clone();
    let backend = e.backend;
    let (text, truncated) = output::part(&source, offset, limits.response_max_chars, |n| {
        format!(
            "sessions action=text session_id={id} backend={backend} offset={n}; CLI: trovador sessions text {id} --backend {backend} --offset {n}"
        )
    });
    Ok(SessionText {
        backend,
        session_id: e.session_id.clone(),
        text,
        truncated,
    })
}

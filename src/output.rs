//! Model text bounded by `response_max_chars`, with the rest recoverable.
//!
//! A provider reply or a summary can be any length, and whatever a tool returns lands in
//! the caller's context. A fake provider's 120,000-character reply came back whole with
//! the budget set to 1,000 (#32). Text over the budget is cut, the response says so, and
//! the rest is read by offset: a summary from its cache, a reply from the copy saved here.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::sessions::Ctx;

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

/// A saved reply, or one part of it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RunOutput {
    pub run_id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<Truncated>,
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("run id \"{0}\" is not a uuid")]
    BadId(String),
    #[error(
        "no saved output for run {0}: a reply is saved only when it exceeded response_max_chars, and gc removes it after its cutoff"
    )]
    NotFound(String),
    #[error("reading the output of run {run_id}: {source}")]
    Io {
        run_id: String,
        #[source]
        source: std::io::Error,
    },
}

pub fn dir(paths: &Paths) -> PathBuf {
    paths.home.join("outputs")
}

/// Only a uuid names a file, so a run id cannot reach any other path.
fn file(paths: &Paths, run_id: &str) -> Result<PathBuf, OutputError> {
    let id = uuid::Uuid::parse_str(run_id).map_err(|_| OutputError::BadId(run_id.into()))?;
    Ok(dir(paths).join(format!("{}.txt", id.hyphenated())))
}

pub fn continue_run(run_id: &str) -> impl Fn(usize) -> String + '_ {
    move |n| {
        format!(
            "sessions action=output run_id={run_id} offset={n}; CLI: trovador sessions output {run_id} --offset {n}"
        )
    }
}

/// Saved private to the user: a reply can quote secrets, which are redacted only on the
/// way out.
pub(crate) fn save(paths: &Paths, run_id: &str, text: &str) -> std::io::Result<()> {
    let path = file(paths, run_id).map_err(std::io::Error::other)?;
    std::fs::create_dir_all(dir(paths))?;
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(text.as_bytes())
}

pub fn read(ctx: &Ctx, run_id: &str, offset: usize) -> Result<RunOutput, OutputError> {
    let path = file(&ctx.paths, run_id)?;
    let text = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => OutputError::NotFound(run_id.into()),
        _ => OutputError::Io {
            run_id: run_id.into(),
            source: e,
        },
    })?;
    let (text, truncated) = part(
        &source(&text, &ctx.limits),
        offset,
        ctx.limits.response_max_chars,
        continue_run(run_id),
    );
    Ok(RunOutput {
        run_id: run_id.into(),
        text,
        truncated,
    })
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

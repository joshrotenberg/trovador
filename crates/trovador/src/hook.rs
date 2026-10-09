//! `trovador hook session-start`: tells a new Claude Code or Codex session its own id.
//!
//! The MCP server cannot tell which session is calling it, so search leaves out every
//! session written in the last few minutes to be sure of leaving out the caller. That also
//! hides other sessions running right now, on either agent, which are often the ones worth
//! finding. Both hosts pass the session id to a SessionStart hook; this turns it into
//! context naming the exact `exclude_session` to pass.

use serde::Deserialize;
use serde_json::json;

use crate::archive::Backend;

#[derive(Deserialize)]
struct SessionStart {
    session_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
}

/// The hook's stdout for this input, or None when the input is not a SessionStart payload.
/// A hook must never block a session from starting, so the caller prints nothing and exits
/// successfully on None.
pub fn session_start(input: &str, backend: Option<Backend>) -> Option<String> {
    let start: SessionStart = serde_json::from_str(input).ok()?;
    let id = start.session_id.trim();
    if id.is_empty() || id.len() > 1024 || id.contains(['"', '\n', '\0']) {
        return None;
    }
    let backend = backend.unwrap_or_else(|| {
        // Codex writes rollouts under ~/.codex; anything else is Claude Code.
        match start.transcript_path.as_deref() {
            Some(p) if p.contains("/.codex/") => Backend::Codex,
            _ => Backend::Claude,
        }
    });
    let context = format!(
        "This conversation is {backend} session {id}. When searching past sessions with \
         trovador's search tool, pass exclude_session=\"{id}\" and include_active=true: that \
         leaves out only this conversation and keeps other sessions running now, in Claude \
         Code or Codex."
    );
    Some(
        json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": context}})
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(out: &str) -> String {
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn names_the_session_and_its_backend_from_either_host() {
        let claude = r#"{"session_id":"27d1f434-b559-4583-8026-7c2ba30ac060","transcript_path":"/Users/me/.claude/projects/-w/27d1f434-b559-4583-8026-7c2ba30ac060.jsonl","cwd":"/w","hook_event_name":"SessionStart","source":"startup"}"#;
        let c = context(&session_start(claude, None).unwrap());
        assert!(c.contains("claude session 27d1f434-b559-4583-8026-7c2ba30ac060"));
        assert!(c.contains("exclude_session=\"27d1f434-b559-4583-8026-7c2ba30ac060\""));

        let codex = r#"{"session_id":"01a007a0-134e-70e2-84e5-a19ae6996317","transcript_path":"/Users/me/.codex/sessions/2026/10/08/rollout-x.jsonl"}"#;
        assert!(context(&session_start(codex, None).unwrap()).contains("codex session 01a007a0"));
        // An explicit backend wins over the path.
        assert!(
            context(&session_start(codex, Some(Backend::Claude)).unwrap())
                .contains("claude session")
        );
    }

    #[test]
    fn anything_else_produces_no_output() {
        for bad in [
            "",
            "not json",
            "{}",
            r#"{"session_id":""}"#,
            r#"{"session_id":"a\"b"}"#,
            r#"{"session_id":7}"#,
        ] {
            assert_eq!(session_start(bad, None), None, "{bad}");
        }
    }
}

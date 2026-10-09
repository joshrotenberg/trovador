//! Validate explicit settings without rewriting them or guessing CLI defaults.

use crate::types::{Backend, Effort};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::LazyLock};

#[derive(Deserialize)]
struct Capabilities {
    models: BTreeMap<String, BTreeMap<String, Vec<Effort>>>,
}

static CAPABILITIES: LazyLock<Capabilities> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../resources/model-capabilities.json"))
        .expect("embedded model capability table must be valid")
});

/// Check backend values and a small, dated table of known models.
/// Aliases, unknown IDs and omitted models defer compatibility to the provider;
/// validation is not proof of account/model availability or effective settings.
pub fn validate(
    backend: Backend,
    model: Option<&str>,
    effort: Option<Effort>,
) -> Result<(), String> {
    let Some(effort) = effort else {
        return Ok(());
    };
    if backend == Backend::Claude
        && matches!(effort, Effort::None | Effort::Minimal | Effort::Ultra)
    {
        return Err(format!(
            "Claude effort {} is unsupported; choose low, medium, high, xhigh, or max",
            effort.as_str()
        ));
    }
    let Some(model) = model else {
        return Ok(());
    };
    let provider = match backend {
        Backend::Claude => "claude",
        Backend::Codex => "codex",
    };
    let models = &CAPABILITIES.models[provider];
    // Match only known generations, not substrings of aliases/deployment names.
    let canonical = if backend == Backend::Claude {
        let base = model.strip_suffix("[1m]").unwrap_or(model);
        if let Some((base, date)) = base.rsplit_once('-')
            && date.len() == 8
            && date.bytes().all(|b| b.is_ascii_digit())
        {
            base
        } else {
            base
        }
    } else {
        model
    };
    if let Some(supported) = models.get(canonical)
        && !supported.contains(&effort)
    {
        return Err(format!(
            "effort {} is unsupported for known {provider} model {model}; supported: {}. Choose supported settings explicitly; no fallback was started",
            effort.as_str(),
            supported
                .iter()
                .map(|e| e.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_known_incompatibilities_without_guessing_aliases() {
        for model in ["claude-opus-4-6", "claude-sonnet-4-6-20260217[1m]"] {
            assert!(validate(Backend::Claude, Some(model), Some(Effort::Xhigh)).is_err());
            assert!(validate(Backend::Claude, Some(model), Some(Effort::Max)).is_ok());
        }
        assert!(validate(Backend::Codex, Some("gpt-5.4"), Some(Effort::Max)).is_err());
        assert!(validate(Backend::Codex, Some("gpt-6-luna"), Some(Effort::Ultra)).is_err());
        assert!(validate(Backend::Codex, Some("gpt-6-sol"), Some(Effort::None)).is_err());
        assert!(validate(Backend::Codex, Some("gpt-6-sol"), Some(Effort::Ultra)).is_ok());
        for model in [None, Some("opus"), Some("my-custom-deployment")] {
            assert!(validate(Backend::Claude, model, Some(Effort::Xhigh)).is_ok());
            assert!(validate(Backend::Claude, model, Some(Effort::Minimal)).is_err());
        }
        assert!(validate(Backend::Codex, Some("future-model"), Some(Effort::Max)).is_ok());
        assert!(validate(Backend::Codex, Some("gpt-5.4"), None).is_ok());
    }
}

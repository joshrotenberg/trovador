//! Environment compatibility for the Task Runners to Tosi rename.

use std::{env, ffi::OsString};

/// Read a TOSI_* value, using TASK_RUNNERS_* only when the primary key is absent.
/// Empty and non-Unicode primary values remain authoritative.
pub fn var_os(suffix: &str) -> Option<OsString> {
    lookup(suffix, |name| env::var_os(name))
}

/// Read a text setting without falling back after an invalid primary value.
pub fn var(suffix: &str) -> Result<String, env::VarError> {
    var_os(suffix)
        .ok_or(env::VarError::NotPresent)?
        .into_string()
        .map_err(env::VarError::NotUnicode)
}

/// Either generation's marker blocks nested servers, independently of precedence.
pub fn is_delegated_child() -> bool {
    ["TOSI_CHILD", "TASK_RUNNERS_CHILD"]
        .iter()
        .any(|name| env::var_os(name).is_some_and(|value| value == "1"))
}

fn lookup(suffix: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
    get(&format!("TOSI_{suffix}")).or_else(|| get(&format!("TASK_RUNNERS_{suffix}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_test_opt_in_and_backend_selection_keep_primary_precedence() {
        for suffix in ["LIVE_TESTS", "LIVE_BACKENDS"] {
            let legacy = if suffix == "LIVE_TESTS" {
                "1"
            } else {
                "claude"
            };
            for primary in [None, Some(""), Some("0"), Some("invalid")] {
                let value = lookup(suffix, |key| {
                    if key.starts_with("TOSI_") {
                        primary.map(OsString::from)
                    } else {
                        Some(OsString::from(legacy))
                    }
                });
                assert_eq!(value, Some(OsString::from(primary.unwrap_or(legacy))));
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn binary_overrides_preserve_non_unicode_paths_for_both_names() {
        use std::os::unix::ffi::OsStringExt;

        let path = OsString::from_vec(b"/tmp/provider-\xff".to_vec());
        for primary_exists in [false, true] {
            let value = lookup("CLAUDE_BIN", |key| {
                if key.starts_with("TOSI_") == primary_exists {
                    Some(path.clone())
                } else {
                    None
                }
            });
            assert_eq!(value, Some(path.clone()));
        }
    }
}

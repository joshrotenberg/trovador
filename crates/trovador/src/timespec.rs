//! Times given by a person or an agent: an RFC 3339 time, a date, or a duration back from
//! now. Used by `since` and `until`.

use std::time::{Duration, SystemTime};

pub const FORMS: &str = "an RFC 3339 time (2026-10-01T09:00:00Z), a date (2026-10-01, midnight UTC), or a duration back from now (90m, 36h, 2d, 1w)";

pub fn parse(s: &str, now: SystemTime) -> Result<SystemTime, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(format!("an empty time; give {FORMS}"));
    }
    // A bare number is ambiguous between a date, a timestamp, and a duration.
    if s.chars().next().is_some_and(|c| c.is_ascii_digit())
        && !s.contains('-')
        && let Ok(d) = humantime::parse_duration(s)
    {
        return now
            .checked_sub(d)
            .ok_or_else(|| format!("\"{s}\" reaches before 1970"));
    }
    if s.len() == 10 && s.as_bytes().get(4) == Some(&b'-') {
        return humantime::parse_rfc3339(&format!("{s}T00:00:00Z"))
            .map_err(|_| format!("\"{s}\" is not a date; give {FORMS}"));
    }
    humantime::parse_rfc3339_weak(s).map_err(|_| format!("\"{s}\" is not a time; give {FORMS}"))
}

/// Nanoseconds since the epoch, as the index stores mtimes.
pub fn ns(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> SystemTime {
        humantime::parse_rfc3339(s).unwrap()
    }

    #[test]
    fn durations_measure_back_from_now() {
        let now = at("2026-10-07T12:00:00Z");
        assert_eq!(parse("36h", now).unwrap(), at("2026-10-06T00:00:00Z"));
        assert_eq!(parse("2d", now).unwrap(), at("2026-10-05T12:00:00Z"));
        assert_eq!(parse("1w", now).unwrap(), at("2026-09-30T12:00:00Z"));
        assert_eq!(parse("90m", now).unwrap(), at("2026-10-07T10:30:00Z"));
    }

    #[test]
    fn dates_and_times_are_absolute_and_utc() {
        let now = at("2026-10-07T12:00:00Z");
        assert_eq!(
            parse("2026-10-01", now).unwrap(),
            at("2026-10-01T00:00:00Z")
        );
        assert_eq!(
            parse("2026-10-01T09:30:00Z", now).unwrap(),
            at("2026-10-01T09:30:00Z")
        );
        assert_eq!(
            parse("2026-10-01 09:30:00", now).unwrap(),
            at("2026-10-01T09:30:00Z")
        );
    }

    #[test]
    fn anything_else_is_an_error_naming_the_forms() {
        let now = at("2026-10-07T12:00:00Z");
        for bad in ["", "yesterday", "2026-13-40", "12"] {
            let e = parse(bad, now).unwrap_err();
            assert!(e.contains("RFC 3339"), "{bad}: {e}");
        }
    }
}

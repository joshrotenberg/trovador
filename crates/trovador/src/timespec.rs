//! Times given by a person or an agent: an RFC 3339 time, a date, or a duration back from
//! now. Used by `since` and `until`.

use std::time::{Duration, SystemTime};

pub const FORMS: &str = "an RFC 3339 time (2026-10-01T09:00:00Z or 2026-10-01T09:00:00-07:00), a date (2026-10-01, midnight UTC), or a duration back from now (90m, 36h, 2d, 1w)";

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
    parse_absolute(s).ok_or_else(|| format!("\"{s}\" is not a time; give {FORMS}"))
}

fn parse_absolute(s: &str) -> Option<SystemTime> {
    let offset_start = s.len().saturating_sub(6);
    if let Some(offset) = s.get(offset_start..)
        && matches!(offset.as_bytes().first(), Some(b'+' | b'-'))
    {
        // Issue #32: humantime rejects numeric zones, so apply the offset separately.
        let bytes = offset.as_bytes();
        if bytes.len() != 6
            || bytes[3] != b':'
            || ![bytes[1], bytes[2], bytes[4], bytes[5]]
                .iter()
                .all(u8::is_ascii_digit)
        {
            return None;
        }
        let hours = offset[1..3].parse::<u64>().ok()?;
        let minutes = offset[4..6].parse::<u64>().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        let datetime = &s[..offset_start];
        // Issue #32: humantime can accept trailing junk after Z, including a second zone.
        let fraction = datetime.as_bytes().get(19..)?;
        if !fraction.is_empty()
            && (fraction[0] != b'.'
                || fraction.len() == 1
                || !fraction[1..].iter().all(u8::is_ascii_digit))
        {
            return None;
        }
        let local = humantime::parse_rfc3339_weak(&format!("{datetime}Z")).ok()?;
        let adjustment = Duration::from_secs(hours * 3600 + minutes * 60);
        if bytes[0] == b'+' {
            local.checked_sub(adjustment)
        } else {
            local.checked_add(adjustment)
        }
    } else {
        humantime::parse_rfc3339_weak(s).ok()
    }
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
    fn numeric_offsets_resolve_to_the_equivalent_utc_instant() {
        let now = at("2026-10-07T12:00:00Z");
        for (input, utc) in [
            ("2026-10-01T09:00:00+02:00", "2026-10-01T07:00:00Z"),
            ("2026-10-01T09:00:00-07:00", "2026-10-01T16:00:00Z"),
            (
                "2026-10-01T00:15:00.123456789+05:30",
                "2026-09-30T18:45:00.123456789Z",
            ),
            ("2026-12-31T23:45:00-00:30", "2027-01-01T00:15:00Z"),
            ("2026-10-01T09:00:00+00:00", "2026-10-01T09:00:00Z"),
            ("2026-10-01T09:00:00-00:00", "2026-10-01T09:00:00Z"),
        ] {
            assert_eq!(parse(input, now).unwrap(), at(utc), "{input}");
        }
    }

    #[test]
    fn malformed_numeric_offsets_are_rejected_without_panicking() {
        let now = at("2026-10-07T12:00:00Z");
        for offset in [
            "+24:00",
            "-01:60",
            "+2:00",
            "+0200",
            "+aa:00",
            "+00:0a",
            "+00:00Z",
            "+💥:0",
            "+00:00+01:00",
            "Z-07:00",
            "Z+00:00",
            "Zabc+02:00",
            "z-07:00",
            ".123Z+02:00",
            ".+02:00",
            ".12.3+02:00",
        ] {
            let input = format!("2026-10-01T09:00:00{offset}");
            assert!(
                parse(&input, now).unwrap_err().contains("RFC 3339"),
                "{input}"
            );
        }
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

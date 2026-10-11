//! The daily automatic check (`check_due` of `update.py`) and the timestamp the GUI keeps in
//! settings.json (`last_update_check`, written by Python as `datetime.isoformat()`).

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};

use crate::CHECK_INTERVAL;

/// An ISO timestamp with a UTC offset: RFC 3339 (what Python's `isoformat()` writes for an aware
/// datetime, with or without fractional seconds, or with `Z`) or a `+hhmm` offset.
fn parse_aware(text: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(text)
        .or_else(|_| DateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .ok()
}

/// Whether the daily automatic check should run: `last_check` is the timestamp of the last one;
/// an empty, unparsable or offset-less one means yes.
pub fn check_due(last_check: &str, now: DateTime<Utc>) -> bool {
    match parse_aware(last_check) {
        Some(then) => now.signed_duration_since(then) >= CHECK_INTERVAL,
        None => true,
    }
}

/// `now` as Python's `datetime.isoformat()` writes a UTC time without microseconds:
/// `2026-10-11T09:05:11+00:00`.
pub fn now_stamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Secs, false)
}

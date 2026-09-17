//! Timestamps: stored as RFC 3339 UTC with millisecond precision and a `Z` suffix
//! (`2026-09-17T06:30:00.000Z`) so string order is time order; digest dates are the local day.

use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TimeError {
    #[error("'{0}' is not an IANA timezone")]
    BadTimezone(String),
}

/// The stored form of a timestamp.
pub fn to_iso(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Parses anything RFC 3339 back into UTC; `None` for garbage.
pub fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// The stored form of `now - days`.
pub fn days_ago_iso(now: DateTime<Utc>, days: u32) -> String {
    to_iso(now - Duration::days(i64::from(days)))
}

/// Parses a validated timezone name.
pub fn parse_tz(name: &str) -> Result<Tz, TimeError> {
    Tz::from_str(name).map_err(|_| TimeError::BadTimezone(name.to_string()))
}

/// The local calendar day (`YYYY-MM-DD`) of an instant in a zone.
pub fn date_in_zone(t: DateTime<Utc>, tz: Tz) -> String {
    t.with_timezone(&tz).format("%Y-%m-%d").to_string()
}

/// `true` for a well-formed `YYYY-MM-DD` that names a real calendar day.
pub fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    if s.len() != 10 {
        return None;
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// The stored-form UTC bounds `[start, end)` of a local calendar day in `tz`.
pub fn day_bounds_utc(date: &str, tz: Tz) -> Option<(String, String)> {
    use chrono::TimeZone;
    let day = parse_date(date)?;
    let next = day.succ_opt()?;
    let start = tz
        .from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
        .single()?;
    let end = tz
        .from_local_datetime(&next.and_hms_opt(0, 0, 0)?)
        .single()?;
    Some((
        to_iso(start.with_timezone(&Utc)),
        to_iso(end.with_timezone(&Utc)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn formats_millis_z() {
        let t = Utc.with_ymd_and_hms(2026, 9, 17, 6, 30, 0).unwrap();
        assert_eq!(to_iso(t), "2026-09-17T06:30:00.000Z");
        let with_millis = t + Duration::milliseconds(7);
        assert_eq!(to_iso(with_millis), "2026-09-17T06:30:00.007Z");
    }

    #[test]
    fn parse_roundtrips_and_rejects_garbage() {
        let t = Utc.with_ymd_and_hms(2026, 9, 17, 6, 30, 0).unwrap();
        assert_eq!(parse_iso(&to_iso(t)), Some(t));
        assert_eq!(parse_iso("yesterday"), None);
    }

    #[test]
    fn days_ago_is_stringly_ordered_before_now() {
        let now = Utc.with_ymd_and_hms(2026, 9, 17, 6, 30, 0).unwrap();
        let ago = days_ago_iso(now, 7);
        assert_eq!(ago, "2026-09-10T06:30:00.000Z");
        assert!(ago < to_iso(now));
    }

    #[test]
    fn day_bounds_in_ho_chi_minh_are_utc_shifted() {
        let tz = parse_tz("Asia/Ho_Chi_Minh").unwrap();
        let (start, end) = day_bounds_utc("2026-09-17", tz).unwrap();
        assert_eq!(start, "2026-09-16T17:00:00.000Z");
        assert_eq!(end, "2026-09-17T17:00:00.000Z");
        assert!(day_bounds_utc("2026-13-01", tz).is_none());
        assert!(parse_date("2026-9-7").is_none());
        assert!(parse_date("2026-02-30").is_none());
    }

    #[test]
    fn local_date_in_ho_chi_minh_crosses_midnight_utc() {
        let tz = parse_tz("Asia/Ho_Chi_Minh").unwrap();
        let late_utc = Utc.with_ymd_and_hms(2026, 9, 17, 23, 30, 0).unwrap();
        assert_eq!(date_in_zone(late_utc, tz), "2026-09-18");
        assert_eq!(date_in_zone(late_utc, chrono_tz::UTC), "2026-09-17");
        assert_eq!(
            parse_tz("Mars/Olympus").unwrap_err(),
            TimeError::BadTimezone("Mars/Olympus".into())
        );
    }
}

//! Calendar dates in UTC, from and to unix seconds, over `jiff`.

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

/// The UTC calendar date of unix seconds: `(year, month, day)`.
pub fn civil_date(at: u64) -> (i64, u32, u32) {
    let date = date_of(at);
    (
        i64::from(date.year()),
        u32::from(date.month() as u8),
        u32::from(date.day() as u8),
    )
}

fn date_of(at: u64) -> Date {
    let timestamp = i64::try_from(at)
        .ok()
        .and_then(|secs| Timestamp::from_second(secs).ok())
        .unwrap_or(Timestamp::MAX);
    timestamp.to_zoned(TimeZone::UTC).date()
}

/// The calendar date, when it exists (`2026-02-30` does not).
fn date(year: i64, month: u32, day: u32) -> Option<Date> {
    Date::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
    )
    .ok()
}

/// Days since 1970-01-01 of a calendar date; `None` for a date that does not
/// exist.
pub fn days_since_epoch(year: i64, month: u32, day: u32) -> Option<i64> {
    Some(unix_of_date(year, month, day)?.div_euclid(86_400))
}

/// Unix seconds of midnight UTC on a calendar date; `None` for a date that
/// does not exist.
pub fn unix_of_date(year: i64, month: u32, day: u32) -> Option<i64> {
    let zoned = date(year, month, day)?.to_zoned(TimeZone::UTC).ok()?;
    Some(zoned.timestamp().as_second())
}

/// Unix seconds as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn format_time(secs: u64) -> String {
    let date = date_of(secs);
    let rem = secs % 86_400;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        date.year(),
        date.month(),
        date.day(),
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// A day as `YYYY-MM-DD` (UTC), the way a date input gives it.
pub fn format_date(secs: u64) -> String {
    format_time(secs)[..10].to_owned()
}

/// Midnight UTC of a `YYYY-MM-DD` day, as unix seconds; `None` for anything
/// else, a day the month does not have, or a day before 1970.
pub fn parse_date(value: &str) -> Option<u64> {
    let mut parts = value.trim().splitn(3, '-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    u64::try_from(unix_of_date(year, month, day)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_go_to_seconds_and_back() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(951_782_400), (2000, 2, 29));
        assert_eq!(unix_of_date(2000, 2, 29), Some(951_782_400));
        assert_eq!(unix_of_date(1970, 1, 1), Some(0));
        assert_eq!(unix_of_date(2026, 2, 30), None);
        assert_eq!(days_since_epoch(1970, 1, 2), Some(1));
        assert_eq!(format_time(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13:20 UTC");
        assert_eq!(format_date(1_709_164_800), "2024-02-29");
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("2023-11-14"), Some(1_699_920_000));
        assert_eq!(parse_date(" 2024-02-29 "), Some(1_709_164_800));
        for bad in [
            "",
            "2024",
            "2024-13-01",
            "2023-02-29",
            "1969-12-31",
            "x-y-z",
        ] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
    }
}

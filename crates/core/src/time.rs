//! Minimal UTC formatting for epoch-millisecond timestamps.
//!
//! Timada stores event/read-model timestamps as epoch milliseconds; these
//! helpers render them for humans without pulling a date dependency into the
//! framework.

/// Epoch milliseconds → `YYYY-MM-DD` (UTC).
pub fn format_utc_date(epoch_millis: i64) -> String {
    let (year, month, day) = civil_of(epoch_millis);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Epoch milliseconds → `YYYY-MM-DD HH:MM UTC`.
pub fn format_utc_datetime(epoch_millis: i64) -> String {
    let (year, month, day) = civil_of(epoch_millis);
    let second_of_day = epoch_millis.div_euclid(1000).rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        second_of_day / 3600,
        (second_of_day % 3600) / 60
    )
}

fn civil_of(epoch_millis: i64) -> (i64, i64, i64) {
    civil_from_days(epoch_millis.div_euclid(1000).div_euclid(86_400))
}

/// Days since 1970-01-01 to a civil `(year, month, day)`, by Howard Hinnant's
/// `civil_from_days`. Exact for every representable date.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;

    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    (year, month, day)
}

/// Current wall-clock time as epoch milliseconds, the workspace's timestamp
/// convention. A clock before 1970 yields 0, which fails safe everywhere a
/// timestamp gates validity.
pub fn now_millis() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_leap_day_correctly() {
        // 2024-02-29T13:45:07Z
        let millis = 1_709_214_307_000;
        assert_eq!(format_utc_date(millis), "2024-02-29");
        assert_eq!(format_utc_datetime(millis), "2024-02-29 13:45 UTC");
    }

    #[test]
    fn formats_the_epoch_and_pre_epoch_dates() {
        assert_eq!(format_utc_datetime(0), "1970-01-01 00:00 UTC");
        // 1969-12-31T23:00:00Z
        assert_eq!(format_utc_date(-3_600_000), "1969-12-31");
    }
}

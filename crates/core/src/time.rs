use std::time::{SystemTime, UNIX_EPOCH};

/// Current wall-clock time as Unix seconds. Event dates come from the event
/// store's own timestamp; this is only for expiry checks in commands.
pub fn now_unix_secs() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

/// Calendar year of a Unix-seconds timestamp (proleptic Gregorian, UTC).
pub fn year_of(unix_secs: u64) -> i32 {
    // Howard Hinnant's civil-from-days, restricted to the year.
    let days = (unix_secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }) as i32
}

#[cfg(test)]
mod tests {
    use super::year_of;

    #[test]
    fn computes_year() {
        assert_eq!(year_of(0), 1970);
        assert_eq!(year_of(1_732_147_200), 2024); // 2024-11-21
        assert_eq!(year_of(1_735_689_599), 2024); // 2024-12-31T23:59:59Z
        assert_eq!(year_of(1_735_689_600), 2025); // 2025-01-01T00:00:00Z
    }
}

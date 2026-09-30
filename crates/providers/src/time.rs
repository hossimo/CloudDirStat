//! Timestamps from REST APIs as seconds since the Unix epoch. Only what the providers
//! send is supported; anything else is `None` (and the object shows no date).

/// `2026-09-29T12:34:56.789Z` or `2026-09-29T12:34:56+02:00` (RFC 3339).
pub(crate) fn rfc3339_seconds(text: &str) -> Option<u64> {
    let (date, time) = text.split_once('T')?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;

    let clock = time.get(..8)?;
    let seconds = clock_seconds(clock)?;
    let zone = time[8..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match zone {
        "Z" | "z" | "" => 0,
        _ => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (hours, minutes) = zone.get(1..)?.split_once(':')?;
            sign * (hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60)
        }
    };
    unix_seconds(year, month, day, seconds - offset)
}

/// `Wed, 30 Sep 2026 01:27:01 GMT` (the HTTP date format, RFC 7231).
pub(crate) fn http_date_seconds(text: &str) -> Option<u64> {
    let mut parts = text.split_whitespace().skip(1);
    let day = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year = parts.next()?.parse().ok()?;
    let seconds = clock_seconds(parts.next()?)?;
    unix_seconds(year, month, day, seconds)
}

/// Seconds since the Unix epoch as an HTTP date: `Wed, 30 Sep 2026 01:27:01 GMT`.
pub(crate) fn http_date(seconds: u64) -> String {
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = seconds / 86_400;
    let of_day = seconds % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[(days % 7) as usize],
        MONTHS[(month - 1) as usize],
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    )
}

/// `HH:MM:SS` as seconds after midnight.
fn clock_seconds(clock: &str) -> Option<i64> {
    let mut parts = clock.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next()?.parse().ok()?;
    let seconds: i64 = parts.next()?.parse().ok()?;
    Some(hours * 3600 + minutes * 60 + seconds)
}

fn unix_seconds(year: i64, month: i64, day: i64, seconds_of_day: i64) -> Option<u64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + seconds_of_day).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339() {
        assert_eq!(rfc3339_seconds("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(
            rfc3339_seconds("2026-09-29T00:00:00.123Z"),
            Some(1_790_640_000)
        );
        assert_eq!(rfc3339_seconds("2024-02-29T23:59:59Z"), Some(1_709_251_199));
        assert_eq!(
            rfc3339_seconds("2026-09-29T02:00:00+02:00"),
            Some(1_790_640_000)
        );
        assert_eq!(
            rfc3339_seconds("2026-09-28T22:00:00-02:00"),
            Some(1_790_640_000)
        );
        assert_eq!(rfc3339_seconds("not a date"), None);
    }

    #[test]
    fn parses_http_dates() {
        assert_eq!(
            http_date_seconds("Tue, 29 Sep 2026 00:00:00 GMT"),
            Some(1_790_640_000)
        );
        assert_eq!(
            http_date_seconds("Thu, 29 Feb 2024 23:59:59 GMT"),
            Some(1_709_251_199)
        );
        assert_eq!(http_date_seconds("yesterday"), None);
    }

    #[test]
    fn formats_http_dates() {
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(http_date(1_790_640_000), "Tue, 29 Sep 2026 00:00:00 GMT");
        assert_eq!(http_date(1_709_251_199), "Thu, 29 Feb 2024 23:59:59 GMT");
        for seconds in [0, 951_782_400, 1_709_251_199, 4_102_444_800] {
            assert_eq!(http_date_seconds(&http_date(seconds)), Some(seconds));
        }
    }
}

use std::fmt;

const SECONDS_PER_DAY: u64 = 86_400;

/// A calendar day (UTC), stored as days since 1970-01-01 so it fits the two bytes a
/// tree node has to spare. Covers 1970-01-02 to 2149-06-06.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(u16);

impl Date {
    /// The day containing `seconds` since the Unix epoch. `None` for 1970-01-01 itself,
    /// which nodes use to mean "unknown", and for days past 2149.
    pub fn from_unix_seconds(seconds: u64) -> Option<Self> {
        let days = u16::try_from(seconds / SECONDS_PER_DAY).ok()?;
        Self::from_days(days)
    }

    pub(crate) fn from_days(days: u16) -> Option<Self> {
        (days != 0).then_some(Self(days))
    }

    pub(crate) fn days(self) -> u16 {
        self.0
    }

    /// Year, month (1-12), and day (1-31).
    pub fn ymd(self) -> (u32, u32, u32) {
        // Howard Hinnant's civil_from_days, for days on or after 1970-01-01.
        let z = u32::from(self.0) + 719_468;
        let era = z / 146_097;
        let day_of_era = z % 146_097;
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
        let year = year_of_era + era * 400 + u32::from(month <= 2);
        (year, month, day)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = self.ymd();
        write!(f, "{year:04}-{month:02}-{day:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(seconds: u64) -> String {
        Date::from_unix_seconds(seconds).unwrap().to_string()
    }

    #[test]
    fn formats_calendar_days() {
        assert_eq!(date(86_400), "1970-01-02");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_709_251_199), "2024-02-29");
        assert_eq!(date(1_790_640_000), "2026-09-29");
        assert_eq!(date(4_102_444_800), "2100-01-01");
    }

    #[test]
    fn epoch_day_and_far_future_are_unknown() {
        assert_eq!(Date::from_unix_seconds(0), None);
        assert_eq!(Date::from_unix_seconds(86_399), None);
        assert_eq!(
            Date::from_unix_seconds(u64::from(u16::MAX) * 86_400 + 1).map(|d| d.0),
            Some(u16::MAX)
        );
        assert_eq!(
            Date::from_unix_seconds(u64::from(u16::MAX) * 86_400 + 86_400),
            None
        );
    }
}

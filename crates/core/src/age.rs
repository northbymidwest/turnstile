use time::OffsetDateTime;

/// How long ago a release was published, as a unit and a count. The caller
/// owns the grammar; singular and plural are separate variants so a localized
/// template can be chosen without this crate knowing any language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Age {
    Minute,
    Minutes(u32),
    Hour,
    Hours(u32),
    Day,
    Days(u32),
    Month,
    Months(u32),
    Year,
    Years(u32),
}

const DAYS_PER_MONTH: u64 = 30;
const DAYS_PER_YEAR: u64 = 365;

/// The current instant, for callers that must not depend on the `time` crate
/// themselves. `turnstile` (the app crate) is one: its dependency list is
/// fixed and does not include `time`.
pub fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

impl Age {
    pub fn since(published: OffsetDateTime, now: OffsetDateTime) -> Age {
        let seconds = (now - published).whole_seconds().max(0) as u64;

        let minutes = seconds / 60;
        if minutes < 60 {
            return pick(minutes, Age::Minute, Age::Minutes);
        }
        let hours = minutes / 60;
        if hours < 24 {
            return pick(hours, Age::Hour, Age::Hours);
        }
        let days = hours / 24;
        if days < DAYS_PER_MONTH {
            return pick(days, Age::Day, Age::Days);
        }
        if days < DAYS_PER_YEAR {
            return pick(days / DAYS_PER_MONTH, Age::Month, Age::Months);
        }
        pick(days / DAYS_PER_YEAR, Age::Year, Age::Years)
    }
}

fn pick(n: u64, singular: Age, plural: fn(u32) -> Age) -> Age {
    if n == 1 { singular } else { plural(n as u32) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;
    use time::macros::datetime;

    fn age_after(d: Duration) -> Age {
        let now = datetime!(2026-09-16 12:00 UTC);
        Age::since(now - d, now)
    }

    #[test]
    fn under_a_minute_still_reports_minutes() {
        assert_eq!(age_after(Duration::seconds(5)), Age::Minutes(0));
    }

    #[test]
    fn exactly_one_unit_uses_the_singular_variant() {
        assert_eq!(age_after(Duration::minutes(1)), Age::Minute);
        assert_eq!(age_after(Duration::hours(1)), Age::Hour);
        assert_eq!(age_after(Duration::days(1)), Age::Day);
        assert_eq!(age_after(Duration::days(30)), Age::Month);
        assert_eq!(age_after(Duration::days(365)), Age::Year);
    }

    #[test]
    fn every_variant_is_reachable() {
        let reached = [
            age_after(Duration::minutes(1)),
            age_after(Duration::minutes(5)),
            age_after(Duration::hours(1)),
            age_after(Duration::hours(5)),
            age_after(Duration::days(1)),
            age_after(Duration::days(5)),
            age_after(Duration::days(30)),
            age_after(Duration::days(90)),
            age_after(Duration::days(365)),
            age_after(Duration::days(365 * 3)),
        ];
        assert_eq!(
            reached,
            [
                Age::Minute,
                Age::Minutes(5),
                Age::Hour,
                Age::Hours(5),
                Age::Day,
                Age::Days(5),
                Age::Month,
                Age::Months(3),
                Age::Year,
                Age::Years(3),
            ]
        );
    }

    #[test]
    fn the_hour_boundary_switches_units() {
        assert_eq!(age_after(Duration::minutes(59)), Age::Minutes(59));
        assert_eq!(age_after(Duration::minutes(60)), Age::Hour);
    }

    #[test]
    fn the_day_boundary_switches_units() {
        assert_eq!(age_after(Duration::hours(23)), Age::Hours(23));
        assert_eq!(age_after(Duration::hours(24)), Age::Day);
    }

    #[test]
    fn the_thirty_day_boundary_switches_to_months() {
        assert_eq!(age_after(Duration::days(29)), Age::Days(29));
        assert_eq!(age_after(Duration::days(30)), Age::Month);
        assert_eq!(age_after(Duration::days(60)), Age::Months(2));
    }

    #[test]
    fn the_twelve_month_boundary_switches_to_years() {
        assert_eq!(age_after(Duration::days(359)), Age::Months(11));
        assert_eq!(age_after(Duration::days(365)), Age::Year);
        assert_eq!(age_after(Duration::days(365 * 2)), Age::Years(2));
    }

    #[test]
    fn a_future_timestamp_does_not_panic_or_underflow() {
        let now = datetime!(2026-09-16 12:00 UTC);
        assert_eq!(Age::since(now + Duration::days(7), now), Age::Minutes(0));
    }
}

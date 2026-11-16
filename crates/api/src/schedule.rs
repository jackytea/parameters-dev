use time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

const YEARS_AHEAD: i32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Field {
    bits: u64,
    full: u64,
}

impl Field {
    fn parse(text: &str, min: u32, max: u32, what: &str) -> Result<Self, String> {
        let full = (min..=max).fold(0, |bits, value| bits | 1 << value);
        let mut bits = 0_u64;
        for part in text.split(',') {
            let (range, step) = match part.split_once('/') {
                Some((range, step)) => {
                    let step: u32 =
                        step.parse().ok().filter(|step| *step > 0).ok_or_else(|| {
                            format!("`{part}` has an invalid step in the {what} field")
                        })?;
                    (range, step)
                }
                None => (part, 1),
            };
            let (start, end) = if range == "*" {
                (min, max)
            } else if let Some((start, end)) = range.split_once('-') {
                (number(start, min, max, what)?, number(end, min, max, what)?)
            } else {
                let value = number(range, min, max, what)?;
                (value, if step > 1 { max } else { value })
            };
            if start > end {
                return Err(format!("`{part}` is a backwards range in the {what} field"));
            }
            for value in (start..=end).step_by(step as usize) {
                bits |= 1 << value;
            }
        }
        Ok(Self { bits, full })
    }

    const fn has(self, value: u32) -> bool {
        self.bits & (1 << value) != 0
    }

    const fn restricted(self) -> bool {
        self.bits & self.full != self.full
    }
}

fn number(text: &str, min: u32, max: u32, what: &str) -> Result<u32, String> {
    text.parse::<u32>()
        .ok()
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| format!("`{text}` is not a valid {what}: use {min} to {max}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    minutes: Field,
    hours: Field,
    days: Field,
    months: Field,
    weekdays: Field,
}

impl Schedule {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let expanded = match text {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            other => other,
        };
        let fields: Vec<&str> = expanded.split_whitespace().collect();
        let [minutes, hours, days, months, weekdays] = fields.as_slice() else {
            return Err(
                "a schedule has five fields: minute, hour, day of month, month and day of week, such as `0 9 * * 1-5`"
                    .into(),
            );
        };
        let mut weekdays = Field::parse(weekdays, 0, 7, "day of week")?;
        if weekdays.has(7) {
            weekdays.bits = (weekdays.bits | 1) & !(1 << 7);
        }
        weekdays.full = 0b0111_1111;
        Ok(Self {
            minutes: Field::parse(minutes, 0, 59, "minute")?,
            hours: Field::parse(hours, 0, 23, "hour")?,
            days: Field::parse(days, 1, 31, "day of month")?,
            months: Field::parse(months, 1, 12, "month")?,
            weekdays,
        })
    }

    fn day_matches(&self, date: Date) -> bool {
        let day = self.days.has(u32::from(date.day()));
        let weekday = self
            .weekdays
            .has(u32::from(date.weekday().number_days_from_sunday()));
        match (self.days.restricted(), self.weekdays.restricted()) {
            (true, true) => day || weekday,
            (true, false) => day,
            (false, true) => weekday,
            (false, false) => true,
        }
    }

    #[must_use]
    pub fn next_after(&self, after: OffsetDateTime) -> Option<OffsetDateTime> {
        let after = after.to_offset(UtcOffset::UTC);
        let limit = after.year() + YEARS_AHEAD;
        let start = PrimitiveDateTime::new(
            after.date(),
            Time::from_hms(after.hour(), after.minute(), 0).ok()?,
        );
        let mut at = start.checked_add(Duration::MINUTE)?;
        while at.year() <= limit {
            if !self.months.has(u32::from(u8::from(at.month()))) {
                let (year, month) = match at.month() {
                    Month::December => (at.year() + 1, Month::January),
                    month => (at.year(), month.next()),
                };
                at = PrimitiveDateTime::new(
                    Date::from_calendar_date(year, month, 1).ok()?,
                    Time::MIDNIGHT,
                );
            } else if !self.day_matches(at.date()) {
                at = PrimitiveDateTime::new(at.date().next_day()?, Time::MIDNIGHT);
            } else if !self.hours.has(u32::from(at.hour())) {
                at = PrimitiveDateTime::new(at.date(), Time::from_hms(at.hour(), 0, 0).ok()?)
                    .checked_add(Duration::HOUR)?;
            } else if !self.minutes.has(u32::from(at.minute())) {
                at = at.checked_add(Duration::MINUTE)?;
            } else {
                return Some(at.assume_utc());
            }
        }
        None
    }

    #[must_use]
    pub fn upcoming(&self, after: OffsetDateTime, count: usize) -> Vec<OffsetDateTime> {
        std::iter::successors(self.next_after(after), |previous| {
            self.next_after(*previous)
        })
        .take(count)
        .collect()
    }

    #[must_use]
    pub fn shortest_gap(&self, after: OffsetDateTime) -> Option<Duration> {
        self.upcoming(after, 120)
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .min()
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn next(schedule: &str, after: OffsetDateTime) -> OffsetDateTime {
        Schedule::parse(schedule)
            .unwrap()
            .next_after(after)
            .unwrap()
    }

    #[test]
    fn finds_the_next_matching_minute() {
        let at = datetime!(2026-11-16 09:30:15 UTC);
        assert_eq!(next("* * * * *", at), datetime!(2026-11-16 09:31 UTC));
        assert_eq!(next("*/15 * * * *", at), datetime!(2026-11-16 09:45 UTC));
        assert_eq!(next("0 9 * * *", at), datetime!(2026-11-17 09:00 UTC));
        assert_eq!(
            next("0 9 * * 1-5", datetime!(2026-11-20 10:00 UTC)),
            datetime!(2026-11-23 09:00 UTC)
        );
        assert_eq!(next("@monthly", at), datetime!(2026-12-01 00:00 UTC));
        assert_eq!(next("@yearly", at), datetime!(2027-01-01 00:00 UTC));
        assert_eq!(next("0 0 29 2 *", at), datetime!(2028-02-29 00:00 UTC));
        assert_eq!(next("30 2 1,15 * *", at), datetime!(2026-12-01 02:30 UTC));
        assert_eq!(next("0 12 * * 7", at), datetime!(2026-11-22 12:00 UTC));
    }

    #[test]
    fn day_of_month_and_weekday_are_either_or_when_both_are_set() {
        let at = datetime!(2026-11-16 00:00 UTC);
        assert_eq!(next("0 0 20 * 5", at), datetime!(2026-11-20 00:00 UTC));
        assert_eq!(next("0 0 18 * 5", at), datetime!(2026-11-18 00:00 UTC));
    }

    #[test]
    fn impossible_and_invalid_schedules_are_rejected() {
        assert_eq!(
            Schedule::parse("0 0 31 2 *")
                .unwrap()
                .next_after(datetime!(2026-01-01 00:00 UTC)),
            None
        );
        for bad in [
            "",
            "* * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "* * * * 8",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
        ] {
            assert!(Schedule::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn measures_the_shortest_gap() {
        let at = datetime!(2026-11-16 00:00 UTC);
        let every_five = Schedule::parse("*/5 * * * *").unwrap();
        assert_eq!(every_five.shortest_gap(at), Some(Duration::minutes(5)));
        assert_eq!(every_five.upcoming(at, 2).len(), 2);
    }
}

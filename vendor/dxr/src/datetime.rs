use std::fmt::Display;
use std::str::FromStr;

use winnow::Parser;
use winnow::error::{ContextError, StrContext, StrContextValue};
use winnow::token::take;

use crate::error::Error;

/// Naive date / time type representing XML-RPC values of type `dateTime.iso8601`
///
/// The date / time format used for XML-RPC values of type `dateTime.iso8601` does not provide
/// sub-second precision nor timezone information.
///
/// Conversions to / from "naive" date / time types are available for `chrono`, `jiff`, and `time`
/// if the respective crate feature is enabled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DateTime {
    year: u16,
    month: u8,
    day: u8,

    hour: u8,
    minute: u8,
    second: u8,
}

#[allow(missing_docs)]
impl DateTime {
    #[must_use]
    pub const fn year(&self) -> u16 {
        self.year
    }

    #[must_use]
    pub const fn month(&self) -> u8 {
        self.month
    }

    #[must_use]
    pub const fn day(&self) -> u8 {
        self.day
    }

    #[must_use]
    pub const fn hour(&self) -> u8 {
        self.hour
    }

    #[must_use]
    pub const fn minute(&self) -> u8 {
        self.minute
    }

    #[must_use]
    pub const fn second(&self) -> u8 {
        self.second
    }
}

impl Display for DateTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:04}{:02}{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second,
        )
    }
}

impl FromStr for DateTime {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        DateTimeParser
            .parse(s)
            .map_err(|e| Error::invalid_datetime(e.to_string()))
    }
}

struct DateTimeParser;

impl Parser<&str, DateTime, ContextError> for DateTimeParser {
    fn parse_next(&mut self, input: &mut &str) -> winnow::Result<DateTime> {
        let year = YearParser.parse_next(input)?;
        let month = MonthParser.parse_next(input)?;
        let day = DayParser {
            max: length_of_month(year, month),
        }
        .parse_next(input)?;

        _ = parse_datetime_sep(input)?;

        let hour = HourParser.parse_next(input)?;
        _ = parse_time_sep(input)?;
        let minute = MinuteParser.parse_next(input)?;
        _ = parse_time_sep(input)?;
        let second = SecondParser.parse_next(input)?;

        Ok(DateTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }
}

struct YearParser;

impl Parser<&str, u16, ContextError> for YearParser {
    fn parse_next(&mut self, input: &mut &str) -> winnow::Result<u16> {
        take(4usize)
            .parse_to()
            .context(StrContext::Label("year"))
            .parse_next(input)
    }
}

struct MonthParser;

impl Parser<&str, u8, ContextError> for MonthParser {
    fn parse_next(&mut self, input: &mut &str) -> winnow::Result<u8> {
        take(2usize)
            .parse_to()
            .verify(|m| *m > 0 && *m <= 12)
            .context(StrContext::Label("month"))
            .parse_next(input)
    }
}

struct DayParser {
    max: u8,
}

impl Parser<&str, u8, ContextError> for DayParser {
    fn parse_next(&mut self, input: &mut &str) -> winnow::Result<u8> {
        take(2usize)
            .parse_to()
            .verify(|d| *d > 0 && *d <= self.max)
            .context(StrContext::Label("day"))
            .parse_next(input)
    }
}

struct HourParser;

impl Parser<&str, u8, ContextError> for HourParser {
    fn parse_next(&mut self, input: &mut &str) -> winnow::Result<u8> {
        take(2usize)
            .parse_to()
            .verify(|h| *h < 24)
            .context(StrContext::Label("hour"))
            .parse_next(input)
    }
}

struct MinuteParser;

impl Parser<&str, u8, ContextError> for MinuteParser {
    fn parse_next(&mut self, input: &mut &str) -> winnow::Result<u8> {
        take(2usize)
            .parse_to()
            .verify(|m| *m < 60)
            .context(StrContext::Label("minute"))
            .parse_next(input)
    }
}

struct SecondParser;

impl Parser<&str, u8, ContextError> for SecondParser {
    fn parse_next(&mut self, input: &mut &str) -> winnow::Result<u8> {
        take(2usize)
            .parse_to()
            .verify(|s| *s < 60)
            .context(StrContext::Label("second"))
            .parse_next(input)
    }
}

fn parse_datetime_sep(input: &mut &str) -> winnow::Result<char> {
    'T'.context(StrContext::Label("T separator"))
        .context(StrContext::Expected(StrContextValue::CharLiteral('T')))
        .parse_next(input)
}

fn parse_time_sep(input: &mut &str) -> winnow::Result<char> {
    ':'.context(StrContext::Label(": separator"))
        .context(StrContext::Expected(StrContextValue::CharLiteral(':')))
        .parse_next(input)
}

#[allow(clippy::match_same_arms)]
fn length_of_month(year: u16, month: u8) -> u8 {
    match month {
        1 => 31,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        },
        3 => 31,
        4 => 30,
        5 => 31,
        6 => 30,
        7 => 31,
        8 => 31,
        9 => 30,
        10 => 31,
        11 => 30,
        12 => 31,
        _ => unreachable!(),
    }
}

#[allow(clippy::needless_bool)]
const fn is_leap_year(year: u16) -> bool {
    if year % 400 == 0 {
        true
    } else if year % 100 == 0 {
        false
    } else if year % 4 == 0 {
        true
    } else {
        false
    }
}

#[cfg(feature = "chrono")]
impl From<chrono::NaiveDateTime> for DateTime {
    fn from(value: chrono::NaiveDateTime) -> Self {
        use chrono::{Datelike, Timelike};

        let date = value.date();
        let time = value.time();

        #[allow(clippy::expect_used)]
        DateTime {
            year: date.year().try_into().expect("Invalid year"),
            month: date.month().try_into().expect("Invalid month"),
            day: date.day().try_into().expect("Invalid day"),

            hour: time.hour().try_into().expect("Invalid hours"),
            minute: time.minute().try_into().expect("Invalid minutes"),
            second: time.second().try_into().expect("Invalid seconds"),
        }
    }
}

#[cfg(feature = "chrono")]
impl From<DateTime> for chrono::NaiveDateTime {
    fn from(value: DateTime) -> Self {
        #[allow(clippy::expect_used)]
        chrono::NaiveDateTime::new(
            chrono::NaiveDate::from_ymd_opt(value.year.into(), value.month.into(), value.day.into())
                .expect("Invalid date"),
            chrono::NaiveTime::from_hms_opt(value.hour.into(), value.minute.into(), value.second.into())
                .expect("Invalid time"),
        )
    }
}

#[cfg(feature = "jiff")]
impl From<jiff::civil::DateTime> for DateTime {
    fn from(value: jiff::civil::DateTime) -> Self {
        let date = value.date();
        let time = value.time();

        #[allow(clippy::expect_used)]
        DateTime {
            year: date.year().try_into().expect("Invalid year"),
            month: date.month().try_into().expect("Invalid month"),
            day: date.day().try_into().expect("Invalid day"),

            hour: time.hour().try_into().expect("Invalid hours"),
            minute: time.minute().try_into().expect("Invalid minutes"),
            second: time.second().try_into().expect("Invalid seconds"),
        }
    }
}

#[cfg(feature = "jiff")]
impl From<DateTime> for jiff::civil::DateTime {
    fn from(value: DateTime) -> Self {
        #[allow(clippy::expect_used)]
        jiff::civil::DateTime::new(
            value.year.try_into().expect("Invalid year"),
            value.month.try_into().expect("Invalid month"),
            value.day.try_into().expect("Invalid day"),
            value.hour.try_into().expect("Invalid hours"),
            value.minute.try_into().expect("Invalid minutes"),
            value.second.try_into().expect("Invalid seconds"),
            0,
        )
        .expect("invalid datetime")
    }
}

#[cfg(feature = "time")]
impl From<time::PrimitiveDateTime> for DateTime {
    fn from(value: time::PrimitiveDateTime) -> Self {
        let date = value.date();
        let time = value.time();

        #[allow(clippy::expect_used)]
        DateTime {
            year: date.year().try_into().expect("Invalid year"),
            month: date.month().into(),
            day: date.day(),

            hour: time.hour(),
            minute: time.minute(),
            second: time.second(),
        }
    }
}

#[cfg(feature = "time")]
impl From<DateTime> for time::PrimitiveDateTime {
    fn from(value: DateTime) -> Self {
        #[allow(clippy::expect_used)]
        time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(
                value.year.into(),
                time::Month::try_from(value.month).expect("invalid month"),
                value.day,
            )
            .expect("invalid date"),
            time::Time::from_hms(value.hour, value.minute, value.second).expect("invalid time"),
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    use quickcheck::Arbitrary;
    use quickcheck_macros::quickcheck;

    impl Arbitrary for DateTime {
        fn arbitrary(g: &mut quickcheck::Gen) -> Self {
            DateTime {
                year: u16::arbitrary(g) % 10000,
                month: u8::arbitrary(g) % 12 + 1,
                day: u8::arbitrary(g) % 28 + 1,
                hour: u8::arbitrary(g) % 24,
                minute: u8::arbitrary(g) % 60,
                second: u8::arbitrary(g) % 60,
            }
        }
    }

    #[test]
    fn basic() {
        assert_eq!(
            "20250711T22:19:00".parse::<DateTime>().unwrap(),
            DateTime {
                year: 2025,
                month: 7,
                day: 11,
                hour: 22,
                minute: 19,
                second: 0
            }
        );
    }

    #[quickcheck]
    fn roundtrip(dt: DateTime) {
        assert_eq!(dt.to_string().parse::<DateTime>().unwrap(), dt);
    }
}

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};
use time::{
    Date as CalendarDate, OffsetDateTime, Time as TimeOfDay, UtcOffset,
    format_description::{parse_borrowed, well_known::Rfc3339},
};

use crate::Error;

use super::DATE_NEWTYPE;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Date(OffsetDateTime);

impl Date {
    pub const UNIX_EPOCH: Self = Self(OffsetDateTime::UNIX_EPOCH);

    pub const fn from_datetime(value: OffsetDateTime) -> Self {
        Self(value)
    }

    pub const fn as_datetime(self) -> OffsetDateTime {
        self.0
    }

    pub fn from_unix_seconds(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let nanos = (value * 1_000_000_000.0).round();
        if nanos < i128::MIN as f64 || nanos > i128::MAX as f64 {
            return None;
        }
        OffsetDateTime::from_unix_timestamp_nanos(nanos as i128).ok().map(Self)
    }

    pub fn unix_seconds(self) -> f64 {
        self.0.unix_timestamp_nanos() as f64 / 1_000_000_000.0
    }

    pub(crate) fn parse(value: &str) -> Result<Self, Error> {
        let datetime = if let Ok(datetime) = OffsetDateTime::parse(value, &Rfc3339) {
            datetime
        } else {
            let format = parse_borrowed::<3>("[year]-[month]-[day]").map_err(|error| Error::InvalidValue(error.to_string()))?;
            let date = CalendarDate::parse(value, &format).map_err(|error| Error::InvalidValue(error.to_string()))?;
            date.with_time(TimeOfDay::MIDNIGHT).assume_utc()
        };
        Ok(Self(datetime.to_offset(UtcOffset::UTC)))
    }

    pub(crate) fn format(self) -> Result<String, Error> {
        self.0.to_offset(UtcOffset::UTC).format(&Rfc3339).map_err(|error| Error::InvalidValue(error.to_string()))
    }
}

impl From<OffsetDateTime> for Date {
    fn from(value: OffsetDateTime) -> Self {
        Self(value)
    }
}

impl From<Date> for OffsetDateTime {
    fn from(value: Date) -> Self {
        value.0
    }
}

impl Serialize for Date {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let value = self.0.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
        serializer.serialize_newtype_struct(DATE_NEWTYPE, &value)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_newtype_struct(DATE_NEWTYPE, DateVisitor)
    }
}

struct DateVisitor;

impl<'de> Visitor<'de> for DateVisitor {
    type Value = Date;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LLSD date")
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_any(DatePayloadVisitor)
    }
}

struct DatePayloadVisitor;

impl<'de> Visitor<'de> for DatePayloadVisitor {
    type Value = Date;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LLSD date or Unix timestamp")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Date::parse(value).map_err(E::custom)
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> std::result::Result<Self::Value, E> {
        self.visit_str(&value)
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> std::result::Result<Self::Value, E> {
        Date::from_unix_seconds(value).ok_or_else(|| E::custom(Error::InvalidValue("invalid LLSD date timestamp".into())))
    }
}

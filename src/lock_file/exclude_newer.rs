use chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};
use std::str::FromStr;

/// A wrapper around a chrono DateTime that is used to exclude packages after
/// a certain point in time.
///
/// The difference between a normal DateTime and this one is that this one can
/// be parsed from both a RFC 3339 timestamps (e.g., `2006-12-02T02:07:43Z`)
/// and UTC dates in the same (e.g., `2006-12-02`).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ExcludeNewer(pub DateTime<Utc>);

impl FromStr for ExcludeNewer {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let date_err = match NaiveDate::from_str(s) {
            Ok(date) => {
                // Midnight that day is 00:00:00 the next day
                return Ok(Self(
                    (date + Days::new(1)).and_time(NaiveTime::MIN).and_utc(),
                ));
            }
            Err(err) => err,
        };
        let datetime_err = match DateTime::parse_from_rfc3339(s) {
            Ok(datetime) => return Ok(Self(datetime.with_timezone(&Utc))),
            Err(err) => err,
        };
        Err(format!(
            "`{s}` is neither a valid date ({date_err}) nor a valid datetime ({datetime_err})"
        ))
    }
}

impl std::fmt::Display for ExcludeNewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_from_str() {
        // Specifying just a date is equivalent to specifying the date at midnight of the next day.
        assert_eq!(
            ExcludeNewer::from_str("2006-12-02").unwrap(),
            ExcludeNewer::from_str("2006-12-03T00:00:00Z").unwrap(),);
    }

}
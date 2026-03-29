use chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};
use std::str::FromStr;

/// Specifies how to exclude newer packages from the solve.
///
/// Can be either:
/// - An absolute timestamp (RFC 3339 or YYYY-MM-DD date)
/// - A relative duration (e.g., `7d`, `1h`, `30m`, `1h30m`)
///
/// When a duration is specified, it is interpreted as "exclude packages newer
/// than `now - duration`" at solve time.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum ExcludeNewer {
    /// An absolute point in time. Packages newer than this are excluded.
    Timestamp(DateTime<Utc>),
    /// A relative duration. At solve time, packages newer than `now - duration`
    /// are excluded.
    Duration(std::time::Duration),
}

fn format_duration(duration: std::time::Duration) -> String {
    let mut remaining = duration.as_secs();
    let mut formatted = String::new();

    for (unit, suffix) in [(86_400, "d"), (3_600, "h"), (60, "m"), (1, "s")] {
        let count = remaining / unit;
        if count > 0 {
            formatted.push_str(&format!("{count}{suffix}"));
            remaining %= unit;
        }
    }

    let nanos = duration.subsec_nanos();
    let millis = nanos / 1_000_000;
    let micros = (nanos % 1_000_000) / 1_000;
    let nanos = nanos % 1_000;

    if millis > 0 {
        formatted.push_str(&format!("{millis}ms"));
    }
    if micros > 0 {
        formatted.push_str(&format!("{micros}us"));
    }
    if nanos > 0 {
        formatted.push_str(&format!("{nanos}ns"));
    }

    if formatted.is_empty() {
        formatted.push_str("0s");
    }

    formatted
}

impl From<ExcludeNewer> for rattler_solve::ExcludeNewer {
    fn from(value: ExcludeNewer) -> Self {
        match value {
            ExcludeNewer::Timestamp(dt) => Self::from_datetime(dt),
            ExcludeNewer::Duration(dur) => Self::from_duration(dur),
        }
    }
}

#[cfg(feature = "rattler_lock")]
impl From<ExcludeNewer> for rattler_lock::ExcludeNewer {
    fn from(value: ExcludeNewer) -> Self {
        match value {
            ExcludeNewer::Timestamp(dt) => Self::Timestamp(dt),
            ExcludeNewer::Duration(dur) => Self::Duration(dur),
        }
    }
}

impl FromStr for ExcludeNewer {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Try parsing as a duration first (e.g., "7d", "1h30m", "30days")
        if let Ok(duration) = humantime::parse_duration(s) {
            return Ok(ExcludeNewer::Duration(duration));
        }

        // Try parsing as a date (YYYY-MM-DD)
        let date_err = match NaiveDate::from_str(s) {
            Ok(date) => {
                // Midnight that day is 00:00:00 the next day
                return Ok(ExcludeNewer::Timestamp(
                    (date + Days::new(1)).and_time(NaiveTime::MIN).and_utc(),
                ));
            }
            Err(err) => err,
        };

        // Try parsing as an RFC 3339 timestamp
        let datetime_err = match DateTime::parse_from_rfc3339(s) {
            Ok(datetime) => return Ok(ExcludeNewer::Timestamp(datetime.with_timezone(&Utc))),
            Err(err) => err,
        };

        Err(format!(
            "`{s}` is neither a valid duration, date ({date_err}), nor datetime ({datetime_err})"
        ))
    }
}

impl std::fmt::Display for ExcludeNewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExcludeNewer::Timestamp(dt) => dt.fmt(f),
            ExcludeNewer::Duration(dur) => write!(f, "{}", format_duration(*dur)),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_from_str_timestamp() {
        // Specifying just a date is equivalent to specifying the date at midnight of the next day.
        assert_eq!(
            ExcludeNewer::from_str("2006-12-02").unwrap(),
            ExcludeNewer::from_str("2006-12-03T00:00:00Z").unwrap(),
        );

        // A more readable case that RFC3339 allowed
        match (
            ExcludeNewer::from_str("2006-12-02T00:00:00Z").unwrap(),
            ExcludeNewer::from_str("2006-12-02 00:00:00Z").unwrap(),
        ) {
            (ExcludeNewer::Timestamp(a), ExcludeNewer::Timestamp(b)) => assert_eq!(a, b),
            _ => panic!("expected timestamps"),
        }
    }

    #[test]
    fn test_from_str_duration() {
        // Various duration formats supported by humantime
        assert_eq!(
            ExcludeNewer::from_str("7d").unwrap(),
            ExcludeNewer::Duration(std::time::Duration::from_secs(7 * 24 * 60 * 60)),
        );
        assert_eq!(
            ExcludeNewer::from_str("1h").unwrap(),
            ExcludeNewer::Duration(std::time::Duration::from_secs(60 * 60)),
        );
        assert_eq!(
            ExcludeNewer::from_str("30m").unwrap(),
            ExcludeNewer::Duration(std::time::Duration::from_secs(30 * 60)),
        );
        assert_eq!(
            ExcludeNewer::from_str("1h30m").unwrap(),
            ExcludeNewer::Duration(std::time::Duration::from_secs(90 * 60)),
        );
        assert_eq!(
            ExcludeNewer::from_str("7days").unwrap(),
            ExcludeNewer::Duration(std::time::Duration::from_secs(7 * 24 * 60 * 60)),
        );
    }

    #[test]
    fn test_display_duration() {
        let d = ExcludeNewer::Duration(std::time::Duration::from_secs(7 * 24 * 60 * 60));
        let display = format!("{d}");
        assert_eq!(display, "7d");
    }

    #[test]
    fn test_display_timestamp() {
        let t = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        let display = format!("{t}");
        assert!(display.contains("2006"), "got: {display}");
    }

    #[test]
    fn test_into_rattler_solve_timestamp() {
        let t = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        let config: rattler_solve::ExcludeNewer = t.into();
        assert_eq!(
            config.cutoff_for_channel(None),
            DateTime::parse_from_rfc3339("2006-12-02T02:07:43Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn test_into_rattler_solve_duration() {
        let before = Utc::now();
        let d = ExcludeNewer::Duration(std::time::Duration::from_secs(3600));
        let config: rattler_solve::ExcludeNewer = d.into();
        let resolved = config.cutoff_for_channel(None);
        let after = Utc::now();

        // resolved should be approximately 1 hour ago
        let one_hour = chrono::Duration::seconds(3600);
        assert!(resolved >= before - one_hour);
        assert!(resolved <= after - one_hour + chrono::Duration::seconds(1));
    }
}

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
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ExcludeNewer {
    /// An absolute point in time. Packages newer than this are excluded.
    Timestamp(DateTime<Utc>),
    /// A relative duration. At solve time, packages newer than `now - duration`
    /// are excluded.
    Duration(std::time::Duration),
}

impl ExcludeNewer {
    /// Resolve to an absolute timestamp.
    ///
    /// For `Timestamp` variants, returns the timestamp directly.
    /// For `Duration` variants, computes `now - duration`.
    pub fn resolve(&self) -> DateTime<Utc> {
        match self {
            ExcludeNewer::Timestamp(dt) => *dt,
            ExcludeNewer::Duration(dur) => {
                let chrono_dur =
                    chrono::Duration::from_std(*dur).expect("duration is too large to represent");
                Utc::now() - chrono_dur
            }
        }
    }

    /// Returns `true` if this is a duration-based exclude-newer.
    pub fn is_duration(&self) -> bool {
        matches!(self, ExcludeNewer::Duration(_))
    }
}

impl From<ExcludeNewer> for DateTime<Utc> {
    fn from(value: ExcludeNewer) -> Self {
        value.resolve()
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
            ExcludeNewer::Duration(dur) => {
                write!(f, "{}", humantime::format_duration(*dur))
            }
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
        assert!(display.contains("7days"), "got: {display}");
    }

    #[test]
    fn test_display_timestamp() {
        let t = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        let display = format!("{t}");
        assert!(display.contains("2006"), "got: {display}");
    }

    #[test]
    fn test_resolve_timestamp() {
        let t = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        let resolved = t.resolve();
        assert_eq!(
            resolved,
            DateTime::parse_from_rfc3339("2006-12-02T02:07:43Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn test_resolve_duration() {
        let before = Utc::now();
        let d = ExcludeNewer::Duration(std::time::Duration::from_secs(3600));
        let resolved = d.resolve();
        let after = Utc::now();

        // resolved should be approximately 1 hour ago
        let one_hour = chrono::Duration::seconds(3600);
        assert!(resolved >= before - one_hour);
        assert!(resolved <= after - one_hour + chrono::Duration::seconds(1));
    }

    #[test]
    fn test_is_duration() {
        assert!(ExcludeNewer::from_str("7d").unwrap().is_duration());
        assert!(!ExcludeNewer::from_str("2006-12-02").unwrap().is_duration());
    }
}

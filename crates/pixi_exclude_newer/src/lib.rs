//! The scalar `exclude-newer` cutoff: an absolute timestamp or a duration
//! relative to "now".
//!
//! This crate only holds the value type and its parsing, so that both the
//! manifest (`pixi_spec`, `pixi_manifest`) and the configuration
//! (`pixi_config`) can use the same syntax without depending on each other.

use std::str::FromStr;

use chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};

/// Specifies how to exclude newer packages from the solve.
///
/// Can be either:
/// - An absolute timestamp
/// - A relative duration (e.g., `7d`, `1h`, `30m`, `1h30m`)
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum ExcludeNewer {
    /// An absolute point in time. Packages newer than this are excluded.
    Timestamp(DateTime<Utc>),
    /// A relative duration. At solve time, packages newer than `now - duration`
    /// are excluded.
    Duration(std::time::Duration),
}

impl ExcludeNewer {
    /// Returns the effective cutoff for the current time.
    pub fn cutoff(&self) -> DateTime<Utc> {
        self.cutoff_at(Utc::now())
    }

    /// Returns the effective cutoff if the current time were `now`.
    pub fn cutoff_at(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Self::Timestamp(cutoff) => *cutoff,
            Self::Duration(duration) => {
                let duration = chrono::Duration::from_std(*duration)
                    .expect("exclude-newer duration is too large");
                now - duration
            }
        }
    }
}

impl FromStr for ExcludeNewer {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_exclude_newer_str(s)
    }
}

impl std::fmt::Display for ExcludeNewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExcludeNewer::Timestamp(dt) => dt.fmt(f),
            ExcludeNewer::Duration(dur) => humantime::format_duration(*dur).fmt(f),
        }
    }
}

impl serde::Serialize for ExcludeNewer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Timestamp(cutoff) => cutoff.serialize(serializer),
            Self::Duration(duration) => {
                serializer.collect_str(&humantime::Duration::from(*duration))
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for ExcludeNewer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum RawExcludeNewer {
            Timestamp(DateTime<Utc>),
            Duration(String),
        }

        match RawExcludeNewer::deserialize(deserializer)? {
            RawExcludeNewer::Timestamp(cutoff) => Ok(ExcludeNewer::Timestamp(cutoff)),
            RawExcludeNewer::Duration(value) => {
                parse_exclude_newer_str(&value).map_err(serde::de::Error::custom)
            }
        }
    }
}

fn parse_exclude_newer_str(s: &str) -> Result<ExcludeNewer, String> {
    if let Ok(duration) = s.parse::<humantime::Duration>() {
        return Ok(ExcludeNewer::Duration(duration.into()));
    }

    let date_err = match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        Ok(date) => {
            let next_midnight = date
                .checked_add_days(Days::new(1))
                .expect("valid exclude-newer date should have a following day")
                .and_time(NaiveTime::MIN)
                .and_utc();
            return Ok(ExcludeNewer::Timestamp(next_midnight));
        }
        Err(err) => err,
    };

    let timestamp_err = match DateTime::parse_from_rfc3339(s) {
        Ok(timestamp) => return Ok(ExcludeNewer::Timestamp(timestamp.with_timezone(&Utc))),
        Err(err) => err,
    };

    Err(format!(
        "`{s}` is neither a valid duration, date ({date_err}), nor timestamp ({timestamp_err})"
    ))
}

#[cfg(test)]
mod test {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_from_str_timestamp() {
        assert_eq!(
            ExcludeNewer::from_str("2006-12-02T00:00:00Z").unwrap(),
            ExcludeNewer::from_str("2006-12-02T00:00:00+00:00").unwrap(),
        );

        match (
            ExcludeNewer::from_str("2006-12-02T00:00:00Z").unwrap(),
            ExcludeNewer::from_str("2006-12-02T00:00:00+00:00").unwrap(),
        ) {
            (ExcludeNewer::Timestamp(a), ExcludeNewer::Timestamp(b)) => assert_eq!(a, b),
            _ => panic!("expected timestamps"),
        }
    }

    #[test]
    fn test_from_str_date() {
        assert_eq!(
            ExcludeNewer::from_str("2006-12-02").unwrap(),
            ExcludeNewer::from_str("2006-12-03T00:00:00Z").unwrap(),
        );
    }

    #[test]
    fn test_from_str_duration() {
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
    fn test_from_str_invalid_reports_supported_formats() {
        let err = ExcludeNewer::from_str("date").unwrap_err();
        assert!(err.contains("valid duration"), "got: {err}");
        assert!(err.contains("date ("), "got: {err}");
        assert!(err.contains("timestamp ("), "got: {err}");
    }

    #[test]
    fn test_cutoff_for_duration_is_relative_to_now() {
        let before = Utc::now();
        let cutoff = ExcludeNewer::Duration(std::time::Duration::from_secs(60 * 60)).cutoff();
        let after = Utc::now();

        assert!(
            cutoff >= before - chrono::Duration::hours(1) - chrono::Duration::seconds(1),
            "cutoff {cutoff} should be close to one hour before {before}",
        );
        assert!(
            cutoff <= after - chrono::Duration::hours(1) + chrono::Duration::seconds(1),
            "cutoff {cutoff} should be close to one hour before {after}",
        );
    }

    #[test]
    fn test_serde_deserializes_timestamp_with_space_separator() {
        let parsed: ExcludeNewer = serde_json::from_value(json!("2006-12-02 02:07:43Z")).unwrap();

        assert_eq!(
            parsed,
            ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap()
        );
    }

    #[test]
    fn test_serde_roundtrips_duration() {
        let value = ExcludeNewer::Duration(std::time::Duration::from_secs(90 * 60));

        let serialized = serde_json::to_value(value).unwrap();
        assert_eq!(serialized, json!("1h 30m"));

        let deserialized: ExcludeNewer = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized, value);
    }

    #[test]
    fn test_display_duration() {
        let d = ExcludeNewer::Duration(std::time::Duration::from_secs(7 * 24 * 60 * 60));
        let display = format!("{d}");
        assert_eq!(display, "7days");
    }

    #[test]
    fn test_display_timestamp() {
        let t = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        let display = format!("{t}");
        assert!(display.contains("2006"), "got: {display}");
    }
}

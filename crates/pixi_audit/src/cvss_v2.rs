//! CVSS v2.0 base-score computation for NVD-sourced severity entries.
//!
//! The `cvss` crate only implements v3 and v4, but basilisk serves
//! `CVSS_V2.0` entries for older CVEs that predate CVSS v3. The base-score
//! formula below is from the CVSS v2.0 specification, section 3.2.1.

/// Computes the CVSS v2.0 base score for a base vector such as
/// `AV:N/AC:L/Au:N/C:P/I:P/A:P`. Returns `None` when any of the six base
/// metrics is missing or has an unknown value.
pub(crate) fn base_score(vector: &str) -> Option<f64> {
    let vector = vector
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim_start_matches("CVSS:2.0/");

    let mut av: Option<f64> = None;
    let mut ac: Option<f64> = None;
    let mut au: Option<f64> = None;
    let mut c: Option<f64> = None;
    let mut i: Option<f64> = None;
    let mut a: Option<f64> = None;
    for metric in vector.split('/') {
        let (name, value) = metric.split_once(':')?;
        match name {
            "AV" => {
                av = Some(match value {
                    "L" => 0.395,
                    "A" => 0.646,
                    "N" => 1.0,
                    _ => return None,
                })
            }
            "AC" => {
                ac = Some(match value {
                    "H" => 0.35,
                    "M" => 0.61,
                    "L" => 0.71,
                    _ => return None,
                })
            }
            "Au" => {
                au = Some(match value {
                    "M" => 0.45,
                    "S" => 0.56,
                    "N" => 0.704,
                    _ => return None,
                })
            }
            "C" | "I" | "A" => {
                let impact = match value {
                    "N" => 0.0,
                    "P" => 0.275,
                    "C" => 0.660,
                    _ => return None,
                };
                match name {
                    "C" => c = Some(impact),
                    "I" => i = Some(impact),
                    _ => a = Some(impact),
                }
            }
            // Temporal/environmental metrics do not affect the base score.
            _ => {}
        }
    }

    let (av, ac, au, c, i, a) = (av?, ac?, au?, c?, i?, a?);
    let impact = 10.41 * (1.0 - (1.0 - c) * (1.0 - i) * (1.0 - a));
    let exploitability = 20.0 * av * ac * au;
    let f_impact = if impact == 0.0 { 0.0 } else { 1.176 };
    let score = (0.6 * impact + 0.4 * exploitability - 1.5) * f_impact;
    Some((score * 10.0).round() / 10.0)
}

#[cfg(test)]
mod tests {
    use super::base_score;

    #[test]
    fn matches_nvd_reference_scores() {
        // Vectors and scores taken from NVD entries.
        assert_eq!(base_score("AV:N/AC:L/Au:N/C:P/I:P/A:P"), Some(7.5));
        assert_eq!(base_score("AV:N/AC:M/Au:N/C:P/I:N/A:N"), Some(4.3));
        assert_eq!(base_score("AV:N/AC:L/Au:N/C:C/I:C/A:C"), Some(10.0));
        assert_eq!(base_score("AV:L/AC:L/Au:N/C:C/I:C/A:C"), Some(7.2));
        assert_eq!(base_score("AV:N/AC:L/Au:N/C:N/I:N/A:N"), Some(0.0));
    }

    #[test]
    fn tolerates_wrapping_and_prefix() {
        assert_eq!(base_score("(AV:N/AC:L/Au:N/C:P/I:P/A:P)"), Some(7.5));
        assert_eq!(base_score("CVSS:2.0/AV:N/AC:L/Au:N/C:P/I:P/A:P"), Some(7.5));
    }

    #[test]
    fn incomplete_or_invalid_vectors_are_rejected() {
        assert_eq!(base_score("AV:N/AC:L/Au:N"), None);
        assert_eq!(base_score("AV:X/AC:L/Au:N/C:P/I:P/A:P"), None);
        assert_eq!(base_score("not a vector"), None);
    }
}

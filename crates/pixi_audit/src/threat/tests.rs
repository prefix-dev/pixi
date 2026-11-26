use super::*;

fn epss(probability: f64) -> Vec<EpssScore> {
    vec![EpssScore {
        cve: "CVE-2026-1".to_string(),
        epss: probability,
        percentile: 0.5,
        date: "2026-09-01".to_string(),
    }]
}

fn kev(ransomware: RansomwareCampaignUse) -> Vec<KevEntry> {
    vec![KevEntry {
        cve: "CVE-2026-1".to_string(),
        vendor_project: "Vendor".to_string(),
        product: "Product".to_string(),
        vulnerability_name: "Name".to_string(),
        date_added: "2026-08-01".to_string(),
        required_action: "Apply updates".to_string(),
        due_date: "2026-08-22".to_string(),
        known_ransomware_campaign_use: ransomware,
        notes: String::new(),
    }]
}

fn risk(severity: SeverityRating, cvss: Option<f64>, epss: &[EpssScore], kev: &[KevEntry]) -> f64 {
    RiskScore::compute(severity, cvss, epss, kev).value()
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn risk_is_epss_times_severity_scaled_to_100() {
    // No CVSS: severity is the rating midpoint alone. High = 7.5 / 10.
    assert_close(risk(SeverityRating::High, None, &epss(0.5), &[]), 37.5);
    // With CVSS: the mean of rating midpoint and base score, both over 10.
    // Critical (9.0) and CVSS 9.8 => 0.94; times 0.5 => 47.0.
    assert_close(
        risk(SeverityRating::Critical, Some(9.8), &epss(0.5), &[]),
        47.0,
    );
    // A zero base score is not a real score and is ignored.
    assert_close(risk(SeverityRating::High, Some(0.0), &epss(0.5), &[]), 37.5);
}

#[test]
fn risk_uses_the_highest_epss_probability() {
    let scores = [epss(0.1), epss(0.7)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_close(risk(SeverityRating::High, None, &scores, &[]), 52.5);

    let reversed = scores.into_iter().rev().collect::<Vec<_>>();
    assert_close(risk(SeverityRating::High, None, &reversed, &[]), 52.5);
}

#[test]
fn risk_without_threat_data_is_zero() {
    assert_close(risk(SeverityRating::Critical, Some(9.8), &[], &[]), 0.0);
    assert_eq!(
        RiskScore::compute(SeverityRating::Critical, None, &[], &[]),
        RiskScore::NONE
    );
}

#[test]
fn unknown_severity_sits_in_the_middle() {
    assert_close(risk(SeverityRating::Unknown, None, &epss(1.0), &[]), 50.0);
    assert_close(risk(SeverityRating::Low, None, &epss(1.0), &[]), 30.0);
    assert_close(risk(SeverityRating::Medium, None, &epss(1.0), &[]), 50.0);
}

#[test]
fn kev_overrides_epss_and_boosts() {
    // Threat becomes 1.0 regardless of (even absent) EPSS, times 1.05.
    let listed = kev(RansomwareCampaignUse::Unknown);
    assert_close(risk(SeverityRating::Medium, None, &[], &listed), 52.5);
    assert_close(
        risk(SeverityRating::Medium, None, &epss(0.01), &listed),
        52.5,
    );
    // Ransomware use boosts by 1.1 instead.
    let ransomware = kev(RansomwareCampaignUse::Known);
    assert_close(risk(SeverityRating::Medium, None, &[], &ransomware), 55.0);
}

#[test]
fn risk_is_capped_at_100() {
    let ransomware = kev(RansomwareCampaignUse::Known);
    assert_close(
        risk(SeverityRating::Critical, Some(10.0), &[], &ransomware),
        100.0,
    );
}

#[test]
fn risk_scores_order_totally() {
    let mut scores = [
        RiskScore::compute(SeverityRating::Low, None, &epss(0.1), &[]),
        RiskScore::NONE,
        RiskScore::compute(SeverityRating::Critical, None, &epss(0.9), &[]),
    ];
    scores.sort();
    let values: Vec<f64> = scores.iter().map(|score| score.value()).collect();
    assert_close(values[0], 0.0);
    assert_close(values[1], 3.0);
    assert_close(values[2], 81.0);
}

#[test]
fn status_serializes_with_a_tag() {
    let status = ThreatDataStatus {
        epss: ThreatSourceStatus::Loaded {
            date: Some("2026-09-01".to_string()),
        },
        kev: ThreatSourceStatus::Failed {
            error: "boom".to_string(),
        },
    };
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "epss": {"status": "loaded", "date": "2026-09-01"},
            "kev": {"status": "failed", "error": "boom"},
        })
    );
    assert_eq!(
        serde_json::to_value(ThreatSourceStatus::Disabled).unwrap(),
        serde_json::json!({"status": "disabled"})
    );
}

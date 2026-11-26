use super::*;

#[test]
fn ransomware_use_parses_case_insensitively() {
    assert_eq!(
        RansomwareCampaignUse::parse("Known"),
        RansomwareCampaignUse::Known
    );
    assert_eq!(
        RansomwareCampaignUse::parse(" known "),
        RansomwareCampaignUse::Known
    );
    assert_eq!(
        RansomwareCampaignUse::parse("Unknown"),
        RansomwareCampaignUse::Unknown
    );
    assert_eq!(
        RansomwareCampaignUse::parse(""),
        RansomwareCampaignUse::Unknown
    );
}

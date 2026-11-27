//! 验证上游套餐余额和历史套餐合同

use crate::transport::account::Tier;
use serde_json::json;

#[test]
fn google_credit_requires_known_balance_and_threshold() {
    for (amount, minimum, usable) in [
        (json!("12"), json!("10"), true),
        (json!("9"), json!("10"), false),
        (json!(null), json!("10"), false),
        (json!("12"), json!("invalid"), false),
    ] {
        let tier: Tier = serde_json::from_value(json!({"availableCredits": [{
            "creditType": "GOOGLE_ONE_AI", "creditAmount": amount,
            "minimumCreditAmountForUsage": minimum
        }]}))
        .unwrap();
        assert!(tier.available_credits[0].is_google_one());
        assert_eq!(tier.available_credits[0].usable(), usable);
    }
}

#[test]
fn tier_description_round_trips_and_old_observations_remain_readable() {
    let old: Tier = serde_json::from_value(json!({"id":"g1-pro-tier","name":"Pro"})).unwrap();
    assert!(old.description.is_none());
    let new: Tier = serde_json::from_value(
        json!({"id":"g1-pro-tier","name":"Pro","description":"Google AI Pro"}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(new).unwrap()["description"],
        "Google AI Pro"
    );
}

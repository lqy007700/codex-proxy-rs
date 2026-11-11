//! 账号额度窗口上限的边界、序列化与输入约束

use std::collections::BTreeMap;

use gateway_core::account::AccountQuotaLimits;

#[test]
fn limit_reaches_at_the_exact_threshold_and_ignores_unknown_usage() {
    let limits = AccountQuotaLimits::try_from(BTreeMap::from([
        ("primary".to_owned(), 80),
        ("secondary".to_owned(), 100),
    ]))
    .expect("valid limits");

    assert!(!limits.reached("primary", Some(79.99)));
    assert!(limits.reached("primary", Some(80.0)));
    assert!(limits.reached("primary", Some(81.0)));
    assert!(!limits.reached("secondary", Some(99.99)));
    assert!(limits.reached("secondary", Some(100.0)));
    assert!(!limits.reached("primary", None));
    assert!(!limits.reached("missing", Some(100.0)));
    assert!(!limits.reached("primary", Some(f64::NAN)));
    assert_eq!(
        limits.keys().collect::<Vec<_>>(),
        vec!["primary", "secondary"]
    );
    assert!(limits.contains("primary"));
    assert!(!limits.contains("missing"));
}

#[test]
fn disabled_limits_round_trip_as_an_empty_map() {
    let disabled = AccountQuotaLimits::disabled();
    assert!(disabled.is_empty());
    assert_eq!(
        serde_json::to_value(&disabled).expect("serialize"),
        serde_json::json!({})
    );
    assert_eq!(
        serde_json::from_value::<AccountQuotaLimits>(serde_json::json!({})).expect("deserialize"),
        disabled
    );
    let limits: AccountQuotaLimits =
        serde_json::from_value(serde_json::json!({"primary": 1, "secondary": 100}))
            .expect("valid boundary values");
    assert_eq!(
        serde_json::to_value(limits).expect("serialize"),
        serde_json::json!({"primary": 1, "secondary": 100})
    );
}

#[test]
fn deserialization_rejects_invalid_windows_and_limits() {
    for invalid in [
        serde_json::json!({"primary": 0}),
        serde_json::json!({"primary": 101}),
        serde_json::json!({"primary": 1.5}),
        serde_json::json!({"": 80}),
        serde_json::json!({"line\nbreak": 80}),
        serde_json::json!({("x".repeat(257)): 80}),
        serde_json::json!([]),
    ] {
        assert!(
            serde_json::from_value::<AccountQuotaLimits>(invalid.clone()).is_err(),
            "accepted invalid quota limits: {invalid}"
        );
    }
}

//! 验证账号映射的有界输入、精确匹配和非递归语义

use gateway_core::account::AccountModelMappings;
use serde_json::json;

#[test]
fn mappings_reject_invalid_ids_values_and_unbounded_documents() {
    for invalid in [
        json!([]),
        json!({"":"target"}),
        json!({"alias":""}),
        json!({" alias":"target"}),
        json!({"alias":"target "}),
        json!({"alias":"*"}),
        json!({"__internal":"target"}),
        json!({"alias":"\n"}),
        json!({"alias":42}),
        json!({"alias":"x".repeat(257)}),
    ] {
        assert!(serde_json::from_value::<AccountModelMappings>(invalid).is_err());
    }
    let too_many = (0..257)
        .map(|i| (format!("alias-{i}"), "target".to_owned()))
        .collect();
    assert!(AccountModelMappings::new(too_many).is_err());
}

#[test]
fn mappings_use_exact_source_and_return_final_target_without_recursion() {
    let mappings: AccountModelMappings =
        serde_json::from_value(json!({"Alias":"target", "target":"other", "self":"self"})).unwrap();
    assert_eq!(mappings.target("Alias"), Some("target"));
    assert_eq!(mappings.target("alias"), None);
    assert_eq!(mappings.target("self"), Some("self"));
    assert_eq!(
        serde_json::to_value(mappings).unwrap(),
        json!({"Alias":"target", "target":"other", "self":"self"})
    );
}

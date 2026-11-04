//! 用量服务的 wire 精度、可信主体和独立 SDK 合同校验
use gateway_admin::model::usage_data::*;
use serde_json::json;
#[test]
fn counters_preserve_integer_precision_and_reject_json_numbers() {
    assert_eq!(
        serde_json::from_value::<UsageCount>(json!(u64::MAX.to_string()))
            .unwrap()
            .get(),
        u64::MAX
    );
    assert_eq!(
        serde_json::from_value::<UsageDelta>(json!(i64::MIN.to_string()))
            .unwrap()
            .get(),
        i64::MIN
    );
    for value in [json!(1), json!("1.5"), json!("+1")] {
        assert!(serde_json::from_value::<UsageCount>(value.clone()).is_err());
        assert!(serde_json::from_value::<UsageDelta>(value).is_err());
    }
}

//! 验证 Messages 缓存用量合并与统一输入量

use gateway_protocol::anthropic::{MessageUsage, messages_to_responses_response};
use gateway_protocol::openai::events::{billable_usage_is_complete, extract_usage};
use serde_json::{Map, json};

#[test]
fn usage_deltas_replace_counters_and_keep_cache_facts() {
    let mut usage = MessageUsage::from_value(
        &json!({"input_tokens":4,"output_tokens":1,"cache_read_input_tokens":20,"cache_creation_input_tokens":10}),
    );
    usage.merge(&json!({"output_tokens":9}));
    assert_eq!(usage.total_input_tokens(), Some(34));
    assert_eq!(usage.output_tokens, Some(9));
}

#[test]
fn converted_usage_matches_canonical_cache_write_accounting() {
    let converted=messages_to_responses_response(&json!({"id":"m1","content":[],"stop_reason":"end_turn","usage":{"input_tokens":4,"output_tokens":9,"cache_read_input_tokens":20,"cache_creation_input_tokens":10}}),&Map::new()).expect("用量转换");
    assert!(converted["usage"].get("output_tokens_details").is_none());
    let usage = extract_usage(&converted).expect("提取统一用量");
    assert_eq!(usage.input_tokens, 34);
    assert_eq!(usage.cache_write_input_tokens, 10);
    assert_eq!(usage.cached_input_tokens, 20);
    assert_eq!(usage.total_tokens, 43);
    assert!(billable_usage_is_complete(&converted, usage));
}

#[test]
fn missing_counters_never_become_complete_billable_usage() {
    for raw in [
        json!({"output_tokens":9,"cache_read_input_tokens":20,"cache_creation_input_tokens":10}),
        json!({"input_tokens":4,"cache_read_input_tokens":20,"cache_creation_input_tokens":10}),
        json!({"cache_read_input_tokens":20,"cache_creation_input_tokens":10}),
    ] {
        let converted = messages_to_responses_response(
            &json!({"content":[],"stop_reason":"end_turn","usage":raw}),
            &Map::new(),
        )
        .expect("保留未知用量");
        let usage = extract_usage(&converted).expect("缓存计数仍然可观测");
        assert!(!billable_usage_is_complete(&converted, usage));
        assert_eq!(usage.cached_input_tokens, 20);
        assert_eq!(usage.cache_write_input_tokens, 10);
        assert_eq!(
            converted["usage"]
                .get("input_tokens")
                .and_then(serde_json::Value::as_u64),
            raw.get("input_tokens").map(|_| 34)
        );
        assert_eq!(
            converted["usage"].get("output_tokens"),
            raw.get("output_tokens")
        );
        assert!(converted["usage"].get("total_tokens").is_none());
        let restored =
            gateway_protocol::anthropic::responses_to_messages_response(&converted, &Map::new())
                .expect("反向转换保留未知用量");
        for field in [
            "input_tokens",
            "output_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ] {
            assert_eq!(restored["usage"].get(field), raw.get(field));
        }
    }
}

#[test]
fn explicit_zero_usage_is_complete_and_overflow_is_not() {
    let zero = messages_to_responses_response(
        &json!({"content":[],"usage":{"input_tokens":0,"output_tokens":0}}),
        &Map::new(),
    )
    .unwrap();
    assert!(billable_usage_is_complete(
        &zero,
        extract_usage(&zero).unwrap()
    ));
    let raw = json!({"input_tokens":u64::MAX,"output_tokens":0,"cache_read_input_tokens":1});
    assert_eq!(MessageUsage::from_value(&raw).total_input_tokens(), None);
    let overflow =
        messages_to_responses_response(&json!({"content":[],"usage":raw}), &Map::new()).unwrap();
    assert!(!billable_usage_is_complete(
        &overflow,
        extract_usage(&overflow).unwrap()
    ));
}

#[test]
fn explicitly_reported_thinking_usage_survives_both_protocols() {
    // anthropics/anthropic-sdk-python v2.8.0 的 OutputTokensDetails 区分 thinking_tokens
    let source = json!({"content":[],"usage":{"input_tokens":4,"output_tokens":9,"output_tokens_details":{"thinking_tokens":6}}});
    let response = messages_to_responses_response(&source, &Map::new()).unwrap();
    assert_eq!(extract_usage(&response).unwrap().reasoning_output_tokens, 6);
    let message =
        gateway_protocol::anthropic::responses_to_messages_response(&response, &Map::new())
            .unwrap();
    assert_eq!(
        message["usage"]["output_tokens_details"]["thinking_tokens"],
        6
    );
}

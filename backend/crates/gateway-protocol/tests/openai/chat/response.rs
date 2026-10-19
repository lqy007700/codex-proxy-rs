//! Chat 完整响应的正文、拒答、工具与用量投影测试

use gateway_protocol::openai::chat::chat_completion;
use serde_json::{Value, json};

pub(super) fn response(output: Value) -> Value {
    json!({"id":"resp_synthetic","created_at":123,"model":"model-a","status":"completed","output":output,
        "usage":{"input_tokens":10,"output_tokens":7,"total_tokens":17,"input_tokens_details":{"cached_tokens":4},"output_tokens_details":{"reasoning_tokens":2}}})
}

#[test]
fn aggregates_text_without_exposing_reasoning_and_projects_usage() {
    let result = chat_completion(&response(json!([
        {"type":"reasoning","summary":[{"text":"private reasoning"}]},
        {"type":"message","content":[{"type":"output_text","text":"hello"},{"type":"output_text","text":" world"}]}
    ])), "fallback").unwrap();
    assert_eq!(result["choices"][0]["message"]["content"], "hello world");
    assert_eq!(result["choices"][0]["finish_reason"], "stop");
    assert_eq!(
        result["usage"],
        json!({"prompt_tokens":10,"completion_tokens":7,"total_tokens":17,"prompt_tokens_details":{"cached_tokens":4},"completion_tokens_details":{"reasoning_tokens":2}})
    );
    assert!(!result.to_string().contains("private reasoning"));
}

#[test]
fn exposes_call_id_instead_of_response_item_id() {
    let result = chat_completion(&response(json!([{"type":"function_call","id":"fc_item","call_id":"call_client","name":"f","arguments":"{}"}])), "fallback").unwrap();
    assert_eq!(
        result["choices"][0]["message"]["tool_calls"][0]["id"],
        "call_client"
    );
    assert_eq!(result["choices"][0]["finish_reason"], "tool_calls");
    assert!(result["choices"][0]["message"]["content"].is_null());
}

#[test]
fn preserves_refusal_and_incomplete_reasons_without_turning_failures_into_success() {
    let mut wire = response(
        json!([{"type":"message","content":[{"type":"refusal","refusal":"cannot comply"}]}]),
    );
    let result = chat_completion(&wire, "fallback").unwrap();
    assert_eq!(result["choices"][0]["message"]["refusal"], "cannot comply");
    wire["status"] = json!("incomplete");
    wire["incomplete_details"] = json!({"reason":"max_output_tokens"});
    assert_eq!(
        chat_completion(&wire, "fallback").unwrap()["choices"][0]["finish_reason"],
        "length"
    );
    wire["status"] = json!("failed");
    assert!(chat_completion(&wire, "fallback").is_err());
}

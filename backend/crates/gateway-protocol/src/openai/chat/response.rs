//! Responses 完整输出与用量到 Chat Completion 的投影

use serde_json::{Value, json};
use thiserror::Error;

/// 上游输出无法安全表达为 Chat Completion
#[derive(Debug, Error)]
#[error("Responses output cannot be delivered as a Chat Completion.")]
pub struct ChatResponseError;

/// 将已聚合的 Responses 终态转换为单候选 Chat 响应
pub fn chat_completion(response: &Value, model: &str) -> Result<Value, ChatResponseError> {
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .ok_or(ChatResponseError)?;
    let mut text = String::new();
    let mut refusal = String::new();
    let mut calls = Vec::new();
    for item in response
        .get("output")
        .and_then(Value::as_array)
        .ok_or(ChatResponseError)?
    {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for part in item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or(ChatResponseError)?
                {
                    match part.get("type").and_then(Value::as_str) {
                        Some("output_text") => text.push_str(
                            part.get("text")
                                .and_then(Value::as_str)
                                .ok_or(ChatResponseError)?,
                        ),
                        Some("refusal") => refusal.push_str(
                            part.get("refusal")
                                .and_then(Value::as_str)
                                .ok_or(ChatResponseError)?,
                        ),
                        _ => return Err(ChatResponseError),
                    }
                }
            }
            Some("function_call") => {
                let call = function_call(item)?;
                if calls
                    .iter()
                    .any(|existing: &Value| existing["id"] == call["id"])
                {
                    return Err(ChatResponseError);
                }
                calls.push(call);
            }
            Some("reasoning") => {}
            _ => return Err(ChatResponseError),
        }
    }
    let reason = finish_reason(response, !calls.is_empty())?;
    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() && (!calls.is_empty() || !refusal.is_empty()) { Value::Null } else { json!(text) },
        "refusal": if refusal.is_empty() { Value::Null } else { json!(refusal) },
    });
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(calls);
    }
    let mut result = json!({
        "id": id,
        "object": "chat.completion",
        "created": response.get("created_at").and_then(Value::as_u64).ok_or(ChatResponseError)?,
        "model": response.get("model").and_then(Value::as_str).unwrap_or(model),
        "choices": [{"index": 0, "message": message, "finish_reason": reason, "logprobs": null}],
    });
    if let Some(usage) = response.get("usage").filter(|v| !v.is_null()) {
        result["usage"] = chat_usage(usage)?;
    }
    if let Some(tier) = response.get("service_tier") {
        result["service_tier"] = tier.clone();
    }
    Ok(result)
}

pub(super) fn function_call(item: &Value) -> Result<Value, ChatResponseError> {
    Ok(json!({
        "id": item.get("call_id").and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or(ChatResponseError)?,
        "type": "function",
        "function": {
            "name": item.get("name").and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or(ChatResponseError)?,
            "arguments": item.get("arguments").and_then(Value::as_str).ok_or(ChatResponseError)?,
        },
    }))
}

pub(super) fn finish_reason(
    response: &Value,
    tools: bool,
) -> Result<&'static str, ChatResponseError> {
    match response.get("status").and_then(Value::as_str) {
        Some("completed") => Ok(if tools { "tool_calls" } else { "stop" }),
        Some("incomplete") => match response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
        {
            Some("max_output_tokens") => Ok("length"),
            Some("content_filter") => Ok("content_filter"),
            _ => Err(ChatResponseError),
        },
        _ => Err(ChatResponseError),
    }
}

pub(super) fn chat_usage(usage: &Value) -> Result<Value, ChatResponseError> {
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .ok_or(ChatResponseError)?;
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .ok_or(ChatResponseError)?;
    let mut result = json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": input.checked_add(output).ok_or(ChatResponseError)?,
    });
    if let Some(details) = usage.get("input_tokens_details").filter(|v| !v.is_null()) {
        result["prompt_tokens_details"] = details.clone();
    }
    if let Some(details) = usage.get("output_tokens_details").filter(|v| !v.is_null()) {
        result["completion_tokens_details"] = details.clone();
    }
    Ok(result)
}

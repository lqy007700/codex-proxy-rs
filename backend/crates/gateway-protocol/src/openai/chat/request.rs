//! Chat 消息、图片与函数工具到 Responses 输入的无状态转换

use serde_json::{Map, Value, json};
use thiserror::Error;

/// 已验证的 Chat 请求及其 Responses 表达
pub struct ChatRequest {
    pub responses: Map<String, Value>,
    pub stream: bool,
    pub include_usage: bool,
}

/// 不可按当前上游合同执行的 Chat 参数
#[derive(Debug, Error)]
#[error("{message}")]
pub struct ChatRequestError {
    pub param: String,
    pub message: &'static str,
}

fn invalid(param: impl Into<String>, message: &'static str) -> ChatRequestError {
    ChatRequestError {
        param: param.into(),
        message,
    }
}

/// 验证 Chat 请求并转换消息、工具和输出格式
pub fn decode_chat_request(body: &[u8]) -> Result<ChatRequest, ChatRequestError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| invalid("body", "Request body must be valid JSON."))?;
    let object = value
        .as_object()
        .ok_or_else(|| invalid("body", "Request body must be a JSON object."))?;
    for (key, value) in object {
        if value.is_null() {
            continue;
        }
        match key.as_str() {
            "model"
            | "messages"
            | "stream"
            | "stream_options"
            | "tools"
            | "tool_choice"
            | "parallel_tool_calls"
            | "max_tokens"
            | "max_completion_tokens"
            | "temperature"
            | "top_p"
            | "verbosity"
            | "reasoning_effort"
            | "response_format"
            | "metadata"
            | "service_tier"
            | "prompt_cache_key"
            | "prompt_cache_retention"
            | "prompt_cache_options"
            | "moderation"
            | "user"
            | "safety_identifier" => {}
            "n" if value.as_u64() == Some(1) => {}
            "store" if value.as_bool() == Some(false) => {}
            _ => {
                return Err(invalid(
                    key,
                    "This parameter is not supported by the Responses compatibility endpoint.",
                ));
            }
        }
    }
    let model = required_string(object.get("model"), "model")?;
    let stream = optional_bool(object.get("stream"), "stream", false)?;
    let mut include_usage = false;
    if let Some(options) = object.get("stream_options").filter(|v| !v.is_null()) {
        let options = options
            .as_object()
            .ok_or_else(|| invalid("stream_options", "stream_options must be an object."))?;
        if !stream {
            return Err(invalid(
                "stream_options",
                "stream_options requires stream=true.",
            ));
        }
        for key in options.keys() {
            if key != "include_usage" {
                return Err(invalid(
                    format!("stream_options.{key}"),
                    "This streaming option is not supported.",
                ));
            }
        }
        include_usage = optional_bool(
            options.get("include_usage"),
            "stream_options.include_usage",
            false,
        )?;
    }
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| invalid("messages", "messages must be a non-empty array."))?;
    let mut input = Vec::new();
    for message in messages {
        convert_message(message, &mut input)?;
    }
    let mut responses = Map::new();
    responses.insert("model".into(), json!(model));
    responses.insert("input".into(), Value::Array(input));
    responses.insert("stream".into(), json!(stream));
    responses.insert("store".into(), json!(false));
    for field in ["max_tokens", "max_completion_tokens"] {
        if let Some(value) = object.get(field).filter(|v| !v.is_null()) {
            let tokens = value.as_u64().filter(|tokens| *tokens > 0).ok_or_else(|| {
                invalid(
                    field,
                    if field == "max_tokens" {
                        "max_tokens must be a positive integer."
                    } else {
                        "max_completion_tokens must be a positive integer."
                    },
                )
            })?;
            // 新字段优先，具体上游是否接受长度控制仍由 Provider 决定
            responses.insert("max_output_tokens".into(), json!(tokens));
        }
    }
    for (field, maximum) in [("temperature", 2.0), ("top_p", 1.0)] {
        if let Some(value) = object.get(field).filter(|v| !v.is_null()) {
            if !value
                .as_f64()
                .is_some_and(|number| (0.0..=maximum).contains(&number))
            {
                return Err(invalid(field, "The sampling parameter is out of range."));
            }
            responses.insert(field.into(), value.clone());
        }
    }
    for field in [
        "user",
        "service_tier",
        "prompt_cache_key",
        "prompt_cache_retention",
        "safety_identifier",
    ] {
        if let Some(value) = object.get(field).filter(|v| !v.is_null()) {
            if !value.is_string() {
                return Err(invalid(field, "This parameter must be a string."));
            }
            responses.insert(field.into(), value.clone());
        }
    }
    for field in ["metadata", "prompt_cache_options", "moderation"] {
        if let Some(value) = object.get(field).filter(|v| !v.is_null()) {
            if !value.is_object() {
                return Err(invalid(field, "This parameter must be an object."));
            }
            responses.insert(field.into(), value.clone());
        }
    }
    if let Some(value) = object.get("parallel_tool_calls").filter(|v| !v.is_null()) {
        optional_bool(Some(value), "parallel_tool_calls", false)?;
        responses.insert("parallel_tool_calls".into(), value.clone());
    }
    if let Some(effort) = object.get("reasoning_effort").filter(|v| !v.is_null()) {
        required_string(Some(effort), "reasoning_effort")?;
        responses.insert("reasoning".into(), json!({"effort": effort}));
    }
    if let Some(tools) = object.get("tools").filter(|v| !v.is_null()) {
        let tools = tools
            .as_array()
            .ok_or_else(|| invalid("tools", "tools must be an array."))?;
        responses.insert(
            "tools".into(),
            Value::Array(tools.iter().map(convert_tool).collect::<Result<_, _>>()?),
        );
    }
    if let Some(choice) = object.get("tool_choice").filter(|v| !v.is_null()) {
        let choice = match choice.as_str() {
            Some("auto" | "none" | "required") => choice.clone(),
            _ if choice.get("type").and_then(Value::as_str) == Some("function") => {
                let name = required_string(
                    choice.pointer("/function/name"),
                    "tool_choice.function.name",
                )?;
                json!({"type": "function", "name": name})
            }
            _ => return Err(invalid("tool_choice", "Unsupported tool_choice.")),
        };
        responses.insert("tool_choice".into(), choice);
    }
    if let Some(format) = object.get("response_format").filter(|v| !v.is_null()) {
        let format = match format.get("type").and_then(Value::as_str) {
            Some("text" | "json_object") => format.clone(),
            Some("json_schema") => {
                let mut schema = format
                    .get("json_schema")
                    .and_then(Value::as_object)
                    .cloned()
                    .ok_or_else(|| {
                        invalid(
                            "response_format.json_schema",
                            "json_schema must be an object.",
                        )
                    })?;
                required_string(schema.get("name"), "response_format.json_schema.name")?;
                if !schema.get("schema").is_some_and(Value::is_object) {
                    return Err(invalid(
                        "response_format.json_schema.schema",
                        "schema must be an object.",
                    ));
                }
                schema.insert("type".into(), json!("json_schema"));
                Value::Object(schema)
            }
            _ => return Err(invalid("response_format", "Unsupported response_format.")),
        };
        responses.insert("text".into(), json!({"format": format}));
    }
    if let Some(verbosity) = object.get("verbosity").filter(|value| !value.is_null()) {
        required_string(Some(verbosity), "verbosity")?;
        responses.entry("text").or_insert_with(|| json!({}))["verbosity"] = verbosity.clone();
    }
    Ok(ChatRequest {
        responses,
        stream,
        include_usage,
    })
}

fn convert_message(value: &Value, input: &mut Vec<Value>) -> Result<(), ChatRequestError> {
    let message = value
        .as_object()
        .ok_or_else(|| invalid("messages", "Each message must be an object."))?;
    let role = required_string(message.get("role"), "messages.role")?;
    if !matches!(role, "system" | "developer" | "user" | "assistant" | "tool") {
        return Err(invalid("messages.role", "Unsupported message role."));
    }
    for key in message.keys() {
        if !matches!(
            key.as_str(),
            "role" | "content" | "tool_calls" | "tool_call_id" | "refusal"
        ) {
            return Err(invalid(
                format!("messages.{key}"),
                "This message field is not supported.",
            ));
        }
    }
    for (field, allowed_role) in [
        ("tool_calls", "assistant"),
        ("refusal", "assistant"),
        ("tool_call_id", "tool"),
    ] {
        if role != allowed_role && message.get(field).is_some_and(|value| !value.is_null()) {
            return Err(invalid(
                format!("messages.{field}"),
                "This field is not valid for the message role.",
            ));
        }
    }
    let mut content = convert_content(message.get("content"), role)?;
    if role == "tool" {
        let id = required_string(message.get("tool_call_id"), "messages.tool_call_id")?;
        let output = match message.get("content") {
            Some(Value::String(text)) => json!(text),
            Some(Value::Array(_)) => Value::Array(content),
            _ => return Err(invalid("messages.content", "Tool results require content.")),
        };
        input.push(json!({"type": "function_call_output", "call_id": id, "output": output}));
        return Ok(());
    }
    if let Some(refusal) = message.get("refusal").filter(|v| !v.is_null()) {
        if role != "assistant" {
            return Err(invalid(
                "messages.refusal",
                "Only assistant messages may contain refusal.",
            ));
        }
        content.push(json!({"type": "refusal", "refusal": required_string(Some(refusal), "messages.refusal")?}));
    }
    if !content.is_empty() {
        input.push(json!({"type": "message", "role": role, "content": content}));
    }
    let calls = message.get("tool_calls").filter(|v| !v.is_null());
    if let Some(calls) = calls {
        if role != "assistant" {
            return Err(invalid(
                "messages.tool_calls",
                "Only assistant messages may call tools.",
            ));
        }
        for call in calls
            .as_array()
            .ok_or_else(|| invalid("messages.tool_calls", "tool_calls must be an array."))?
        {
            if call.get("type").and_then(Value::as_str) != Some("function") {
                return Err(invalid(
                    "messages.tool_calls",
                    "Only function tools are supported.",
                ));
            }
            input.push(json!({
                "type": "function_call",
                "call_id": required_string(call.get("id"), "messages.tool_calls.id")?,
                "name": required_string(call.pointer("/function/name"), "messages.tool_calls.function.name")?,
                "arguments": call.pointer("/function/arguments").and_then(Value::as_str)
                    .ok_or_else(|| invalid("messages.tool_calls.function.arguments", "Tool arguments must be a string."))?,
            }));
        }
    }
    if content_is_missing(message) && calls.and_then(Value::as_array).is_none_or(Vec::is_empty) {
        return Err(invalid("messages.content", "Message content is required."));
    }
    Ok(())
}

fn content_is_missing(message: &Map<String, Value>) -> bool {
    message.get("content").is_none_or(Value::is_null)
        && message.get("refusal").is_none_or(Value::is_null)
}

fn convert_content(value: Option<&Value>, role: &str) -> Result<Vec<Value>, ChatRequestError> {
    let kind = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(text)) => Ok(vec![json!({"type": kind, "text": text})]),
        Some(Value::Array(parts)) => parts.iter().map(|part| match part.get("type").and_then(Value::as_str) {
            Some("text") => Ok(json!({"type": kind, "text": part.get("text").and_then(Value::as_str)
                .ok_or_else(|| invalid("messages.content.text", "Text content must be a string."))?})),
            Some("refusal") if role == "assistant" => Ok(json!({"type":"refusal", "refusal": required_string(part.get("refusal"), "messages.content.refusal")?})),
            Some("image_url") if role == "user" => {
                let image = part.get("image_url").ok_or_else(|| invalid("messages.content.image_url", "Missing image_url."))?;
                let url = required_string(image.get("url"), "messages.content.image_url.url")?;
                let detail = match image.get("detail") {
                    None | Some(Value::Null) => "auto",
                    Some(Value::String(detail)) => detail,
                    _ => return Err(invalid("messages.content.image_url.detail", "Image detail must be a string.")),
                };
                if !matches!(detail, "auto" | "low" | "high" | "original") {
                    return Err(invalid("messages.content.image_url.detail", "Unsupported image detail."));
                }
                Ok(json!({"type": "input_image", "image_url": url, "detail": detail}))
            }
            _ => Err(invalid("messages.content", "Only text and user image_url content are supported.")),
        }).collect(),
        _ => Err(invalid("messages.content", "content must be a string or an array of content parts.")),
    }
}

fn convert_tool(value: &Value) -> Result<Value, ChatRequestError> {
    if value.get("type").and_then(Value::as_str) != Some("function") {
        return Err(invalid("tools.type", "Only function tools are supported."));
    }
    let mut function = value
        .get("function")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| invalid("tools.function", "function must be an object."))?;
    required_string(function.get("name"), "tools.function.name")?;
    for key in function.keys() {
        if !matches!(
            key.as_str(),
            "name" | "description" | "parameters" | "strict"
        ) {
            return Err(invalid(
                format!("tools.function.{key}"),
                "Unsupported function field.",
            ));
        }
    }
    if let Some(strict) = function.get("strict").filter(|v| !v.is_null()) {
        optional_bool(Some(strict), "tools.function.strict", false)?;
    } else {
        // Chat 默认非严格；不能让 Responses 的缺省严格化改变调用方的 schema 语义
        function.insert("strict".into(), json!(false));
    }
    function.insert("type".into(), json!("function"));
    Ok(Value::Object(function))
}

fn required_string<'a>(value: Option<&'a Value>, param: &str) -> Result<&'a str, ChatRequestError> {
    value
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| invalid(param, "A non-empty string is required."))
}

fn optional_bool(
    value: Option<&Value>,
    param: &str,
    default: bool,
) -> Result<bool, ChatRequestError> {
    match value {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(invalid(param, "A boolean is required.")),
    }
}

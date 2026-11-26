//! 下游客户端模型请求正文的 Codex 协议兼容

use std::collections::HashSet;

use serde_json::{Map, Value, json};

/// 非官方客户端及跨客户端历史回填的兼容入口；调用方必须已选定 Codex/OAuth 上游
/// 只处理已验证被拒绝的字段，不根据客户端品牌推断整份历史是否合法
pub(super) fn normalize_non_codex_request_body(body: &mut Map<String, Value>) {
    normalize_codex_oauth_options(body);
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.get("type").and_then(Value::as_str) == Some("reasoning") {
            normalize_replayed_reasoning(item);
        }
    }
}

/// 清理 Responses 输入历史中不符合类型前缀的可选内联条目 ID
/// 保留合法前缀、被 item_reference 引用的条目以及绑定密文的条目
pub(super) fn normalize_replayed_responses_input_ids(body: &mut Map<String, Value>) {
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    let mut candidates = Vec::new();
    let mut referenced_ids = HashSet::new();
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        let kind = item.get("type").and_then(Value::as_str);
        let is_reference = kind == Some("item_reference")
            || (item.get("type").is_none_or(Value::is_null)
                && !item.contains_key("role")
                && !item.contains_key("content"));
        if is_reference {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                referenced_ids.insert(id.to_owned());
            }
            continue;
        }
        if invalid_inline_item_id(item, kind).is_some() {
            candidates.push(item);
        }
    }
    // 引用可以出现在目标条目前后，收齐后才能省略未被引用的可选 ID
    for item in candidates {
        let kind = item.get("type").and_then(Value::as_str);
        if invalid_inline_item_id(item, kind).is_some_and(|id| !referenced_ids.contains(id)) {
            item.shift_remove("id");
        }
    }
}

fn invalid_inline_item_id<'a>(item: &'a Map<String, Value>, kind: Option<&str>) -> Option<&'a str> {
    let id = item.get("id")?.as_str()?;
    let expected_prefix = match kind {
        Some("message") | None if has_inline_message_content(item) => "msg_",
        Some("reasoning") if has_plaintext_reasoning_shape(item) => "rs_",
        Some("function_call") if has_plaintext_tool_call_shape(item) => "fc_",
        Some("function_call_output") if has_plaintext_tool_call_shape(item) => "fco_",
        Some("custom_tool_call") if has_plaintext_tool_call_shape(item) => "ctc_",
        Some("custom_tool_call_output") if has_plaintext_tool_call_shape(item) => "ctco_",
        _ => return None,
    };
    (!has_expected_id_prefix(id, expected_prefix)).then_some(id)
}

fn has_expected_id_prefix(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.is_empty())
}

fn has_non_empty_encrypted_value(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(_) => true,
    }
}

fn has_encrypted_fields(item: &Map<String, Value>) -> bool {
    has_non_empty_encrypted_value(item.get("encrypted_content"))
        || has_non_empty_encrypted_value(item.get("encrypted_function_args"))
}

fn is_inline_message_part(part: &Value) -> bool {
    let Some(part) = part.as_object() else {
        return false;
    };
    if has_encrypted_fields(part) {
        return false;
    }
    match part.get("type").and_then(Value::as_str) {
        Some("input_text" | "output_text") => part.get("text").is_some_and(Value::is_string),
        Some("refusal") => part.get("refusal").is_some_and(Value::is_string),
        Some("input_image" | "input_file" | "input_audio") => true,
        _ => false,
    }
}

fn has_inline_message_content(item: &Map<String, Value>) -> bool {
    matches!(
        item.get("role").and_then(Value::as_str),
        Some("user" | "assistant" | "developer" | "system")
    ) && !has_encrypted_fields(item)
        && match item.get("content") {
            Some(Value::String(text)) => !text.is_empty(),
            Some(Value::Array(parts)) => {
                !parts.is_empty() && parts.iter().all(is_inline_message_part)
            }
            _ => false,
        }
}

fn has_plaintext_reasoning_shape(item: &Map<String, Value>) -> bool {
    !has_encrypted_fields(item)
        && (item.contains_key("summary")
            || item.contains_key("content")
            || item.contains_key("encrypted_content"))
        && item
            .get("content")
            .and_then(Value::as_array)
            .is_none_or(|parts| {
                !parts.iter().any(|part| {
                    part.as_object().is_some_and(|part| {
                        part.get("type").and_then(Value::as_str) == Some("encrypted_content")
                            || has_encrypted_fields(part)
                    })
                })
            })
}

fn has_plaintext_tool_call_shape(item: &Map<String, Value>) -> bool {
    !has_encrypted_fields(item)
        && item
            .get("call_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
}

fn normalize_replayed_reasoning(item: &mut Map<String, Value>) {
    // SDK 输出项的 status 不属于 Codex reasoning 输入合同；其他项的 status 可能合法
    item.shift_remove("status");
    // Codex 只接受空 content 数组；仅在加密历史仍可回填时去掉冗余明文，
    // 没有加密内容的历史不自动丢弃，保留给上游明确拒绝
    if item
        .get("encrypted_content")
        .and_then(Value::as_str)
        .is_some_and(|content| !content.trim().is_empty())
        && item
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|content| !content.is_empty())
    {
        item.shift_remove("content");
    }
}

fn normalize_codex_oauth_options(body: &mut Map<String, Value>) {
    // Codex OAuth 拒绝这些客户端选项，即使采样参数取默认值
    for field in [
        "top_p",
        "frequency_penalty",
        "presence_penalty",
        "chat_template_kwargs",
        "user",
        "metadata",
        "safety_identifier",
    ] {
        body.shift_remove(field);
    }
    // Responses 自带用量；保留官方流式推理摘要等其他选项
    if let Some(options) = body
        .get_mut("stream_options")
        .and_then(Value::as_object_mut)
    {
        options.shift_remove("include_usage");
    }
    // 只省略没有实际效果的配置，不静默取消客户端要求的截断、停止条件或缓存策略
    if body.get("truncation").and_then(Value::as_str) == Some("disabled") {
        body.shift_remove("truncation");
    }
    if body
        .get("stop_sequences")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        body.shift_remove("stop_sequences");
    }
    if body
        .get("prompt_cache_options")
        .and_then(Value::as_object)
        .is_some_and(Map::is_empty)
    {
        body.shift_remove("prompt_cache_options");
    }
    // 原生上限优先，Chat 别名只补缺，非法值仍交给上游校验
    for alias in ["max_completion_tokens", "max_tokens"] {
        if let Some(limit) = body.shift_remove(alias)
            && !limit.is_null()
            && body.get("max_output_tokens").is_none_or(Value::is_null)
        {
            body.insert("max_output_tokens".to_owned(), limit);
        }
    }
}

/// 补齐 Codex 请求缺省字段并适配已确认不兼容的请求形状，不递归清洗业务正文
///
/// 兼容基准是 Codex Core/Desktop 的模型请求，不是公开 OpenAI Responses API
/// 未知字段继续透传，不能因官方请求结构中没有某个字段就将其列入过滤规则
pub(crate) fn normalize_codex_request_body(body: &mut Map<String, Value>) {
    // 官方 Core/Desktop 显式发送 store=false；仅为缺字段的下游请求补齐，保留显式值
    body.entry("store").or_insert(Value::Bool(false));

    // 公开 Responses API 允许 `input` 为字符串（等价于一条 user 文本消息），
    // 而 Codex 后端只接受条目数组，否则返回 400 "Input must be a list"
    // 这里按官方 ResponseItem::Message 的形状展开；其他非数组类型不猜测语义，交给上游判定
    if let Some(Value::String(text)) = body.get_mut("input") {
        let text = std::mem::take(text);
        body.insert(
            "input".to_owned(),
            json!([{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": text}],
            }]),
        );
    }

    // Codex 上游拒绝显式 message 的 system role；沿用官方客户端的 developer
    // role 承载指令，只转换已确认的消息形状，保留内容与其他字段
    if let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) {
        for item in input {
            let Some(item) = item.as_object_mut() else {
                continue;
            };
            if item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("role").and_then(Value::as_str) == Some("system")
            {
                item.insert("role".to_owned(), Value::String("developer".to_owned()));
            }
        }
    }

    // Codex 原生请求拒绝显式输出上限；同时带有 Chat 别名时保留原生值供 OAuth 兼容层优先使用
    if !body.contains_key("max_completion_tokens") && !body.contains_key("max_tokens") {
        body.remove("max_output_tokens");
    }
    // Codex 拒绝显式采样温度和缓存保留时长，独立的缓存键仍须保留
    for field in ["temperature", "prompt_cache_retention"] {
        body.remove(field);
    }
}

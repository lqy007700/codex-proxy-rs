//! 智谱 Responses SSE 的原始交付、终态核验与 canonical 用量投影

use bytes::Bytes;
use gateway_core::{
    error::{ClientVisibleUpstreamError, ProviderError, ProviderErrorKind},
    event::{
        ContentItem, ContentKind, FinishReason, GatewayEvent, ProtocolWireEvent, ProviderEvent,
        ReasoningDelta, ResponseMeta, TextDelta, ToolCallDelta,
    },
    metering::Usage,
    upstream::{OpaqueUpstreamValue, UpstreamSendState},
};
use gateway_protocol::openai::sse::{MAX_SSE_EVENT_BUFFER_BYTES, SseEventDecoder, SseFrame};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn error(kind: ProviderErrorKind, state: UpstreamSendState) -> ProviderError {
    ProviderError::new(kind, state)
}

/// 明确的业务拒绝才提供重放证明，未知服务端失败不能证明上游未执行
pub(crate) fn upstream_error(status: u16, value: &Value) -> ProviderError {
    let code = value
        .get("error")
        .unwrap_or(value)
        .get("code")
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_u64().map(|number| number.to_string()))
        })
        .unwrap_or_default();
    let classified = match code.as_str() {
        "authentication_error"
        | "invalid_api_key"
        | "expired"
        | "1000"
        | "1001"
        | "1003"
        | "1005" => Some((ProviderErrorKind::Unauthorized, true)),
        "permission_denied" | "1220" | "1311" | "1315" => {
            Some((ProviderErrorKind::PermissionDenied, true))
        }
        "insufficient_quota"
        | "quota_exceeded"
        | "usage_limit_reached"
        | "usage_not_included"
        | "1113"
        | "1308"
        | "1309"
        | "1310"
        | "1314"
        | "1316"
        | "1317"
        | "1318"
        | "1319"
        | "1320"
        | "1321" => Some((ProviderErrorKind::QuotaExhausted, true)),
        "rate_limit_exceeded" | "1302" | "1313" => Some((ProviderErrorKind::RateLimited, true)),
        "server_is_overloaded" | "overloaded" | "slow_down" | "1305" => {
            Some((ProviderErrorKind::UpstreamCapacityUnavailable, true))
        }
        "invalid_request"
        | "model_not_found"
        | "context_length_exceeded"
        | "1210"
        | "1211"
        | "1212"
        | "1213"
        | "1214"
        | "1215"
        | "1221"
        | "1222"
        | "1261" => Some((ProviderErrorKind::InvalidRequest, false)),
        "not_implemented" => Some((ProviderErrorKind::Unsupported, false)),
        "request_too_large" => Some((ProviderErrorKind::MessageTooBig, false)),
        "server_error" | "1200" | "1230" | "1234" => Some((ProviderErrorKind::Unavailable, false)),
        "content_filter" | "cyber_policy" | "1301" => {
            Some((ProviderErrorKind::PermissionDenied, false))
        }
        _ => None,
    };
    let (kind, replay_safe) = classified.unwrap_or(match status {
        401 => (ProviderErrorKind::Unauthorized, true),
        403 => (ProviderErrorKind::PermissionDenied, false),
        429 => (ProviderErrorKind::RateLimited, true),
        500..=599 => (ProviderErrorKind::Unavailable, false),
        400..=499 => (ProviderErrorKind::InvalidRequest, false),
        _ => (ProviderErrorKind::Protocol, false),
    });
    // 错误码也可能包含认证材料，只公开官方定义的有限集合
    let public_code = if classified.is_some() {
        code.as_str()
    } else {
        "glm_upstream_error"
    };
    let mut failure = error(kind, UpstreamSendState::Sent)
        .with_status(status)
        .with_upstream_code(OpaqueUpstreamValue::new(public_code))
        .with_client_visible_upstream_error(ClientVisibleUpstreamError::new(
            "智谱上游请求失败",
            Some(public_code.to_owned()),
            None,
        ));
    if replay_safe {
        failure = failure.with_replay_safe();
    }
    if matches!(code.as_str(), "content_filter" | "cyber_policy" | "1301") {
        failure = failure.with_retry_prohibited();
    }
    failure
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ContentType {
    Text,
    Reasoning,
    Summary,
    Tool,
}

impl ContentType {
    fn kind(self) -> ContentKind {
        match self {
            Self::Text => ContentKind::Text,
            Self::Reasoning | Self::Summary => ContentKind::Reasoning,
            Self::Tool => ContentKind::ToolCall,
        }
    }
}

struct Content {
    index: u32,
    text: String,
    done: bool,
}

struct Item {
    id: Option<String>,
    kind: String,
    call: Option<(String, String)>,
    done: bool,
}

pub(crate) struct Decoder {
    sse: SseEventDecoder,
    model: String,
    response_id: Option<String>,
    items: BTreeMap<u64, Item>,
    contents: BTreeMap<(u64, u64, ContentType), Content>,
    buffered_content_bytes: usize,
    pricing: Option<gateway_core::metering::ModelPriceOverride>,
    pub completed: bool,
    pub semantic_output: bool,
    failed: bool,
}

impl Decoder {
    pub fn new(model: String, pricing: Option<gateway_core::metering::ModelPriceOverride>) -> Self {
        Self {
            sse: SseEventDecoder::default(),
            model,
            response_id: None,
            items: BTreeMap::new(),
            contents: BTreeMap::new(),
            buffered_content_bytes: 0,
            pricing,
            completed: false,
            semantic_output: false,
            failed: false,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<ProviderEvent, ProviderError>> {
        if self.failed {
            return Vec::new();
        }
        let frames = self.sse.push_frames(bytes);
        self.frames(frames)
    }

    pub fn finish(&mut self) -> Vec<Result<ProviderEvent, ProviderError>> {
        if self.failed {
            return Vec::new();
        }
        let frames = self.sse.finish_frames();
        let mut events = self.frames(frames);
        if !self.completed && !self.failed {
            self.failed = true;
            events.push(Err(self.failure(protocol())));
        }
        events
    }

    fn frames(&mut self, frames: Vec<SseFrame>) -> Vec<Result<ProviderEvent, ProviderError>> {
        let mut output = Vec::new();
        for frame in frames {
            if let Err(failure) = self.frame(frame, &mut output) {
                self.failed = true;
                output.push(Err(self.failure(failure)));
                break;
            }
        }
        output
    }

    fn failure(&self, failure: ProviderError) -> ProviderError {
        if self.semantic_output {
            failure.with_retry_prohibited()
        } else {
            failure
        }
    }

    fn frame(
        &mut self,
        frame: SseFrame,
        output: &mut Vec<Result<ProviderEvent, ProviderError>>,
    ) -> Result<(), ProviderError> {
        let (raw, events) = frame.into_parts();
        if events.len() > 1 {
            return Err(protocol());
        }
        let Some(event) = events.into_iter().next() else {
            if !self.completed {
                let replayable = std::str::from_utf8(&raw).is_ok_and(|frame| {
                    frame.lines().all(|line| {
                        let line = line.trim();
                        line.is_empty()
                            || line.starts_with(':')
                            || line.starts_with("id:")
                            || line.starts_with("retry:")
                    })
                });
                let mut event = ProviderEvent::wire(
                    ProtocolWireEvent::raw_sse("openai", Bytes::from(raw))
                        .map_err(|_| protocol())?,
                );
                if replayable {
                    event = event.with_replayable_prelude();
                }
                output.push(Ok(event));
            }
            return Ok(());
        };
        if event.data.trim() == "[DONE]" {
            return if self.completed {
                Ok(())
            } else {
                Err(protocol())
            };
        }
        if self.completed {
            return Err(protocol());
        }
        let value: Value = serde_json::from_str(&event.data).map_err(|_| protocol())?;
        if !value.is_object() {
            return Err(protocol());
        }
        let body_type = optional_text(&value, "type")?;
        if let (Some(body), Some(named)) = (body_type, event.event.as_deref())
            && body != named
        {
            return Err(protocol());
        }
        let kind = body_type.or(event.event.as_deref()).ok_or_else(protocol)?;
        let mut facts = Vec::new();
        if kind != "error" {
            self.observe_response(&value, kind, &mut facts)?;
        } else {
            self.validate_response_id(&value)?;
        }
        if matches!(kind, "response.failed" | "error") {
            let response = if kind == "response.failed" {
                let response = value.get("response").ok_or_else(protocol)?;
                if text(response, "status")? != "failed" {
                    return Err(protocol());
                }
                self.complete_items(response, &mut facts)?;
                response
            } else {
                &value
            };
            self.usage(response, &mut facts)?;
            // 计量先进入 Core，脱敏后的失败终态留给恢复决策原子交付
            output.extend(
                facts
                    .into_iter()
                    .map(|fact| Ok(ProviderEvent::canonical(fact).with_replayable_prelude())),
            );
            let failure = upstream_error(200, response);
            let public = failure
                .client_visible_upstream_error()
                .ok_or_else(protocol)?;
            let safe_error = json!({"code":public.code(), "message":public.message()});
            // 错误字段不属于业务透传，未知扩展和 SSE 标识也可能回显认证材料
            let terminal = if kind == "response.failed" {
                let mut safe_response = json!({
                    "id":self.response_id,
                    "object":"response",
                    "status":"failed",
                    "model":self.model,
                    "error":safe_error,
                });
                for field in ["output", "usage"] {
                    if let Some(value) = response.get(field) {
                        safe_response[field] = value.clone();
                    }
                }
                json!({"type":kind, "response":safe_response})
            } else {
                json!({"type":kind, "code":public.code(), "message":public.message()})
            };
            let wire = ProtocolWireEvent::json("openai", Some(kind.to_owned()), terminal)
                .map_err(|_| protocol())?;
            return Err(failure.with_atomic_client_events(vec![ProviderEvent::wire(wire)]));
        }
        self.facts(kind, &value, &mut facts)?;
        let replayable = matches!(
            kind,
            "response.created" | "response.in_progress" | "response.queued"
        ) && !self.semantic_output;
        let wire = ProtocolWireEvent::json_with_raw_sse_metadata(
            "openai",
            Some(kind.to_owned()),
            value,
            Bytes::from(raw),
            event.id,
            event.retry,
        )
        .map_err(|_| protocol())?;
        let mut event = ProviderEvent::canonical_with_wire(facts, wire);
        if replayable {
            event = event.with_replayable_prelude();
        }
        output.push(Ok(event));
        Ok(())
    }

    fn observe_response(
        &mut self,
        value: &Value,
        kind: &str,
        facts: &mut Vec<GatewayEvent>,
    ) -> Result<(), ProviderError> {
        if self.response_id.is_none() {
            let id = value
                .pointer("/response/id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(protocol)?;
            self.response_id = Some(id.to_owned());
            facts.push(GatewayEvent::Started(ResponseMeta::new(id, &self.model)));
        } else if kind == "response.created" {
            return Err(protocol());
        }
        self.validate_response_id(value)
    }

    fn validate_response_id(&self, value: &Value) -> Result<(), ProviderError> {
        for id in [value.get("response_id"), value.pointer("/response/id")]
            .into_iter()
            .flatten()
        {
            if id.as_str() != self.response_id.as_deref() {
                return Err(protocol());
            }
        }
        Ok(())
    }

    fn bind_item(
        &mut self,
        index: u64,
        kind: &str,
        id: Option<&str>,
        call: Option<(&str, &str)>,
    ) -> Result<(), ProviderError> {
        if self.items.iter().any(|(other_index, item)| {
            *other_index != index
                && (id.is_some() && item.id.as_deref() == id
                    || call.is_some_and(|(call_id, _)| {
                        item.call.as_ref().is_some_and(|item| item.0 == call_id)
                    }))
        }) {
            return Err(protocol());
        }
        if let Some(item) = self.items.get_mut(&index) {
            if item.kind != kind
                || item
                    .id
                    .as_deref()
                    .zip(id)
                    .is_some_and(|(old, new)| old != new)
                || item
                    .call
                    .as_ref()
                    .zip(call)
                    .is_some_and(|(old, new)| old.0 != new.0 || old.1 != new.1)
            {
                return Err(protocol());
            }
            if item.id.is_none() {
                item.id = id.map(str::to_owned);
            }
            if item.call.is_none() {
                item.call = call.map(|(id, name)| (id.to_owned(), name.to_owned()));
            }
        } else {
            if self.items.len() >= 4096 {
                return Err(protocol());
            }
            self.items.insert(
                index,
                Item {
                    id: id.map(str::to_owned),
                    kind: kind.to_owned(),
                    call: call.map(|(id, name)| (id.to_owned(), name.to_owned())),
                    done: false,
                },
            );
        }
        Ok(())
    }

    fn content(
        &mut self,
        key: (u64, u64, ContentType),
        facts: &mut Vec<GatewayEvent>,
    ) -> Result<&mut Content, ProviderError> {
        if !self.contents.contains_key(&key) {
            let index = u32::try_from(self.contents.len()).map_err(|_| protocol())?;
            if index >= 4096 {
                return Err(protocol());
            }
            self.contents.insert(
                key,
                Content {
                    index,
                    text: String::new(),
                    done: false,
                },
            );
            facts.push(GatewayEvent::ContentAdded(ContentItem::new(
                index,
                key.2.kind(),
            )));
        }
        self.contents.get_mut(&key).ok_or_else(protocol)
    }

    fn fragment(
        &mut self,
        key: (u64, u64, ContentType),
        text: &str,
        snapshot: bool,
        done: bool,
        facts: &mut Vec<GatewayEvent>,
    ) -> Result<(), ProviderError> {
        let buffered = self.buffered_content_bytes;
        let content = self.content(key, facts)?;
        let delta = if snapshot {
            text.strip_prefix(&content.text).ok_or_else(protocol)?
        } else {
            text
        };
        if content.done && (!snapshot || !delta.is_empty()) {
            return Err(protocol());
        }
        let next_buffered = buffered
            .checked_add(delta.len())
            .filter(|bytes| *bytes <= MAX_SSE_EVENT_BUFFER_BYTES)
            .ok_or_else(protocol)?;
        let index = content.index;
        content.text.push_str(delta);
        content.done |= done;
        let delta = delta.to_owned();
        self.buffered_content_bytes = next_buffered;
        if delta.is_empty() {
            return Ok(());
        }
        self.semantic_output = true;
        match key.2 {
            ContentType::Text => facts.push(GatewayEvent::TextDelta(TextDelta {
                content_index: index,
                text: delta,
            })),
            ContentType::Reasoning | ContentType::Summary => {
                facts.push(GatewayEvent::ReasoningDelta(ReasoningDelta {
                    content_index: index,
                    text: delta,
                }))
            }
            ContentType::Tool => {
                let call = self
                    .items
                    .get(&key.0)
                    .and_then(|item| item.call.as_ref())
                    .ok_or_else(protocol)?;
                facts.push(GatewayEvent::ToolCallDelta(ToolCallDelta {
                    content_index: index,
                    call_id: call.0.clone(),
                    name: None,
                    arguments_delta: delta,
                }));
            }
        }
        Ok(())
    }

    fn item(
        &mut self,
        output_index: u64,
        item: &Value,
        done: bool,
        facts: &mut Vec<GatewayEvent>,
    ) -> Result<(), ProviderError> {
        let kind = text(item, "type")?;
        let call = if matches!(kind, "function_call" | "custom_tool_call") {
            Some((text(item, "call_id")?, text(item, "name")?))
        } else {
            None
        };
        self.bind_item(output_index, kind, optional_text(item, "id")?, call)?;
        if let Some((call_id, name)) = call {
            let key = (output_index, 0, ContentType::Tool);
            if !self.contents.contains_key(&key) {
                let index = self.content(key, facts)?.index;
                self.semantic_output = true;
                facts.push(GatewayEvent::ToolCallDelta(ToolCallDelta {
                    content_index: index,
                    call_id: call_id.to_owned(),
                    name: Some(name.to_owned()),
                    arguments_delta: String::new(),
                }));
            }
            let field = if kind == "custom_tool_call" {
                "input"
            } else {
                "arguments"
            };
            if let Some(arguments) = item.get(field) {
                self.fragment(
                    key,
                    arguments.as_str().ok_or_else(protocol)?,
                    true,
                    done,
                    facts,
                )?;
            }
        } else {
            for field in ["content", "summary"] {
                if let Some(parts) = item.get(field) {
                    if let Some(parts) = parts.as_array() {
                        for (part_index, part) in parts.iter().enumerate() {
                            self.part(
                                output_index,
                                part_index as u64,
                                part,
                                done,
                                field == "summary",
                                facts,
                            )?;
                        }
                    } else if field == "summary" && parts.is_string() {
                        self.fragment(
                            (output_index, 0, ContentType::Summary),
                            parts.as_str().ok_or_else(protocol)?,
                            true,
                            done,
                            facts,
                        )?;
                    } else {
                        return Err(protocol());
                    }
                }
            }
            if let Some(value) = item.get("text") {
                self.fragment(
                    (
                        output_index,
                        0,
                        if kind == "reasoning" {
                            ContentType::Reasoning
                        } else {
                            ContentType::Text
                        },
                    ),
                    value.as_str().ok_or_else(protocol)?,
                    true,
                    done,
                    facts,
                )?;
            }
        }
        if done {
            self.items.get_mut(&output_index).ok_or_else(protocol)?.done = true;
        }
        Ok(())
    }

    fn part(
        &mut self,
        output: u64,
        index: u64,
        part: &Value,
        done: bool,
        summary: bool,
        facts: &mut Vec<GatewayEvent>,
    ) -> Result<(), ProviderError> {
        let (kind, field) = match text(part, "type")? {
            "output_text" | "text" => (ContentType::Text, "text"),
            "refusal" => (ContentType::Text, "refusal"),
            "reasoning_text" => (ContentType::Reasoning, "text"),
            "summary_text" => (ContentType::Summary, "text"),
            _ => return Ok(()),
        };
        let kind = if summary { ContentType::Summary } else { kind };
        self.fragment(
            (output, index, kind),
            part.get(field)
                .and_then(Value::as_str)
                .ok_or_else(protocol)?,
            true,
            done,
            facts,
        )
    }

    fn complete_items(
        &mut self,
        response: &Value,
        facts: &mut Vec<GatewayEvent>,
    ) -> Result<(), ProviderError> {
        if let Some(items) = response.get("output") {
            for (index, item) in items.as_array().ok_or_else(protocol)?.iter().enumerate() {
                self.item(index as u64, item, true, facts)?;
            }
        }
        Ok(())
    }

    fn usage(&self, response: &Value, facts: &mut Vec<GatewayEvent>) -> Result<(), ProviderError> {
        let Some(usage) = response.get("usage").filter(|value| !value.is_null()) else {
            return Ok(());
        };
        if !usage.is_object() {
            return Err(protocol());
        }
        let mut measured = Usage::new();
        measured.input_tokens = optional_count(usage, "input_tokens")?;
        measured.output_tokens = optional_count(usage, "output_tokens")?;
        measured.cached_input_tokens = usage
            .get("input_tokens_details")
            .map(|details| optional_count(details, "cached_tokens"))
            .transpose()?
            .flatten();
        measured.reasoning_output_tokens = usage
            .get("output_tokens_details")
            .map(|details| optional_count(details, "reasoning_tokens"))
            .transpose()?
            .flatten();
        measured.total_tokens = optional_count(usage, "total_tokens")?;
        if let Some(breakdown) = crate::pricing::breakdown(&measured, self.pricing.as_ref()) {
            facts.push(GatewayEvent::CalculatedCost(breakdown.calculated_cost()));
        }
        facts.push(GatewayEvent::Usage(measured));
        Ok(())
    }

    fn facts(
        &mut self,
        kind: &str,
        value: &Value,
        facts: &mut Vec<GatewayEvent>,
    ) -> Result<(), ProviderError> {
        match kind {
            "response.output_item.added" | "response.output_item.done" => {
                let index = count(value, "output_index")?;
                if self.items.get(&index).is_some_and(|item| item.done) {
                    return Err(protocol());
                }
                self.item(
                    index,
                    value.get("item").ok_or_else(protocol)?,
                    kind.ends_with(".done"),
                    facts,
                )?;
            }
            "response.output_text.delta"
            | "response.refusal.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.output_text.done"
            | "response.refusal.done"
            | "response.reasoning_text.done"
            | "response.reasoning_summary_text.done" => {
                let output = count(value, "output_index")?;
                let reasoning = kind.contains("reasoning");
                let summary = kind.contains("summary");
                self.bind_item(
                    output,
                    if reasoning { "reasoning" } else { "message" },
                    optional_text(value, "item_id")?,
                    None,
                )?;
                if self.items.get(&output).is_some_and(|item| item.done) {
                    return Err(protocol());
                }
                let index = optional_count(
                    value,
                    if summary {
                        "summary_index"
                    } else {
                        "content_index"
                    },
                )?
                .unwrap_or(0);
                let content_type = if summary {
                    ContentType::Summary
                } else if reasoning {
                    ContentType::Reasoning
                } else {
                    ContentType::Text
                };
                let done = kind.ends_with(".done");
                let field = if !done {
                    "delta"
                } else if kind.contains("refusal") {
                    "refusal"
                } else {
                    "text"
                };
                self.fragment(
                    (output, index, content_type),
                    value
                        .get(field)
                        .and_then(Value::as_str)
                        .ok_or_else(protocol)?,
                    done,
                    done,
                    facts,
                )?;
            }
            "response.content_part.added"
            | "response.content_part.done"
            | "response.reasoning_part.added"
            | "response.reasoning_part.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done" => {
                let output = count(value, "output_index")?;
                let reasoning = kind.contains("reasoning");
                let summary = kind.contains("summary");
                self.bind_item(
                    output,
                    if reasoning { "reasoning" } else { "message" },
                    optional_text(value, "item_id")?,
                    None,
                )?;
                if self.items.get(&output).is_some_and(|item| item.done) {
                    return Err(protocol());
                }
                let index = optional_count(
                    value,
                    if summary {
                        "summary_index"
                    } else {
                        "content_index"
                    },
                )?
                .unwrap_or(0);
                self.part(
                    output,
                    index,
                    value
                        .get("part")
                        .or_else(|| value.get("summary_part"))
                        .ok_or_else(protocol)?,
                    kind.ends_with(".done"),
                    summary,
                    facts,
                )?;
            }
            "response.function_call_arguments.delta"
            | "response.custom_tool_call_input.delta"
            | "response.function_call_arguments.done"
            | "response.custom_tool_call_input.done" => {
                let output = count(value, "output_index")?;
                let expected = if kind.starts_with("response.function") {
                    "function_call"
                } else {
                    "custom_tool_call"
                };
                let item = self.items.get(&output).ok_or_else(protocol)?;
                if item.done
                    || item.kind != expected
                    || item.call.is_none()
                    || optional_text(value, "call_id")?
                        .is_some_and(|id| item.call.as_ref().is_none_or(|call| call.0 != id))
                {
                    return Err(protocol());
                }
                self.bind_item(output, expected, optional_text(value, "item_id")?, None)?;
                let done = kind.ends_with(".done");
                let field = if !done {
                    "delta"
                } else if expected == "function_call" {
                    "arguments"
                } else {
                    "input"
                };
                self.fragment(
                    (output, 0, ContentType::Tool),
                    value
                        .get(field)
                        .and_then(Value::as_str)
                        .ok_or_else(protocol)?,
                    done,
                    done,
                    facts,
                )?;
            }
            "response.completed" | "response.incomplete" => {
                let response = value.get("response").ok_or_else(protocol)?;
                let expected = if kind == "response.completed" {
                    "completed"
                } else {
                    "incomplete"
                };
                if text(response, "status")? != expected {
                    return Err(protocol());
                }
                self.complete_items(response, facts)?;
                if kind == "response.completed" && !self.semantic_output {
                    return Err(protocol());
                }
                self.usage(response, facts)?;
                let reason = if kind == "response.incomplete" {
                    match response
                        .pointer("/incomplete_details/reason")
                        .and_then(Value::as_str)
                    {
                        Some("max_output_tokens") => FinishReason::Length,
                        Some("content_filter") => FinishReason::ContentFilter,
                        _ => FinishReason::Other,
                    }
                } else if self.items.values().any(|item| item.call.is_some()) {
                    FinishReason::ToolCall
                } else {
                    FinishReason::Stop
                };
                self.completed = true;
                facts.push(GatewayEvent::Completed(
                    ResponseMeta::new(
                        self.response_id.as_deref().ok_or_else(protocol)?,
                        &self.model,
                    )
                    .with_finish_reason(reason),
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

fn protocol() -> ProviderError {
    error(ProviderErrorKind::Protocol, UpstreamSendState::Sent)
}

fn optional_text<'a>(value: &'a Value, field: &str) -> Result<Option<&'a str>, ProviderError> {
    value
        .get(field)
        .map(|value| {
            value
                .as_str()
                .filter(|text| !text.is_empty())
                .ok_or_else(protocol)
        })
        .transpose()
}

fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, ProviderError> {
    optional_text(value, field)?.ok_or_else(protocol)
}

fn optional_count(value: &Value, field: &str) -> Result<Option<u64>, ProviderError> {
    value
        .get(field)
        .filter(|value| !value.is_null())
        .map(|value| value.as_u64().ok_or_else(protocol))
        .transpose()
}

fn count(value: &Value, field: &str) -> Result<u64, ProviderError> {
    optional_count(value, field)?.ok_or_else(protocol)
}

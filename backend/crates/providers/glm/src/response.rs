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
use gateway_protocol::openai::sse::SseEventDecoder;
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) fn error(kind: ProviderErrorKind, state: UpstreamSendState) -> ProviderError {
    ProviderError::new(kind, state)
}

/// 明确的业务拒绝允许 Core 在交付前按统一预算恢复，网络结果不明时不作重放证明
pub(crate) fn upstream_error(status: u16, value: &Value) -> ProviderError {
    let code = value
        .get("error")
        .unwrap_or(value)
        .get("code")
        .and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .unwrap_or_default();
    let kind = match code.as_str() {
        "1113" | "1308" | "1309" | "1310" | "1314" | "1316" | "1317" | "1318" | "1319" | "1320"
        | "1321" => ProviderErrorKind::QuotaExhausted,
        "1000" | "1001" | "1003" => ProviderErrorKind::Unauthorized,
        "1220" | "1311" | "1315" => ProviderErrorKind::PermissionDenied,
        "1302" | "1313" => ProviderErrorKind::RateLimited,
        "1305" => ProviderErrorKind::UpstreamCapacityUnavailable,
        _ => match status {
            401 => ProviderErrorKind::Unauthorized,
            403 => ProviderErrorKind::PermissionDenied,
            429 => ProviderErrorKind::RateLimited,
            500..=599 => ProviderErrorKind::Unavailable,
            400..=499 => ProviderErrorKind::InvalidRequest,
            _ => ProviderErrorKind::Protocol,
        },
    };
    // 仅公开受控业务码，防止上游错误正文回显认证材料
    let public_code =
        if !code.is_empty() && code.len() <= 8 && code.bytes().all(|b| b.is_ascii_digit()) {
            code
        } else {
            "glm_upstream_error".to_owned()
        };
    let mut failure = error(kind, UpstreamSendState::Sent)
        .with_status(status)
        .with_upstream_code(OpaqueUpstreamValue::new(public_code.clone()))
        .with_client_visible_upstream_error(ClientVisibleUpstreamError::new(
            "智谱上游请求失败",
            Some(public_code),
            None,
        ));
    if matches!(
        kind,
        ProviderErrorKind::RateLimited
            | ProviderErrorKind::QuotaExhausted
            | ProviderErrorKind::UpstreamCapacityUnavailable
            | ProviderErrorKind::Unavailable
    ) {
        failure = failure.with_replay_safe();
    }
    failure
}

pub(crate) struct Decoder {
    sse: SseEventDecoder,
    model: String,
    response_id: Option<String>,
    contents: BTreeMap<(u64, u8), u32>,
    tools: BTreeMap<u64, (String, String)>,
    pricing: Option<gateway_core::metering::ModelPriceOverride>,
    pub completed: bool,
    pub semantic_output: bool,
}

impl Decoder {
    pub fn new(model: String, pricing: Option<gateway_core::metering::ModelPriceOverride>) -> Self {
        Self {
            sse: SseEventDecoder::default(),
            model,
            pricing,
            response_id: None,
            contents: BTreeMap::new(),
            tools: BTreeMap::new(),
            completed: false,
            semantic_output: false,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<ProviderEvent, ProviderError>> {
        let frames = self.sse.push_frames(bytes);
        self.frames(frames)
    }
    pub fn finish(&mut self) -> Vec<Result<ProviderEvent, ProviderError>> {
        let frames = self.sse.finish_frames();
        let mut events = self.frames(frames);
        if !self.completed && events.last().is_none_or(Result::is_ok) {
            events.push(Err(error(
                ProviderErrorKind::Protocol,
                UpstreamSendState::Sent,
            )));
        }
        events
    }
    fn frames(
        &mut self,
        frames: Vec<gateway_protocol::openai::sse::SseFrame>,
    ) -> Vec<Result<ProviderEvent, ProviderError>> {
        let mut output = vec![];
        for frame in frames {
            match self.frame(frame) {
                Ok(Some(event)) => output.push(Ok(event)),
                Ok(None) => {}
                Err(failure) => {
                    output.push(Err(if self.semantic_output {
                        failure.with_retry_prohibited()
                    } else {
                        failure
                    }));
                    break;
                }
            }
        }
        output
    }
    fn frame(
        &mut self,
        frame: gateway_protocol::openai::sse::SseFrame,
    ) -> Result<Option<ProviderEvent>, ProviderError> {
        let (raw, events) = frame.into_parts();
        if events.len() > 1 {
            return Err(error(ProviderErrorKind::Protocol, UpstreamSendState::Sent));
        }
        let Some(event) = events.into_iter().next() else {
            return if self.completed {
                Ok(None)
            } else {
                Ok(Some(ProviderEvent::wire(
                    ProtocolWireEvent::raw_sse("openai", Bytes::from(raw))
                        .map_err(|_| error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?,
                )))
            };
        };
        if event.data == "[DONE]" {
            return Ok(None);
        }
        if self.completed {
            return Err(error(ProviderErrorKind::Protocol, UpstreamSendState::Sent));
        }
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|_| error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .or(event.event.as_deref())
            .unwrap_or("")
            .to_owned();
        let facts = self.facts(&kind, &value)?;
        let wire = ProtocolWireEvent::json_with_raw_sse_metadata(
            "openai",
            Some(kind),
            value,
            Bytes::from(raw),
            event.id,
            event.retry,
        )
        .map_err(|_| error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
        Ok(Some(ProviderEvent::canonical_with_wire(facts, wire)))
    }
    fn content(
        &mut self,
        index: u64,
        kind: u8,
        facts: &mut Vec<GatewayEvent>,
    ) -> Result<u32, ProviderError> {
        if let Some(index) = self.contents.get(&(index, kind)) {
            return Ok(*index);
        }
        let id = u32::try_from(self.contents.len())
            .ok()
            .filter(|id| *id < 4096)
            .ok_or_else(|| error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
        self.contents.insert((index, kind), id);
        facts.push(GatewayEvent::ContentAdded(ContentItem::new(
            id,
            match kind {
                0 => ContentKind::Text,
                1 => ContentKind::Reasoning,
                _ => ContentKind::ToolCall,
            },
        )));
        Ok(id)
    }
    fn facts(&mut self, kind: &str, value: &Value) -> Result<Vec<GatewayEvent>, ProviderError> {
        if matches!(kind, "response.failed" | "error") {
            let body = if kind == "response.failed" {
                value.get("response").unwrap_or(value)
            } else {
                value
            };
            let mut failure = upstream_error(200, body);
            if self.semantic_output {
                failure = failure.with_retry_prohibited();
            }
            return Err(failure);
        }
        let mut facts = vec![];
        if self.response_id.is_none() {
            let id = value
                .pointer("/response/id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
            self.response_id = Some(id.to_owned());
            facts.push(GatewayEvent::Started(ResponseMeta::new(id, &self.model)));
        }
        let output_index = value
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        match kind {
            "response.output_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_text.delta" => {
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(|| error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
                if !delta.is_empty() {
                    self.semantic_output = true;
                    let reasoning = kind != "response.output_text.delta";
                    let index = self.content(output_index, u8::from(reasoning), &mut facts)?;
                    facts.push(if reasoning {
                        GatewayEvent::ReasoningDelta(ReasoningDelta {
                            content_index: index,
                            text: delta.to_owned(),
                        })
                    } else {
                        GatewayEvent::TextDelta(TextDelta {
                            content_index: index,
                            text: delta.to_owned(),
                        })
                    });
                }
            }
            "response.output_item.added" => {
                if let Some(item) = value.get("item")
                    && matches!(
                        item.get("type").and_then(Value::as_str),
                        Some("function_call" | "custom_tool_call")
                    )
                {
                    let call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?
                        .to_owned();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?
                        .to_owned();
                    let index = self.content(output_index, 2, &mut facts)?;
                    self.tools
                        .insert(output_index, (call_id.clone(), name.clone()));
                    self.semantic_output = true;
                    facts.push(GatewayEvent::ToolCallDelta(ToolCallDelta {
                        content_index: index,
                        call_id,
                        name: Some(name),
                        arguments_delta: String::new(),
                    }));
                }
            }
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                let (call_id, _) =
                    self.tools.get(&output_index).cloned().ok_or_else(|| {
                        error(ProviderErrorKind::Protocol, UpstreamSendState::Sent)
                    })?;
                let arguments = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(|| error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?
                    .to_owned();
                let index = self.content(output_index, 2, &mut facts)?;
                self.semantic_output = true;
                facts.push(GatewayEvent::ToolCallDelta(ToolCallDelta {
                    content_index: index,
                    call_id,
                    name: None,
                    arguments_delta: arguments,
                }));
            }
            "response.completed" | "response.incomplete" => {
                let response = value
                    .get("response")
                    .ok_or_else(|| error(ProviderErrorKind::Protocol, UpstreamSendState::Sent))?;
                let expected_status = if kind == "response.completed" {
                    "completed"
                } else {
                    "incomplete"
                };
                if response.get("status").and_then(Value::as_str) != Some(expected_status) {
                    return Err(error(ProviderErrorKind::Protocol, UpstreamSendState::Sent));
                }
                if response.get("id").and_then(Value::as_str) != self.response_id.as_deref() {
                    return Err(error(ProviderErrorKind::Protocol, UpstreamSendState::Sent));
                }
                if kind == "response.completed"
                    && !self.semantic_output
                    && response
                        .get("output")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty)
                {
                    return Err(error(ProviderErrorKind::Protocol, UpstreamSendState::Sent));
                }
                if let Some(usage) = response.get("usage").filter(|v| v.is_object()) {
                    let mut measured = Usage::new();
                    measured.input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
                    measured.output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
                    measured.cached_input_tokens = usage
                        .pointer("/input_tokens_details/cached_tokens")
                        .and_then(Value::as_u64);
                    measured.reasoning_output_tokens = usage
                        .pointer("/output_tokens_details/reasoning_tokens")
                        .and_then(Value::as_u64);
                    measured.total_tokens = usage.get("total_tokens").and_then(Value::as_u64);
                    if let Some(breakdown) =
                        crate::pricing::breakdown(&measured, self.pricing.as_ref())
                    {
                        facts.push(GatewayEvent::CalculatedCost(breakdown.calculated_cost()));
                    }
                    facts.push(GatewayEvent::Usage(measured));
                }
                self.completed = true;
                let reason = if kind == "response.incomplete" {
                    match response
                        .pointer("/incomplete_details/reason")
                        .and_then(Value::as_str)
                    {
                        Some("max_output_tokens") => FinishReason::Length,
                        Some("content_filter") => FinishReason::ContentFilter,
                        _ => FinishReason::Other,
                    }
                } else if self.tools.is_empty() {
                    FinishReason::Stop
                } else {
                    FinishReason::ToolCall
                };
                facts.push(GatewayEvent::Completed(
                    ResponseMeta::new(self.response_id.as_deref().unwrap_or_default(), &self.model)
                        .with_finish_reason(reason),
                ));
            }
            _ => {}
        }
        Ok(facts)
    }
}

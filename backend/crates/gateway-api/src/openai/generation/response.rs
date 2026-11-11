//! 原生协议出口保留 wire，只在非流式交付时聚合消息正文

use axum::body::Bytes;
use gateway_core::{error::GatewayError, event::ProviderEvent};
use gateway_protocol::{
    anthropic::MessagesAccumulator,
    openai::sse::{
        DONE_SSE_FRAME, encode_sse_event_with_metadata, response_failed_sse_event_with_id,
    },
};
use serde_json::{Value, json};

use super::GenerationProtocol;
use crate::openai::{
    error::gateway_error_contract,
    messages,
    response_model::ClientResponseModel,
    responses::{OpenAiResponsesEncoder, ResponseEncodeError},
};

pub(super) enum GenerationEncoder {
    Responses(OpenAiResponsesEncoder),
    Messages {
        requested_model: Option<ClientResponseModel>,
        completed: bool,
        failed: bool,
    },
}

impl GenerationEncoder {
    pub(super) fn new(protocol: GenerationProtocol) -> Self {
        match protocol {
            GenerationProtocol::Responses => Self::Responses(OpenAiResponsesEncoder::new()),
            GenerationProtocol::Messages | GenerationProtocol::MessageTokenCount => {
                Self::Messages {
                    requested_model: None,
                    completed: false,
                    failed: false,
                }
            }
        }
    }

    pub(super) fn set_requested_model(&mut self, model: String) {
        match self {
            Self::Responses(encoder) => {
                *encoder = std::mem::take(encoder).with_requested_model(model)
            }
            Self::Messages {
                requested_model, ..
            } => *requested_model = Some(ClientResponseModel::new(model)),
        }
    }

    pub(super) fn push_sse(&mut self, event: &ProviderEvent) -> Vec<Bytes> {
        match self {
            Self::Responses(encoder) => encoder.push_sse(event),
            Self::Messages {
                requested_model,
                completed,
                failed,
            } => {
                let Some(wire) = event
                    .wire_event()
                    .filter(|wire| wire.protocol() == "anthropic")
                else {
                    return Vec::new();
                };
                if let Some(raw) = wire.raw_json_body() {
                    let events = serde_json::from_slice(raw).ok().and_then(|mut value| {
                        if let Some(model) = requested_model.as_ref() {
                            model.project_message(&mut value);
                        }
                        gateway_protocol::anthropic::message_to_stream_events(value).ok()
                    });
                    let Some(events) = events else {
                        *failed = true;
                        return vec![messages::stream_error(
                            &json!({"error":{"message":"The upstream message is invalid."}}),
                        )];
                    };
                    *completed = true;
                    return events
                        .into_iter()
                        .map(|value| {
                            Bytes::from(encode_sse_event_with_metadata(
                                value["type"].as_str().unwrap_or_default(),
                                &value.to_string(),
                                None,
                                None,
                            ))
                        })
                        .collect();
                }
                let kind = wire
                    .event_type()
                    .or_else(|| wire.data().get("type").and_then(Value::as_str));
                match kind {
                    Some("message_stop") => *completed = true,
                    Some("error") => *failed = true,
                    _ => {}
                }
                if let Some(raw) = wire.raw_sse_frame() {
                    return vec![
                        requested_model
                            .as_ref()
                            .and_then(|model| {
                                model.project_message_sse(raw, kind.unwrap_or_default())
                            })
                            .unwrap_or_else(|| raw.clone()),
                    ];
                }
                let data = wire.data().to_string();
                let data = requested_model
                    .as_ref()
                    .and_then(|model| model.project_message_json(&data, kind.unwrap_or_default()))
                    .unwrap_or(data);
                vec![Bytes::from(encode_sse_event_with_metadata(
                    kind.unwrap_or_default(),
                    &data,
                    wire.sse_id(),
                    wire.sse_retry(),
                ))]
            }
        }
    }

    pub(super) fn is_completed(&self) -> bool {
        match self {
            Self::Responses(encoder) => encoder.is_completed(),
            Self::Messages { completed, .. } => *completed,
        }
    }

    pub(super) fn has_wire_failure(&self) -> bool {
        match self {
            Self::Responses(encoder) => encoder.has_wire_failure(),
            Self::Messages { failed, .. } => *failed,
        }
    }

    pub(super) fn error_frame(&self, error: &GatewayError) -> Bytes {
        let (status, error_type, code) = gateway_error_contract(error.kind());
        match self {
            Self::Responses(encoder) => Bytes::from(response_failed_sse_event_with_id(
                encoder.response_id(),
                error.client_error_type().unwrap_or(error_type),
                crate::client_error::client_error_code(error.client_error_code().unwrap_or(code)),
                error.client_message(),
            )),
            Self::Messages { .. } => {
                let value = messages::error_payload(
                    &json!({"error":{"message":error.client_message(), "code":crate::client_error::client_error_code(error.client_error_code().unwrap_or(code))}}),
                    status,
                );
                messages::stream_error(&value)
            }
        }
    }
}

impl GenerationProtocol {
    pub(super) fn terminal_bytes(self) -> Bytes {
        match self {
            Self::Responses => Bytes::from_static(DONE_SSE_FRAME.as_bytes()),
            // Messages 的 message_stop 已在 wire 中交付，空帧只确认宿主交付终结
            Self::Messages | Self::MessageTokenCount => Bytes::new(),
        }
    }

    pub(super) fn collect(
        self,
        events: &[ProviderEvent],
        client_model: Option<&ClientResponseModel>,
    ) -> Result<Vec<u8>, ResponseEncodeError> {
        let mut response = if self == Self::Responses {
            crate::openai::responses::response::collect_response(events)?
        } else {
            let mut accumulator = MessagesAccumulator::default();
            let mut response = None;
            for wire in events
                .iter()
                .filter_map(ProviderEvent::wire_event)
                .filter(|wire| wire.protocol() == self.wire())
            {
                if let Some(raw) = wire.raw_json_body() {
                    if self != Self::MessageTokenCount
                        && let Some(model) = client_model
                        && let Ok(json) = std::str::from_utf8(raw)
                        && let Some(projected) = model.project_message_json(json, "message")
                    {
                        return Ok(projected.into_bytes());
                    }
                    return Ok(raw.to_vec());
                }
                let value = wire.data();
                if self == Self::MessageTokenCount {
                    if value.get("input_tokens").and_then(Value::as_u64).is_some() {
                        response = Some(value.clone());
                    }
                } else if value.get("type").and_then(Value::as_str) == Some("message") {
                    response = Some(value.clone());
                } else if wire.has_json_data()
                    && let Some(message) = accumulator
                        .push(value)
                        .map_err(|_| ResponseEncodeError::InvalidOutputItems)?
                {
                    response = Some(message);
                }
            }
            response.ok_or(ResponseEncodeError::MissingWireTerminal)?
        };
        if let Some(model) = client_model {
            match self {
                Self::Responses => model.project_response(&mut response),
                Self::Messages => model.project_message(&mut response),
                Self::MessageTokenCount => {}
            }
        }
        serde_json::to_vec(&response).map_err(|_| ResponseEncodeError::Serialization)
    }
}

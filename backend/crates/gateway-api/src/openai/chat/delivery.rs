//! Chat 编码包装保留原执行守卫的提交、取消和终结责任

use std::collections::VecDeque;

use bytes::Bytes;
use futures::future::BoxFuture;
use gateway_core::{
    engine::middleware::{
        MiddlewareBody, MiddlewareError, MiddlewareFrame, MiddlewareFraming, MiddlewareResponse,
    },
    error::{GatewayError, GatewayErrorKind},
};
use gateway_protocol::openai::{
    chat::{ChatStreamEncoder, chat_completion},
    sse::{DONE_SSE_FRAME, SseEventDecoder},
};
use serde_json::{Value, json};

use super::validation::ChatValidationFacts;

pub(super) async fn stream_failure(response: axum::response::Response) -> axum::body::Body {
    let retry = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let mut value = serde_json::from_slice::<Value>(&bytes).ok().filter(|v| v.get("error").is_some_and(Value::is_object))
        .unwrap_or_else(|| json!({"error": {"type": "server_error", "code": "upstream_error", "message": "Response stream failed."}}));
    if let Some(retry) = retry {
        value["error"]["headers"] = json!({"retry-after": retry});
    }
    axum::body::Body::from(format!("data: {value}\n\n{DONE_SSE_FRAME}"))
}

pub(super) fn chat_response(
    response: MiddlewareResponse,
    model: String,
    stream: bool,
    include_usage: bool,
    facts: ChatValidationFacts,
) -> MiddlewareResponse {
    let (protocol, status, mut headers, body, envelope) = response.into_parts();
    if (200..300).contains(&status) {
        headers.retain(|header| !header.name().eq_ignore_ascii_case("content-length"));
    }
    let mut response = MiddlewareResponse::new(
        protocol,
        status,
        headers,
        Box::new(ChatBody {
            facts,
            inner: body,
            model: model.clone(),
            stream,
            success: (200..300).contains(&status),
            encoder: ChatStreamEncoder::new(
                model,
                chrono::Utc::now().timestamp().max(0) as u64,
                include_usage,
            ),
            decoder: SseEventDecoder::default(),
            pending: VecDeque::new(),
            committed: false,
            done: false,
        }),
    );
    if let Some(envelope) = envelope {
        response = response.with_envelope(envelope);
    }
    response
}

struct ChatBody {
    facts: ChatValidationFacts,
    inner: Box<dyn MiddlewareBody>,
    model: String,
    stream: bool,
    success: bool,
    encoder: ChatStreamEncoder,
    decoder: SseEventDecoder,
    pending: VecDeque<MiddlewareFrame>,
    committed: bool,
    done: bool,
}

impl ChatBody {
    async fn fill(&mut self) -> Result<(), MiddlewareError> {
        while self.pending.is_empty() && !self.done {
            let Some(frame) = self.inner.next_frame().await? else {
                return Err(invalid_response());
            };
            if !self.stream {
                let value: Value =
                    serde_json::from_slice(frame.bytes()).map_err(|_| invalid_response())?;
                let value = chat_completion(&value, &self.model).map_err(|_| invalid_response())?;
                self.facts.observe(&value);
                self.pending.push_back(
                    MiddlewareFrame::new(
                        Bytes::from(value.to_string()),
                        MiddlewareFraming::JsonDocument,
                        true,
                    )
                    .with_transformed(true),
                );
                self.done = true;
                return Ok(());
            }
            let events = self
                .decoder
                .push(frame.bytes())
                .map_err(|_| invalid_response())?;
            for event in events {
                if self.done {
                    return Err(invalid_response());
                }
                if event.data == "[DONE]" {
                    if !self.encoder.is_terminal() {
                        return Err(invalid_response());
                    }
                    self.pending.push_back(done_frame());
                    self.done = true;
                    continue;
                }
                let value: Value =
                    serde_json::from_str(&event.data).map_err(|_| invalid_response())?;
                let kind = event
                    .event
                    .as_deref()
                    .or_else(|| value.get("type").and_then(Value::as_str))
                    .ok_or_else(invalid_response)?;
                let chunks = self
                    .encoder
                    .push(kind, &value)
                    .map_err(|_| invalid_response())?;
                for chunk in &chunks {
                    self.facts.observe(chunk);
                }
                self.pending
                    .extend(chunks.into_iter().map(|chunk| sse_frame(&chunk)));
            }
        }
        Ok(())
    }
}

impl MiddlewareBody for ChatBody {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<MiddlewareFrame>, MiddlewareError>> {
        Box::pin(async move {
            if !self.success {
                return self.inner.next_frame().await;
            }
            if let Some(frame) = self.pending.pop_front() {
                return Ok(Some(frame));
            }
            if self.done {
                return if self.stream {
                    Ok(None)
                } else {
                    self.inner.next_frame().await.and_then(|frame| {
                        if frame.is_none() {
                            Ok(None)
                        } else {
                            Err(invalid_response())
                        }
                    })
                };
            }
            if let Err(error) = self.fill().await {
                if !self.committed {
                    return Err(error);
                }
                self.pending.clear();
                let failure = json!({"error": {"type": "server_error", "code": "chat_conversion_failed", "message": "Response stream could not be converted to Chat Completions."}});
                self.facts.observe(&failure);
                self.pending.push_back(sse_frame(&failure));
                self.pending.push_back(done_frame());
                self.done = true;
            }
            Ok(self.pending.pop_front())
        })
    }

    fn commit_downstream(
        &mut self,
        status: Option<u16>,
    ) -> BoxFuture<'_, Result<(), MiddlewareError>> {
        Box::pin(async move {
            self.inner.commit_downstream(status).await?;
            self.committed = true;
            Ok(())
        })
    }
    fn record_client_status(&mut self, status: u16) -> BoxFuture<'_, Result<(), MiddlewareError>> {
        self.inner.record_client_status(status)
    }
    fn is_finalized(&self) -> bool {
        self.inner.is_finalized()
    }
    fn close(self: Box<Self>) -> BoxFuture<'static, ()> {
        self.inner.close()
    }
}

fn sse_frame(value: &Value) -> MiddlewareFrame {
    MiddlewareFrame::new(
        Bytes::from(format!("data: {value}\n\n")),
        MiddlewareFraming::SseEvent,
        false,
    )
    .with_transformed(true)
}

fn done_frame() -> MiddlewareFrame {
    MiddlewareFrame::new(
        Bytes::from_static(DONE_SSE_FRAME.as_bytes()),
        MiddlewareFraming::SseEvent,
        true,
    )
    .with_transformed(true)
}

fn invalid_response() -> MiddlewareError {
    MiddlewareError::Gateway(GatewayError::new(
        GatewayErrorKind::UpstreamUnavailable,
        "Responses output cannot be delivered as Chat Completions.",
    ))
}

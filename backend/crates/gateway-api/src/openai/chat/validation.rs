//! 中间件改写后的 Chat 身份与终态复核，不重新解释上游业务事实

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use bytes::Bytes;
use futures::future::BoxFuture;
use gateway_core::engine::middleware::{
    MiddlewareBody, MiddlewareError, MiddlewareFrame, MiddlewareFraming, MiddlewareResponse,
};
use gateway_protocol::openai::sse::{DONE_SSE_FRAME, SseEventDecoder};
use serde_json::{Value, json};

#[derive(Default)]
struct Expected {
    id: Option<String>,
    finish: Option<Value>,
    failed: bool,
}

#[derive(Clone, Default)]
pub(super) struct ChatValidationFacts(Arc<Mutex<Expected>>);

impl ChatValidationFacts {
    pub(super) fn observe(&self, value: &Value) {
        let mut expected = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            expected.id = Some(id.to_owned());
        }
        if let Some(reason) = value
            .pointer("/choices/0/finish_reason")
            .filter(|v| !v.is_null())
        {
            expected.finish = Some(reason.clone());
        }
        expected.failed |= value.get("error").is_some();
    }
}

pub(super) fn validate_response(
    response: MiddlewareResponse,
    facts: ChatValidationFacts,
    stream: bool,
) -> MiddlewareResponse {
    let (protocol, status, headers, body, envelope) = response.into_parts();
    let mut response = MiddlewareResponse::new(
        protocol,
        status,
        headers,
        Box::new(ValidatedBody {
            inner: body,
            facts,
            stream,
            success: (200..300).contains(&status),
            decoder: SseEventDecoder::default(),
            id: None,
            terminal: false,
            done: false,
            committed: false,
            pending: VecDeque::new(),
        }),
    );
    if let Some(envelope) = envelope {
        response = response.with_envelope(envelope);
    }
    response
}

struct ValidatedBody {
    inner: Box<dyn MiddlewareBody>,
    facts: ChatValidationFacts,
    stream: bool,
    success: bool,
    decoder: SseEventDecoder,
    id: Option<String>,
    terminal: bool,
    done: bool,
    committed: bool,
    pending: VecDeque<MiddlewareFrame>,
}

impl ValidatedBody {
    fn validate(&mut self, value: &Value) -> Result<(), MiddlewareError> {
        let expected = self
            .facts
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if value.get("error").is_some_and(Value::is_object) {
            if !self.stream || self.terminal || expected.finish.is_some() {
                return Err(MiddlewareError::InvalidState);
            }
            self.terminal = true;
            return Ok(());
        }
        let object = if self.stream {
            "chat.completion.chunk"
        } else {
            "chat.completion"
        };
        if value.get("object").and_then(Value::as_str) != Some(object) {
            return Err(MiddlewareError::InvalidState);
        }
        if value.get("created").and_then(Value::as_u64).is_none()
            || !value
                .get("model")
                .and_then(Value::as_str)
                .is_some_and(|model| !model.is_empty())
        {
            return Err(MiddlewareError::InvalidState);
        }
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or(MiddlewareError::InvalidState)?;
        if self.id.as_deref().is_some_and(|previous| previous != id)
            || expected
                .id
                .as_deref()
                .is_some_and(|original| original != id)
        {
            return Err(MiddlewareError::InvalidState);
        }
        self.id = Some(id.to_owned());
        let choices = value
            .get("choices")
            .and_then(Value::as_array)
            .ok_or(MiddlewareError::InvalidState)?;
        if self.stream && choices.is_empty() {
            return if self.terminal && value.get("usage").is_some_and(Value::is_object) {
                Ok(())
            } else {
                Err(MiddlewareError::InvalidState)
            };
        }
        if self.terminal
            || choices.len() != 1
            || choices[0].get("index").and_then(Value::as_u64) != Some(0)
        {
            return Err(MiddlewareError::InvalidState);
        }
        if self.stream {
            if !choices[0].get("delta").is_some_and(Value::is_object) {
                return Err(MiddlewareError::InvalidState);
            }
        } else if choices[0].pointer("/message/role").and_then(Value::as_str) != Some("assistant") {
            return Err(MiddlewareError::InvalidState);
        }
        let finish = choices[0]
            .get("finish_reason")
            .ok_or(MiddlewareError::InvalidState)?;
        if !finish.is_null() {
            if !matches!(
                finish.as_str(),
                Some("stop" | "length" | "tool_calls" | "content_filter")
            ) || expected.failed
                || expected
                    .finish
                    .as_ref()
                    .is_some_and(|original| original != finish)
            {
                return Err(MiddlewareError::InvalidState);
            }
            self.terminal = true;
        } else if !self.stream {
            return Err(MiddlewareError::InvalidState);
        }
        Ok(())
    }

    fn check_frame(&mut self, frame: &MiddlewareFrame) -> Result<(), MiddlewareError> {
        if !self.stream {
            let value =
                serde_json::from_slice(frame.bytes()).map_err(|_| MiddlewareError::InvalidState)?;
            return self.validate(&value);
        }
        for event in self
            .decoder
            .push(frame.bytes())
            .map_err(|_| MiddlewareError::InvalidState)?
        {
            if self.done {
                return Err(MiddlewareError::InvalidState);
            }
            if event.data == "[DONE]" {
                if !self.terminal {
                    return Err(MiddlewareError::InvalidState);
                }
                self.done = true;
            } else {
                let value =
                    serde_json::from_str(&event.data).map_err(|_| MiddlewareError::InvalidState)?;
                self.validate(&value)?;
            }
        }
        if frame.terminal() && !self.done {
            return Err(MiddlewareError::InvalidState);
        }
        Ok(())
    }
}

impl MiddlewareBody for ValidatedBody {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<MiddlewareFrame>, MiddlewareError>> {
        Box::pin(async move {
            if let Some(frame) = self.pending.pop_front() {
                return Ok(Some(frame));
            }
            if !self.success && self.done {
                return Ok(None);
            }
            let frame = self.inner.next_frame().await;
            if !self.success {
                return frame;
            }
            let valid = match &frame {
                Ok(Some(frame)) => self.check_frame(frame),
                Ok(None) if self.terminal && (!self.stream || self.done) => Ok(()),
                _ => Err(MiddlewareError::InvalidState),
            };
            if valid.is_err() {
                if !self.stream || !self.committed {
                    return Err(MiddlewareError::InvalidState);
                }
                self.pending.push_back(MiddlewareFrame::new(
                    Bytes::from_static(DONE_SSE_FRAME.as_bytes()),
                    MiddlewareFraming::SseEvent,
                    true,
                ));
                self.success = false;
                self.done = true;
                return Ok(Some(MiddlewareFrame::new(
                    Bytes::from(format!(
                        "data: {}\n\n",
                        json!({"error":{"type":"server_error","code":"chat_delivery_invalid","message":"Middleware returned an invalid Chat Completions response."}})
                    )),
                    MiddlewareFraming::SseEvent,
                    false,
                )));
            }
            frame
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

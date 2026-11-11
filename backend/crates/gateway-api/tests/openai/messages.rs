//! Messages 原生交付、错误格式、计数与公共生命周期的接口合同

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use bytes::Bytes;
use futures::{StreamExt as _, future::BoxFuture};
use gateway_core::{
    engine::ModelRequestId,
    engine::execution::{
        AuthenticatedClient, ClientAuthenticationError, ExecutionService, StartExecution,
        StartProviderExecution, StartedExecution,
    },
    error::GatewayError,
    event::{ProtocolWireEvent, ProviderEvent},
    operation::Operation,
    routing::PublicModelId,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, atomic::Ordering};
use tower::ServiceExt as _;

use super::{api_router, authenticated_client};
use gateway_core::engine::execution::{ExecutionSession, ResponseCommit};
use gateway_core::engine::{CommitRequirement, CoordinatedEvent, EngineError};
use gateway_core::event::ProviderResponseHeader;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::{collections::VecDeque, time::Duration};

#[derive(Default)]
struct Trace {
    pub commits: AtomicUsize,
    pub discarded: AtomicUsize,
    pub cancelled: AtomicBool,
    pub finalized: AtomicBool,
    pub detached: AtomicUsize,
    pub response_commit: ResponseCommit,
}

#[derive(Default)]
enum Finish {
    #[default]
    Success,
    Error,
    Pending,
}

struct Session {
    events: VecDeque<Vec<ProviderEvent>>,
    pub trace: Arc<Trace>,
    pub finish: Finish,
    pub delay: Option<Duration>,
    pub fail_commit: bool,
    pub response_headers: Vec<ProviderResponseHeader>,
    committed: bool,
    buffered: bool,
}

impl Session {
    pub fn new(events: Vec<ProviderEvent>) -> Self {
        Self {
            events: events.into_iter().map(|event| vec![event]).collect(),
            trace: Arc::default(),
            finish: Finish::Success,
            delay: None,
            fail_commit: false,
            response_headers: Vec::new(),
            committed: false,
            buffered: false,
        }
    }
}

impl ExecutionSession for Session {
    fn response_commit(&self) -> ResponseCommit {
        self.trace.response_commit.clone()
    }

    fn next_event(&mut self) -> BoxFuture<'_, Result<Option<CoordinatedEvent>, EngineError>> {
        Box::pin(async move {
            if let Some(delay) = self.delay.take() {
                tokio::time::sleep(delay).await;
            }
            if let Some(event) = self.events.pop_front() {
                let requirement = if self.committed {
                    CommitRequirement::AlreadyCommitted
                } else {
                    CommitRequirement::CommitBeforeDelivery
                };
                return CoordinatedEvent::try_batch(event, requirement).map(Some);
            }
            match self.finish {
                Finish::Success => {
                    self.trace.finalized.store(true, Ordering::SeqCst);
                    Ok(None)
                }
                Finish::Error => {
                    self.trace.finalized.store(true, Ordering::SeqCst);
                    Err(EngineError::ProviderMetadataMismatch)
                }
                Finish::Pending => futures::future::pending().await,
            }
        })
    }

    fn collect_uncommitted(&mut self) -> BoxFuture<'_, Result<Vec<ProviderEvent>, EngineError>> {
        self.buffered = true;
        Box::pin(async move { Ok(self.events.drain(..).flatten().collect()) })
    }

    fn response_headers(&self) -> &[ProviderResponseHeader] {
        &self.response_headers
    }

    fn discard_pending_delivery(&mut self) -> Result<(), EngineError> {
        self.trace.discarded.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn commit_downstream(&mut self, status: Option<u16>) -> BoxFuture<'_, Result<(), EngineError>> {
        Box::pin(async move {
            self.trace.commits.fetch_add(1, Ordering::SeqCst);
            if self.fail_commit {
                return Err(EngineError::ProviderMetadataMismatch);
            }
            self.trace.response_commit.record(status);
            self.committed = true;
            if self.buffered {
                self.trace.finalized.store(true, Ordering::SeqCst);
            }
            Ok(())
        })
    }

    fn record_client_status(&mut self, status: u16) -> BoxFuture<'_, Result<(), EngineError>> {
        Box::pin(async move {
            self.trace.response_commit.record(Some(status));
            Ok(())
        })
    }

    fn is_finalized(&self) -> bool {
        self.trace.finalized.load(Ordering::SeqCst)
    }
    fn cancel(&self) {
        self.trace.cancelled.store(true, Ordering::SeqCst);
    }
    fn detach_finalize(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            self.trace.detached.fetch_add(1, Ordering::SeqCst);
            self.trace.finalized.store(true, Ordering::SeqCst);
        })
    }
}

struct Execution {
    middleware: Option<gateway_core::engine::middleware::FrozenMiddlewarePlan>,
    session: Mutex<Option<Session>>,
    captured: Mutex<Option<Operation>>,
}

impl ExecutionService for Execution {
    fn middleware_plan(
        &self,
        _: &gateway_core::engine::execution::PreparedRootExecution,
    ) -> Option<gateway_core::engine::middleware::FrozenMiddlewarePlan> {
        self.middleware.clone()
    }
    fn authenticate(&self, key: &str) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        if key == "sk_messages_test" {
            Ok(authenticated_client(key))
        } else {
            Err(ClientAuthenticationError::InvalidKey)
        }
    }
    fn public_models(&self, _: &AuthenticatedClient) -> Vec<PublicModelId> {
        vec![]
    }
    fn contains_public_model(&self, _: &AuthenticatedClient, _: &PublicModelId) -> bool {
        true
    }
    fn start(
        &self,
        request: StartExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move {
            assert_eq!(request.metadata.protocol, "anthropic");
            assert!(request.metadata.endpoint.starts_with("/v1/messages"));
            *self.captured.lock().unwrap() = Some(request.operation);
            Ok(StartedExecution {
                request_id: ModelRequestId::new("req_messages").unwrap(),
                created_at: std::time::SystemTime::now(),
                stream: request.metadata.stream,
                session: Box::new(self.session.lock().unwrap().take().unwrap()),
            })
        })
    }
    fn start_provider_endpoint(
        &self,
        _: StartProviderExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async { panic!("Messages uses model execution") })
    }
}

fn wire(value: Value) -> ProviderEvent {
    ProviderEvent::wire(
        ProtocolWireEvent::json(
            "anthropic",
            value.get("type").and_then(Value::as_str).map(str::to_owned),
            value,
        )
        .unwrap(),
    )
}

fn message() -> Value {
    json!({"id":"msg_native","type":"message","role":"assistant","model":"native-model","content":[{"type":"text","text":"你好"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2},"future_extension":{"preserved":true}})
}

fn stream_events() -> Vec<ProviderEvent> {
    vec![
        wire(
            json!({"type":"message_start","message":{"id":"msg_native","type":"message","role":"assistant","model":"native-model","content":[],"stop_reason":null,"usage":{"input_tokens":3,"output_tokens":0}}}),
        ),
        wire(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ),
        wire(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}),
        ),
        wire(json!({"type":"content_block_stop","index":0})),
        wire(
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}}),
        ),
        wire(json!({"type":"message_stop"})),
    ]
}

fn request(path: &str, stream: bool) -> Request<Body> {
    Request::post(path).header("x-api-key", "sk_messages_test").header("anthropic-version", "2023-06-01").header("content-type", "application/json")
        .body(Body::from(json!({"model":"model-a","max_tokens":0,"stream":stream,"messages":[{"role":"user","content":"你好"}],"future_extension":{"preserved":true}}).to_string())).unwrap()
}

async fn fixture(session: Session) -> (axum::Router, Arc<Execution>) {
    let execution = Arc::new(Execution {
        middleware: None,
        session: Mutex::new(Some(session)),
        captured: Mutex::default(),
    });
    (api_router(execution.clone()).await, execution)
}

#[tokio::test]
async fn native_json_preserves_extensions_and_uses_client_key_authentication() {
    let raw = Bytes::from(format!("  {}\n", message()));
    let session = Session::new(vec![ProviderEvent::wire(
        ProtocolWireEvent::raw_json("anthropic", raw.clone()).unwrap(),
    )]);
    let trace = session.trace.clone();
    let (router, execution) = fixture(session).await;
    let response = router
        .oneshot(request("/v1/messages", false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let mut expected = message();
    expected["model"] = json!("model-a");
    assert_eq!(body, expected);
    let captured = execution.captured.lock().unwrap();
    let Operation::Generate(generation) = captured.as_ref().unwrap() else {
        panic!("generate")
    };
    assert_eq!(generation.protocol_payload().protocol(), "anthropic");
    assert_eq!(
        generation.protocol_payload().body()["future_extension"]["preserved"],
        true
    );
    assert_eq!(generation.max_output_tokens(), Some(0));
    assert_eq!(trace.commits.load(Ordering::SeqCst), 1);
    assert!(trace.finalized.load(Ordering::SeqCst));
}

#[tokio::test]
async fn native_stream_ends_at_message_stop_without_openai_done() {
    let session = Session::new(stream_events());
    let trace = session.trace.clone();
    let (router, _) = fixture(session).await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("你好"));
    assert!(text.ends_with("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
    assert!(!text.contains("[DONE]"));
    assert_eq!(trace.commits.load(Ordering::SeqCst), 1);
    assert!(trace.finalized.load(Ordering::SeqCst));
}

#[tokio::test]
async fn buffered_client_aggregates_native_events_and_cumulative_usage() {
    let (router, _) = fixture(Session::new(stream_events())).await;
    let response = router
        .oneshot(request("/v1/messages", false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["content"][0]["text"], "你好");
    assert_eq!(body["usage"], json!({"input_tokens":3,"output_tokens":2}));
    assert_eq!(body["stop_reason"], "end_turn");
}

#[tokio::test]
async fn count_tokens_ignores_generation_capabilities_without_using_generation_budget() {
    use gateway_core::{
        engine::middleware::MiddlewareCapabilityDeclaration,
        operation::{Feature, OperationKind},
    };

    let (router, execution) = fixture_with_middleware(
        Session::new(vec![wire(json!({"input_tokens":9}))]),
        super::middleware::RequestMiddleware {
            expected_operation: Some(OperationKind::CountTokens),
            request_model: Some("count-model"),
            request_capabilities: Some(MiddlewareCapabilityDeclaration {
                handled: Default::default(),
                required: [Feature::Tools].into(),
            }),
            ..Default::default()
        },
    )
    .await;
    let request = Request::post("/v1/messages/count_tokens")
        .header("x-api-key", "sk_messages_test")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"model":"model-a","messages":[{"role":"user","content":"hello"}]}).to_string(),
        ))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body, json!({"input_tokens":9}));
    let captured = execution.captured.lock().unwrap();
    let operation = captured.as_ref().unwrap();
    assert!(matches!(operation, Operation::CountTokens(_)));
    assert!(operation.capability_requirements().features().is_empty());
}

#[tokio::test]
async fn authentication_errors_use_messages_envelope() {
    let (router, _) = fixture(Session::new(vec![])).await;
    let response = router
        .oneshot(Request::post("/v1/messages").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "authentication_error");
}

#[tokio::test]
async fn stream_failure_and_disconnect_finalize_the_same_execution() {
    let mut session = Session::new(vec![stream_events().remove(0)]);
    session.finish = Finish::Error;
    let trace = session.trace.clone();
    let (router, _) = fixture(session).await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("event: error\n"));
    assert!(!text.contains("[DONE]"));
    assert!(trace.finalized.load(Ordering::SeqCst));

    let mut session = Session::new(vec![stream_events().remove(0)]);
    session.finish = Finish::Pending;
    let trace = session.trace.clone();
    let (router, _) = fixture(session).await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    let mut stream = response.into_body().into_data_stream();
    assert!(
        stream
            .next()
            .await
            .unwrap()
            .unwrap()
            .starts_with(b"event: message_start")
    );
    drop(stream);
    tokio::task::yield_now().await;
    assert!(trace.cancelled.load(Ordering::SeqCst));
    assert!(trace.finalized.load(Ordering::SeqCst));
}

#[tokio::test]
async fn sse_preserves_raw_frames() {
    let raw = Bytes::from_static(b"event: message_start\nid: frame-1\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_native\"},\"future\":true}\n\n");
    let event = ProtocolWireEvent::json_with_raw_sse_metadata(
        "anthropic",
        Some("message_start".to_owned()),
        json!({"type":"message_start","message":{"id":"msg_native"},"future":true}),
        raw.clone(),
        Some("frame-1".to_owned()),
        None,
    )
    .unwrap();
    let (router, _) = fixture(Session::new(vec![
        ProviderEvent::wire(event),
        wire(json!({"type":"message_stop"})),
    ]))
    .await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(bytes.starts_with(&raw));
}

#[tokio::test]
async fn streaming_client_can_receive_a_complete_native_json_response() {
    let event = ProviderEvent::wire(
        ProtocolWireEvent::raw_json("anthropic", message().to_string().into()).unwrap(),
    );
    let (router, _) = fixture(Session::new(vec![event])).await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("event: message_start\n"));
    assert!(text.contains("future_extension"));
    assert!(text.contains("你好"));
    assert!(text.contains("event: message_stop\n"));
}

#[tokio::test]
async fn native_future_non_json_events_remain_opaque_but_missing_stop_is_an_error() {
    let raw = Bytes::from_static(b"event: future_extension\ndata: opaque future payload\n\n");
    let mut events = stream_events();
    events.insert(
        1,
        ProviderEvent::wire(ProtocolWireEvent::raw_sse("anthropic", raw.clone()).unwrap()),
    );
    let (router, _) = fixture(Session::new(events)).await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(bytes.windows(raw.len()).any(|window| window == raw));
    let (router, _) = fixture(Session::new(vec![stream_events().remove(0)])).await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        std::str::from_utf8(&bytes)
            .unwrap()
            .contains("event: error\n")
    );
}

#[tokio::test]
async fn api_key_header_is_not_accepted_by_openai_only_entries() {
    let (router, execution) = fixture(Session::new(vec![])).await;
    for path in [
        "/v1/responses",
        "/v1/chat/completions",
        "/v1/images/generations",
        "/v1/alpha/search",
    ] {
        let response = router.clone().oneshot(request(path, false)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert!(
            body.get("type").is_none(),
            "OpenAI errors keep their original envelope"
        );
    }
    assert!(execution.captured.lock().unwrap().is_none());
}

#[tokio::test]
async fn messages_bearer_takes_precedence_and_does_not_fall_back_to_api_key() {
    let (router, execution) = fixture(Session::new(vec![])).await;
    let mut invalid = request("/v1/messages", false);
    invalid
        .headers_mut()
        .insert("authorization", "Bearer invalid-key".parse().unwrap());
    let response = router.oneshot(invalid).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["error"]["type"], "authentication_error");
    assert!(execution.captured.lock().unwrap().is_none());

    let (router, _) = fixture(Session::new(vec![wire(message())])).await;
    let mut valid = request("/v1/messages", false);
    valid.headers_mut().remove("x-api-key");
    valid
        .headers_mut()
        .insert("authorization", "Bearer sk_messages_test".parse().unwrap());
    assert_eq!(
        router.oneshot(valid).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn invalid_generation_and_count_requests_never_start_execution() {
    for (path, body) in [
        ("/v1/messages", "not-json"),
        ("/v1/messages", r#"{"model":"model-a","messages":[]}"#),
        ("/v1/messages/count_tokens", r#"{"messages":[]}"#),
    ] {
        let (router, execution) = fixture(Session::new(vec![])).await;
        let response = router
            .oneshot(
                Request::post(path)
                    .header("x-api-key", "sk_messages_test")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert!(execution.captured.lock().unwrap().is_none());
    }
}

#[tokio::test]
async fn client_credentials_are_excluded_from_generation_and_count_payload_context() {
    for counted in [false, true] {
        let events = if counted {
            vec![wire(json!({"input_tokens":9}))]
        } else {
            vec![wire(message())]
        };
        let (router, execution) = fixture(Session::new(events)).await;
        let response = router
            .oneshot(request(
                if counted {
                    "/v1/messages/count_tokens"
                } else {
                    "/v1/messages"
                },
                false,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let captured = execution.captured.lock().unwrap();
        let context = match captured.as_ref().unwrap() {
            Operation::Generate(request) => request.protocol_payload().context(),
            Operation::CountTokens(request) => request.payload().context(),
            _ => panic!("model execution"),
        };
        let headers = context
            .get("opaque_request_headers")
            .and_then(Value::as_array)
            .unwrap();
        assert!(
            headers
                .iter()
                .all(|header| !matches!(header[0].as_str(), Some("x-api-key" | "authorization")))
        );
        assert!(
            headers
                .iter()
                .any(|header| header[0] == "anthropic-version")
        );
    }
}

async fn fixture_with_middleware(
    session: Session,
    middleware: super::middleware::RequestMiddleware,
) -> (axum::Router, Arc<Execution>) {
    let middleware = Arc::new(middleware);
    let execution = Arc::new(Execution {
        middleware: Some(middleware.frozen()),
        session: Mutex::new(Some(session)),
        captured: Mutex::default(),
    });
    (api_router(execution.clone()).await, execution)
}

#[tokio::test]
async fn middleware_model_rewrite_keeps_the_entry_model_projection() {
    let (router, execution) = fixture_with_middleware(
        Session::new(vec![wire(message())]),
        super::middleware::RequestMiddleware {
            expected_operation: Some(gateway_core::operation::OperationKind::Generate),
            request_model: Some("rewritten-model"),
            ..Default::default()
        },
    )
    .await;
    let response = router
        .oneshot(request("/v1/messages", false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["model"], "model-a");
    let captured = execution.captured.lock().unwrap();
    let Operation::Generate(generation) = captured.as_ref().unwrap() else {
        panic!("generate")
    };
    assert_eq!(
        generation.protocol_payload().body()["model"],
        "rewritten-model"
    );
}

#[tokio::test]
async fn middleware_cannot_replace_message_identity_before_commit() {
    let session = Session::new(stream_events());
    let trace = session.trace.clone();
    let (router, _) = fixture_with_middleware(session, super::middleware::RequestMiddleware {
        expected_operation: Some(gateway_core::operation::OperationKind::Generate),
        ..Default::default()
    }.with_response_actions(vec![super::middleware::ResponseFrameAction::Replace(Bytes::from_static(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_foreign\"}}\n\n"))])).await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(trace.commits.load(Ordering::SeqCst), 0);
    assert!(trace.finalized.load(Ordering::SeqCst));
}

#[tokio::test]
async fn failed_commit_finalizes_without_delivering_a_success_message() {
    let mut session = Session::new(vec![wire(message())]);
    session.fail_commit = true;
    let trace = session.trace.clone();
    let (router, _) = fixture(session).await;
    let response = router
        .oneshot(request("/v1/messages", false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["type"], "error");
    assert!(trace.finalized.load(Ordering::SeqCst));
    assert!(trace.cancelled.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn delayed_first_message_keeps_the_same_commit_and_finalization_path() {
    let mut session = Session::new(stream_events());
    session.delay = Some(Duration::from_secs(16));
    let trace = session.trace.clone();
    let (router, _) = fixture(session).await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(bytes.starts_with(b": keep-alive\n\n"));
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("event: message_stop\n"));
    assert!(!text.contains("[DONE]"));
    assert_eq!(trace.commits.load(Ordering::SeqCst), 1);
    assert!(trace.finalized.load(Ordering::SeqCst));
}

#[tokio::test]
async fn messages_method_rejections_use_native_error_envelopes() {
    for path in ["/v1/messages", "/v1/messages/count_tokens"] {
        let (router, _) = fixture(Session::new(vec![])).await;
        let response = router
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "invalid_request_error");
    }
}

#[tokio::test(start_paused = true)]
async fn delayed_first_error_keeps_anthropic_wire_and_retry_hints_after_keepalive() {
    let mut session = Session::new(vec![]);
    session.delay = Some(Duration::from_secs(16));
    session.finish = Finish::Error;
    session.response_headers.push(ProviderResponseHeader::new(
        "retry-after",
        Bytes::from_static(b"9"),
    ));
    let trace = session.trace.clone();
    let (router, _) = fixture(session).await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.starts_with(": keep-alive\n\n"));
    assert!(text.contains("event: error\n"));
    assert!(!text.contains("response.failed"));
    assert!(!text.contains("[DONE]"));
    let events = gateway_protocol::openai::sse::parse_sse_events(text).unwrap();
    let error: Value = serde_json::from_str(&events.last().unwrap().data).unwrap();
    assert_eq!(error["type"], "error");
    assert_eq!(error["error"]["headers"]["retry-after"], "9");
    assert!(trace.finalized.load(Ordering::SeqCst));
}

#[tokio::test]
async fn native_json_projection_changes_only_the_top_level_model_span() {
    let raw = r#" { "id":"msg_native", "type":"message", "model" : "upstream-model", "content":[{"type":"text","text":"upstream-model","model":"business-model"}], "headers":{"openai-model":"business-header"}, "n":1.2300e+42 } "#;
    let (router, _) = fixture(Session::new(vec![ProviderEvent::wire(
        ProtocolWireEvent::raw_json("anthropic", Bytes::from_static(raw.as_bytes())).unwrap(),
    )]))
    .await;
    let response = router
        .oneshot(request("/v1/messages", false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        bytes.as_ref(),
        raw.replacen("\"upstream-model\"", "\"model-a\"", 1)
            .as_bytes()
    );
}

#[tokio::test]
async fn sse_projection_preserves_wire_and_the_pre_middleware_model_identity() {
    let raw = ": heartbeat\r\nid: opaque\r\nretry: 0042\r\nevent: message_start\r\ndata: {\"type\":\"message_start\",\r\ndata:  \"message\":{\"id\":\"msg_native\",\"model\":\"upstream-model\",\"content\":[{\"model\":\"business-model\"}],\"headers\":{\"openai-model\":\"business-header\"}},\"model\":\"business-root\",\"n\":1.200e+40}\r\n\r\n";
    let parsed = gateway_protocol::openai::sse::parse_sse_events(raw).unwrap();
    let event = ProviderEvent::wire(
        ProtocolWireEvent::json_with_raw_sse_metadata(
            "anthropic",
            Some("message_start".to_owned()),
            serde_json::from_str(&parsed[0].data).unwrap(),
            Bytes::from_static(raw.as_bytes()),
            Some("opaque".to_owned()),
            Some(42),
        )
        .unwrap(),
    );
    let original = event.clone();
    let unknown = Bytes::from_static(b"event: future_extension\ndata: {\"type\":\"future_extension\",\"message\":{\"model\":\"upstream-model\"}}\n\n");
    let (router, execution) = fixture_with_middleware(
        Session::new(vec![
            event,
            ProviderEvent::wire(ProtocolWireEvent::raw_sse("anthropic", unknown.clone()).unwrap()),
            wire(json!({"type":"message_stop"})),
        ]),
        super::middleware::RequestMiddleware {
            expected_operation: Some(gateway_core::operation::OperationKind::Generate),
            request_model: Some("rewritten-model"),
            ..Default::default()
        },
    )
    .await;
    let response = router.oneshot(request("/v1/messages", true)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        bytes.starts_with(
            raw.replacen("\"upstream-model\"", "\"model-a\"", 1)
                .as_bytes()
        )
    );
    assert!(bytes.windows(unknown.len()).any(|window| window == unknown));
    assert_eq!(
        original.wire_event().unwrap().data()["message"]["model"],
        "upstream-model"
    );
    assert_eq!(
        original
            .wire_event()
            .unwrap()
            .raw_sse_frame()
            .unwrap()
            .as_ref(),
        raw.as_bytes()
    );
    let captured = execution.captured.lock().unwrap();
    let Operation::Generate(generation) = captured.as_ref().unwrap() else {
        panic!("generate")
    };
    assert_eq!(
        generation.protocol_payload().body()["model"],
        "rewritten-model"
    );
}

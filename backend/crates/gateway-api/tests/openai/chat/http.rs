//! Chat HTTP 的鉴权、原始路径、流式错误、取消与提交顺序测试

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use bytes::Bytes;
use futures::{StreamExt, future::BoxFuture};
use gateway_core::{
    engine::{
        CommitRequirement, CoordinatedEvent, EngineError, ModelRequestId,
        execution::{
            AuthenticatedClient, ClientAuthenticationError, ExecutionService, ExecutionSession,
            ResponseCommit, StartExecution, StartProviderExecution, StartedExecution,
        },
        middleware::FrozenMiddlewarePlan,
    },
    error::{ClientVisibleUpstreamError, GatewayError, ProviderError, ProviderErrorKind},
    event::{ProtocolWireEvent, ProviderEvent, ProviderResponseHeader},
    operation::{Operation, OperationKind},
    routing::PublicModelId,
};
use gateway_protocol::openai::sse::parse_sse_events;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::openai::middleware::ResponseFrameAction;
use crate::openai::{api_router, authenticated_client_for_provider, middleware::RequestMiddleware};

const PATH: &str = "/v1/chat/completions";

#[derive(Default)]
struct Trace {
    commits: AtomicUsize,
    cancellations: AtomicUsize,
    finalized: AtomicBool,
    observed: Mutex<Option<(String, Value)>>,
    response_commit: ResponseCommit,
}

struct Session {
    trace: Arc<Trace>,
    events: VecDeque<Result<ProviderEvent, EngineError>>,
    buffered: bool,
    committed: bool,
    first_delay: Option<Duration>,
    fail_commit: bool,
}

impl ExecutionSession for Session {
    fn response_commit(&self) -> ResponseCommit {
        self.trace.response_commit.clone()
    }
    fn next_event(&mut self) -> BoxFuture<'_, Result<Option<CoordinatedEvent>, EngineError>> {
        Box::pin(async move {
            if let Some(delay) = self.first_delay.take() {
                tokio::time::sleep(delay).await;
            }
            match self.events.pop_front() {
                Some(Ok(event)) => Ok(Some(CoordinatedEvent::single(
                    event,
                    if self.committed {
                        CommitRequirement::AlreadyCommitted
                    } else {
                        CommitRequirement::CommitBeforeDelivery
                    },
                ))),
                Some(Err(error)) => {
                    self.trace.finalized.store(true, Ordering::SeqCst);
                    Err(error)
                }
                None => {
                    self.trace.finalized.store(true, Ordering::SeqCst);
                    Ok(None)
                }
            }
        })
    }
    fn collect_uncommitted(&mut self) -> BoxFuture<'_, Result<Vec<ProviderEvent>, EngineError>> {
        Box::pin(async move { self.events.drain(..).collect() })
    }
    fn response_headers(&self) -> &[ProviderResponseHeader] {
        &[]
    }
    fn discard_pending_delivery(&mut self) -> Result<(), EngineError> {
        Ok(())
    }
    fn commit_downstream(&mut self, status: Option<u16>) -> BoxFuture<'_, Result<(), EngineError>> {
        Box::pin(async move {
            if self.fail_commit {
                return Err(EngineError::ProviderMetadataMismatch);
            }
            self.trace.response_commit.record(status);
            self.trace.commits.fetch_add(1, Ordering::SeqCst);
            self.committed = true;
            if self.buffered {
                self.trace.finalized.store(true, Ordering::SeqCst);
            }
            Ok(())
        })
    }
    fn record_client_status(&mut self, _: u16) -> BoxFuture<'_, Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }
    fn is_finalized(&self) -> bool {
        self.trace.finalized.load(Ordering::SeqCst)
    }
    fn cancel(&self) {
        self.trace.cancellations.fetch_add(1, Ordering::SeqCst);
    }
    fn detach_finalize(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            self.trace.finalized.store(true, Ordering::SeqCst);
        })
    }
}

struct Execution {
    client: AuthenticatedClient,
    trace: Arc<Trace>,
    session: Mutex<Option<Session>>,
    middleware: Option<FrozenMiddlewarePlan>,
}

impl ExecutionService for Execution {
    fn authenticate(&self, key: &str) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        if key == "synthetic-chat-key" {
            Ok(self.client.clone())
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
    fn middleware_plan(
        &self,
        _: &gateway_core::engine::execution::PreparedRootExecution,
    ) -> Option<FrozenMiddlewarePlan> {
        self.middleware.clone()
    }
    fn start(
        &self,
        request: StartExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async move {
            let Operation::Generate(generate) = request.operation else {
                panic!("Chat must use Generate");
            };
            *self.trace.observed.lock().unwrap() = Some((
                request.metadata.endpoint,
                Value::Object(generate.protocol_payload().body().clone()),
            ));
            Ok(StartedExecution {
                request_id: ModelRequestId::new("req_chat_synthetic").unwrap(),
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
        Box::pin(async {
            panic!("Chat must use model execution");
        })
    }
}

fn wire(kind: &str, mut value: Value) -> ProviderEvent {
    value["type"] = json!(kind);
    ProviderEvent::wire(ProtocolWireEvent::json("openai", Some(kind.to_owned()), value).unwrap())
}

fn output() -> Value {
    json!({"type":"message","id":"msg_synthetic","role":"assistant","content":[{"type":"output_text","text":"hello"}]})
}

fn terminal() -> Value {
    json!({"id":"resp_chat","created_at":123,"model":"model-a","status":"completed","output":[output()],"usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}})
}

fn events() -> Vec<Result<ProviderEvent, EngineError>> {
    vec![
        Ok(wire(
            "response.created",
            json!({"response":{"id":"resp_chat","model":"model-a","created_at":123,"status":"in_progress"}}),
        )),
        Ok(wire(
            "response.output_text.delta",
            json!({"output_index":0,"content_index":0,"delta":"hello"}),
        )),
        Ok(wire("response.completed", json!({"response":terminal()}))),
    ]
}

fn session(
    trace: Arc<Trace>,
    stream: bool,
    events: Vec<Result<ProviderEvent, EngineError>>,
) -> Session {
    Session {
        trace,
        events: events.into(),
        buffered: !stream,
        committed: false,
        first_delay: None,
        fail_commit: false,
    }
}

async fn app(session: Session, middleware: Option<Arc<RequestMiddleware>>) -> axum::Router {
    let execution = Execution {
        client: authenticated_client_for_provider("synthetic-chat-key", "openai"),
        trace: session.trace.clone(),
        session: Mutex::new(Some(session)),
        middleware: middleware.as_ref().map(|m| m.frozen()),
    };
    api_router(Arc::new(execution)).await
}

fn request(stream: bool) -> Value {
    let mut value =
        json!({"model":"model-a","messages":[{"role":"user","content":"hello"}],"stream":stream});
    if stream {
        value["stream_options"] = json!({"include_usage":true});
    }
    value
}

fn http_request(value: Value) -> Request<Body> {
    Request::post(PATH)
        .header("authorization", "Bearer synthetic-chat-key")
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

async fn text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn preserves_chat_endpoint_and_converts_images_before_starting_execution() {
    let trace = Arc::new(Trace::default());
    let app = app(session(trace.clone(), false, events()), None).await;
    let mut body = request(false);
    body["messages"][0]["content"] = json!([{"type":"text","text":"describe"},{"type":"image_url","image_url":{"url":"https://example.invalid/image.png"}}]);
    let response = app.oneshot(http_request(body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-gateway-request-id"],
        "req_chat_synthetic"
    );
    let value: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(value["object"], "chat.completion");
    assert_eq!(value["choices"][0]["message"]["content"], "hello");
    assert_eq!(value["usage"]["prompt_tokens"], 10);
    let observed = trace.observed.lock().unwrap();
    let (path, body) = observed.as_ref().unwrap();
    assert_eq!(path, PATH);
    assert_eq!(
        body["input"][0]["content"][1],
        json!({"type":"input_image","image_url":"https://example.invalid/image.png","detail":"auto"})
    );
    assert!(body.get("messages").is_none());
    assert_eq!(trace.commits.load(Ordering::SeqCst), 1);
    assert_eq!(trace.cancellations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn streaming_delivers_chat_chunks_usage_and_one_done_after_finalization() {
    let trace = Arc::new(Trace::default());
    let response = app(session(trace.clone(), true, events()), None)
        .await
        .oneshot(http_request(request(true)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let body = text(response).await;
    assert!(!body.contains("response.completed"));
    assert_eq!(body.matches("data: [DONE]").count(), 1);
    let frames = parse_sse_events(&body).unwrap();
    let values: Vec<Value> = frames
        .iter()
        .filter(|frame| frame.data != "[DONE]")
        .map(|frame| serde_json::from_str(&frame.data).unwrap())
        .collect();
    assert_eq!(values[0]["choices"][0]["delta"]["role"], "assistant");
    assert_eq!(values[1]["choices"][0]["delta"]["content"], "hello");
    assert_eq!(values[2]["choices"][0]["finish_reason"], "stop");
    assert_eq!(values[3]["choices"], json!([]));
    assert!(trace.finalized.load(Ordering::SeqCst));
    assert_eq!(trace.commits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn completion_token_budget_reaches_execution_for_json_and_sse_requests() {
    for field in ["max_tokens", "max_completion_tokens"] {
        for stream in [false, true] {
            let trace = Arc::new(Trace::default());
            let mut body = request(stream);
            body[field] = json!(1024);
            let response = app(session(trace.clone(), stream, events()), None)
                .await
                .oneshot(http_request(body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let response_body = text(response).await;
            if stream {
                assert!(response_body.contains("data: [DONE]"));
            } else {
                let value: Value = serde_json::from_str(&response_body).unwrap();
                assert_eq!(value["choices"][0]["message"]["content"], "hello");
            }
            let observed = trace.observed.lock().unwrap();
            let (_, body) = observed.as_ref().unwrap();
            assert_eq!(body["max_output_tokens"], 1024);
            assert!(body.get(field).is_none());
            assert_eq!(trace.commits.load(Ordering::SeqCst), 1);
            assert!(trace.finalized.load(Ordering::SeqCst));
        }
    }
}

#[tokio::test]
async fn invalid_completion_token_budget_fails_before_execution() {
    let trace = Arc::new(Trace::default());
    let mut body = request(false);
    body["max_completion_tokens"] = json!(0);
    let response = app(session(trace.clone(), false, events()), None)
        .await
        .oneshot(http_request(body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let value: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(value["error"]["param"], "max_completion_tokens");
    assert!(trace.observed.lock().unwrap().is_none());
}

#[tokio::test]
async fn unsupported_parameters_and_authentication_fail_before_execution() {
    let trace = Arc::new(Trace::default());
    let app = app(session(trace.clone(), false, events()), None).await;
    let mut body = request(false);
    body["stop"] = json!(["END"]);
    let response = app.clone().oneshot(http_request(body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let value: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(value["error"]["param"], "stop");
    let response = app
        .oneshot(
            Request::post(PATH)
                .body(Body::from("invalid JSON"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(trace.observed.lock().unwrap().is_none());
}

#[tokio::test]
async fn upstream_failure_after_output_is_an_error_without_success_finish() {
    let trace = Arc::new(Trace::default());
    let mut events = events();
    events.pop();
    events.push(Err(upstream_failure("synthetic upstream failed")));
    let response = app(session(trace.clone(), true, events), None)
        .await
        .oneshot(http_request(request(true)))
        .await
        .unwrap();
    let body = text(response).await;
    assert!(body.contains("synthetic upstream failed"));
    assert!(!body.contains("\"finish_reason\":\"stop\""));
    assert_eq!(body.matches("data: [DONE]").count(), 1);
    assert!(!body.contains("response.failed"));
    assert!(trace.finalized.load(Ordering::SeqCst));
}

#[tokio::test]
async fn conversion_failure_and_commit_failure_do_not_commit_success() {
    for fail_commit in [false, true] {
        let trace = Arc::new(Trace::default());
        let mut end = terminal();
        if !fail_commit {
            end["output"] = json!([{"type":"image_generation_call"}]);
        }
        let mut session = session(
            trace.clone(),
            false,
            vec![Ok(wire("response.completed", json!({"response":end})))],
        );
        session.fail_commit = fail_commit;
        let response = app(session, None)
            .await
            .oneshot(http_request(request(false)))
            .await
            .unwrap();
        assert!(response.status().is_server_error());
        let _ = text(response).await;
        assert_eq!(trace.commits.load(Ordering::SeqCst), 0);
        assert_eq!(trace.cancellations.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn dropping_stream_releases_original_session() {
    let trace = Arc::new(Trace::default());
    let response = app(session(trace.clone(), true, events()), None)
        .await
        .oneshot(http_request(request(true)))
        .await
        .unwrap();
    drop(response);
    tokio::task::yield_now().await;
    assert_eq!(trace.cancellations.load(Ordering::SeqCst), 1);
    assert!(trace.finalized.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn first_frame_keepalive_turns_late_failure_into_chat_error() {
    let trace = Arc::new(Trace::default());
    let mut session = session(
        trace.clone(),
        true,
        vec![Err(upstream_failure("late failure"))],
    );
    session.first_delay = Some(Duration::from_secs(20));
    let response = app(session, None)
        .await
        .oneshot(http_request(request(true)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(
        stream.next().await.unwrap().unwrap(),
        Bytes::from_static(b": keep-alive\n\n")
    );
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.unwrap());
    }
    let body = String::from_utf8(body).unwrap();
    assert!(body.contains("late failure"));
    assert!(!body.contains("response.failed"));
    assert!(body.ends_with("data: [DONE]\n\n"));
    assert_eq!(trace.commits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn request_middleware_observes_chat_path_and_execution_still_commits_once() {
    let trace = Arc::new(Trace::default());
    let middleware = Arc::new(RequestMiddleware {
        expected_operation: Some(OperationKind::Generate),
        ..RequestMiddleware::default()
    });
    let response = app(
        session(trace.clone(), false, events()),
        Some(middleware.clone()),
    )
    .await
    .oneshot(http_request(request(false)))
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = text(response).await;
    assert_eq!(*middleware.endpoints.lock().unwrap(), vec![PATH]);
    assert_eq!(trace.commits.load(Ordering::SeqCst), 1);
}

fn upstream_failure(message: &str) -> EngineError {
    EngineError::Provider(
        ProviderError::new(
            ProviderErrorKind::Transport,
            gateway_core::upstream::UpstreamSendState::Sent,
        )
        .with_client_visible_upstream_error(ClientVisibleUpstreamError::new(
            message,
            Some("server_error".into()),
            Some("server_error".into()),
        )),
    )
}

#[tokio::test]
async fn middleware_cannot_replace_response_identity_or_turn_tool_completion_into_stop() {
    for (id, finish) in [("other_response", "tool_calls"), ("resp_chat", "stop")] {
        let trace = Arc::new(Trace::default());
        let mut end = terminal();
        end["output"] = json!([{"type":"function_call","id":"fc_item","call_id":"call_a","name":"f","arguments":"{}"}]);
        let replacement = json!({"id":id,"object":"chat.completion","created":123,"model":"model-a","choices":[{"index":0,"message":{"role":"assistant","content":"rewritten"},"finish_reason":finish}]});
        let middleware = Arc::new(
            RequestMiddleware {
                expected_operation: Some(OperationKind::Generate),
                ..RequestMiddleware::default()
            }
            .with_response_actions(vec![ResponseFrameAction::Replace(Bytes::from(
                replacement.to_string(),
            ))]),
        );
        let response = app(
            session(
                trace.clone(),
                false,
                vec![Ok(wire("response.completed", json!({"response":end})))],
            ),
            Some(middleware),
        )
        .await
        .oneshot(http_request(request(false)))
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let _ = text(response).await;
        assert_eq!(trace.commits.load(Ordering::SeqCst), 0);
        assert_eq!(trace.cancellations.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn middleware_cannot_remove_required_chat_metadata_before_commit() {
    let trace = Arc::new(Trace::default());
    let replacement = json!({"id":"resp_chat","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"rewritten"},"finish_reason":"stop"}]});
    let middleware = Arc::new(
        RequestMiddleware {
            expected_operation: Some(OperationKind::Generate),
            ..RequestMiddleware::default()
        }
        .with_response_actions(vec![ResponseFrameAction::Replace(Bytes::from(
            replacement.to_string(),
        ))]),
    );
    let response = app(session(trace.clone(), false, events()), Some(middleware))
        .await
        .oneshot(http_request(request(false)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let _ = text(response).await;
    assert_eq!(trace.commits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dropping_finish_chunk_produces_stream_error_and_releases_session() {
    let trace = Arc::new(Trace::default());
    let middleware = Arc::new(
        RequestMiddleware {
            expected_operation: Some(OperationKind::Generate),
            ..RequestMiddleware::default()
        }
        .with_response_actions(vec![
            ResponseFrameAction::Continue,
            ResponseFrameAction::Continue,
            ResponseFrameAction::Drop,
        ]),
    );
    let response = app(session(trace.clone(), true, events()), Some(middleware))
        .await
        .oneshot(http_request(request(true)))
        .await
        .unwrap();
    let body = text(response).await;
    assert!(body.contains("chat_delivery_invalid"));
    assert!(!body.contains("\"finish_reason\":\"stop\""));
    assert_eq!(body.matches("data: [DONE]").count(), 1);
    assert!(trace.finalized.load(Ordering::SeqCst));
}

#[tokio::test]
async fn function_result_history_and_tool_stream_share_the_same_call_id() {
    let trace = Arc::new(Trace::default());
    let item = json!({"type":"function_call","id":"fc_internal","call_id":"call_client","name":"f","arguments":"{}"});
    let mut end = terminal();
    end["output"] = json!([item]);
    let events = vec![
        Ok(wire(
            "response.created",
            json!({"response":{"id":"resp_chat","created_at":123}}),
        )),
        Ok(wire(
            "response.output_item.added",
            json!({"output_index":0,"item":item}),
        )),
        Ok(wire(
            "response.function_call_arguments.delta",
            json!({"item_id":"fc_internal","delta":"{}"}),
        )),
        Ok(wire("response.completed", json!({"response":end}))),
    ];
    let mut body = request(true);
    body["messages"] = json!([
        {"role":"assistant","tool_calls":[{"id":"call_previous","type":"function","function":{"name":"f","arguments":"{}"}}]},
        {"role":"tool","tool_call_id":"call_previous","content":"previous result"}
    ]);
    let response = app(session(trace.clone(), true, events), None)
        .await
        .oneshot(http_request(body))
        .await
        .unwrap();
    let body = text(response).await;
    assert!(body.contains("call_client"));
    assert!(!body.contains("fc_internal"));
    assert!(body.contains("\"finish_reason\":\"tool_calls\""));
    assert_eq!(
        trace.observed.lock().unwrap().as_ref().unwrap().1["input"][1],
        json!({"type":"function_call_output","call_id":"call_previous","output":"previous result"})
    );
}

#[tokio::test]
async fn compressed_chat_request_is_decoded_before_conversion() {
    use std::io::Write;
    let trace = Arc::new(Trace::default());
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(request(false).to_string().as_bytes())
        .unwrap();
    let response = app(session(trace.clone(), false, events()), None)
        .await
        .oneshot(
            Request::post(PATH)
                .header("authorization", "Bearer synthetic-chat-key")
                .header("content-encoding", "gzip")
                .body(Body::from(encoder.finish().unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = text(response).await;
    assert_eq!(trace.commits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn real_http_socket_delivers_chat_sse_and_completes_session() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let trace = Arc::new(Trace::default());
    let app = app(session(trace.clone(), true, events()), None).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let payload = request(true).to_string();
    socket.write_all(format!("POST {PATH} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer synthetic-chat-key\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",payload.len()).as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    server.abort();
    let response = String::from_utf8(bytes).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("text/event-stream"));
    assert!(response.contains("chat.completion.chunk"));
    assert!(response.contains("data: [DONE]"));
    assert!(trace.finalized.load(Ordering::SeqCst));
    assert_eq!(trace.commits.load(Ordering::SeqCst), 1);
}

//! 模拟 New API 身份信封，覆盖真实 HTTP、SSE 和 Responses WebSocket 执行链

use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use futures::{SinkExt, StreamExt};
use gateway_core::routing::{ProviderCatalogGeneration, ProviderModelCapabilities};
use gateway_core::{
    engine::{
        AttemptContext,
        provider::{Provider, ProviderCallMetadata, ProviderRequest, ProviderStream},
    },
    error::ProviderError,
    event::{GatewayEvent, ProtocolWireEvent, ProviderEvent, ResponseMeta},
    operation::OperationKind,
    policy::{CallerIdentity, CallerIdentityPolicy},
    upstream::UpstreamTransport,
};
use serde_json::{Value, json};
use std::sync::Mutex;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tower::ServiceExt as _;

const SOURCE: &str = "b7bb7de9-7b06-4caa-8070-352af86a8a4a";
fn identity_snapshot(policy: CallerIdentityPolicy, revision: u64) -> RuntimeSnapshot {
    let base = super::snapshot("synthetic-shared-key", "openai");
    RuntimeSnapshot::new(
        ConfigRevision::new(revision).unwrap(),
        gateway_core::settings::SettingsValues::new(2, "smart", Default::default(), None, None),
        vec![ProviderKind::new("openai").unwrap()],
        vec![ProviderModel::new(
            ProviderKind::new("openai").unwrap(),
            UpstreamModelId::new("model-a").unwrap(),
            ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), Some(16000))
                .with_feature(
                    gateway_core::operation::Feature::NativeContinuation,
                    gateway_core::routing::SupportLevel::Native,
                ),
        )],
        base.client_policies()
            .map(|client| client.clone().with_identity_policy(policy.clone()))
            .collect(),
    )
    .unwrap()
    .with_account_directory(Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([(
        ProviderAccountId::new("acct_api_test").unwrap(),
        RuntimeAccount::new(ProviderKind::new("openai").unwrap(), BTreeSet::new()),
    )]))))
}
#[derive(Default)]
struct CapturingProvider(
    Mutex<Vec<(CallerIdentity, String)>>,
    Mutex<Vec<Option<String>>>,
);
#[async_trait]
impl Provider for CapturingProvider {
    fn name(&self) -> &'static str {
        "openai"
    }
    fn catalog_generation(&self) -> ProviderCatalogGeneration {
        ProviderCatalogGeneration::default()
    }
    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError> {
        Ok(vec![])
    }
    async fn execute(
        self: Arc<Self>,
        request: ProviderRequest,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        self.0.lock().unwrap().push((
            context.caller_identity(),
            context.client_api_key_ref().as_str().into(),
        ));
        self.1
            .lock()
            .unwrap()
            .push(context.caller_token_name().map(str::to_owned));
        let response_id = format!("resp_{}", context.request_id());
        let response = ResponseMeta::new(response_id.clone(), "model-a");
        let events = [
            ProviderEvent::canonical_with_wire(vec![GatewayEvent::Started(response.clone())], ProtocolWireEvent::json("openai", Some("response.created".into()), json!({"type":"response.created","response":{"id":response_id,"model":"model-a","status":"in_progress"}})).unwrap()),
            ProviderEvent::canonical_with_wire(vec![GatewayEvent::Completed(response)], ProtocolWireEvent::json("openai", Some("response.completed".into()), json!({"type":"response.completed","response":{"id":response_id,"model":"model-a","status":"completed","output":[]}})).unwrap()),
        ];
        Ok(ProviderStream::new(
            ProviderCallMetadata::new(
                request.candidate().provider().clone(),
                request.candidate().upstream_model().unwrap().clone(),
                ProviderAccountId::new("acct_api_test").unwrap(),
                UpstreamTransport::new("http_sse").unwrap(),
            ),
            futures::stream::iter(events.map(Ok)),
            (),
        ))
    }
}
#[derive(Default)]
struct Continuations(Mutex<Vec<NativeContinuationPin>>);
impl NativeContinuationPort for Continuations {
    fn resolve<'a>(
        &'a self,
        key: &'a ClientApiKeyId,
        previous: &'a PreviousResponseId,
    ) -> BoxFuture<'a, Result<Option<NativeContinuationPin>, NativeContinuationStoreError>> {
        Box::pin(async move {
            self.resolve_for_caller(key, &CallerIdentity::direct(key), previous)
                .await
        })
    }
    fn resolve_for_caller<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        caller: &'a CallerIdentity,
        previous: &'a PreviousResponseId,
    ) -> BoxFuture<'a, Result<Option<NativeContinuationPin>, NativeContinuationStoreError>> {
        Box::pin(async move {
            let pin = self
                .0
                .lock()
                .unwrap()
                .iter()
                .find(|pin| pin.previous_response_id() == previous)
                .cloned();
            if pin.as_ref().is_some_and(|pin| !pin.matches_caller(caller)) {
                return Err(NativeContinuationStoreError::ownership_mismatch());
            }
            Ok(pin)
        })
    }
    fn record<'a>(
        &'a self,
        pin: NativeContinuationPin,
    ) -> BoxFuture<'a, Result<(), NativeContinuationStoreError>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(pin);
            Ok(())
        })
    }
}
#[derive(Default)]
struct RecordingStore(Mutex<Vec<NewModelRequest>>);

#[async_trait]
impl ExecutionStore for RecordingStore {
    async fn create_model_request(&self, request: NewModelRequest) -> Result<(), StoreError> {
        self.0.lock().unwrap().push(request);
        Ok(())
    }

    async fn record_attempt(&self, _: AttemptRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn mark_send_state(
        &self,
        _: &ModelRequestId,
        _: UpstreamSendState,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn mark_downstream_committed(
        &self,
        _: &ModelRequestId,
        _: SystemTime,
        _: Option<u16>,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn record_client_status(&self, _: &ModelRequestId, _: u16) -> Result<(), StoreError> {
        Ok(())
    }

    async fn record_intermediate_failure(&self, _: IntermediateFailure) -> Result<(), StoreError> {
        Ok(())
    }

    async fn finalize_model_request(&self, _: ModelRequestFinalization) -> Result<(), StoreError> {
        Ok(())
    }

    async fn recover_expired(&self, _: SystemTime) -> Result<RecoveryReport, StoreError> {
        Ok(RecoveryReport::default())
    }
}

struct UnusedAdmissions;

impl ClientAdmissionPort for UnusedAdmissions {
    fn abandon(
        &self,
        key: &gateway_core::policy::ClientApiKeyId,
        request: &gateway_core::engine::ModelRequestId,
    ) {
        let _ = futures::FutureExt::now_or_never(self.release(key, request));
    }

    fn admit(
        &self,
        _: ClientAdmissionRequest,
    ) -> BoxFuture<'_, Result<ClientAdmissionDecision, ClientAdmissionError>> {
        Box::pin(async { Ok(ClientAdmissionDecision::Granted) })
    }

    fn release<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a ModelRequestId,
    ) -> BoxFuture<'a, Result<bool, ClientAdmissionError>> {
        Box::pin(async { Ok(true) })
    }

    fn restore(
        &self,
        _: ClientAdmissionRecovery,
    ) -> BoxFuture<'_, Result<ClientAdmissionRestoreResult, ClientAdmissionError>> {
        Box::pin(async { Ok(ClientAdmissionRestoreResult::default()) })
    }
}

struct Fixture {
    app: axum::Router,
    snapshots: RuntimeSnapshotHandle,
    provider: Arc<CapturingProvider>,
    store: Arc<RecordingStore>,
    continuations: Arc<Continuations>,
}
async fn fixture(policy: CallerIdentityPolicy) -> Fixture {
    let snapshots = RuntimeSnapshotHandle::new(identity_snapshot(policy, 1));
    let provider = Arc::new(CapturingProvider::default());
    let store = Arc::new(RecordingStore::default());
    let continuations = Arc::new(Continuations::default());
    let service = Arc::new(DefaultExecutionService::new(
        snapshots.clone(),
        store.clone(),
        ProviderRegistry::new([provider.clone() as Arc<dyn Provider>]).unwrap(),
        Arc::new(UnusedAdmissions),
        continuations.clone(),
        Arc::new(IgnoredClientApiKeyUsage),
        Arc::new(crate::support::RecordingDiagnostics::default()),
    ));
    Fixture {
        app: super::api_router(service).await,
        snapshots,
        provider,
        store,
        continuations,
    }
}
fn proxy(allow_unidentified: bool) -> CallerIdentityPolicy {
    CallerIdentityPolicy::NewApi {
        source_id: SOURCE.into(),
        allow_unidentified,
    }
}
fn request(stream: bool, user: Option<&str>, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("authorization", "Bearer synthetic-shared-key")
        .header("content-type", "application/json");
    if let Some(user) = user {
        builder = builder.header("x-cpr-user-id", user);
    }
    if let Some(token) = token {
        builder = builder.header("x-cpr-token-id", token);
    }
    builder.body(Body::from(json!({"model":"model-a","input":"synthetic","stream":stream,"session_id":"same-session"}).to_string())).unwrap()
}
#[tokio::test]
async fn direct_and_newapi_http_json_sse_preserve_auth_key_and_record_distinct_callers() {
    for policy in [CallerIdentityPolicy::Direct, proxy(false)] {
        let fixture = fixture(policy.clone()).await;
        for (index, (user, token)) in [("1", "11"), ("1", "12"), ("2", "21")]
            .into_iter()
            .enumerate()
        {
            let response = fixture
                .app
                .clone()
                .oneshot(request(index % 2 == 1, Some(user), Some(token)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            assert!(!bytes.is_empty());
        }
        let calls = fixture.provider.0.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|(_, key)| key == "key_api_test"));
        if matches!(policy, CallerIdentityPolicy::Direct) {
            assert!(
                calls
                    .iter()
                    .all(|(caller, _)| matches!(caller, CallerIdentity::Direct { .. }))
            );
        } else {
            assert_ne!(calls[0].0, calls[1].0);
            assert_ne!(calls[0].0, calls[2].0);
        }
        let records = fixture.store.0.lock().unwrap();
        assert_eq!(records.len(), 3);
        for (record, (caller, _)) in records.iter().zip(calls.iter()) {
            assert_eq!(record.caller_identity.as_ref(), Some(caller));
        }
    }
}
#[tokio::test]
async fn newapi_identity_errors_preserve_http_and_sse_requests_in_the_configured_fallback_scope() {
    for isolated in [false, true] {
        let fixture = fixture(proxy(isolated)).await;
        for stream in [false, true] {
            let mut duplicate = request(stream, Some("1"), Some("2"));
            duplicate
                .headers_mut()
                .append("x-cpr-user-id", "1".parse().unwrap());
            for request in [
                request(stream, None, None),
                request(stream, Some("1"), None),
                request(stream, None, Some("2")),
                request(stream, Some("01"), Some("2")),
                request(stream, Some("1,2"), Some("2")),
                duplicate,
            ] {
                let response = fixture.app.clone().oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert!(
                    !to_bytes(response.into_body(), 1024 * 1024)
                        .await
                        .unwrap()
                        .is_empty()
                );
            }
        }
        let calls = fixture.provider.0.lock().unwrap();
        let records = fixture.store.0.lock().unwrap();
        assert_eq!(calls.len(), 12);
        assert_eq!(records.len(), calls.len());
        for (record, (caller, key)) in records.iter().zip(calls.iter()) {
            assert_eq!(key, "key_api_test");
            if isolated {
                assert!(caller.is_unidentified());
            } else {
                assert_eq!(
                    caller,
                    &CallerIdentity::direct(&ClientApiKeyId::new(key).unwrap())
                );
            }
            assert_eq!(record.caller_identity.as_ref(), Some(caller));
        }
    }
}
#[tokio::test]
async fn identity_fallback_does_not_bypass_client_key_authentication() {
    let fixture = fixture(proxy(false)).await;
    let mut request = request(false, None, None);
    request
        .headers_mut()
        .insert("authorization", "Bearer wrong-key".parse().unwrap());
    let response = fixture.app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(fixture.provider.0.lock().unwrap().is_empty());
    assert!(fixture.store.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn optional_probe_requests_get_fresh_callers_and_cannot_continue_responses() {
    let fixture = fixture(proxy(true)).await;
    for _ in 0..2 {
        let response = fixture
            .app
            .clone()
            .oneshot(request(false, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    }
    {
        let calls = fixture.provider.0.lock().unwrap();
        assert!(calls.iter().all(|(caller, _)| caller.is_unidentified()));
        assert_ne!(calls[0].0, calls[1].0);
    }
    assert!(fixture.continuations.0.lock().unwrap().is_empty());
    let mut continuation = request(false, None, None);
    *continuation.body_mut() = Body::from(
        json!({"model":"model-a","input":"next","previous_response_id":"resp_probe"}).to_string(),
    );
    let response = fixture.app.oneshot(continuation).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(fixture.provider.0.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn responses_websocket_keeps_handshake_identity_and_rejects_changed_source_before_next_turn()
{
    for (policy, identity) in [
        (CallerIdentityPolicy::Direct, Some(("1", "11"))),
        (proxy(false), Some(("1", "11"))),
        (proxy(false), None),
        (proxy(false), Some(("01", "11"))),
    ] {
        let fixture = fixture(policy.clone()).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, fixture.app).await.unwrap();
        });
        let mut request = format!("ws://{address}/v1/responses")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "authorization",
            "Bearer synthetic-shared-key".parse().unwrap(),
        );
        if let Some((user, token)) = identity {
            request
                .headers_mut()
                .insert("x-cpr-user-id", user.parse().unwrap());
            request
                .headers_mut()
                .insert("x-cpr-token-id", token.parse().unwrap());
        }
        let (mut socket, _) = connect_async(request).await.unwrap();
        socket.send(Message::text(json!({"type":"response.create","model":"model-a","input":"synthetic","stream":true}).to_string())).await.unwrap();
        loop {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if let Message::Text(text) = frame {
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["type"] == "response.completed" {
                    break;
                }
            }
        }
        assert_eq!(fixture.provider.0.lock().unwrap().len(), 1);
        {
            let calls = fixture.provider.0.lock().unwrap();
            let expected = if matches!(policy, CallerIdentityPolicy::NewApi { .. })
                && identity == Some(("1", "11"))
            {
                CallerIdentity::NewApi {
                    source_id: SOURCE.into(),
                    user_id: "1".into(),
                    token_id: "11".into(),
                }
            } else {
                CallerIdentity::direct(&ClientApiKeyId::new("key_api_test").unwrap())
            };
            assert_eq!(calls[0].0, expected);
            assert_eq!(calls[0].1, "key_api_test");
        }
        if matches!(policy, CallerIdentityPolicy::NewApi { .. }) {
            fixture.snapshots.publish(identity_snapshot(
                CallerIdentityPolicy::NewApi {
                    source_id: "95ff5bb4-4016-457a-a780-1788f595635a".into(),
                    allow_unidentified: false,
                },
                2,
            ));
            socket.send(Message::text(json!({"type":"response.create","model":"model-a","input":"next","stream":true}).to_string())).await.unwrap();
            loop {
                let frame = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
                    .await
                    .unwrap();
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        let value: Value = serde_json::from_str(&text).unwrap();
                        if value["type"] == "error" {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            assert_eq!(fixture.provider.0.lock().unwrap().len(), 1);
        }
        let _ = socket.close(None).await;
        server.abort();
    }
}

#[tokio::test]
async fn fallback_can_continue_direct_sessions_but_never_claims_a_trusted_users_response() {
    let fixture = fixture(proxy(false)).await;
    let key = ClientApiKeyId::new("key_api_test").unwrap();
    for (response, caller) in [
        ("resp_direct", CallerIdentity::direct(&key)),
        (
            "resp_user",
            CallerIdentity::NewApi {
                source_id: SOURCE.into(),
                user_id: "1".into(),
                token_id: "11".into(),
            },
        ),
    ] {
        fixture.continuations.0.lock().unwrap().push(
            NativeContinuationPin::new(
                PreviousResponseId::new(response),
                PreviousResponseId::new(format!("upstream_{response}")),
                key.clone(),
                ProviderKind::new("openai").unwrap(),
                ProviderAccountId::new("acct_api_test").unwrap(),
            )
            .with_caller_identity(caller)
            .with_scope(gateway_core::engine::continuation::NativeContinuationScope::Persisted),
        );
    }
    for (user, token, previous, allowed) in [
        (None, None, "resp_direct", true),
        (Some("bad"), Some("11"), "resp_direct", true),
        (Some("1"), Some("11"), "resp_direct", false),
        (None, None, "resp_user", false),
        (Some("bad"), Some("11"), "resp_user", false),
        (Some("1"), Some("11"), "resp_user", true),
        (Some("1"), Some("12"), "resp_user", false),
    ] {
        let mut request = request(false, user, token);
        *request.body_mut() = Body::from(
            json!({"model":"model-a","input":"next","previous_response_id":previous}).to_string(),
        );
        let response = fixture.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(
            status,
            if allowed {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
            "{user:?}:{token:?} {previous}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    assert_eq!(fixture.provider.0.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn native_continuation_requires_exact_caller_and_missing_proxy_pins_fail_closed() {
    for policy in [CallerIdentityPolicy::Direct, proxy(false)] {
        let fixture = fixture(policy.clone()).await;
        let owner = match policy {
            CallerIdentityPolicy::Direct => CallerIdentity::Direct {
                key_id: "key_api_test".into(),
            },
            _ => CallerIdentity::NewApi {
                source_id: SOURCE.into(),
                user_id: "1".into(),
                token_id: "11".into(),
            },
        };
        fixture.continuations.0.lock().unwrap().push(
            NativeContinuationPin::new(
                PreviousResponseId::new("resp_owned"),
                PreviousResponseId::new("upstream_owned"),
                ClientApiKeyId::new("key_api_test").unwrap(),
                ProviderKind::new("openai").unwrap(),
                ProviderAccountId::new("acct_api_test").unwrap(),
            )
            .with_caller_identity(owner)
            .with_scope(gateway_core::engine::continuation::NativeContinuationScope::Persisted),
        );
        for (user, token, previous, allowed) in [
            ("1", "11", "resp_owned", true),
            (
                "1",
                "12",
                "resp_owned",
                matches!(policy, CallerIdentityPolicy::Direct),
            ),
            (
                "2",
                "11",
                "resp_owned",
                matches!(policy, CallerIdentityPolicy::Direct),
            ),
            (
                "1",
                "11",
                "resp_missing",
                matches!(policy, CallerIdentityPolicy::Direct),
            ),
        ] {
            let mut request = request(false, Some(user), Some(token));
            *request.body_mut() = Body::from(
                json!({"model":"model-a","input":"next","previous_response_id":previous})
                    .to_string(),
            );
            let response = fixture.app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            assert_eq!(
                status,
                if allowed {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
                "{user}:{token} {previous}: {}",
                String::from_utf8_lossy(&body)
            );
        }
    }
}

#[tokio::test]
async fn unsupported_live_ownership_paths_reject_proxy_callers_before_provider_execution() {
    let fixture = fixture(proxy(false)).await;
    for path in [
        "/v1/live",
        "/v1/realtime",
        "/v1/realtime/calls",
        "/v1/realtime/calls/synthetic/hangup",
    ] {
        let mut request = request(false, Some("1"), Some("11"));
        *request.uri_mut() = path.parse().unwrap();
        let response = fixture.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }
    assert!(fixture.provider.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn optional_token_names_cross_http_and_sse_without_changing_identity_or_rejecting_bad_names()
{
    use axum::http::HeaderValue;
    for policy in [CallerIdentityPolicy::Direct, proxy(false)] {
        let fixture = fixture(policy.clone()).await;
        let cases: Vec<(Vec<Vec<u8>>, Option<&str>)> = vec![
            (vec![], None),
            (vec![b"Developer".to_vec()], Some("Developer")),
            (vec!["开发用 Key".as_bytes().to_vec()], Some("开发用 Key")),
            (
                vec![b"%E5%BC%80%E5%8F%91%E7%94%A8%20Key+100%25".to_vec()],
                Some("开发用 Key+100%"),
            ),
            (vec![b"%25E4%25B8%25AD".to_vec()], Some("%E4%B8%AD")),
            (vec![b"bad%2".to_vec()], None),
            (vec![b"bad%0Avalue".to_vec()], None),
            (vec![b" ".to_vec()], None),
            (vec![b"bad\tvalue".to_vec()], None),
            (vec![vec![0xff]], None),
            (vec![vec![b'x'; 257]], None),
            (vec![b"a".to_vec(), b"a".to_vec()], None),
        ];
        for (index, (values, name)) in cases.into_iter().enumerate() {
            let mut request = request(index % 2 == 0, Some("1"), Some("11"));
            for value in values {
                request
                    .headers_mut()
                    .append("x-cpr-token-name", HeaderValue::from_bytes(&value).unwrap());
            }
            let response = fixture.app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            let expected = if matches!(policy, CallerIdentityPolicy::NewApi { .. }) {
                name
            } else {
                None
            };
            assert_eq!(
                fixture.provider.1.lock().unwrap()[index].as_deref(),
                expected
            );
        }
        let calls = fixture.provider.0.lock().unwrap();
        assert!(calls.iter().all(|call| call.0 == calls[0].0));
    }
}

#[tokio::test]
async fn websocket_optional_names_do_not_change_session_identity_or_block_followup_turns() {
    use axum::http::HeaderValue;
    for (names, expected) in [
        (vec![], None),
        (vec!["中文 Key".as_bytes().to_vec()], Some("中文 Key")),
        (
            vec![b"%E4%B8%AD%E6%96%87%20Key+1".to_vec()],
            Some("中文 Key+1"),
        ),
        (vec![b"%25E4%25B8%25AD".to_vec()], Some("%E4%B8%AD")),
        (vec![b"bad%GG".to_vec()], None),
        (vec![b"bad%0D%0Avalue".to_vec()], None),
        (vec![vec![0xff]], None),
        (vec![b"one".to_vec(), b"two".to_vec()], None),
    ] {
        let fixture = fixture(proxy(false)).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, fixture.app).await.unwrap() });
        let mut request = format!("ws://{address}/v1/responses")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_static("Bearer synthetic-shared-key"),
        );
        request
            .headers_mut()
            .insert("x-cpr-user-id", HeaderValue::from_static("1"));
        request
            .headers_mut()
            .insert("x-cpr-token-id", HeaderValue::from_static("11"));
        for name in &names {
            request
                .headers_mut()
                .append("x-cpr-token-name", HeaderValue::from_bytes(name).unwrap());
        }
        let (mut socket, _) = connect_async(request).await.unwrap();
        for token_name in ["attempted rename", "another name"] {
            socket.send(Message::text(json!({"type":"response.create","model":"model-a","input":"synthetic","stream":true,"token_name":token_name}).to_string())).await.unwrap();
            loop {
                let frame = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if let Message::Text(text) = frame {
                    let event: Value = serde_json::from_str(&text).unwrap();
                    if event["type"] == "response.completed" {
                        break;
                    }
                }
            }
        }
        let values = fixture.provider.1.lock().unwrap();
        assert_eq!(
            values.iter().map(Option::as_deref).collect::<Vec<_>>(),
            vec![expected, expected]
        );
        let calls = fixture.provider.0.lock().unwrap();
        assert_eq!(calls[0].0, calls[1].0);
        server.abort();
    }
}

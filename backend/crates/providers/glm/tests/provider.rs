//! 验证智谱冷发送、原生事件、错误分类、续接和账号范围

use super::support::Store;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use futures::{StreamExt, future::BoxFuture};
use gateway_core::{
    account::{
        AccountFeedbackStats, AccountSelectionPolicy, CredentialState,
        scope::{ClientRoutingScope, FrozenAccountScope, RuntimeAccount, RuntimeAccountDirectory},
    },
    engine::{
        AccountAttemptContext, AttemptContext, ModelRequestId, RequestAttemptContext,
        execution::ClientTransport,
        middleware::{
            FrozenMiddlewarePlan, MiddlewareContext, MiddlewareError, MiddlewareHeader,
            MiddlewareNext, MiddlewarePlan, MiddlewareRequest, MiddlewareResponse,
        },
        provider::{Provider, ProviderRequest},
    },
    error::ProviderErrorKind,
    event::GatewayEvent,
    lifecycle::CancellationToken,
    operation::{Feature, GenerateRequest, Operation, OperationKind, ProtocolPayload},
    policy::ClientApiKeyId,
    routing::{
        ConfigRevision, ModelCapabilities, ProviderKind, ProviderModel, PublicModelId,
        RoutingContext, RuntimeSnapshot, SupportLevel, UpstreamModelId,
        extensions::{ExtensionSetId, ExtensionSetLease, ExtensionSetReference},
    },
};
use provider_glm::GlmProvider;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
    sync::{Arc, atomic::Ordering},
    time::{Duration, SystemTime},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

pub(super) fn scope(store: &Store) -> Arc<FrozenAccountScope> {
    Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(
            store
                .rows
                .lock()
                .unwrap()
                .iter()
                .map(|(id, row)| {
                    (
                        id.clone(),
                        RuntimeAccount::new(row.account.provider().clone(), BTreeSet::new()),
                    )
                })
                .collect(),
        )),
        ClientRoutingScope::AllAccounts,
    ))
}
pub(super) fn context(scope: Arc<FrozenAccountScope>) -> AttemptContext {
    context_with_continuation(scope, None)
}
fn context_with_continuation(
    scope: Arc<FrozenAccountScope>,
    continuation: Option<gateway_core::engine::continuation::ContinuationBinding>,
) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_glm_fixture").unwrap(),
            ClientApiKeyId::new("key_glm_fixture").unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(5),
        AccountSelectionPolicy::new(
            gateway_core::account::RotationStrategy::RoundRobin,
            gateway_core::account::AccountConcurrency::new(2),
        ),
        AccountAttemptContext::new(BTreeSet::new(), None, None).with_account_scope(scope),
        continuation,
        CancellationToken::new(),
    )
}
pub(super) fn request(scope: Arc<FrozenAccountScope>, body: Value) -> ProviderRequest {
    let kind = ProviderKind::new("glm").unwrap();
    let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", body.as_object().unwrap().clone()).unwrap(),
    ));
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        gateway_core::settings::SettingsValues::new(
            2,
            "round_robin",
            Default::default(),
            None,
            None,
        ),
        vec![kind.clone()],
        vec![ProviderModel::new(
            kind,
            UpstreamModelId::new("glm-5.3").unwrap(),
            ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), Some(131072))
                .with_upstream_feature_validation()
                .with_feature(Feature::NativeContinuation, SupportLevel::Native),
        )],
        vec![],
    )
    .unwrap();
    let plan = snapshot
        .plan(
            &PublicModelId::new("glm-5.3").unwrap(),
            &operation,
            scope,
            &RoutingContext::default(),
        )
        .unwrap();
    ProviderRequest::new(operation, plan.candidates()[0].clone())
}
pub(super) fn provider(store: Arc<Store>) -> Arc<GlmProvider> {
    Arc::new(GlmProvider::new(
        ProviderKind::new("glm").unwrap(),
        store.clone(),
        store.clone(),
        store,
        Arc::new(AccountFeedbackStats::default()),
    ))
}
pub(super) fn stream(events: Vec<Value>) -> String {
    events
        .into_iter()
        .map(|v| format!("data: {v}\n\n"))
        .collect()
}
fn normal() -> Vec<Value> {
    vec![
        json!({"type":"response.created","response":{"id":"resp_glm","model":"glm-5.3"}}),
        json!({"type":"response.output_text.delta","output_index":0,"delta":"hello"}),
        json!({"type":"response.completed","response":{"id":"resp_glm","status":"completed","output":[{"type":"message","id":"msg_glm","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":10,"input_tokens_details":{"cached_tokens":3},"output_tokens":4,"output_tokens_details":{"reasoning_tokens":1}}}}),
    ]
}
pub(super) async fn setup() -> (MockServer, Arc<Store>) {
    let server = MockServer::start().await;
    let store = Arc::new(Store::default());
    store.add(
        "acct_glm",
        "glm",
        &format!("{}/api/v1", server.uri()),
        "glm-5.3",
    );
    (server, store)
}

#[tokio::test]
async fn sends_only_when_polled_and_preserves_wire_usage_and_lease() {
    let (server, store) = setup().await;
    let body = stream(normal());
    Mock::given(method("POST"))
        .and(path("/api/v1/responses"))
        .and(header("authorization", "Bearer fixture-key"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body.clone(), "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let scope = scope(&store);
    let mut result = provider(store.clone())
        .execute(
            request(
                scope.clone(),
                json!({"model":"glm-5.3","input":"test","store":true}),
            ),
            context(scope),
        )
        .await
        .unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(store.in_flight.load(Ordering::SeqCst), 1);
    let mut wire = Vec::new();
    let mut usage = None;
    let mut completed = false;
    let mut state = false;
    while let Some(event) = result.next().await {
        let event = event.unwrap();
        if let Some(w) = event.wire_event()
            && let Some(raw) = w.raw_sse_frame()
        {
            wire.extend_from_slice(raw);
        }
        for fact in event.canonical_facts() {
            match fact {
                GatewayEvent::Usage(v) => usage = Some(v.clone()),
                GatewayEvent::Completed(_) => completed = true,
                _ => {}
            }
        }
        state |= event.session_update().is_some();
    }
    assert_eq!(String::from_utf8(wire).unwrap(), body);
    let usage = usage.unwrap();
    assert_eq!(usage.cached_input_tokens, Some(3));
    assert_eq!(usage.total_tokens, None);
    assert!(completed && state);
    drop(result);
    assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
    let requests = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(sent["stream"], true);
}

#[tokio::test]
async fn preserves_tool_call_pairing_without_protocol_translation() {
    let (server, store) = setup().await;
    let mut events = normal();
    events.remove(1);
    events.insert(1,json!({"type":"response.output_item.added","output_index":0,"item":{"type":"custom_tool_call","call_id":"call_patch","name":"apply_patch"}}));
    events.insert(2,json!({"type":"response.custom_tool_call_input.delta","output_index":0,"delta":"patch body"}));
    events.last_mut().unwrap()["response"]["output"] = json!([{ "type":"custom_tool_call", "call_id":"call_patch", "name":"apply_patch", "input":"patch body" }]);
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(stream(events), "text/event-stream"))
        .mount(&server)
        .await;
    let scope = scope(&store);
    let mut result = provider(store)
        .execute(
            request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
            context(scope),
        )
        .await
        .unwrap();
    let mut calls = vec![];
    while let Some(event) = result.next().await {
        for fact in event.unwrap().canonical_facts() {
            if let GatewayEvent::ToolCallDelta(call) = fact {
                calls.push(call.clone());
            }
        }
    }
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].call_id, "call_patch");
    assert_eq!(calls[1].arguments_delta, "patch body");
}

#[tokio::test]
async fn distinguishes_balance_exhaustion_from_rate_limit_and_keeps_other_providers_untouched() {
    for (code, kind) in [
        ("1113", ProviderErrorKind::QuotaExhausted),
        ("1302", ProviderErrorKind::RateLimited),
    ] {
        let (server, store) = setup().await;
        store.add("acct_openai", "openai", "http://127.0.0.1:1/v1", "glm-5.3");
        store.add("acct_xai", "xai", "http://127.0.0.1:1/v1", "glm-5.3");
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "7")
                    .set_body_json(json!({"error":{"code":code,"message":"fixture-key"}})),
            )
            .mount(&server)
            .await;
        let scope = scope(&store);
        let mut result = provider(store.clone())
            .execute(
                request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
                context(scope),
            )
            .await
            .unwrap();
        let mut failure = None;
        while let Some(event) = result.next().await {
            if let Err(e) = event {
                failure = Some(e);
                break;
            }
        }
        let failure = failure.unwrap();
        assert_eq!(failure.kind(), kind);
        assert_eq!(failure.retry_after(), Some(Duration::from_secs(7)));
        assert!(!format!("{failure:?}").contains("fixture-key"));
        drop(result);
        assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
        let cooldowns = store.cooldowns.lock().unwrap();
        assert_eq!(cooldowns.len(), 1);
        assert!(cooldowns.keys().all(|id| id.as_str() == "acct_glm"));
    }
}

#[tokio::test]
async fn keeps_output_before_failure_in_the_same_network_chunk() {
    let (server, store) = setup().await;
    let mut events = normal();
    events.pop();
    events.push(
        json!({"type":"response.failed","response":{"id":"resp_glm","error":{"code":"1305"}}}),
    );
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(stream(events), "text/event-stream"))
        .mount(&server)
        .await;
    let scope = scope(&store);
    let mut result = provider(store)
        .execute(
            request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
            context(scope),
        )
        .await
        .unwrap();
    let mut text = false;
    let mut failed = false;
    while let Some(event) = result.next().await {
        match event {
            Ok(event) => {
                text |= event
                    .canonical_facts()
                    .iter()
                    .any(|f| matches!(f, GatewayEvent::TextDelta(_)))
            }
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    assert!(text && failed);
}

#[tokio::test]
async fn dropping_unpolled_stream_releases_lease_without_sending() {
    let (server, store) = setup().await;
    let scope = scope(&store);
    let result = provider(store.clone())
        .execute(
            request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
            context(scope),
        )
        .await
        .unwrap();
    drop(result);
    assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn missing_scope_and_unowned_continuation_never_send() {
    for continuation in [false, true] {
        let (server, store) = setup().await;
        let allowed = scope(&store);
        let mut body = json!({"model":"glm-5.3","input":"test"});
        if continuation {
            body["previous_response_id"] = json!("foreign_response");
        }
        let restricted = Arc::new(FrozenAccountScope::new(
            Arc::new(RuntimeAccountDirectory::new(BTreeMap::new())),
            ClientRoutingScope::AllAccounts,
        ));
        let error = provider(store.clone())
            .execute(request(allowed, body), context(restricted))
            .await
            .err()
            .unwrap();
        assert!(matches!(
            error.kind(),
            ProviderErrorKind::NoEligibleAccount | ProviderErrorKind::ContinuationRecoveryRequired
        ));
        assert!(server.received_requests().await.unwrap().is_empty());
        assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn rejects_truncated_stream_and_releases_resources() {
    let (server, store) = setup().await;
    let mut events = normal();
    events.pop();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(stream(events), "text/event-stream"))
        .mount(&server)
        .await;
    let scope = scope(&store);
    let mut result = provider(store.clone())
        .execute(
            request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
            context(scope),
        )
        .await
        .unwrap();
    let mut failure = None;
    while let Some(event) = result.next().await {
        if let Err(error) = event {
            failure = Some(error);
            break;
        }
    }
    assert_eq!(failure.unwrap().kind(), ProviderErrorKind::Protocol);
    drop(result);
    assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_after_headers_stops_delivery_and_releases_lease() {
    let (server, store) = setup().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(stream(normal()), "text/event-stream"),
        )
        .mount(&server)
        .await;
    let scope = scope(&store);
    let ctx = context(scope.clone());
    let cancellation = ctx.cancellation().clone();
    let mut result = provider(store.clone())
        .execute(
            request(scope, json!({"model":"glm-5.3","input":"test"})),
            ctx,
        )
        .await
        .unwrap();
    result.next().await.unwrap().unwrap();
    cancellation.cancel();
    assert_eq!(
        result.next().await.unwrap().unwrap_err().kind(),
        ProviderErrorKind::Cancelled
    );
    drop(result);
    assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn catalog_and_selection_use_only_configured_glm_models() {
    let (server, store) = setup().await;
    store.add(
        "acct_other",
        "glm",
        &format!("{}/api/v1", server.uri()),
        "glm-other",
    );
    store.add(
        "acct_openai",
        "openai",
        "http://127.0.0.1:1/v1",
        "gpt-example",
    );
    let provider = provider(store.clone());
    let catalog = provider.query_model_capabilities().await.unwrap();
    assert_eq!(catalog.len(), 2);
    assert!(
        catalog
            .iter()
            .all(|entry| entry.upstream_model().as_str().starts_with("glm-"))
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(stream(normal()), "text/event-stream"),
        )
        .mount(&server)
        .await;
    let scope = scope(&store);
    let result = provider
        .execute(
            request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
            context(scope),
        )
        .await
        .unwrap();
    assert_eq!(result.metadata().provider_account_id().as_str(), "acct_glm");
    drop(result);
}

#[tokio::test]
async fn native_continuation_stays_on_the_original_credential_revision() {
    use gateway_core::{
        account::{
            CredentialRevision, CredentialState, ProviderAccount, ProviderAccountId, QuotaState,
        },
        engine::continuation::{ContinuationBinding, NativeContinuationPin, PreviousResponseId},
        operation::ProviderSessionState,
    };
    for revision in [1, 2] {
        let (server, store) = setup().await;
        let id = ProviderAccountId::new("acct_glm").unwrap();
        store.rows.lock().unwrap().get_mut(&id).unwrap().account = ProviderAccount::new(
            id.clone(),
            ProviderKind::new("glm").unwrap(),
            "fixture".into(),
            None,
            "api_key".into(),
            CredentialRevision::new(revision).unwrap(),
            None,
        )
        .with_account_facts(
            true,
            CredentialState::Ready,
            QuotaState::unknown(),
            None,
            None,
        );
        let state = ProviderSessionState::new(
            "glm",
            json!({"credential_revision":1})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        let pin = NativeContinuationPin::new(
            PreviousResponseId::new("resp_previous"),
            PreviousResponseId::new("resp_previous"),
            ClientApiKeyId::new("key_glm_fixture").unwrap(),
            ProviderKind::new("glm").unwrap(),
            id,
        )
        .with_session_state(state);
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(stream(normal()), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let scope = scope(&store);
        let result=provider(store.clone()).execute(request(scope.clone(),json!({"model":"glm-5.3","previous_response_id":"resp_previous","input":"test","store":true})),context_with_continuation(scope,Some(ContinuationBinding::Pinned(pin)))).await;
        if revision == 1 {
            let mut result = result.unwrap();
            while let Some(event) = result.next().await {
                event.unwrap();
            }
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
        } else {
            assert_eq!(
                result.err().unwrap().kind(),
                ProviderErrorKind::ContinuationRecoveryRequired
            );
            assert!(server.received_requests().await.unwrap().is_empty());
        }
        assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn no_glm_accounts_add_no_model_or_unknown_route_candidate() {
    let store = Arc::new(Store::default());
    store.add(
        "acct_openai",
        "openai",
        "http://127.0.0.1:1/v1",
        "gpt-example",
    );
    assert!(
        provider(store)
            .query_model_capabilities()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn does_not_report_empty_completed_response_as_success() {
    let (server, store) = setup().await;
    let events = vec![
        json!({"type":"response.completed","response":{"id":"resp_empty","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
    ];
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(stream(events), "text/event-stream"))
        .mount(&server)
        .await;
    let scope = scope(&store);
    let mut result = provider(store)
        .execute(
            request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
            context(scope),
        )
        .await
        .unwrap();
    let mut failed = false;
    while let Some(event) = result.next().await {
        if let Err(e) = event {
            assert_eq!(e.kind(), ProviderErrorKind::Protocol);
            failed = true;
        }
    }
    assert!(failed);
}

#[tokio::test]
async fn busy_account_returns_capacity_error_without_sending_or_leaking_lease() {
    let (server, store) = setup().await;
    store.busy.store(true, Ordering::SeqCst);
    let scope = scope(&store);
    let failure = provider(store.clone())
        .execute(
            request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
            context(scope),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        failure.kind(),
        ProviderErrorKind::AccountCapacityUnavailable
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn adapter_selection_preserves_native_execution_and_rejects_unresolved_accounts() {
    use gateway_core::{
        account::ProviderAccountId,
        engine::upstream_adapter::{
            FrozenUpstreamAdapterPlan, UpstreamAdapterPlan, UpstreamAdapterSelection,
        },
        error::ProviderError,
        routing::extensions::{ExtensionSetId, ExtensionSetLease, ExtensionSetReference},
        upstream::UpstreamSendState,
    };

    struct ReadyLease;
    impl ExtensionSetLease for ReadyLease {
        fn is_ready(&self) -> bool {
            true
        }
    }

    #[derive(Debug)]
    struct SelectionPlan {
        pending: bool,
    }
    impl UpstreamAdapterPlan for SelectionPlan {
        fn select(
            &self,
            _: &AttemptContext,
            _: &ProviderKind,
            _: &UpstreamModelId,
            _: Option<&ProviderAccountId>,
        ) -> Result<UpstreamAdapterSelection, ProviderError> {
            Ok(if self.pending {
                UpstreamAdapterSelection::PendingAccount
            } else {
                UpstreamAdapterSelection::Native
            })
        }
    }

    for pending in [false, true] {
        let (server, store) = setup().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(stream(normal()), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let scope = scope(&store);
        let ctx = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_glm_adapter").unwrap(),
                ClientApiKeyId::new("key_glm_fixture").unwrap(),
            )
            .with_upstream_adapters(Some(FrozenUpstreamAdapterPlan::new(
                Arc::new(SelectionPlan { pending }),
                ExtensionSetReference::new(
                    ExtensionSetId::new("glm-adapter-selection".to_owned()).unwrap(),
                    Arc::new(ReadyLease),
                ),
            ))),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(5),
            AccountSelectionPolicy::new(
                gateway_core::account::RotationStrategy::RoundRobin,
                gateway_core::account::AccountConcurrency::new(2),
            ),
            AccountAttemptContext::new(BTreeSet::new(), None, None)
                .with_account_scope(scope.clone()),
            None,
            CancellationToken::new(),
        );
        let result = provider(store.clone())
            .execute(
                request(scope, json!({"model":"glm-5.3","input":"test"})),
                ctx,
            )
            .await;
        if pending {
            let failure = result.err().unwrap();
            assert_eq!(failure.kind(), ProviderErrorKind::Unsupported);
            assert_eq!(failure.send_state(), UpstreamSendState::NotSent);
            assert!(server.received_requests().await.unwrap().is_empty());
        } else {
            let mut result = result.unwrap();
            while let Some(event) = result.next().await {
                event.unwrap();
            }
            drop(result);
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
        }
        assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
    }
}

#[derive(Debug)]
struct HeaderOverrides;
impl ExtensionSetLease for HeaderOverrides {
    fn is_ready(&self) -> bool {
        true
    }
}
impl MiddlewarePlan for HeaderOverrides {
    fn handle(
        &self,
        _: MiddlewareContext,
        request: MiddlewareRequest,
        next: MiddlewareNext,
    ) -> BoxFuture<'static, Result<MiddlewareResponse, MiddlewareError>> {
        let (protocol, mut headers, body) = request.clone().into_parts();
        headers.extend([
            MiddlewareHeader::new("x-business", Bytes::from_static(b"override-first")),
            MiddlewareHeader::new("x-business", Bytes::from_static(b"override-second")),
            MiddlewareHeader::new("authorization", Bytes::from_static(b"Bearer untrusted")),
            MiddlewareHeader::new("connection", Bytes::from_static(b"x-hop")),
        ]);
        next.run(
            request
                .replace_parts(protocol, headers, body, None)
                .unwrap(),
        )
    }
}

#[tokio::test]
async fn forwards_multivalue_business_headers_and_protects_selected_credentials() {
    let (server, store) = setup().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            stream(vec![json!({
                "type":"response.completed","response":{"id":"resp_headers","status":"completed",
                    "output":[{"type":"message","content":[{"type":"output_text","text":"ok"}]}]}
            })]),
            "text/event-stream",
        ))
        .mount(&server)
        .await;
    for overrides in [false, true] {
        let scope = scope(&store);
        let initial = request(scope.clone(), json!({"model":"glm-5.3","input":"test"}));
        let entries: Vec<_> = [
            ("x-business", "original-first"),
            ("x-business", "original-second"),
            ("x-hop", "hop-secret"),
            ("authorization", "Bearer client-secret"),
            ("cookie", "client-session"),
            ("chatgpt-account-id", "client-account"),
            ("x-codex-installation-id", "client-installation"),
            ("proxy-authorization", "proxy-secret"),
        ]
        .into_iter()
        .map(|(name, value)| json!([name, STANDARD.encode(value)]))
        .collect();
        let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object(
                "openai",
                json!({"model":"glm-5.3","input":"test"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap()
            .with_context(
                json!({"opaque_request_headers":entries})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        ));
        let mut attempt = context(scope.clone());
        if overrides {
            attempt = AttemptContext::new(
                RequestAttemptContext::new(
                    ModelRequestId::new("req_glm_headers").unwrap(),
                    ClientApiKeyId::new("key_glm_fixture").unwrap(),
                )
                .with_middleware(
                    Some(FrozenMiddlewarePlan::new(
                        Arc::new(HeaderOverrides),
                        ExtensionSetReference::new(
                            ExtensionSetId::new("glm-headers".to_owned()).unwrap(),
                            Arc::new(HeaderOverrides),
                        ),
                    )),
                    Arc::from([]),
                    "/v1/responses".to_owned(),
                    ClientTransport::HttpSse,
                ),
                NonZeroU32::MIN,
                SystemTime::now() + Duration::from_secs(5),
                AccountSelectionPolicy::new(
                    gateway_core::account::RotationStrategy::RoundRobin,
                    gateway_core::account::AccountConcurrency::new(2),
                ),
                AccountAttemptContext::new(BTreeSet::new(), None, None).with_account_scope(scope),
                None,
                CancellationToken::new(),
            );
        }
        let mut response = provider(store.clone())
            .execute(
                ProviderRequest::new(operation, initial.candidate().clone()),
                attempt,
            )
            .await
            .unwrap();
        while let Some(event) = response.next().await {
            event.unwrap();
        }
        drop(response);
        let requests = server.received_requests().await.unwrap();
        let headers = &requests.last().unwrap().headers;
        assert_eq!(headers["authorization"], "Bearer fixture-key");
        let expected = if overrides {
            ["override-first", "override-second"]
        } else {
            ["original-first", "original-second"]
        };
        assert_eq!(
            headers
                .get_all("x-business")
                .iter()
                .map(|value| value.to_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        for name in [
            "cookie",
            "chatgpt-account-id",
            "x-codex-installation-id",
            "proxy-authorization",
        ] {
            assert!(!headers.contains_key(name), "leaked {name}");
        }
        assert_eq!(headers.contains_key("x-hop"), !overrides);
        assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn http_and_stream_errors_apply_the_same_credential_and_cooldown_facts() {
    for streaming in [false, true] {
        for (code, status, kind, invalid, cooldown) in [
            ("1000", 401, ProviderErrorKind::Unauthorized, true, true),
            ("1113", 429, ProviderErrorKind::QuotaExhausted, false, true),
            ("1302", 429, ProviderErrorKind::RateLimited, false, true),
            (
                "1305",
                429,
                ProviderErrorKind::UpstreamCapacityUnavailable,
                false,
                false,
            ),
            ("1200", 500, ProviderErrorKind::Unavailable, false, false),
        ] {
            let (server, store) = setup().await;
            let response = if streaming {
                ResponseTemplate::new(200).set_body_raw(
                    stream(vec![json!({"type":"error","code":code})]),
                    "text/event-stream",
                )
            } else {
                ResponseTemplate::new(status).set_body_json(json!({"error":{"code":code}}))
            }
            .insert_header("retry-after", "7");
            Mock::given(method("POST"))
                .respond_with(response)
                .mount(&server)
                .await;
            let scope = scope(&store);
            let mut response = provider(store.clone())
                .execute(
                    request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
                    context(scope),
                )
                .await
                .unwrap();
            let mut failure = None;
            while let Some(event) = response.next().await {
                if let Err(error) = event {
                    failure = Some(error);
                    break;
                }
            }
            let failure = failure.unwrap();
            assert_eq!(failure.kind(), kind, "{code} streaming={streaming}");
            assert_eq!(failure.retry_after(), Some(Duration::from_secs(7)));
            drop(response);
            let rows = store.rows.lock().unwrap();
            let account = &rows.values().next().unwrap().account;
            assert_eq!(
                account.credential_state(),
                if invalid {
                    CredentialState::Invalid
                } else {
                    CredentialState::Ready
                }
            );
            assert_eq!(!store.cooldowns.lock().unwrap().is_empty(), cooldown);
            assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn a_request_deadline_releases_the_selected_account_lease() {
    let (server, store) = setup().await;
    let received = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = received.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            seen.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_delay(Duration::from_secs(30))
        })
        .mount(&server)
        .await;
    let scope = scope(&store);
    let attempt = AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_glm_timeout").unwrap(),
            ClientApiKeyId::new("key_glm_fixture").unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(5),
        AccountSelectionPolicy::new(
            gateway_core::account::RotationStrategy::RoundRobin,
            gateway_core::account::AccountConcurrency::new(2),
        ),
        AccountAttemptContext::new(BTreeSet::new(), None, None).with_account_scope(scope.clone()),
        None,
        CancellationToken::new(),
    );
    let mut response = provider(store.clone())
        .execute(
            request(scope, json!({"model":"glm-5.3","input":"test"})),
            attempt,
        )
        .await
        .unwrap();
    assert_eq!(store.in_flight.load(Ordering::SeqCst), 1);
    assert_eq!(
        response.next().await.unwrap().unwrap_err().kind(),
        ProviderErrorKind::Timeout
    );
    drop(response);
    assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
    assert_eq!(received.load(Ordering::SeqCst), 1);
}

//! 验证智谱冷发送、原生事件、错误分类、续接和账号范围

use super::support::Store;
use futures::StreamExt;
use gateway_core::{
    account::{
        AccountFeedbackStats, AccountSelectionPolicy,
        scope::{ClientRoutingScope, FrozenAccountScope, RuntimeAccount, RuntimeAccountDirectory},
    },
    engine::{
        AccountAttemptContext, AttemptContext, ModelRequestId, RequestAttemptContext,
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

fn scope(store: &Store) -> Arc<FrozenAccountScope> {
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
fn context(scope: Arc<FrozenAccountScope>) -> AttemptContext {
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
fn request(scope: Arc<FrozenAccountScope>, body: Value) -> ProviderRequest {
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
fn provider(store: Arc<Store>) -> Arc<GlmProvider> {
    Arc::new(GlmProvider::new(
        ProviderKind::new("glm").unwrap(),
        store.clone(),
        store.clone(),
        store,
        Arc::new(AccountFeedbackStats::default()),
    ))
}
fn stream(events: Vec<Value>) -> String {
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
async fn setup() -> (MockServer, Arc<Store>) {
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

//! 验证账号映射后的实际请求、元数据和原生续接保护

use super::*;
use gateway_core::account::{AccountModelAccess, AccountModelAccessMode, AccountModelMappings};

fn mapped_request(
    operation: Operation,
    required: &str,
    continuation: Option<ContinuationBinding>,
) -> (ProviderRequest, AttemptContext) {
    mapped_request_with_requested_model(operation, required, continuation, Some("alias"))
}

fn mapped_request_with_requested_model(
    operation: Operation,
    required: &str,
    continuation: Option<ContinuationBinding>,
    requested_model: Option<&str>,
) -> (ProviderRequest, AttemptContext) {
    let provider = ProviderKind::new("openai").unwrap();
    let directory = Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
        (
            ProviderAccountId::new("acct_provider_contract").unwrap(),
            RuntimeAccount::new(provider.clone(), BTreeSet::new())
                .with_model_mappings(
                    AccountModelMappings::new(BTreeMap::from([(
                        "alias".to_owned(),
                        "gpt-5.4".to_owned(),
                    )]))
                    .unwrap(),
                )
                .with_model_access(
                    AccountModelAccess::new(
                        AccountModelAccessMode::Allowlist,
                        vec!["alias".to_owned()],
                    )
                    .unwrap(),
                ),
        ),
        (
            ProviderAccountId::new("acct_scope_new").unwrap(),
            RuntimeAccount::new(provider.clone(), BTreeSet::new())
                .with_model_mappings(
                    AccountModelMappings::new(BTreeMap::from([(
                        "alias".to_owned(),
                        "gpt-5.5".to_owned(),
                    )]))
                    .unwrap(),
                )
                .with_model_access(
                    AccountModelAccess::new(
                        AccountModelAccessMode::Allowlist,
                        vec!["alias".to_owned()],
                    )
                    .unwrap(),
                ),
        ),
    ])));
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        gateway_core::settings::SettingsValues::new(2, "smart", Default::default(), None, None),
        vec![provider.clone()],
        vec!["gpt-5.4", "gpt-5.5"]
            .into_iter()
            .map(|name| {
                ProviderModel::new(
                    provider.clone(),
                    UpstreamModelId::new(name).unwrap(),
                    ModelCapabilities::new(BTreeSet::from([operation.kind()]), Some(32_000))
                        .with_upstream_feature_validation(),
                )
            })
            .collect(),
        Vec::new(),
    )
    .unwrap()
    .with_account_directory(directory);
    let public = PublicModelId::new("alias").unwrap();
    let plan = snapshot
        .plan(
            &public,
            &operation,
            snapshot.all_account_scope(),
            &Default::default(),
        )
        .unwrap();
    let candidate = plan.candidates()[0].clone();
    let context = AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_account_mapping").unwrap(),
            ClientApiKeyId::new("key_openai_contract").unwrap(),
        )
        .with_requested_model(requested_model.map(|model| PublicModelId::new(model).unwrap())),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::new(
            BTreeSet::new(),
            Some(ProviderAccountId::new(required).unwrap()),
            None,
        )
        .with_routing_candidate(candidate.clone()),
        continuation,
        CancellationToken::new(),
    );
    (ProviderRequest::new(operation, candidate), context)
}

fn operation(previous: Option<(&str, Option<&str>)>) -> Operation {
    let mut body = json!({"model":"alias", "input":"hello", "stream":true});
    if previous.is_some() {
        body["previous_response_id"] = json!("client_previous");
    }
    let payload = ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
        .unwrap()
        .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))]));
    let mut request = GenerateRequest::from_protocol_payload(payload);
    if let Some((account, model)) = previous {
        let mut state = json!({"account_id":account,"continuation_scope":"persisted","account_model_mapping":true});
        if let Some(model) = model {
            state["upstream_model"] = json!(model);
        }
        request = request.with_provider_session_state(
            ProviderSessionState::new("openai", state.as_object().unwrap().clone()).unwrap(),
        );
    }
    Operation::Generate(request)
}

#[tokio::test]
async fn mapped_allowlists_send_their_own_unlisted_target_and_report_the_same_model() {
    let store = Arc::new(MemoryAccountStore::default());
    let first = MockServer::start().await;
    let second = MockServer::start().await;
    for (id, server) in [
        ("acct_provider_contract", &first),
        ("acct_scope_new", &second),
    ] {
        store
            .seed_api_key(
                id,
                server.uri(),
                provider_openai::credential::ResponsesTransport::Http,
            )
            .await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id":"catalog-only-model"}]
            })))
            .expect(1)
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
            )
            .expect(1)
            .mount(server)
            .await;
    }
    let leases = Arc::new(TestLeaseCoordinator::default());
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        Arc::default(),
        first.uri(),
        leases.clone(),
    );
    provider.query_model_capabilities().await.unwrap();
    for (id, server, target) in [
        ("acct_provider_contract", &first, "gpt-5.4"),
        ("acct_scope_new", &second, "gpt-5.5"),
    ] {
        let (request, context) = mapped_request(operation(None), id, None);
        let mut stream = provider.clone().execute(request, context).await.unwrap();
        assert_eq!(stream.metadata().upstream_model().unwrap().as_str(), target);
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            captured_request_body(requests.last().unwrap())["model"],
            target
        );
    }
    let (_, mapped_context) = mapped_request(operation(None), "acct_provider_contract", None);
    let scope = mapped_context.account_scope().unwrap().clone();
    let acquisitions = leases.requests.lock().unwrap().len();
    for target in ["gpt-5.4", "gpt-5.5"] {
        let payload = ProtocolPayload::json_object(
            "openai",
            json!({"model":target,"input":"hello"})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap()
        .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))]));
        let direct = Operation::Generate(GenerateRequest::from_protocol_payload(payload));
        let context = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_direct_mapped_target").unwrap(),
                ClientApiKeyId::new("key_openai_contract").unwrap(),
            )
            .with_requested_model(Some(PublicModelId::new(target).unwrap())),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(30),
            account_policy(),
            AccountAttemptContext::new(BTreeSet::new(), None, None)
                .with_account_scope(scope.clone()),
            None,
            CancellationToken::new(),
        );
        let error = provider
            .clone()
            .execute(planned_request_for_model("openai", direct, target), context)
            .await
            .err()
            .expect("target IDs are not independently authorized");
        assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
    }
    assert_eq!(leases.requests.lock().unwrap().len(), acquisitions);
}

#[tokio::test]
async fn mapped_catalog_exception_requires_the_frozen_request_name() {
    let upstream = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    store
        .seed_api_key(
            "acct_provider_contract",
            upstream.uri(),
            provider_openai::credential::ResponsesTransport::Http,
        )
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data":[{"id":"catalog-only-model"}]
        })))
        .expect(1)
        .mount(&upstream)
        .await;
    let leases = Arc::new(TestLeaseCoordinator::default());
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        Arc::default(),
        upstream.uri(),
        leases.clone(),
    );
    provider.query_model_capabilities().await.unwrap();
    for requested_model in [None, Some("unapproved-alias")] {
        let (request, context) = mapped_request_with_requested_model(
            operation(None),
            "acct_provider_contract",
            None,
            requested_model,
        );
        let error = provider
            .clone()
            .execute(request, context)
            .await
            .err()
            .expect("mapping must not borrow authorization from another request name");
        assert_eq!(error.kind(), ProviderErrorKind::NoEligibleAccount);
    }
    assert!(leases.requests.lock().unwrap().is_empty());
    assert!(
        upstream
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.method != "POST")
    );
}

#[tokio::test]
async fn mapped_native_continuation_requires_replay_when_target_changes_or_is_unknown() {
    let upstream = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    store
        .seed_api_key(
            "acct_scope_new",
            upstream.uri(),
            provider_openai::credential::ResponsesTransport::Http,
        )
        .await;
    let provider = provider_with_base_url(&store, upstream.uri());
    for previous in [Some("gpt-5.4"), None] {
        let pin = NativeContinuationPin::new(
            PreviousResponseId::new("client_previous"),
            PreviousResponseId::new("upstream_previous"),
            ClientApiKeyId::new("key_openai_contract").unwrap(),
            ProviderKind::new("openai").unwrap(),
            ProviderAccountId::new("acct_scope_new").unwrap(),
        )
        .with_scope(gateway_core::engine::continuation::NativeContinuationScope::Persisted);
        let (request, context) = mapped_request(
            operation(Some(("acct_scope_new", previous))),
            "acct_scope_new",
            Some(ContinuationBinding::Pinned(pin)),
        );
        let error = provider
            .clone()
            .execute(request, context)
            .await
            .err()
            .expect("changed target must not send old continuation");
        assert_eq!(
            error.kind(),
            ProviderErrorKind::ContinuationRecoveryRequired
        );
        assert_eq!(
            error.continuation_recovery_disposition(),
            Some(ContinuationRecoveryDisposition::ClientReplayRequired)
        );
    }
    assert!(upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn mapped_native_continuation_keeps_same_target_on_its_original_account() {
    let upstream = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    store
        .seed_api_key(
            "acct_scope_new",
            upstream.uri(),
            provider_openai::credential::ResponsesTransport::Http,
        )
        .await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
        )
        .expect(1)
        .mount(&upstream)
        .await;
    let pin = NativeContinuationPin::new(
        PreviousResponseId::new("client_previous"),
        PreviousResponseId::new("upstream_previous"),
        ClientApiKeyId::new("key_openai_contract").unwrap(),
        ProviderKind::new("openai").unwrap(),
        ProviderAccountId::new("acct_scope_new").unwrap(),
    )
    .with_scope(gateway_core::engine::continuation::NativeContinuationScope::Persisted);
    let (request, context) = mapped_request(
        operation(Some(("acct_scope_new", Some("gpt-5.5")))),
        "acct_scope_new",
        Some(ContinuationBinding::Pinned(pin)),
    );
    let mut stream = provider_with_base_url(&store, upstream.uri())
        .execute(request, context)
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let requests = upstream.received_requests().await.unwrap();
    let body = captured_request_body(requests.last().unwrap());
    assert_eq!(body["model"], "gpt-5.5");
    assert_eq!(body["previous_response_id"], "upstream_previous");
}

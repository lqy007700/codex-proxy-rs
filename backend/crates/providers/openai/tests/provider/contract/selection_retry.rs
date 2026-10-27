//! 验证真实 OpenAI Provider 经 Core 重试收尾时保留上游错误与本地会话边界

use super::session_binding::turn_request;
use super::*;
use async_trait::async_trait;
use gateway_core::engine::provider::ProviderRegistry;
use gateway_core::engine::{
    AttemptCoordinator, AttemptRecord, EngineError, ExecutionStore, GatewayEngine,
    IntermediateFailure, ModelRequestFinalization, NewModelRequest, RecoveryReport,
};
use gateway_core::error::StoreError;
use gateway_core::routing::AccountRoutingSnapshot;

struct NoopExecutionStore;

#[async_trait]
impl ExecutionStore for NoopExecutionStore {
    async fn create_model_request(&self, _: NewModelRequest) -> Result<(), StoreError> {
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

#[tokio::test]
async fn contributed_model_scope_survives_affinity_and_account_rotation() {
    use gateway_core::routing::ContributedModel;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Catalog(Vec<ContributedModel>);
    impl ExtensionSetLease for Catalog {
        fn is_ready(&self) -> bool {
            true
        }
        fn models(&self) -> &[ContributedModel] {
            &self.0
        }
    }

    for (staged_recovery, fallback_in_scope) in [(false, true), (true, true), (true, false)] {
        for native_supports_tools in [false, true] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let server = MockServer::start().await;
            let calls = AtomicUsize::new(0);
            Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(move |_: &wiremock::Request| {
                if calls.fetch_add(1, Ordering::SeqCst) < if staged_recovery { 2 } else { 1 } {
                    ResponseTemplate::new(if staged_recovery { 503 } else { 429 })
                    .insert_header("retry-after", "0")
                    .set_body_json(json!({
                        "error":{"type":if staged_recovery {"server_error"} else {"rate_limit_error"},"message":"fixture rejection"}
                    }))
                } else {
                    ResponseTemplate::new(200)
                        .set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
                }
            })
            .expect(if staged_recovery && fallback_in_scope {3} else {2})
            .mount(&server)
            .await;
            let provider = provider_with_affinity_and_base_url(
                &store,
                Arc::new(MemorySessionAffinity::default()),
                server.uri(),
            );
            let Operation::Generate(generate) = turn_request("scope-session", "scope-turn") else {
                unreachable!()
            };
            let mut body = generate.protocol_payload().body().clone();
            if native_supports_tools {
                body.insert(
                    "tools".into(),
                    json!([{
                        "type":"function", "name":"fixture", "parameters":{"type":"object"}
                    }]),
                );
            }
            let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object("openai", body)
                    .unwrap()
                    .with_context(generate.protocol_payload().context().clone()),
            ));
            // 先冻结 A 的会话亲和，再开放 B/C；后续选号不能借亲和绕过模型能力范围
            drop(
                provider
                    .clone()
                    .execute(
                        planned_request("openai", operation.clone()),
                        context("req_scope_seed", CancellationToken::new()),
                    )
                    .await
                    .unwrap(),
            );
            for account in ["acct_subagent_a", "acct_subagent_b"] {
                create_account(&store, account).await;
            }
            if staged_recovery {
                store.set_scheduling_role(
                    "acct_subagent_b",
                    gateway_core::account::AccountSchedulingRole::Fallback,
                );
            }
            let kind = ProviderKind::new("openai").unwrap();
            let scope = Arc::new(FrozenAccountScope::new(
                Arc::new(RuntimeAccountDirectory::new(
                    [
                        "acct_provider_contract",
                        "acct_subagent_a",
                        "acct_subagent_b",
                    ]
                    .into_iter()
                    .map(|id| {
                        (
                            ProviderAccountId::new(id).unwrap(),
                            RuntimeAccount::new(kind.clone(), BTreeSet::new())
                                .with_scheduling_role(
                                    if staged_recovery && id == "acct_subagent_b" {
                                        gateway_core::account::AccountSchedulingRole::Fallback
                                    } else {
                                        gateway_core::account::AccountSchedulingRole::Regular
                                    },
                                ),
                        )
                    })
                    .collect(),
                )),
                ClientRoutingScope::all_accounts(),
            ));
            let capabilities =
                ModelCapabilities::new(BTreeSet::from([OperationKind::Generate]), None);
            let models = if native_supports_tools {
                vec![ProviderModel::new(
                    kind.clone(),
                    UpstreamModelId::new("gpt-5.4").unwrap(),
                    capabilities.clone().with_upstream_feature_validation(),
                )]
            } else {
                Vec::new()
            };
            // 适配专属模型只属于 B/C；同名原生模型支持 tools 时，适配账号 A 不支持
            let contributed_accounts = if native_supports_tools {
                if fallback_in_scope {
                    vec!["acct_provider_contract"]
                } else {
                    vec!["acct_provider_contract", "acct_subagent_b"]
                }
            } else {
                if fallback_in_scope {
                    vec!["acct_subagent_a", "acct_subagent_b"]
                } else {
                    vec!["acct_subagent_a"]
                }
            };
            let snapshot = RuntimeSnapshot::new(
                ConfigRevision::new(1).unwrap(),
                SettingsValues::new(3, "smart", Default::default(), None, None)
                    .with_recovery_attempts(3),
                vec![kind.clone()],
                models,
                Vec::new(),
            )
            .unwrap()
            .with_extensions(Some(ExtensionSetReference::new(
                ExtensionSetId::new("scope-test".into()).unwrap(),
                Arc::new(Catalog(vec![ContributedModel {
                    owner: "scope-test".into(),
                    id: PublicModelId::new("gpt-5.4").unwrap(),
                    provider: kind,
                    accounts: contributed_accounts
                        .into_iter()
                        .map(|id| ProviderAccountId::new(id).unwrap())
                        .collect(),
                    capabilities,
                    presentation: Default::default(),
                }])),
            )));
            let plan = snapshot
                .plan(
                    &PublicModelId::new("gpt-5.4").unwrap(),
                    &operation,
                    scope,
                    &RoutingContext::default(),
                )
                .unwrap();
            let providers = ProviderRegistry::new([
                provider as Arc<dyn gateway_core::engine::provider::Provider>
            ])
            .unwrap();
            let coordinator = AttemptCoordinator::new(GatewayEngine::new(
                Arc::new(NoopExecutionStore),
                providers,
            ));
            let mut session = coordinator
                .start(
                    execution_request("req_scope_rotation", &operation, Duration::from_secs(10)),
                    operation,
                    plan,
                    None,
                    None,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            let result = session.collect_uncommitted().await;
            if staged_recovery && !fallback_in_scope {
                assert!(result.is_err());
            } else {
                result.unwrap();
                session.commit_downstream(Some(200)).await.unwrap();
            }
            let requests = server.received_requests().await.unwrap();
            if staged_recovery {
                let accounts: Vec<_> = requests
                    .iter()
                    .map(|request| request.headers["authorization"].to_str().unwrap())
                    .collect();
                let expected = if fallback_in_scope {
                    vec![
                        "Bearer at-acct_subagent_a",
                        "Bearer at-acct_subagent_a",
                        "Bearer at-acct_subagent_b",
                    ]
                } else {
                    vec!["Bearer at-acct_subagent_a", "Bearer at-acct_subagent_a"]
                };
                assert_eq!(
                    accounts, expected,
                    "native_supports_tools={native_supports_tools}"
                );
            } else {
                assert_eq!(requests.len(), 2);
                let selected: BTreeSet<_> = requests
                    .iter()
                    .map(|request| request.headers["authorization"].to_str().unwrap())
                    .collect();
                assert_eq!(
                    selected,
                    BTreeSet::from(["Bearer at-acct_subagent_a", "Bearer at-acct_subagent_b",]),
                    "native_supports_tools={native_supports_tools}"
                );
            }
        }
    }
}

#[tokio::test]
async fn server_busy_retries_same_account_then_rotates_with_shared_budget_after_keepalive() {
    let store = Arc::new(MemoryAccountStore::default());
    for id in [
        "acct_provider_contract",
        "acct_subagent_a",
        "acct_subagent_b",
    ] {
        create_account(&store, id).await;
    }
    let server = MockServer::start().await;
    let failure = json!({
        "type": "response.failed",
        "response": {"error": {
            "type": "service_unavailable_error", "code": "server_busy",
            "message": "Our servers are currently busy. Please try again in a moment.",
            "param": null
        }}
    });
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!("event: response.failed\ndata: {failure}\n\n"),
            "text/event-stream",
        ))
        .expect(4)
        .mount(&server)
        .await;
    let provider = provider_with_base_url(&store, server.uri());
    let providers =
        ProviderRegistry::new([provider as Arc<dyn gateway_core::engine::provider::Provider>])
            .unwrap();
    let coordinator =
        AttemptCoordinator::new(GatewayEngine::new(Arc::new(NoopExecutionStore), providers));
    let operation = http_generate_operation();
    let kind = ProviderKind::new("openai").unwrap();
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        SettingsValues::new(3, "smart", Default::default(), None, None)
            .with_request_location(global_request_location(), true),
        vec![kind.clone()],
        vec![ProviderModel::new(
            kind,
            UpstreamModelId::new("gpt-5.4").unwrap(),
            ModelCapabilities::new(BTreeSet::from([operation.kind()]), Some(32_000))
                .with_upstream_feature_validation(),
        )],
        Vec::new(),
    )
    .unwrap();
    let plan = snapshot
        .plan(
            &PublicModelId::new("gpt-5.4").unwrap(),
            &operation,
            contract_account_scope(),
            &RoutingContext::default(),
        )
        .unwrap();
    let request = NewModelRequest {
        admission_decision_ms: None,
        id: ModelRequestId::new("req_server_busy_budget").unwrap(),
        client_api_key_name_snapshot: None,
        client_api_key_ref: ClientApiKeyId::new("key_openai_contract").unwrap(),
        config_revision: ConfigRevision::new(1).unwrap(),
        routing: AccountRoutingSnapshot::all(),
        protocol: "openai".to_owned(),
        operation: operation.kind(),
        endpoint: "responses".to_owned(),
        client_transport: "http_sse".to_owned(),
        requested_model: Some(PublicModelId::new("gpt-5.4").unwrap()),
        client_ip: None,
        user_agent: None,
        reasoning_effort: None,
        reasoning_preset: None,
        request_kind: None,
        subagent_kind: None,
        compact: false,
        continuation: Default::default(),
        image_generation_requested: false,
        started_at: SystemTime::now(),
        deadline_at: (SystemTime::now() + Duration::from_secs(30)).into(),
    };
    let mut session = coordinator
        .start(
            request,
            operation,
            plan,
            None,
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    // 保活已提交响应头，但没有业务输出，不能阻止这类明确拒绝的有界恢复
    session.response_commit().record(Some(200));
    let EngineError::Provider(error) = session.collect_uncommitted().await.unwrap_err() else {
        panic!("expected final upstream failure");
    };
    assert_eq!(error.upstream_code().unwrap().as_str(), "server_busy");
    assert_eq!(
        serde_json::from_str::<Value>(error.raw_upstream_error().unwrap().as_str()).unwrap(),
        failure
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    let accounts: Vec<_> = requests
        .iter()
        .map(|request| request.headers.get("chatgpt-account-id").unwrap())
        .collect();
    assert!(accounts[..2].iter().all(|account| *account == accounts[0]));
    assert_ne!(accounts[2], accounts[0]);
    assert_ne!(accounts[3], accounts[0]);
    assert_ne!(accounts[3], accounts[2]);
    let trace = session.trace().snapshot().unwrap();
    let decisions: Vec<_> = trace["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["stage"] == "retry.decided")
        .map(|event| &event["data"])
        .collect();
    assert_eq!(decisions.len(), 4);
    for (index, decision) in decisions.iter().enumerate() {
        assert_eq!(decision["boundedTransientRetriesRemaining"], 4 - index);
        assert_eq!(decision["retryable"], index < 3);
        assert_eq!(decision["downstreamCommitted"], true);
    }
}

#[tokio::test]
async fn cyber_policy_rotation_preserves_upstream_response_and_next_request_stays_local() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    let server = MockServer::start().await;
    let body = json!({"error":{"code":"cyber_policy","type":"invalid_request_error","message":"synthetic policy rejection"}});
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(400).set_body_json(&body))
        .mount(&server)
        .await;
    let provider = provider_with_affinity_and_base_url(
        &store,
        Arc::new(MemorySessionAffinity::default()),
        server.uri(),
    );
    let providers =
        ProviderRegistry::new([provider as Arc<dyn gateway_core::engine::provider::Provider>])
            .unwrap();
    let coordinator =
        AttemptCoordinator::new(GatewayEngine::new(Arc::new(NoopExecutionStore), providers));
    let operation = turn_request("policy-session", "policy-turn");
    let plan = execution_plan(&operation);
    let request = |id| execution_request(id, &operation, Duration::from_secs(10));
    let mut session = coordinator
        .start(
            request("req_policy_first"),
            operation.clone(),
            plan.clone(),
            None,
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let error = session
        .collect_uncommitted()
        .await
        .expect_err("policy rejection must surface");
    let EngineError::Provider(error) = error else {
        panic!("expected Provider error: {error:?}")
    };
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(error.upstream_status(), Some(400));
    assert_eq!(
        error.client_visible_upstream_error().unwrap().message(),
        "synthetic policy rejection"
    );
    let response = error
        .client_visible_upstream_response()
        .expect("original HTTP rejection must survive empty account selection");
    assert_eq!(
        serde_json::from_slice::<Value>(response.body()).unwrap(),
        body
    );
    assert_eq!(response.status(), 400);
    // 同一会话的新请求没有可恢复的上游失败，只返回本地策略提示
    let mut next = coordinator
        .start(
            request("req_policy_next"),
            operation.clone(),
            plan,
            None,
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let EngineError::Provider(error) = next.collect_uncommitted().await.unwrap_err() else {
        panic!("expected local selection failure");
    };
    assert_eq!(error.kind(), ProviderErrorKind::SessionPolicyRestricted);
    assert!(error.retry_is_prohibited());
    assert_eq!(error.upstream_status(), None);
    assert!(error.client_visible_upstream_response().is_none());
    assert_eq!(
        error.client_visible_upstream_error().unwrap().code(),
        Some("cyber_policy")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn streaming_not_found_preserves_accounts_and_stops_gateway_retries() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let raw = json!({
        "type": "error", "status": 404,
        "error": {
            "type": "invalid_request_error", "code": null, "param": "input",
            "message": "Item with id 'rs_missing' not found. Items are not persisted when `store` is set to false. Try again with `store` set to true, or remove this item from your input."
        }
    })
    .to_string();
    let created = json!({
        "type": "response.created",
        "response": {"id": "resp_missing_item", "model": "gpt-5.4", "output": []}
    })
    .to_string();
    for websocket in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let requests = Arc::new(AtomicUsize::new(0));
        let upstream_requests = Arc::clone(&requests);
        let http = MockServer::start().await;
        let (base_url, server) = if websocket {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let created = created.clone();
            let raw = raw.clone();
            let server = tokio::spawn(async move {
                loop {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut ws = accept_codex_test_websocket(socket).await;
                    while let Some(Ok(message)) = ws.next().await {
                        match message {
                            Message::Text(_) => {
                                let count = upstream_requests.fetch_add(1, Ordering::SeqCst) + 1;
                                ws.send(Message::Text(created.clone().into()))
                                    .await
                                    .unwrap();
                                ws.send(Message::Ping(Bytes::new())).await.unwrap();
                                ws.send(Message::Text(raw.clone().into())).await.unwrap();
                                if count == 3 {
                                    return;
                                }
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                }
            });
            (base_url, Some(server))
        } else {
            let response = ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {created}\n\n: keepalive\n\ndata: {raw}\n\n"));
            Mock::given(method("POST"))
                .and(path("/codex/responses"))
                .respond_with(move |_: &wiremock::Request| {
                    upstream_requests.fetch_add(1, Ordering::SeqCst);
                    response.clone()
                })
                .mount(&http)
                .await;
            (http.uri(), None)
        };
        let provider = provider_with_base_url(&store, base_url);
        let providers =
            ProviderRegistry::new([provider as Arc<dyn gateway_core::engine::provider::Provider>])
                .unwrap();
        let coordinator =
            AttemptCoordinator::new(GatewayEngine::new(Arc::new(NoopExecutionStore), providers));
        let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object(
                "openai",
                Map::from_iter([
                    ("model".to_owned(), json!("gpt-5.4")),
                    (
                        "input".to_owned(),
                        json!([{"type": "item_reference", "id": "rs_missing"}]),
                    ),
                    ("store".to_owned(), json!(false)),
                ]),
            )
            .unwrap()
            .with_context(Map::from_iter([(
                "use_websocket".to_owned(),
                json!(websocket),
            )])),
        ));
        let plan = execution_plan(&operation);
        // 重复请求必须继续使用同一可调度账号，不能把历史条目缺失累计为凭据失效
        for index in 1..=3 {
            let mut session = coordinator
                .start(
                    execution_request(
                        &format!("req_missing_item_{index}"),
                        &operation,
                        Duration::from_secs(5),
                    ),
                    operation.clone(),
                    plan.clone(),
                    None,
                    None,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            let EngineError::Provider(error) =
                timeout(Duration::from_secs(5), session.collect_uncommitted())
                    .await
                    .unwrap()
                    .unwrap_err()
            else {
                panic!("expected the original upstream rejection");
            };
            assert_eq!(
                error.kind(),
                ProviderErrorKind::InvalidRequest,
                "websocket={websocket}"
            );
            assert_eq!(error.upstream_status(), Some(404));
            assert!(!error.replay_is_safe());
            assert!(error.retry_is_prohibited());
            assert_eq!(error.raw_upstream_error().unwrap().as_str(), raw);
            assert_eq!(requests.load(Ordering::SeqCst), index);
            let account = store.account("acct_provider_contract").unwrap();
            assert_eq!(account.credential_state(), CredentialState::Ready);
            assert_eq!(account.last_error_reason(), None);
        }
        if let Some(server) = server {
            timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
        }
    }
}

#[tokio::test]
async fn stalled_http_proxy_retries_same_account_before_using_reserved_rotation_budget() {
    stalled_proxy_recovery(false).await;
}

#[tokio::test]
async fn stalled_pooled_websocket_and_http_leave_one_opening_for_rotation() {
    stalled_proxy_recovery(true).await;
}

async fn stalled_proxy_recovery(websocket: bool) {
    use provider_openai::credential::ResponsesTransport;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_subagent_a").await;
    create_account_with_enabled(&store, "acct_subagent_b", false).await;
    if !websocket {
        store.set_oauth_transport("acct_subagent_a", ResponsesTransport::Http);
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    store.set_egress(
        "acct_subagent_a",
        Some(
            gateway_core::account::OutboundProxy::parse(&format!(
                "socks5h://{}",
                listener.local_addr().unwrap()
            ))
            .unwrap(),
        ),
        None,
    );
    let openings = Arc::new(AtomicUsize::new(0));
    let proxy_openings = Arc::clone(&openings);
    let proxy_store = Arc::clone(&store);
    let proxy = tokio::spawn(async move {
        let mut sockets = Vec::new();
        loop {
            sockets.push(listener.accept().await.unwrap().0);
            if proxy_openings.fetch_add(1, Ordering::SeqCst) == 0 {
                // A 已选定后再开放 B，避免测试依赖选号权重或随机顺序
                proxy_store
                    .set_enabled(&ProviderAccountId::new("acct_subagent_b").unwrap(), true)
                    .await
                    .unwrap();
            }
        }
    });
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(CAPTURE_COMPLETED_SSE),
        )
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider_with_base_url(&store, server.uri());
    let providers =
        ProviderRegistry::new([provider as Arc<dyn gateway_core::engine::provider::Provider>])
            .unwrap();
    let coordinator =
        AttemptCoordinator::new(GatewayEngine::new(Arc::new(NoopExecutionStore), providers));
    let operation = Operation::Generate(generate_with_session_context(
        "budget-session",
        Some("budget-session"),
        None,
    ));
    let plan = execution_plan(&operation);
    let started = std::time::Instant::now();
    let mut session = coordinator
        .start(
            execution_request("req_reserved_rotation", &operation, Duration::from_secs(40)),
            operation.clone(),
            plan,
            None,
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let result = timeout(Duration::from_secs(30), session.collect_uncommitted()).await;
    let elapsed = started.elapsed();
    proxy.abort();
    let _ = proxy.await;
    result
        .expect("rotation must finish within the original startup window")
        .unwrap();
    let expected_openings = if websocket { 3 } else { 2 };
    assert_eq!(openings.load(Ordering::SeqCst), expected_openings);
    assert!(
        elapsed >= Duration::from_secs(20),
        "A must get its own retry: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(28),
        "B must retain useful time: {elapsed:?}"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "B must skip a fresh WS opening");
    assert_eq!(requests[0].method, "POST");
    eprintln!(
        "stalled proxy: websocket={websocket}, A openings={expected_openings}, one B payload, elapsed={elapsed:?}"
    );
}

fn execution_plan(operation: &Operation) -> gateway_core::routing::RoutingPlan {
    let kind = ProviderKind::new("openai").unwrap();
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        SettingsValues::new(3, "smart", Default::default(), None, None)
            .with_request_location(global_request_location(), true),
        vec![kind.clone()],
        vec![ProviderModel::new(
            kind,
            UpstreamModelId::new("gpt-5.4").unwrap(),
            ModelCapabilities::new(BTreeSet::from([operation.kind()]), Some(32_000))
                .with_upstream_feature_validation(),
        )],
        Vec::new(),
    )
    .unwrap();
    snapshot
        .plan(
            &PublicModelId::new("gpt-5.4").unwrap(),
            operation,
            contract_account_scope(),
            &RoutingContext::default(),
        )
        .unwrap()
}

fn execution_request(id: &str, operation: &Operation, timeout: Duration) -> NewModelRequest {
    NewModelRequest {
        admission_decision_ms: None,
        id: ModelRequestId::new(id).unwrap(),
        client_api_key_name_snapshot: None,
        client_api_key_ref: ClientApiKeyId::new("key_openai_contract").unwrap(),
        config_revision: ConfigRevision::new(1).unwrap(),
        routing: AccountRoutingSnapshot::all(),
        protocol: "openai".to_owned(),
        operation: operation.kind(),
        endpoint: "responses".to_owned(),
        client_transport: "http_sse".to_owned(),
        requested_model: Some(PublicModelId::new("gpt-5.4").unwrap()),
        client_ip: None,
        user_agent: None,
        reasoning_effort: None,
        reasoning_preset: None,
        request_kind: None,
        subagent_kind: None,
        compact: false,
        continuation: Default::default(),
        image_generation_requested: false,
        started_at: SystemTime::now(),
        deadline_at: (SystemTime::now() + timeout).into(),
    }
}

#[tokio::test]
async fn repeated_stream_not_found_errors_do_not_invalidate_accounts_or_retry() {
    for websocket in [false, true] {
        for response_failed in [false, true] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let error = json!({
                "type": "invalid_request_error",
                "code": null,
                "message": "Item with id 'rs_test_missing' not found. Items are not persisted when `store` is set to false.",
                "param": "input"
            });
            let failure = if response_failed {
                json!({"type":"response.failed","status":404,"response":{"id":"resp_missing","status":"failed","error":error}})
            } else {
                json!({"type":"error","status":404,"error":error})
            };
            let events = vec![
                json!({"type":"response.created","response":{"id":"resp_missing","status":"in_progress","output":[]}}),
                json!({"type":"keepalive"}),
                failure.clone(),
            ];
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                for _ in 0..3 {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    if websocket {
                        let mut socket = accept_codex_test_websocket(socket).await;
                        socket.next().await.unwrap().unwrap();
                        for event in &events {
                            socket
                                .send(Message::Text(event.to_string().into()))
                                .await
                                .unwrap();
                        }
                    } else {
                        read_http_request(&mut socket).await;
                        let body = events
                            .iter()
                            .map(|event| {
                                gateway_protocol::openai::sse::encode_sse_event(
                                    event["type"].as_str().unwrap(),
                                    &event.to_string(),
                                )
                            })
                            .collect::<String>();
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                    }
                }
            });
            let provider = provider_with_base_url(&store, base_url);
            let providers = ProviderRegistry::new([
                provider as Arc<dyn gateway_core::engine::provider::Provider>
            ])
            .unwrap();
            let coordinator = AttemptCoordinator::new(GatewayEngine::new(
                Arc::new(NoopExecutionStore),
                providers,
            ));
            let operation = if websocket {
                capacity_websocket_operation()
            } else {
                http_generate_operation()
            };
            for index in 0..3 {
                let mut session = coordinator
                    .start(
                        execution_request(
                            &format!("req_stream_not_found_{index}"),
                            &operation,
                            Duration::from_secs(5),
                        ),
                        operation.clone(),
                        execution_plan(&operation),
                        None,
                        None,
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                session.response_commit().record(Some(200));
                let EngineError::Provider(error) = session.collect_uncommitted().await.unwrap_err()
                else {
                    panic!("expected original request error");
                };
                assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
                assert_eq!(error.upstream_status(), Some(404));
                assert!(error.retry_is_prohibited());
                assert!(!error.replay_is_safe());
                assert_eq!(
                    serde_json::from_str::<Value>(error.raw_upstream_error().unwrap().as_str())
                        .unwrap(),
                    failure
                );
                let trace = session.trace().snapshot().unwrap();
                let attempts = trace["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|event| event["stage"] == "attempt.started")
                    .count();
                assert_eq!(attempts, 1, "request errors must not rotate accounts");
                assert_eq!(
                    store
                        .account("acct_provider_contract")
                        .unwrap()
                        .credential_state(),
                    CredentialState::Ready
                );
            }
            timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
        }
    }
}

#[tokio::test]
async fn recovery_round_robin_uses_api_key_fallback_once_and_preserves_session_binding() {
    use gateway_core::account::AccountSchedulingRole;
    use std::sync::atomic::{AtomicUsize, Ordering};
    for (regular, fallback, failures, scenario, attempts, expected) in [
        (vec!["a", "b", "c"], true, 99, vec!["a", "a", "b", "c", "d"]),
        (vec!["a", "b"], true, 99, vec!["a", "a", "b", "b", "d"]),
        (vec!["a"], true, 99, vec!["a", "a", "d"]),
        (vec![], true, 99, vec!["d"]),
        (vec!["a", "b", "c"], false, 99, vec!["a", "a", "b", "c"]),
        (vec!["a"], false, 99, vec!["a", "a"]),
        (vec![], false, 99, vec![]),
        (vec!["a", "b", "c"], true, 0, vec!["a"]),
        (vec!["a", "b", "c"], true, 3, vec!["a", "a", "b", "c"]),
        (vec!["a", "b", "c"], true, 4, vec!["a", "a", "b", "c", "d"]),
    ]
    .into_iter()
    .map(|(regular, fallback, failures, expected)| {
        (regular, fallback, failures, "available", expected)
    })
    .chain([
        (
            vec!["a", "b", "c"],
            true,
            99,
            "disabled",
            vec!["a", "a", "b", "c"],
        ),
        (
            vec!["a", "b", "c"],
            true,
            99,
            "model_denied",
            vec!["a", "a", "b", "c"],
        ),
        (
            vec!["a", "b", "c"],
            true,
            99,
            "outside_scope",
            vec!["a", "a", "b", "c"],
        ),
        (vec!["a", "b", "c"], true, 99, "bad_request", vec!["a"]),
    ])
    .map(|(regular, fallback, failures, scenario, expected)| {
        (regular, fallback, failures, scenario, 4, expected)
    })
    .chain([
        (vec!["a", "b"], true, 99, "available", 1, vec!["a", "d"]),
        (
            vec!["a", "b"],
            true,
            99,
            "available",
            2,
            vec!["a", "b", "d"],
        ),
        (
            vec!["a", "b"],
            true,
            99,
            "available",
            3,
            vec!["a", "a", "b", "d"],
        ),
        (
            vec!["a", "b"],
            true,
            99,
            "available",
            5,
            vec!["a", "a", "a", "b", "b", "d"],
        ),
        (
            vec!["a", "b"],
            true,
            99,
            "available",
            7,
            vec!["a", "a", "a", "a", "b", "b", "b", "d"],
        ),
        (
            vec!["a", "b", "c"],
            true,
            99,
            "available",
            7,
            vec!["a", "a", "a", "a", "b", "c", "b", "d"],
        ),
        (
            vec!["a"],
            true,
            99,
            "available",
            5,
            vec!["a", "a", "a", "d"],
        ),
        (
            vec!["a", "b"],
            false,
            99,
            "available",
            3,
            vec!["a", "a", "b"],
        ),
    ]) {
        let store = Arc::new(MemoryAccountStore::default());
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&calls);
        Mock::given(method("POST")).respond_with(move |_: &wiremock::Request| {
            let index = responder_calls.fetch_add(1, Ordering::SeqCst);
            if index < failures {
                ResponseTemplate::new(if scenario == "bad_request" { 400 } else { 503 }).insert_header("retry-after", "0")
                    .set_body_json(json!({"error":{"code":if scenario == "bad_request" { "invalid_request_error" } else { "server_error" },"message":format!("failure-{index}")}}))
            } else {
                ResponseTemplate::new(200).insert_header("content-type", "text/event-stream")
                    .set_body_string(CAPTURE_COMPLETED_SSE)
            }
        }).mount(&server).await;
        let kind = ProviderKind::new("openai").unwrap();
        let mut directory = BTreeMap::new();
        for name in &regular {
            let id = format!("acct_{name}");
            create_account(&store, &id).await;
            store.set_oauth_transport(&id, provider_openai::credential::ResponsesTransport::Http);
            directory.insert(
                ProviderAccountId::new(id).unwrap(),
                RuntimeAccount::new(kind.clone(), BTreeSet::new()),
            );
        }
        if fallback {
            store
                .seed_api_key(
                    "acct_d",
                    format!("{}/v1", server.uri()),
                    provider_openai::credential::ResponsesTransport::Http,
                )
                .await;
            store.set_scheduling_role("acct_d", AccountSchedulingRole::Fallback);
            if scenario == "disabled" {
                store
                    .set_enabled(&ProviderAccountId::new("acct_d").unwrap(), false)
                    .await
                    .unwrap();
            }

            directory.insert(
                ProviderAccountId::new("acct_d").unwrap(),
                RuntimeAccount::new(kind.clone(), BTreeSet::new())
                    .with_scheduling_role(AccountSchedulingRole::Fallback)
                    .with_model_access(if scenario == "model_denied" {
                        gateway_core::account::AccountModelAccess::new(
                            gateway_core::account::AccountModelAccessMode::Denylist,
                            vec!["gpt-5.4".into()],
                        )
                        .unwrap()
                    } else {
                        Default::default()
                    }),
            );
        }
        if scenario == "outside_scope" {
            directory.remove(&ProviderAccountId::new("acct_d").unwrap());
        }
        let scope = Arc::new(FrozenAccountScope::new(
            Arc::new(RuntimeAccountDirectory::new(directory)),
            ClientRoutingScope::all_accounts(),
        ));
        let affinity = Arc::new(MemorySessionAffinity::default());
        let provider = provider_with_affinity_and_base_url(&store, affinity, server.uri());
        let providers =
            ProviderRegistry::new([provider as Arc<dyn gateway_core::engine::provider::Provider>])
                .unwrap();
        let coordinator =
            AttemptCoordinator::new(GatewayEngine::new(Arc::new(NoopExecutionStore), providers));
        let operation = Operation::Generate(generate_with_session_context(
            "recovery-root",
            Some("recovery-root"),
            None,
        ));
        let snapshot = RuntimeSnapshot::new(
            ConfigRevision::new(1).unwrap(),
            SettingsValues::new(2, "sticky", Default::default(), None, None)
                .with_request_location(global_request_location(), true)
                .with_recovery_attempts(attempts),
            vec![kind.clone()],
            vec![ProviderModel::new(
                kind,
                UpstreamModelId::new("gpt-5.4").unwrap(),
                ModelCapabilities::new(BTreeSet::from([operation.kind()]), Some(32_000))
                    .with_upstream_feature_validation(),
            )],
            Vec::new(),
        )
        .unwrap();
        let plan = snapshot.plan(
            &PublicModelId::new("gpt-5.4").unwrap(),
            &operation,
            scope,
            &RoutingContext::default(),
        );
        if expected.is_empty() {
            assert!(plan.is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            continue;
        }
        let plan = plan.unwrap();
        let mut session = coordinator
            .start(
                execution_request("req_recovery_matrix", &operation, Duration::from_secs(30)),
                operation.clone(),
                plan.clone(),
                None,
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let result = session.collect_uncommitted().await;
        assert_eq!(
            result.is_ok(),
            failures < expected.len(),
            "case {regular:?}, fallback={fallback}, failures={failures}: {result:?}"
        );
        if let Err(EngineError::Provider(error)) = &result {
            let response = error
                .client_visible_upstream_response()
                .expect("last original upstream response");
            let body: Value = serde_json::from_slice(response.body()).unwrap();
            assert_eq!(
                body["error"]["message"],
                format!("failure-{}", expected.len() - 1)
            );
            assert_eq!(
                error.upstream_status(),
                Some(if scenario == "bad_request" { 400 } else { 503 })
            );
        }
        let requests = server.received_requests().await.unwrap();
        let sequence = requests
            .iter()
            .map(|request| {
                request
                    .headers
                    .get("chatgpt-account-id")
                    .map_or("d".to_owned(), |value| {
                        value
                            .to_str()
                            .unwrap()
                            .trim_start_matches("chatgpt-acct_")
                            .to_owned()
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            sequence, expected,
            "case {regular:?}, fallback={fallback}, failures={failures}"
        );
        if failures < expected.len() {
            session.commit_downstream(Some(200)).await.unwrap();
        }
        drop(session);
        // 保底和临时轮询不能迁移根会话主绑定，下一请求仍回到 A
        if !regular.is_empty() {
            calls.store(100, Ordering::SeqCst);
            let mut next = coordinator
                .start(
                    execution_request("req_recovery_next", &operation, Duration::from_secs(30)),
                    operation,
                    plan,
                    None,
                    None,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            next.collect_uncommitted().await.unwrap();
            next.commit_downstream(Some(200)).await.unwrap();
            let requests = server.received_requests().await.unwrap();
            assert_eq!(
                requests
                    .last()
                    .unwrap()
                    .headers
                    .get("chatgpt-account-id")
                    .unwrap(),
                "chatgpt-acct_a"
            );
            // 同一主账号重试及下一请求复用原有会话指纹
            let first = requests.first().unwrap().headers.get("session-id");
            assert_eq!(requests.last().unwrap().headers.get("session-id"), first);
        }
    }
}

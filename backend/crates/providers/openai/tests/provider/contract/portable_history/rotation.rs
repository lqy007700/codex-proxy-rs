//! 验证请求内历史转换跨账号保底重试、成功缓存归属与原账号目标变化拒绝

use super::*;
use gateway_core::account::AccountSchedulingRole;
use gateway_core::engine::recovery::AccountRecoveryStage;

type CapturedAttempts = Arc<Mutex<Vec<(bool, Value)>>>;

struct HistoryUpstream {
    base: String,
    _http: MockServer,
    websocket: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for HistoryUpstream {
    fn drop(&mut self) {
        if let Some(task) = &self.websocket {
            task.abort();
        }
    }
}

fn terminal(index: usize, fallback: bool, reject_fallback: bool) -> Value {
    if (!fallback && index == 0) || (fallback && reject_fallback) {
        json!({"type":"error","status":400,"error":{"code":"invalid_encrypted_content","type":"invalid_request_error","message":"fixture encrypted rejection"}})
    } else if !fallback && index == 1 {
        json!({"type":"error","error":{"code":"server_is_overloaded","type":"service_unavailable_error","message":"fixture overloaded","headers":{"retry-after":"321","retry-after-ms":"321000"}}})
    } else {
        json!({"type":"response.completed","response":{"id":"resp_rotation_history","model":"gpt-5.4","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}})
    }
}

impl HistoryUpstream {
    async fn start(
        websocket: bool,
        fallback: bool,
        reject_fallback: bool,
        captured: CapturedAttempts,
    ) -> Self {
        let http = MockServer::start().await;
        if !websocket {
            let calls = AtomicUsize::new(0);
            Mock::given(method("POST"))
                .respond_with(move |request: &wiremock::Request| {
                    captured
                        .lock()
                        .unwrap()
                        .push((fallback, captured_request_body(request)));
                    let event = terminal(
                        calls.fetch_add(1, Ordering::SeqCst),
                        fallback,
                        reject_fallback,
                    );
                    ResponseTemplate::new(200).set_body_raw(
                        format!(
                            "event: {}\ndata: {event}\n\n",
                            event["type"].as_str().unwrap()
                        ),
                        "text/event-stream",
                    )
                })
                .mount(&http)
                .await;
            return Self {
                base: http.uri(),
                _http: http,
                websocket: None,
            };
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let captured = captured.clone();
                        let calls = calls.clone();
                        connections.spawn(async move {
                            let mut socket = accept_codex_test_websocket(stream).await;
                            while let Some(Ok(Message::Text(frame))) = socket.next().await {
                                captured.lock().unwrap().push((fallback, serde_json::from_str(&frame).unwrap()));
                                let event = terminal(calls.fetch_add(1, Ordering::SeqCst), fallback, reject_fallback);
                                socket.send(Message::Text(event.to_string().into())).await.unwrap();
                            }
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Self {
            base,
            _http: http,
            websocket: Some(task),
        }
    }
}

fn rotation_scope() -> Arc<FrozenAccountScope> {
    let provider = ProviderKind::new("openai").unwrap();
    Arc::new(
        FrozenAccountScope::new(
            Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
                (
                    ProviderAccountId::new("acct_provider_contract").unwrap(),
                    RuntimeAccount::new(provider.clone(), BTreeSet::new()),
                ),
                (
                    ProviderAccountId::new("acct_history_fallback").unwrap(),
                    RuntimeAccount::new(provider, BTreeSet::new())
                        .with_scheduling_role(AccountSchedulingRole::Fallback),
                ),
            ]))),
            ClientRoutingScope::all_accounts(),
        )
        .with_portable_history(true),
    )
}

fn fixed_history_context(account: &str, caller: &str) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_history_cache_target").unwrap(),
            ClientApiKeyId::new(caller).unwrap(),
        ),
        NonZeroU32::new(1).unwrap(),
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::new(
            BTreeSet::new(),
            Some(ProviderAccountId::new(account).unwrap()),
            None,
        )
        .with_account_scope(rotation_scope())
        .with_recovery_stage(if account == "acct_history_fallback" {
            AccountRecoveryStage::Fallback
        } else {
            AccountRecoveryStage::Initial
        }),
        None,
        CancellationToken::new(),
    )
}

#[tokio::test]
async fn portable_history_conversion_survives_overload_rotation_to_api_key_fallback() {
    for websocket in [false, true] {
        for reject_fallback in [false, true] {
            let captured = Arc::new(Mutex::new(Vec::new()));
            let initial = HistoryUpstream::start(websocket, false, false, captured.clone()).await;
            let fallback =
                HistoryUpstream::start(websocket, true, reject_fallback, captured.clone()).await;
            let accounts = Arc::new(MemoryAccountStore::default());
            create_account(&accounts, "acct_provider_contract").await;
            let transport = if websocket {
                provider_openai::credential::ResponsesTransport::PreferWebsocket
            } else {
                provider_openai::credential::ResponsesTransport::Http
            };
            accounts.set_oauth_transport("acct_provider_contract", transport);
            accounts
                .seed_api_key("acct_history_fallback", fallback.base.clone(), transport)
                .await;
            accounts.set_scheduling_role("acct_history_fallback", AccountSchedulingRole::Fallback);
            let cache = Arc::new(HistoryCache::default());
            let provider = with_cache(
                provider_with_base_url(&accounts, initial.base.clone()),
                cache.clone(),
            );
            let mut body = extended_history();
            body["thread_id"] = json!("thread-history-rotation");
            let operation = history_operation(body.clone(), websocket, false);
            let kind = ProviderKind::new("openai").unwrap();
            let snapshot = RuntimeSnapshot::new(
                ConfigRevision::new(1).unwrap(),
                SettingsValues::new(2, "sticky", Default::default(), None, None)
                    .with_recovery_attempts(2)
                    .with_recovery_rotation_attempts(0)
                    .with_max_retry_after_seconds(240),
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
                    rotation_scope(),
                    &RoutingContext::default(),
                )
                .unwrap();
            let coordinator = AttemptCoordinator::new(GatewayEngine::new(
                Arc::new(super::super::selection_retry::NoopExecutionStore),
                ProviderRegistry::new([provider as Arc<dyn Provider>]).unwrap(),
            ));
            let result = timeout(Duration::from_secs(10), async {
                coordinator
                    .start(
                        super::super::selection_retry::execution_request(
                            "req_history_rotation",
                            &operation,
                            Duration::from_secs(30),
                        ),
                        operation,
                        plan,
                        None,
                        None,
                        CancellationToken::new(),
                    )
                    .await?
                    .collect_uncommitted()
                    .await
            })
            .await
            .unwrap();
            assert_eq!(
                result.is_ok(),
                !reject_fallback,
                "ws={websocket}: {result:?}"
            );
            let expected = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["id"] != "rs_channel_a" && item["id"] != "rs_channel_b")
                .cloned()
                .collect::<Vec<_>>();
            {
                let attempts = captured.lock().unwrap();
                assert_eq!(
                    attempts.iter().map(|(b, _)| *b).collect::<Vec<_>>(),
                    [false, false, true]
                );
                assert_eq!(attempts[0].1["input"], body["input"]);
                for (_, replay) in &attempts[1..] {
                    assert_eq!(replay["input"], json!(expected));
                    assert!(
                        replay["client_metadata"]
                            .get("x-codex-turn-state")
                            .is_none()
                    );
                    for field in ["tools", "instructions", "future_field"] {
                        assert_eq!(replay[field], body[field]);
                    }
                }
            }
            assert_eq!(
                cache.writes.load(Ordering::SeqCst),
                usize::from(!reject_fallback)
            );
            assert_eq!(
                cache.values.lock().unwrap().len(),
                usize::from(!reject_fallback)
            );
            if reject_fallback {
                continue;
            }
            // 重新创建 Provider 清除过载冷却，只复用历史缓存来核对成功目标与调用方隔离
            let provider = with_cache(
                provider_with_base_url(&accounts, initial.base.clone()),
                cache.clone(),
            );
            for (account, caller, thread, filtered) in [
                (
                    "acct_history_fallback",
                    "key_openai_contract",
                    "thread-history-rotation",
                    true,
                ),
                (
                    "acct_provider_contract",
                    "key_openai_contract",
                    "thread-history-rotation",
                    false,
                ),
                (
                    "acct_history_fallback",
                    "key_other_caller",
                    "thread-history-rotation",
                    false,
                ),
                (
                    "acct_history_fallback",
                    "key_openai_contract",
                    "thread-other",
                    false,
                ),
            ] {
                let mut followup = body.clone();
                followup["thread_id"] = json!(thread);
                let mut events = provider
                    .clone()
                    .execute(
                        planned_request("openai", history_operation(followup, websocket, false)),
                        fixed_history_context(account, caller),
                    )
                    .await
                    .unwrap();
                while let Some(event) = events.next().await {
                    event.unwrap();
                }
                let actual = captured.lock().unwrap().last().unwrap().1["input"].clone();
                assert_eq!(
                    actual,
                    if filtered {
                        json!(expected)
                    } else {
                        body["input"].clone()
                    },
                    "account={account} caller={caller} thread={thread}"
                );
            }
            assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn portable_history_same_account_recovery_rejects_changed_target_before_send() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(encrypted_failure(400))
        .expect(1)
        .mount(&server)
        .await;
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_provider_contract").await;
    let cache = Arc::new(HistoryCache::default());
    let provider = with_cache(
        provider_with_base_url(&accounts, server.uri()),
        cache.clone(),
    );
    let context = fixed_history_context("acct_provider_contract", "key_openai_contract");
    let mut body = extended_history();
    body["thread_id"] = json!("thread-original");
    let mut events = provider
        .clone()
        .execute(
            planned_request("openai", history_operation(body.clone(), false, false)),
            context.clone(),
        )
        .await
        .unwrap();
    let error = loop {
        match events.next().await {
            Some(Err(error)) => break error,
            Some(Ok(_)) => (),
            None => panic!("expected encrypted rejection"),
        }
    };
    let recovery = error.request_recovery().cloned().unwrap();
    drop(events);
    body["thread_id"] = json!("thread-changed");
    let error = provider
        .execute(
            planned_request("openai", history_operation(body, false, false)),
            context.with_request_recovery(Some(recovery)),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
    assert_eq!(
        error.diagnostic().unwrap().code(),
        Some("history_target_changed")
    );
    assert_eq!(cache.writes.load(Ordering::SeqCst), 0);
    server.verify().await;
}

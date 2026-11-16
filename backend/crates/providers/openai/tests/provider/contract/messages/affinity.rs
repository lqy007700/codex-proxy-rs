//! Messages 会话在原生与 Responses 上游的选号、隔离和最终身份复验

use super::*;
use gateway_core::account::AccountAffinity;

const SESSION: &str = "12345678-1234-1234-1234-123456789abc";

fn operation(header: Option<&str>, user_id: Value) -> Operation {
    let body = json!({"model":"gpt-5.4", "max_tokens":128, "stream":true,
        "metadata":{"user_id":user_id}, "messages":[{"role":"user","content":"hello"}]});
    let mut context = Map::new();
    if let Some(header) = header {
        context.insert(
            "opaque_request_headers".to_owned(),
            json!([["x-claude-code-session-id", STANDARD.encode(header)]]),
        );
    }
    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("anthropic", body.as_object().unwrap().clone())
            .unwrap()
            .with_context(context),
    ))
}

fn attempt(client: &str) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_messages_affinity").unwrap(),
            ClientApiKeyId::new(client).unwrap(),
        ),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(5),
        account_policy()
            .with_openai_account_affinity(AccountAffinity::Strict)
            .with_queue(gateway_core::concurrency::ConcurrencyQueuePolicy {
                max_waiting: 1,
                timeout: Duration::from_secs(2),
            }),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    )
}

async fn fixture(
    native_messages: bool,
) -> (
    MockServer,
    Arc<CodexProvider>,
    Arc<MemorySessionAffinity>,
    Arc<TestLeaseCoordinator>,
) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(if native_messages {
            "/messages"
        } else {
            "/codex/responses"
        }))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            if native_messages {
                MESSAGES_SSE
            } else {
                CAPTURE_COMPLETED_SSE
            },
            "text/event-stream",
        ))
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    for id in ["acct_affinity_switch_a", "acct_affinity_switch_b"] {
        if native_messages {
            store
                .seed_api_key_for_protocol(
                    id,
                    server.uri(),
                    ResponsesTransport::Http,
                    ApiKeyProtocol::Messages,
                )
                .await;
        } else {
            create_account(&store, id).await;
            store.set_oauth_transport(id, ResponsesTransport::Http);
        }
    }
    let affinity = Arc::new(MemorySessionAffinity::default());
    let leases = Arc::new(TestLeaseCoordinator::default());
    let provider = provider_with_affinity_and_base_url_and_leases(
        &store,
        affinity.clone(),
        server.uri(),
        leases.clone(),
    );
    (server, provider, affinity, leases)
}

#[tokio::test]
async fn claude_sessions_keep_accounts_across_identity_formats_and_isolate_client_keys() {
    for native_messages in [false, true] {
        let (server, provider, affinity, _) = fixture(native_messages).await;
        let mut selected = Vec::new();
        let mut keys = Vec::new();
        for (client, header, user_id) in [
            (
                "key_openai_contract",
                Some(SESSION),
                json!("{\"session_id\":\"ignored-body-session\"}"),
            ),
            (
                "key_openai_contract",
                None,
                json!(json!({"session_id":SESSION}).to_string()),
            ),
            (
                "key_openai_contract",
                None,
                json!(format!("user_device_account__session_{SESSION}")),
            ),
            ("different-client", Some(SESSION), json!("ordinary-user")),
            (
                "key_openai_contract",
                Some("different-session"),
                json!("ordinary-user"),
            ),
        ] {
            let before = affinity.lookup_keys().len();
            let mut stream = provider
                .clone()
                .execute(
                    planned_request("openai", operation(header, user_id)),
                    attempt(client),
                )
                .await
                .unwrap();
            selected.push(stream.metadata().provider_account_id().clone());
            keys.push(affinity.lookup_keys()[before].clone());
            while let Some(event) = stream.next().await {
                event.unwrap();
            }
        }
        assert!(selected[..3].iter().all(|account| account == &selected[0]));
        assert!(keys[..3].iter().all(|key| key == &keys[0]));
        assert_ne!(keys[3], keys[0]);
        assert_ne!(keys[4], keys[0]);
        assert_eq!(affinity.binding_count(), 3);
        assert!(
            affinity.alias_ttls().is_empty(),
            "Claude sessions must not create OpenAI turn aliases"
        );

        let before = affinity.lookup_keys().len();
        drop(
            provider
                .clone()
                .execute(
                    planned_request("openai", operation(None, json!("ordinary-user"))),
                    attempt("key_openai_contract"),
                )
                .await
                .unwrap(),
        );
        assert_eq!(
            affinity.lookup_keys().len(),
            before,
            "ordinary users and message text do not identify sessions"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 5);
    }
}

#[tokio::test]
async fn messages_strict_affinity_does_not_send_to_an_idle_alternate_when_bound_account_is_busy() {
    let (server, provider, affinity, leases) = fixture(false).await;
    let request = || planned_request("openai", operation(Some(SESSION), json!("user")));
    let first = provider
        .clone()
        .execute(request(), attempt("key_openai_contract"))
        .await
        .unwrap();
    let bound = first.metadata().provider_account_id().clone();
    drop(first);
    leases.busy_accounts.lock().unwrap().insert(bound.clone());
    let context = attempt("key_openai_contract");
    let cancellation = context.cancellation().clone();
    let pending = tokio::spawn(provider.execute(request(), context));
    tokio::time::timeout(Duration::from_secs(1), async {
        while leases.requests.lock().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    assert!(pending.await.unwrap().is_err());
    assert!(
        leases
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.account_id() == &bound)
    );
    assert_eq!(affinity.binding_count(), 1);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[derive(Debug)]
struct ChangeClaudeSession;

impl MiddlewarePlan for ChangeClaudeSession {
    fn handle(
        &self,
        _: MiddlewareContext,
        request: MiddlewareRequest,
        next: MiddlewareNext,
    ) -> BoxFuture<'static, Result<MiddlewareResponse, MiddlewareError>> {
        Box::pin(async move {
            let protocol = request.protocol().to_owned();
            let body = request.body().clone();
            let mut headers = request.headers().to_vec();
            headers.push(MiddlewareHeader::new(
                "x-claude-code-session-id",
                Bytes::from_static(b"other-session"),
            ));
            next.run(request.replace_parts(protocol, headers, body, None)?)
                .await
        })
    }
}

#[tokio::test]
async fn rewritten_claude_session_cannot_bypass_another_accounts_binding() {
    for native_messages in [false, true] {
        let (server, provider, affinity, _) = fixture(native_messages).await;
        drop(
            provider
                .clone()
                .execute(
                    planned_request("openai", operation(Some(SESSION), json!("user"))),
                    attempt("key_openai_contract"),
                )
                .await
                .unwrap(),
        );
        let key = affinity.lookup_keys()[0].clone();
        affinity.seed_binding(
            &ProviderKind::new("openai").unwrap(),
            &key,
            ProviderAccountId::new("acct_affinity_switch_a").unwrap(),
        );
        drop(
            provider
                .clone()
                .execute(
                    planned_request("openai", operation(Some("other-session"), json!("user"))),
                    attempt("key_openai_contract"),
                )
                .await
                .unwrap(),
        );
        let other = affinity.lookup_keys().last().unwrap().clone();
        affinity.seed_binding(
            &ProviderKind::new("openai").unwrap(),
            &other,
            ProviderAccountId::new("acct_affinity_switch_b").unwrap(),
        );
        let result = provider
            .execute(
                planned_request("openai", operation(Some(SESSION), json!("user"))),
                context_with_middleware(
                    "req_rewritten_claude_session",
                    Arc::new(ChangeClaudeSession),
                    FastMode::Default,
                ),
            )
            .await;
        let error = result
            .err()
            .expect("conflicting final identity must fail before sending");
        assert_eq!(error.send_state(), UpstreamSendState::NotSent);
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn trusted_caller_messages_recovery_keeps_token_and_session_bindings() {
    use gateway_core::account::AccountSchedulingRole;
    use gateway_core::engine::recovery::AccountRecoveryStage;
    use gateway_core::policy::CallerIdentity;
    use gateway_core::provider_ports::ProviderAffinityScope;

    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_affinity_switch_a").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let server = MockServer::start().await;
    let provider = provider_with_affinity_and_base_url(&store, affinity.clone(), server.uri());
    let caller = CallerIdentity::NewApi {
        source_id: "source".into(),
        user_id: "user".into(),
        token_id: "token".into(),
    };
    let context = |key: &str, stage, scope| {
        AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_messages_trusted").unwrap(),
                ClientApiKeyId::new(key).unwrap(),
            )
            .with_caller_identity(caller.clone())
            .with_request_location(Some(global_request_location())),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(5),
            account_policy(),
            AccountAttemptContext::new(BTreeSet::new(), None, None)
                .with_account_scope(scope)
                .with_recovery_stage(stage),
            None,
            CancellationToken::new(),
        )
    };
    let request = || planned_request("openai", operation(Some(SESSION), json!("user")));
    drop(
        provider
            .clone()
            .execute(
                request(),
                context(
                    "key_shared",
                    AccountRecoveryStage::Initial,
                    contract_account_scope(),
                ),
            )
            .await
            .unwrap(),
    );
    let token = affinity.binding_for_scope(ProviderAffinityScope::Token).1;
    let session = affinity.binding_for_scope(ProviderAffinityScope::Session).1;
    create_account(&store, "acct_affinity_switch_b").await;
    drop(
        provider
            .clone()
            .execute(
                request(),
                context(
                    "key_rotated",
                    AccountRecoveryStage::Initial,
                    contract_account_scope(),
                ),
            )
            .await
            .unwrap(),
    );
    assert_eq!(affinity.binding_count(), 2);
    for fallback in [false, true] {
        let scope = if fallback {
            store.set_scheduling_role("acct_affinity_switch_b", AccountSchedulingRole::Fallback);
            Arc::new(FrozenAccountScope::new(
                Arc::new(RuntimeAccountDirectory::new(
                    ["acct_affinity_switch_a", "acct_affinity_switch_b"]
                        .into_iter()
                        .map(|id| {
                            (
                                ProviderAccountId::new(id).unwrap(),
                                RuntimeAccount::new(
                                    ProviderKind::new("openai").unwrap(),
                                    BTreeSet::new(),
                                )
                                .with_scheduling_role(
                                    if id.ends_with("_b") {
                                        AccountSchedulingRole::Fallback
                                    } else {
                                        AccountSchedulingRole::Regular
                                    },
                                ),
                            )
                        })
                        .collect(),
                )),
                ClientRoutingScope::all_accounts(),
            ))
        } else {
            contract_account_scope()
        };
        let stage = if fallback {
            AccountRecoveryStage::Fallback
        } else {
            AccountRecoveryStage::OtherAccounts {
                initial: ProviderAccountId::new("acct_affinity_switch_a").unwrap(),
                after: None,
            }
        };
        let stream = provider
            .clone()
            .execute(request(), context("key_rotated", stage, scope))
            .await
            .unwrap();
        assert_eq!(
            stream.metadata().provider_account_id().as_str(),
            "acct_affinity_switch_b"
        );
        drop(stream);
        assert_eq!(
            affinity
                .binding_for_scope(ProviderAffinityScope::Token)
                .1
                .account_id(),
            token.account_id()
        );
        assert_eq!(
            affinity
                .binding_for_scope(ProviderAffinityScope::Session)
                .1
                .account_id(),
            session.account_id()
        );
    }
}

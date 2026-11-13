//! 验证通用 HTTP 400 的有界恢复、参数错误隔离与原始响应保留

use super::precommit::stream_failure;
use super::selection_retry::{NoopExecutionStore, execution_request};
use super::*;
use gateway_core::account::AccountSchedulingRole;
use gateway_core::engine::provider::ProviderRegistry;
use gateway_core::engine::{AttemptCoordinator, EngineError, GatewayEngine};
use gateway_core::error::TransientRetryPolicy;
use std::sync::atomic::{AtomicUsize, Ordering};

const BAD_REQUEST: &str = r#"{"detail":"Bad Request"}"#;

#[tokio::test]
async fn generic_bad_request_only_recovers_after_explicit_http_payload_rejection() {
    for websocket in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let server = MockServer::start().await;
        Mock::given(method(if websocket { "GET" } else { "POST" }))
            .and(path("/codex/responses"))
            .respond_with(
                ResponseTemplate::new(400)
                    .insert_header("retry-after", "0")
                    .set_body_raw(BAD_REQUEST, "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let operation = if websocket {
            capacity_websocket_operation()
        } else {
            http_generate_operation()
        };
        let mut stream = provider_with_base_url(&store, server.uri())
            .execute(
                planned_request("openai", operation),
                context("req_generic_bad_request", CancellationToken::new()),
            )
            .await
            .unwrap();
        let (_, error) = stream_failure(&mut stream).await;
        assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
        assert_eq!(error.upstream_status(), Some(400));
        assert_eq!(error.replay_is_safe(), !websocket);
        assert_eq!(error.retry_is_prohibited(), websocket);
        if websocket {
            assert!(error.pre_delivery_retry().is_none());
        } else {
            assert!(matches!(
                error.pre_delivery_retry(),
                Some(PreDeliveryRetry::SameAccountTransientRetry {
                    policy: TransientRetryPolicy::RetryThenRotateAccountWithBudget {
                        track_client_retry_failure: true,
                        ..
                    },
                    ..
                })
            ));
        }
        let response = error.client_visible_upstream_response().unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.body().as_ref(), BAD_REQUEST.as_bytes());
        let account = store.account("acct_provider_contract").unwrap();
        assert_eq!(account.credential_state(), CredentialState::Ready);
        assert_eq!(account.quota().access(), QuotaAccessState::Unknown);
        server.verify().await;
    }
}

#[tokio::test]
async fn generic_bad_request_uses_frozen_recovery_budget_and_preserves_final_response() {
    for (initial, rotations, succeeds_at, mixed_service_error) in [
        (1, 0, None, false),
        (2, 1, None, false),
        (2, 1, Some(2), false),
        (2, 1, Some(3), false),
        (2, 1, Some(4), false),
        (2, 1, None, true),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        let ids = [
            "acct_provider_contract",
            "acct_subagent_a",
            "acct_subagent_b",
        ];
        for id in ids {
            create_account(&store, id).await;
        }
        store.set_scheduling_role(ids[2], AccountSchedulingRole::Fallback);
        let server = MockServer::start().await;
        let calls = AtomicUsize::new(0);
        let expected_attempts = succeeds_at.unwrap_or(initial + rotations + 1);
        Mock::given(method("POST"))
            .and(path("/codex/responses"))
            .respond_with(move |_: &wiremock::Request| {
                let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
                if succeeds_at == Some(attempt) {
                    ResponseTemplate::new(200)
                        .set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
                } else if mixed_service_error && attempt == 2 {
                    ResponseTemplate::new(503)
                        .insert_header("retry-after", "0")
                        .set_body_json(json!({"error":{"code":"server_error"}}))
                } else {
                    ResponseTemplate::new(400)
                        .insert_header("retry-after", "0")
                        .set_body_raw(BAD_REQUEST, "application/json")
                }
            })
            .expect(expected_attempts as u64)
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
        let scope = Arc::new(FrozenAccountScope::new(
            Arc::new(RuntimeAccountDirectory::new(
                ids.into_iter()
                    .map(|id| {
                        (
                            ProviderAccountId::new(id).unwrap(),
                            RuntimeAccount::new(kind.clone(), BTreeSet::new())
                                .with_scheduling_role(if id == ids[2] {
                                    AccountSchedulingRole::Fallback
                                } else {
                                    AccountSchedulingRole::Regular
                                }),
                        )
                    })
                    .collect(),
            )),
            ClientRoutingScope::all_accounts(),
        ));
        let snapshot = RuntimeSnapshot::new(
            ConfigRevision::new(1).unwrap(),
            SettingsValues::new(3, "smart", Default::default(), None, None)
                .with_recovery_attempts(initial as u32)
                .with_recovery_rotation_attempts(rotations as u32),
            vec![kind.clone()],
            vec![ProviderModel::new(
                kind,
                UpstreamModelId::new("gpt-5.4").unwrap(),
                ModelCapabilities::new(BTreeSet::from([operation.kind()]), None)
                    .with_upstream_feature_validation(),
            )],
            Vec::new(),
        )
        .unwrap();
        let plan = snapshot
            .plan(
                &PublicModelId::new("gpt-5.4").unwrap(),
                &operation,
                scope,
                &RoutingContext::default(),
            )
            .unwrap();
        let mut session = coordinator
            .start(
                execution_request(
                    "req_generic_bad_request_budget",
                    &operation,
                    Duration::from_secs(10),
                ),
                operation,
                plan,
                None,
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let result = session.collect_uncommitted().await;
        if succeeds_at.is_some() {
            result.unwrap();
            session.commit_downstream(Some(200)).await.unwrap();
        } else {
            let EngineError::Provider(error) = result.unwrap_err() else {
                panic!("expected final upstream rejection")
            };
            assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
            let response = error.client_visible_upstream_response().unwrap();
            assert_eq!(response.status(), 400);
            assert_eq!(response.body().as_ref(), BAD_REQUEST.as_bytes());
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), expected_attempts);
        let accounts: Vec<_> = requests
            .iter()
            .map(|request| request.headers["chatgpt-account-id"].to_str().unwrap())
            .collect();
        assert!(
            accounts[..initial]
                .iter()
                .all(|account| *account == accounts[0])
        );
        let fallback_upstream_account = format!("chatgpt-{}", ids[2]);
        if expected_attempts > initial + rotations {
            assert_eq!(
                accounts.last().copied(),
                Some(fallback_upstream_account.as_str())
            );
        }
        if rotations > 0 && expected_attempts > initial {
            assert_ne!(accounts[initial], accounts[0]);
            assert_ne!(accounts[initial], fallback_upstream_account.as_str());
        }
        for id in ids {
            let account = store.account(id).unwrap();
            assert_eq!(account.credential_state(), CredentialState::Ready);
            assert_eq!(account.quota().access(), QuotaAccessState::Unknown);
        }
        server.verify().await;
    }
}

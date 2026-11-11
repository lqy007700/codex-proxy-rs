//! 验证预热窗口判断、本地暂停、凭据隔离、请求档位与 SSE 终态

use super::*;
use gateway_core::account::{OpaqueProviderData, QuotaObservation, QuotaWriteOutcome};

fn short_window(
    used_percent: Option<f64>,
    reset_after_seconds: Option<u64>,
    until_reset: Option<i64>,
) -> serde_json::Value {
    let mut window = json!({"limit_window_seconds": 18_000});
    if let Some(used) = used_percent {
        window["used_percent"] = json!(used);
    }
    if let Some(remaining) = reset_after_seconds {
        window["reset_after_seconds"] = json!(remaining);
    }
    if let Some(seconds) = until_reset {
        window["reset_at"] = json!(Utc::now().timestamp() + seconds);
    }
    window
}

async fn seed_quota(
    store: &Arc<MemoryAccountStore>,
    account_id: &str,
    quota: serde_json::Value,
    state: QuotaState,
    observed_at: SystemTime,
) {
    let account = store.account(account_id).expect("account");
    assert_eq!(
        store
            .compare_and_swap_quota(QuotaObservation {
                quota_pause: None,
                expected_connection: None,
                plan_type: None,
                account_id: account.id().clone(),
                expected_revision: account.revision(),
                quota: OpaqueProviderData::new(quota.as_object().unwrap().clone()),
                observed_at,
                state,
            })
            .await
            .unwrap(),
        QuotaWriteOutcome::Updated
    );
}

async fn mount_completion(server: &MockServer, expected: u64) {
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            concat!(
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n"
            ),
            "text/event-stream",
        ))
        .expect(expected)
        .mount(server)
        .await;
}

#[tokio::test]
async fn warmup_requires_positive_account_short_window_usage_and_over_thirty_minutes() {
    for (name, used, remaining, until_reset, warmed) in [
        ("zero_full", Some(0.0), Some(18_000), Some(18_000), true),
        (
            "zero_near_full",
            Some(0.0),
            Some(17_999),
            Some(17_999),
            true,
        ),
        ("zero_lower", Some(0.0), Some(12_000), Some(12_000), true),
        ("zero_no_countdown", Some(0.0), None, Some(14_400), true),
        ("missing_usage", None, Some(14_400), Some(14_400), true),
        ("positive_usage", Some(0.1), None, Some(14_400), false),
        ("over_threshold", Some(1.0), Some(1_860), Some(1_860), false),
        ("at_threshold", Some(1.0), Some(1_800), Some(1_800), true),
        ("under_threshold", Some(1.0), Some(1_799), Some(1_799), true),
        ("expired", Some(1.0), Some(3_600), Some(-1), true),
        ("missing_reset", Some(1.0), None, None, true),
    ] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        let account_id = format!("acct_{name}");
        create_account(&store, &account_id).await;
        let observed_at = SystemTime::now() - Duration::from_secs(3_600);
        seed_quota(
            &store,
            &account_id,
            json!({"rate_limit": {
                "allowed": true,
                "primary_window": short_window(used, remaining, until_reset),
            }}),
            QuotaState::allowed(observed_at),
            observed_at,
        )
        .await;
        mount_completion(&server, u64::from(warmed)).await;
        let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());

        let summary = service.execute_warmup("test-model").await.unwrap();

        assert_eq!(summary.warmed_up, u64::from(warmed), "{name}");
        assert_eq!(summary.skipped_active, u64::from(!warmed), "{name}");
        assert_eq!(summary.skipped_exhausted, 0, "{name}");
        assert_eq!(summary.failed, 0, "{name}");
        assert_eq!(store.quota_reads(), 1, "{name}");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), usize::from(warmed), "{name}");
        assert!(
            requests
                .iter()
                .all(|request| request.url.path() == "/codex/responses")
        );
    }
}

#[tokio::test]
async fn warmup_ignores_model_specific_windows_and_account_scheduling_roles() {
    use gateway_core::account::AccountSchedulingRole;

    for role in [
        AccountSchedulingRole::Regular,
        AccountSchedulingRole::Fallback,
    ] {
        for window_seconds in [18_000, 604_800] {
            let server = MockServer::start().await;
            let store = Arc::new(MemoryAccountStore::default());
            let account_id = "acct_named_window";
            create_account(&store, account_id).await;
            store.set_scheduling_role(account_id, role);
            let now = SystemTime::now();
            seed_quota(
                &store,
                account_id,
                json!({
                    "additional_rate_limits": [{
                        "limit_name": "GPT-5.3-Codex-Spark",
                        "metered_feature": "gpt-5.3-codex-spark",
                        "rate_limit": {"allowed": false, "primary_window": {
                            "used_percent": 100,
                            "limit_window_seconds": window_seconds,
                            "reset_at": Utc::now().timestamp() + window_seconds,
                        }},
                    }],
                }),
                QuotaState::allowed(now),
                now,
            )
            .await;
            mount_completion(&server, 1).await;
            let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());

            let summary = service.execute_warmup("test-model").await.unwrap();

            assert_eq!(summary.warmed_up, 1);
            assert_eq!(summary.skipped_active, 0);
            assert_eq!(summary.skipped_exhausted, 0);
            assert_eq!(summary.failed, 0);
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn warmup_skips_real_exhaustion_weekly_limits_and_local_pause_without_quota_queries() {
    for scenario in ["account", "snapshot", "weekly", "paused"] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        let account_id = format!("acct_{scenario}");
        create_account(&store, &account_id).await;
        let now = SystemTime::now();
        let account = store.account(&account_id).unwrap();
        match scenario {
            "account" => {
                persist_quota_state(&store, &account, exhausted_quota(None)).await;
            }
            "snapshot" => {
                store.set_quota_observation(QuotaObservation {
                    quota_pause: None,
                    expected_connection: None,
                    plan_type: None,
                    account_id: account.id().clone(),
                    expected_revision: account.revision(),
                    quota: OpaqueProviderData::new(
                        json!({"rate_limit": {"allowed": false}})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                    observed_at: now,
                    state: QuotaState::exhausted(QuotaEvidence::UsageLimitReached, now, None),
                });
            }
            "weekly" => {
                seed_quota(
                    &store,
                    &account_id,
                    json!({"rate_limit": {"allowed": true, "secondary_window": {
                        "used_percent": 100,
                        "limit_window_seconds": 604_800,
                        "reset_at": Utc::now().timestamp() + 604_800,
                    }}}),
                    QuotaState::allowed(now),
                    now,
                )
                .await;
            }
            "paused" => store.set_quota_paused(&account_id, true),
            _ => unreachable!(),
        }
        mount_completion(&server, 0).await;
        let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());

        let summary = service.execute_warmup("test-model").await.unwrap();

        assert_eq!(
            summary.skipped_exhausted,
            u64::from(scenario != "paused"),
            "{scenario}"
        );
        assert_eq!(summary.warmed_up, 0, "{scenario}");
        assert_eq!(summary.failed, 0, "{scenario}");
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "{scenario}"
        );
    }
}

#[tokio::test]
async fn warmup_ignores_exhausted_snapshots_from_previous_credential_revisions() {
    let server = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    let account_id = "acct_old_warmup_snapshot";
    create_account(&store, account_id).await;
    let previous = store.account(account_id).unwrap();
    let repository = store.repository();
    let data = repository.load_complete_data(&previous).await.unwrap();
    repository
        .compare_and_swap_data(&previous, data)
        .await
        .unwrap();
    let now = SystemTime::now();
    store.set_quota_observation(QuotaObservation {
        quota_pause: None,
        expected_connection: None,
        plan_type: None,
        account_id: previous.id().clone(),
        expected_revision: previous.revision(),
        quota: OpaqueProviderData::new(
            json!({"rate_limit": {"allowed": false, "secondary_window": {
                "used_percent": 100,
                "limit_window_seconds": 604_800,
                "reset_at": Utc::now().timestamp() + 604_800,
            }}})
            .as_object()
            .unwrap()
            .clone(),
        ),
        observed_at: now,
        state: QuotaState::exhausted(QuotaEvidence::UsageLimitReached, now, None),
    });
    mount_completion(&server, 1).await;
    let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());

    let summary = service.execute_warmup("test-model").await.unwrap();

    assert_eq!(summary.warmed_up, 1);
    assert_eq!(summary.skipped_exhausted, 0);
    assert_eq!(summary.failed, 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn warmup_does_not_send_when_the_account_is_paused_before_credential_loading() {
    let server = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_pause_race").await;
    store.on_credential_load(Arc::new(|store, account, _| {
        Box::pin(async move {
            store.set_quota_paused(account.as_str(), true);
            Ok(())
        })
    }));
    mount_completion(&server, 0).await;
    let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());

    let summary = service.execute_warmup("test-model").await.unwrap();

    assert_eq!(summary.warmed_up, 0);
    assert_eq!(summary.failed, 1);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn warmup_consumes_completed_sse_and_persists_rate_limit_event() {
    let server = MockServer::start().await;
    let sse = concat!(
        "event: codex.rate_limits\n",
        "data: {\"type\":\"codex.rate_limits\",\"plan_type\":\"team\",\"rate_limits\":{\"allowed\":true,\"limit_reached\":false,\"primary\":{\"used_percent\":5,\"window_minutes\":300,\"reset_at\":1900000000}}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_warmup\",\"status\":\"completed\",\"output\":[]}}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_warmup_completed").await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    let summary = service.execute_warmup("test-model").await.expect("warmup");

    assert_eq!(summary.warmed_up, 1);
    assert_eq!(summary.failed, 0);
    assert_eq!(
        store.account("acct_warmup_completed").unwrap().plan_type(),
        Some("team")
    );
    assert!(store.quota_json("acct_warmup_completed").is_some());
}

#[tokio::test]
async fn warmup_sends_the_lowest_reasoning_effort_supported_by_the_catalog() {
    let server = MockServer::start().await;
    let sse = concat!(
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_warmup_effort\",\"status\":\"completed\",\"output\":[]}}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_warmup_effort").await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    let summary = service.execute_warmup("test-model").await.expect("warmup");

    assert_eq!(summary.warmed_up, 1);
    let requests = server.received_requests().await.expect("recorded requests");
    let decoded =
        zstd::stream::decode_all(requests[0].body.as_slice()).expect("zstd body should decode");
    let body: serde_json::Value =
        serde_json::from_slice(&decoded).expect("warmup body should be valid JSON");
    // 上游按目录拒绝目录外的档位，none 曾让全部定时预热只留下 error 终态
    assert_eq!(body["reasoning"]["effort"], "low");
}

#[tokio::test]
async fn warmup_rejects_failed_sse_despite_successful_http_headers() {
    let server = MockServer::start().await;
    let sse = concat!(
        "event: codex.rate_limits\n",
        "data: {\"type\":\"codex.rate_limits\",\"plan_type\":\"team\",\"rate_limits\":{\"allowed\":false,\"limit_reached\":true,\"primary\":{\"used_percent\":100,\"window_minutes\":300,\"reset_at\":1900000000}}}\n\n",
        "event: response.failed\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-codex-allowed", "false")
                .set_body_raw(sse, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_warmup_failed").await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    let summary = service.execute_warmup("test-model").await.expect("warmup");

    assert_eq!(summary.warmed_up, 0);
    assert_eq!(summary.failed, 1);
    assert!(store.quota_json("acct_warmup_failed").is_none());
}

#[tokio::test]
async fn warmup_skips_expired_credential() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_warmup_expired").await;
    let account = store.account("acct_warmup_expired").expect("account");
    persist_credential_state(&store, &account, CredentialState::Expired).await;
    let service = quota_service_with_base_url(
        &store,
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        server.uri(),
    );

    let summary = service.execute_warmup("test-model").await.expect("warmup");

    assert_eq!(summary.warmed_up, 0);
    assert_eq!(summary.failed, 0);
}

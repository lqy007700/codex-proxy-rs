//! 点数独立可用性、耗尽恢复和过期查询隔离

use super::*;

fn exhausted_usage() -> serde_json::Value {
    json!({"rate_limit": {
        "allowed": false,
        "limit_reached": true,
        "primary_window": {"used_percent": 100, "reset_at": 1_900_000_000},
        "secondary_window": {"used_percent": 100, "reset_at": 1_900_604_800}
    }})
}

async fn mount_usage(server: &MockServer, usage: serde_json::Value) {
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(usage))
        .mount(server)
        .await;
}

#[tokio::test]
async fn credits_allow_exhausted_subscription_without_bypassing_spend_restrictions() {
    for (metadata, allowed) in [
        (
            json!({"credits": {"has_credits": true, "unlimited": false}}),
            true,
        ),
        (
            json!({"credits": {"has_credits": false, "unlimited": true}}),
            true,
        ),
        (json!({"credits": {"has_credits": true}}), true),
        (json!({"credits": {"unlimited": true}}), true),
        (
            json!({"credits": {"has_credits": false, "unlimited": false, "balance": "500"}}),
            false,
        ),
        (json!({"credits": null}), false),
        (
            json!({"credits": {"has_credits": true}, "spend_control": {"reached": true}}),
            false,
        ),
        (
            json!({"credits": {"has_credits": true}, "rate_limit_reached_type": {"type": "spend_limit_reached"}}),
            false,
        ),
        (
            json!({"credits": {"has_credits": true}, "rate_limit_reached_type": {"type": "rate_limit_reached"}}),
            true,
        ),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account_with_enabled(&store, "acct_credits", false).await;
        store.set_allow_credits("acct_credits", true);
        let account = store.account("acct_credits").expect("account");
        persist_credential_state(&store, &account, CredentialState::Expired).await;
        let server = MockServer::start().await;
        let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());
        let mut usage = exhausted_usage();
        usage
            .as_object_mut()
            .expect("usage")
            .extend(metadata.as_object().expect("metadata").clone());
        mount_usage(&server, usage).await;

        let snapshot = service
            .refresh_account(account.id())
            .await
            .expect("refresh credits");
        assert_eq!(
            snapshot.quota().with_credits_policy(true).access() == QuotaAccessState::Allowed,
            allowed,
            "{metadata}"
        );
        assert_eq!(snapshot.fact().remaining_percent(), Some(0));
        let current = store.account("acct_credits").expect("current account");
        assert_eq!(current.quota().is_exhausted(), !allowed, "{metadata}");
        assert!(!current.enabled());
        assert_eq!(current.credential_state(), CredentialState::Expired);
    }
}

#[tokio::test]
async fn credits_refresh_clears_old_recovery_and_locks_again_only_when_spent() {
    for periodic in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_credits_recovery").await;
        store.set_allow_credits("acct_credits_recovery", true);
        let account = store.account("acct_credits_recovery").expect("account");
        let server = MockServer::start().await;
        let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());
        mount_usage(&server, exhausted_usage()).await;
        service
            .refresh_account(account.id())
            .await
            .expect("seed exhaustion");
        assert!(
            store
                .quota_json("acct_credits_recovery")
                .expect("quota")
                .get("_quota_recovery")
                .is_some()
        );

        let mut usage = exhausted_usage();
        usage["credits"] =
            json!({"has_credits": !periodic, "unlimited": periodic, "balance": "20"});
        mount_usage(&server, usage.clone()).await;
        if periodic {
            assert_eq!(
                service
                    .synchronize()
                    .await
                    .expect("periodic recovery")
                    .updated,
                1
            );
        } else {
            service
                .refresh_account(account.id())
                .await
                .expect("manual recovery");
        }
        let snapshot = service
            .read_account(account.id())
            .await
            .expect("persisted snapshot")
            .expect("snapshot");
        assert_eq!(
            snapshot.quota().with_credits_policy(true).access(),
            QuotaAccessState::Allowed
        );
        assert_eq!(snapshot.fact().remaining_percent(), Some(0));
        assert_eq!(
            snapshot.credits().expect("credits").balance.as_deref(),
            Some("20")
        );
        assert!(
            store
                .quota_json("acct_credits_recovery")
                .expect("quota")
                .get("_quota_recovery")
                .is_none()
        );

        usage["credits"] = json!({"has_credits": false, "unlimited": false, "balance": "0"});
        mount_usage(&server, usage).await;
        let snapshot = service
            .refresh_account(account.id())
            .await
            .expect("spent credits");
        assert!(snapshot.quota().is_exhausted());
        assert!(
            store
                .account("acct_credits_recovery")
                .expect("account")
                .quota()
                .is_exhausted()
        );
    }
}

#[tokio::test]
async fn credits_query_started_before_new_exhaustion_cannot_restore_access() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_stale_credits").await;
    store.set_allow_credits("acct_stale_credits", true);
    let account = store.account("acct_stale_credits").expect("account");
    let server = MockServer::start().await;
    let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());
    let mut usage = exhausted_usage();
    usage["credits"] = json!({"has_credits": true, "unlimited": false});
    Mock::given(method("GET"))
        .and(path("/api/codex/usage"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(200))
                .set_body_json(usage),
        )
        .mount(&server)
        .await;
    let (result, ()) = tokio::join!(service.refresh_account(account.id()), async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server
                .received_requests()
                .await
                .expect("requests")
                .is_empty()
            {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("query started");
        persist_quota_state(
            &store,
            &account,
            QuotaState::exhausted(QuotaEvidence::UsageLimitReached, SystemTime::now(), None),
        )
        .await;
    });
    result.expect("old display observation can persist independently of newer access state");
    assert!(
        store
            .account("acct_stale_credits")
            .expect("account")
            .quota()
            .is_exhausted()
    );
    assert!(
        service
            .read_account(account.id())
            .await
            .expect("persisted quota")
            .expect("snapshot")
            .quota()
            .is_exhausted()
    );
}

#[tokio::test]
async fn credits_are_disabled_by_default_and_refresh_cannot_enable_them() {
    let store = Arc::new(MemoryAccountStore::default());
    let account_id = "acct_credits_policy";
    create_account(&store, account_id).await;
    let server = MockServer::start().await;
    let service = quota_service_with_base_url(&store, reqwest::Client::new(), server.uri());
    let mut usage = exhausted_usage();
    // 上游 allowed 也可能包含点数的放行，窗口触顶仍需要账号显式允许点数
    usage["rate_limit"]["allowed"] = json!(true);
    usage["credits"] = json!({"has_credits": true, "unlimited": false, "balance": "20"});
    mount_usage(&server, usage).await;
    let id = gateway_core::account::ProviderAccountId::new(account_id).unwrap();
    service.refresh_account(&id).await.unwrap();
    let account = store.account(account_id).unwrap();
    assert!(!account.allow_credits());
    assert!(account.quota().is_exhausted());
    assert_eq!(
        account.quota_fact().evidence(),
        Some(QuotaEvidence::CreditsAvailable)
    );

    store.set_allow_credits(account_id, true);
    assert!(!store.account(account_id).unwrap().quota().is_exhausted());
    service.refresh_account(&id).await.unwrap();
    store.set_allow_credits(account_id, false);
    assert!(store.account(account_id).unwrap().quota().is_exhausted());
    service.synchronize().await.unwrap();
    assert!(!store.account(account_id).unwrap().allow_credits());
    assert!(store.account(account_id).unwrap().quota().is_exhausted());

    let account = store.account(account_id).unwrap();
    service
        .record_successful_inference(&account, SystemTime::now())
        .await
        .unwrap();
    assert!(store.account(account_id).unwrap().quota().is_exhausted());
    let passive = parse_rate_limits_event(&json!({
        "type": "codex.rate_limits",
        "rate_limits": {"primary": {"used_percent": 100, "window_minutes": 300}},
        "credits": {"has_credits": true, "unlimited": false, "balance": "19"}
    }))
    .unwrap();
    service
        .synchronize_passive_rate_limits(&account, &[passive])
        .await
        .unwrap();
    assert!(store.account(account_id).unwrap().quota().is_exhausted());

    mount_usage(
        &server,
        json!({
            "rate_limit": {"allowed": true, "primary_window": {"used_percent": 20}},
            "credits": {"has_credits": true, "unlimited": false, "balance": "19"}
        }),
    )
    .await;
    service.refresh_account(&id).await.unwrap();
    let restored = store.account(account_id).unwrap();
    assert!(!restored.allow_credits());
    assert_eq!(restored.quota().access(), QuotaAccessState::Allowed);

    mount_usage(
        &server,
        json!({
            "rate_limit": {"allowed": true, "limit_reached": true},
            "credits": {"has_credits": true, "unlimited": false, "balance": "19"}
        }),
    )
    .await;
    service.refresh_account(&id).await.unwrap();
    assert!(store.account(account_id).unwrap().quota().is_exhausted());
}

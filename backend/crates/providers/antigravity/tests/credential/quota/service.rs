//! 真实分组额度投影、模型独立调度与迟到观测隔离

use crate::support::Fixture;
use gateway_core::{
    account::{CredentialRevision, CredentialState, ProviderAccount, QuotaState},
    lifecycle::CancellationToken,
};
use serde_json::json;
use std::time::{Duration, SystemTime};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

async fn upstream(server: &MockServer, remaining: serde_json::Value, delay: Duration) {
    Mock::given(path("/v1internal:loadCodeAssist")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"cloudaicompanionProject":"p","currentTier":{"id":"free-tier","name":"Antigravity"},"paidTier":{"id":"g1-pro-tier","name":"Google AI Pro","description":"Google AI Pro subscription"}}))).mount(server).await;
    Mock::given(path("/v1internal:fetchAvailableModels")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"models":{"fixture-model":{"displayName":"Gemini","quotaInfo":{"remainingFraction":remaining,"resetTime":"2099-01-01T00:00:00Z"}},"other-model":{"displayName":"Claude","quotaInfo":{"remainingFraction":0.8,"resetTime":"2099-01-01T00:00:00Z"}}}}))).mount(server).await;
    Mock::given(path("/v1internal:retrieveUserQuotaSummary")).respond_with(ResponseTemplate::new(200).set_delay(delay).set_body_json(json!({"groups":[{"displayName":"Gemini Models","buckets":[{"bucketId":"gemini-weekly","displayName":"Weekly","window":"weekly","remainingFraction":0.9,"resetTime":"2099-01-01T00:00:00Z"},{"bucketId":"gemini-5h","displayName":"Five hour","window":"5h","remainingFraction":remaining,"resetTime":"2099-01-01T00:00:00Z"}]},{"displayName":"Claude and GPT models","buckets":[{"bucketId":"3p-weekly","displayName":"Weekly","window":"weekly","remainingFraction":0.8,"resetTime":"2099-01-01T00:00:00Z"}]}]}))).mount(server).await;
}

#[tokio::test]
async fn window_usage_requires_every_named_member_to_resolve_in_the_observed_catalog() {
    use gateway_admin::model::provider_credentials::QuotaLocalUsageAttribution::{
        Models, Unavailable,
    };
    let server = MockServer::start().await;
    Mock::given(path("/v1internal:loadCodeAssist"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"cloudaicompanionProject":"p"})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/v1internal:fetchAvailableModels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models":{
            "gemini-4-neon-high":{"displayName":"Gemini 4 Neon (High)"},
            "gemini-4-neon-low":{"displayName":"Gemini 4 Neon (Low)"},
            "orchestrator-alias":{"displayName":"Orchestrator (Neon)"},
            "gemini-lite":{"displayName":"Gemini Lite"},
            "gpt-oss-medium":{"displayName":"GPT-OSS (Medium)"}
        }})))
        .mount(&server)
        .await;
    let group = |description: Option<&str>, name: &str| {
        json!({
            "displayName":name, "description":description,
            "buckets":(["5h", "weekly"].map(|window| json!({
                "bucketId":format!("{name}-{window}"), "displayName":window, "window":window
            })))
        })
    };
    Mock::given(path("/v1internal:retrieveUserQuotaSummary"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"groups":[
            group(Some("Models within this group: Gemini 4 Neon, Orchestrator (Neon)"), "gemini"),
            group(Some("Models within this group: GPT-OSS"), "3p"),
            group(Some("Models within this group: Gemini Lite, Missing Model"), "partial"),
            group(None, "unknown")
        ]})))
        .mount(&server)
        .await;
    let f = Fixture::new(&server.uri());
    let account = f.store.primary().loaded.lock().unwrap().account.clone();
    f.quota.refresh(account.id()).await.unwrap();
    let view = f.quota.read(account.id()).await.unwrap();
    for (group, attribution) in [
        (
            "gemini",
            Models(
                [
                    "gemini-4-neon-high",
                    "gemini-4-neon-low",
                    "orchestrator-alias",
                ]
                .map(str::to_owned)
                .into(),
            ),
        ),
        ("3p", Models(["gpt-oss-medium".to_owned()].into())),
        ("partial", Unavailable),
        ("unknown", Unavailable),
    ] {
        for window in ["5h", "weekly"] {
            let key = format!("{group}-{window}");
            let observed = view.windows.iter().find(|w| w.key == key).unwrap();
            assert_eq!(observed.local_usage_attribution, attribution);
        }
    }
}

#[tokio::test]
async fn quota_and_catalog_refresh_use_current_global_identity() {
    let server = MockServer::start().await;
    upstream(&server, json!(0.5), Duration::ZERO).await;
    let f = Fixture::new(&server.uri());
    let account = f.store.primary().loaded.lock().unwrap().account.clone();
    for (version, changelist) in [("1.5.14", ""), ("1.5.15", "; cl=1027820220")] {
        *f.store.request_profile.lock().unwrap() = Some(crate::profile::object(
            json!({"client":"cli","platform":"linux","versionMode":"fixed","clientVersion":version,"arch":null}),
        ));
        let before = server.received_requests().await.unwrap().len();
        f.quota.refresh(account.id()).await.unwrap();
        f.catalog.account_models(account.id(), true).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len() - before, 5);
        for request in &requests[before..] {
            assert_eq!(
                request.headers["user-agent"],
                format!(
                    "antigravity/cli/{version} (aidev_client; os_type=linux; arch=amd64{changelist}; auth_method=consumer)"
                )
            );
        }
    }
    *f.store.request_profile.lock().unwrap() =
        Some(crate::profile::object(json!({"client":"unsupported"})));
    assert!(f.quota.refresh(account.id()).await.is_err());
    assert!(f.catalog.account_models(account.id(), true).await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 10);
}

#[tokio::test]
async fn quota_refresh_projects_paid_plan_without_exhausting_other_models() {
    let server = MockServer::start().await;
    upstream(&server, json!(0.0), Duration::ZERO).await;
    let f = Fixture::new(&server.uri());
    let account = f.store.primary().loaded.lock().unwrap().account.clone();
    f.quota.refresh(account.id()).await.unwrap();
    let view = f.quota.read(account.id()).await.unwrap();
    assert!(view.credits.is_none());
    assert_eq!(view.plan_type.as_deref(), Some("g1-pro-tier"));
    assert_eq!(
        view.plan_type_display.as_deref(),
        Some("Google AI Pro subscription")
    );
    assert_eq!(
        f.quota.plan_type_display(&account).await.as_deref(),
        Some("Google AI Pro subscription")
    );
    {
        let mut quota = f.store.primary().quota.lock().unwrap();
        quota.as_mut().unwrap().expected_revision = CredentialRevision::new(2).unwrap();
    }
    assert_eq!(f.quota.plan_type_display(&account).await, None);
    {
        let mut quota = f.store.primary().quota.lock().unwrap();
        quota.as_mut().unwrap().expected_revision = account.revision();
    }
    assert_eq!(view.windows.len(), 3);
    assert!(
        view.windows
            .iter()
            .all(|window| window.local_usage.is_none())
    );
    assert_eq!(view.windows[1].used_percent, Some(100.0));
    assert!(!view.limit_reached);
    assert_eq!(
        f.store
            .primary()
            .quota
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .state
            .access(),
        gateway_core::account::QuotaAccessState::Allowed
    );
    let gemini = f
        .quota
        .scheduling(std::slice::from_ref(&account), "fixture-model")
        .await
        .unwrap();
    let claude = f
        .quota
        .scheduling(std::slice::from_ref(&account), "other-model")
        .await
        .unwrap();
    assert!(gemini[account.id()].0);
    assert!(!claude[account.id()].0);
    assert_eq!(
        claude[account.id()].1.as_ref().unwrap().remaining_rank(),
        Some(8000)
    );
    {
        let mut old = f.store.primary().quota.lock().unwrap();
        old.as_mut().unwrap().observed_at = SystemTime::now() - Duration::from_secs(601);
    }
    let expired = f
        .quota
        .scheduling(std::slice::from_ref(&account), "fixture-model")
        .await
        .unwrap();
    assert!(!expired[account.id()].0);
    assert!(expired[account.id()].1.is_none());
}

#[tokio::test]
async fn unknown_quota_preserves_only_observed_windows_without_inventing_percentages() {
    let server = MockServer::start().await;
    upstream(&server, json!(null), Duration::ZERO).await;
    let f = Fixture::new(&server.uri());
    let account = f.store.primary().loaded.lock().unwrap().account.clone();
    let empty = f.quota.read(account.id()).await.unwrap();
    assert!(empty.windows.is_empty());
    f.quota.refresh(account.id()).await.unwrap();
    let view = f.quota.read(account.id()).await.unwrap();
    assert_eq!(view.windows[1].used_percent, None);
    assert!(
        !f.quota
            .scheduling(std::slice::from_ref(&account), "fixture-model")
            .await
            .unwrap()[account.id()]
        .0
    );
}

#[tokio::test]
async fn failed_or_stale_refresh_preserves_persisted_quota() {
    let server = MockServer::start().await;
    upstream(&server, json!(0.5), Duration::ZERO).await;
    let f = Fixture::new(&server.uri());
    let account = f.store.primary().loaded.lock().unwrap().account.clone();
    f.quota.refresh(account.id()).await.unwrap();
    let first = f.store.primary().quota.lock().unwrap().clone().unwrap();
    server.reset().await;
    Mock::given(path("/v1internal:loadCodeAssist"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    assert!(f.quota.refresh(account.id()).await.is_err());
    assert_eq!(
        f.store.primary().quota.lock().unwrap().clone().unwrap(),
        first
    );
    server.reset().await;
    upstream(&server, json!(0.0), Duration::from_millis(150)).await;
    let service = f.quota.clone();
    let id = account.id().clone();
    let pending = tokio::spawn(async move { service.refresh(&id).await });
    for _ in 0..100 {
        if server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/v1internal:retrieveUserQuotaSummary")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    f.store.primary().loaded.lock().unwrap().account = ProviderAccount::new(
        account.id().clone(),
        account.provider().clone(),
        "Fixture".into(),
        account.upstream_user_id().map(str::to_owned),
        "oauth".into(),
        CredentialRevision::new(2).unwrap(),
        account.access_token_expires_at(),
    )
    .with_account_facts(
        true,
        CredentialState::Ready,
        QuotaState::unknown(),
        None,
        None,
    );
    assert!(pending.await.unwrap().is_err());
    assert_eq!(
        f.store.primary().quota.lock().unwrap().clone().unwrap(),
        first
    );
    assert!(
        f.quota
            .read(account.id())
            .await
            .unwrap()
            .plan_type
            .is_none()
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    f.quota.reconcile(&cancelled).await.unwrap();
}

#[tokio::test]
async fn exhausted_model_quota_blocks_execution_for_that_model() {
    use gateway_core::{engine::provider::Provider, error::ProviderErrorKind};
    let server = MockServer::start().await;
    upstream(&server, json!(0.0), Duration::ZERO).await;
    let f = Fixture::new(&server.uri());
    let id = f
        .store
        .primary()
        .loaded
        .lock()
        .unwrap()
        .account
        .id()
        .clone();
    f.quota.refresh(&id).await.unwrap();
    let result = f
        .provider
        .clone()
        .execute(
            crate::support::request(false),
            crate::support::context(
                crate::profile::Selection::default()
                    .resolve("2.34.0")
                    .frozen(),
                CancellationToken::new(),
            ),
        )
        .await;
    assert!(matches!(result, Err(e) if e.kind() == ProviderErrorKind::QuotaExhausted));
}

#[tokio::test]
async fn periodic_refresh_requires_five_minutes_and_usage_after_snapshot() {
    use std::sync::atomic::Ordering;
    let server = MockServer::start().await;
    upstream(&server, json!(0.5), Duration::ZERO).await;
    let f = Fixture::new(&server.uri());
    let id = f
        .store
        .primary()
        .loaded
        .lock()
        .unwrap()
        .account
        .id()
        .clone();
    let cancel = CancellationToken::new();
    f.quota.reconcile(&cancel).await.unwrap();
    let initial = f.store.primary().quota.lock().unwrap().clone().unwrap();
    f.store.quota_used.store(true, Ordering::SeqCst);
    f.quota.reconcile(&cancel).await.unwrap();
    assert_eq!(
        f.store
            .primary()
            .quota
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .observed_at,
        initial.observed_at
    );
    f.store
        .primary()
        .quota
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .observed_at = SystemTime::now() - Duration::from_secs(301);
    f.store.quota_used.store(false, Ordering::SeqCst);
    f.quota.reconcile(&cancel).await.unwrap();
    let stale = f.store.primary().quota.lock().unwrap().clone().unwrap();
    f.store.quota_used.store(true, Ordering::SeqCst);
    f.quota.reconcile(&cancel).await.unwrap();
    assert!(
        f.store
            .primary()
            .quota
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .observed_at
            > stale.observed_at
    );
    assert_eq!(
        f.store
            .primary()
            .quota
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .account_id,
        id
    );
}

#[tokio::test]
async fn older_inflight_refresh_cannot_overwrite_newer_manual_snapshot() {
    let server = MockServer::start().await;
    upstream(&server, json!(0.0), Duration::ZERO).await;
    let f = Fixture::new(&server.uri());
    let id = f
        .store
        .primary()
        .loaded
        .lock()
        .unwrap()
        .account
        .id()
        .clone();
    let (arrived_tx, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *f.store.quota_write_gate.lock().unwrap() = Some((arrived_tx, release_rx));
    let service = f.quota.clone();
    let old_id = id.clone();
    let older = tokio::spawn(async move { service.refresh(&old_id).await });
    tokio::time::timeout(Duration::from_secs(10), arrived_rx)
        .await
        .unwrap()
        .unwrap();
    server.reset().await;
    upstream(&server, json!(0.8), Duration::ZERO).await;
    let newer_result = f.quota.refresh(&id).await;
    let newer = f.store.primary().quota.lock().unwrap().clone();
    let _ = release_tx.send(());
    newer_result.unwrap();
    let newer = newer.unwrap();
    assert!(older.await.unwrap().is_err());
    assert_eq!(
        f.store.primary().quota.lock().unwrap().as_ref().unwrap(),
        &newer
    );
    let view = f.quota.read(&id).await.unwrap();
    let window = view
        .windows
        .iter()
        .find(|window| window.key == "gemini-5h")
        .unwrap();
    assert!((window.used_percent.unwrap() - 20.0).abs() < 0.001);
}

#[tokio::test]
async fn credits_project_and_only_bypass_exhaustion_when_usable_and_enabled() {
    for (amount, usable) in [
        (json!("12"), true),
        (json!("9"), false),
        (json!(null), false),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/v1internal:loadCodeAssist"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"cloudaicompanionProject":"p", "paidTier":{"availableCredits":[{"creditType":"GOOGLE_ONE_AI","creditAmount":amount,"minimumCreditAmountForUsage":"10"}]}})))
            .mount(&server).await;
        Mock::given(path("/v1internal:fetchAvailableModels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"models":{"m":{"quotaInfo":{"remainingFraction":0,"resetTime":null}}}}),
            ))
            .mount(&server)
            .await;
        Mock::given(path("/v1internal:retrieveUserQuotaSummary"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"groups":[]})))
            .mount(&server)
            .await;
        let f = Fixture::new(&server.uri());
        let account = f.store.primary().loaded.lock().unwrap().account.clone();
        f.quota.refresh(account.id()).await.unwrap();
        assert_eq!(
            f.quota
                .read(account.id())
                .await
                .unwrap()
                .credits
                .unwrap()
                .has_credits,
            usable
        );
        let snapshot = f.store.primary().quota.lock().unwrap().clone().unwrap();
        assert!(snapshot.state.with_credits_policy(false).is_exhausted());
        assert_eq!(
            snapshot.state.with_credits_policy(true).is_exhausted(),
            !usable
        );
        for enabled in [false, true] {
            let scoped = account.clone().with_allow_credits(enabled);
            let signals = f.quota.scheduling(&[scoped], "m").await.unwrap();
            assert_eq!(signals[account.id()].0, !(enabled && usable));
        }
    }
}

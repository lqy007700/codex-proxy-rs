//! 验证账号管理接口的身份要求、查询校验与服务调用边界

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use gateway_api::admin;
use tower::ServiceExt as _;

use super::super::{AdminTestFixture, AdminTestState};

#[tokio::test]
async fn account_model_reads_and_refreshes_preserve_provider_observation_time() {
    use chrono::{DateTime, Utc};
    use gateway_admin::model::provider_credentials::{ProviderModel, ProviderModels};
    use gateway_core::{
        account::{AccountStatusFacts, CredentialState, QuotaState},
        routing::UpstreamModelId,
    };
    use serde_json::json;

    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let observed_at: DateTime<Utc> = "2026-09-12T08:00:00Z".parse().unwrap();
    *fixture.account.lock().unwrap() = Some(account_page_item(
        AccountStatusFacts {
            quota_paused: false,
            enabled: true,
            credential_state: CredentialState::Ready,
            access_token_expires_at: None,
            quota: QuotaState::unknown(),
            cooldown: None,
            last_error_reason: None,
            last_error_message: None,
        },
        observed_at,
        "api_key",
    ));
    let router = admin::accounts::router::<AdminTestState>().with_state(fixture.state());
    for observed_at in [Some(observed_at), None] {
        *fixture.provider_models.lock().unwrap() = Some(ProviderModels {
            models: vec![ProviderModel {
                id: UpstreamModelId::new("model-a").unwrap(),
                name: "Model A".to_owned(),
            }],
            observed_at,
        });
        for (method, uri, body) in [
            (
                "GET",
                "/api/admin/accounts/models?accountId=acct_cooldown",
                "",
            ),
            (
                "POST",
                "/api/admin/accounts/models/refresh",
                r#"{"accountId":"acct_cooldown"}"#,
            ),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("x-request-id", "req_model_catalog")
                        .header(header::COOKIE, "cpr_session=valid-session")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{method} {uri}");
            let body = to_bytes(response.into_body(), 8192).await.unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                value["data"]["models"],
                json!([{ "id": "model-a", "label": "model-a" }])
            );
            assert_eq!(
                value["data"]["observedAt"],
                json!(observed_at.map(|_| "2026-09-12T16:00:00+08:00"))
            );
            assert_eq!(
                value["data"]["observedAtDisplay"],
                observed_at.map_or("—", |_| "2026-09-12 16:00:00")
            );
        }
    }
}

#[tokio::test]
async fn account_proxy_binding_is_distinct_from_the_redacted_endpoint() {
    use chrono::Utc;
    use gateway_core::account::{AccountStatusFacts, CredentialState, OutboundProxy, QuotaState};

    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (proxy_id, proxy_url, enabled) in [
        (
            Some("proxy-a"),
            Some("socks5h://alice:secret@127.0.0.1:50101"),
            true,
        ),
        (
            Some("proxy-b"),
            Some("socks5h://bob:secret@127.0.0.1:50101"),
            false,
        ),
        (None, Some("socks5h://alice:secret@127.0.0.1:50101"), true),
        (None, None, true),
    ] {
        let now = Utc::now();
        let facts = AccountStatusFacts {
            quota_paused: false,
            enabled: true,
            credential_state: CredentialState::Ready,
            access_token_expires_at: None,
            quota: QuotaState::unknown(),
            cooldown: None,
            last_error_reason: None,
            last_error_message: None,
        };
        let mut item = account_page_item(facts, now, "oauth");
        item.account.outbound_proxy_id = proxy_id.map(str::to_owned);
        item.account.outbound_proxy = proxy_url.map(|url| OutboundProxy::parse(url).unwrap());
        item.account.outbound_proxy_enabled = enabled;
        *fixture.account.lock().unwrap() = Some(item);
        let response = admin::accounts::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .uri("/api/admin/accounts")
                    .header("x-request-id", "req_proxy_binding")
                    .header(header::COOKIE, "cpr_session=valid-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 32768).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let account = &value["data"]["items"][0];
        assert_eq!(account["outboundProxyId"], serde_json::json!(proxy_id));
        assert_eq!(
            account["outboundProxyEndpoint"],
            serde_json::json!(proxy_url.map(|_| "socks5h://127.0.0.1:50101"))
        );
        assert!(!String::from_utf8(body.to_vec()).unwrap().contains("secret"));
    }
}

#[tokio::test]
async fn account_cooldown_returns_an_instant_and_server_formatted_recovery() {
    use chrono::{DateTime, Duration, Utc};
    use gateway_core::account::{
        AccountCooldown, AccountCooldownKind, AccountStatusFacts, CredentialState, QuotaState,
    };

    for zone in ["Asia/Shanghai", "UTC", "Asia/Kathmandu", "America/New_York"] {
        let zone = zone
            .parse::<gateway_core::time::DeploymentTimeZone>()
            .unwrap();
        let fixture = AdminTestFixture::with_timezone(zone).await;
        fixture.auth.insert_session("valid-session");
        for (seconds, kind, expected) in [
            (
                Some(1200),
                AccountCooldownKind::RateLimit,
                Some("剩余 20 分钟"),
            ),
            (
                Some(7200),
                AccountCooldownKind::ErrorProtection,
                Some("剩余 2 小时"),
            ),
            (
                Some(7500),
                AccountCooldownKind::ErrorProtectionProbe,
                Some("剩余 2 小时 5 分"),
            ),
            (
                Some(-60),
                AccountCooldownKind::ErrorProtectionProbe,
                Some("等待探测成功"),
            ),
            (Some(20), AccountCooldownKind::RateLimit, None),
            (None, AccountCooldownKind::RateLimit, None),
        ] {
            let now = Utc::now();
            let until = seconds.map(|seconds| now + Duration::seconds(seconds));
            let facts = AccountStatusFacts {
                quota_paused: false,
                enabled: true,
                credential_state: CredentialState::Ready,
                access_token_expires_at: None,
                quota: QuotaState::unknown(),
                cooldown: until.map(|until| AccountCooldown {
                    until: until.into(),
                    kind,
                }),
                last_error_reason: None,
                last_error_message: None,
            };
            let mut item = account_page_item(facts, now, "oauth");
            if kind.is_error_protection() {
                item.protection_rules = vec![gateway_core::account::AccountProtectionMatch {
                    id: "removed-rule".to_owned(),
                    name: "触发时的规则名称".to_owned(),
                }];
            }
            *fixture.account.lock().unwrap() = Some(item);
            let response = admin::accounts::router::<AdminTestState>()
                .with_state(fixture.state())
                .oneshot(
                    Request::builder()
                        .uri("/api/admin/accounts")
                        .header("x-request-id", "req_cooldown_timezone")
                        .header(header::COOKIE, "cpr_session=valid-session")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 32768).await.unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["data"]["items"][0]["websocketDegraded"], false);
            let quota = &value["data"]["items"][0]["quota"];
            if kind.is_error_protection() {
                assert_eq!(quota["rateLimitReason"], "error_protection");
                assert_eq!(
                    quota["protectionRules"],
                    serde_json::json!([
                        {"id":"removed-rule", "name":"触发时的规则名称"}
                    ])
                );
            } else {
                assert_eq!(quota["protectionRules"], serde_json::json!([]));
            }
            match until {
                Some(until) => {
                    assert_eq!(
                        DateTime::parse_from_rfc3339(quota["rateLimitedUntil"].as_str().unwrap())
                            .unwrap()
                            .to_utc(),
                        until
                    );
                    let expected = expected.map(str::to_owned).unwrap_or_else(|| {
                        zone.local(until).format("%Y-%m-%d %H:%M:%S").to_string()
                    });
                    assert_eq!(quota["rateLimitRecoveryDisplay"], expected);
                }
                None => {
                    assert!(quota["rateLimitedUntil"].is_null());
                    assert!(quota["rateLimitRecoveryDisplay"].is_null());
                }
            }
        }
    }
}

fn account_page_item(
    facts: gateway_core::account::AccountStatusFacts,
    now: chrono::DateTime<chrono::Utc>,
    authentication_kind: &str,
) -> gateway_admin::model::accounts::AccountPageItem {
    use gateway_admin::model::{
        Revision,
        accounts::{AccountCapacity, AccountPageItem, AccountRecord},
    };
    use gateway_core::account::resolve_account_status;

    AccountPageItem {
        protection_rules: Vec::new(),
        account: AccountRecord {
            quota_limits: Default::default(),
            quota_paused: facts.quota_paused,
            model_mappings: Default::default(),
            scheduling_role: gateway_core::account::AccountSchedulingRole::Regular,
            allow_credits: false,
            outbound_proxy_id: None,
            outbound_proxy_enabled: true,
            id: "acct_cooldown".to_owned(),
            provider_kind: gateway_core::routing::ProviderKind::new("openai").unwrap(),
            groups: Vec::new(),
            name: "synthetic cooldown account".to_owned(),
            notes: None,
            email: None,
            upstream_user_id: None,
            upstream_account_id: None,
            plan_type: None,
            authentication_kind: authentication_kind.to_owned(),
            credential_revision: Revision::new(1).unwrap(),
            has_refresh_token: authentication_kind == "oauth",
            access_token_expires_at: None,
            next_refresh_at: None,
            enabled: true,
            concurrency_limit: None,
            weight: Default::default(),
            model_access: Default::default(),
            outbound_proxy: None,
            credential_state: facts.credential_state,
            credential_observed_at: now,
            quota: facts.quota,
            last_error_reason: None,
            last_error_message: None,
            created_at: now,
            updated_at: now,
        },
        capacity: AccountCapacity {
            used_slots: None,
            total_slots: None,
        },
        projection: resolve_account_status(&facts, now.into()),
    }
}

#[tokio::test]
async fn account_usage_preserves_periods_cache_counts_and_missing_values() {
    use chrono::Utc;
    use gateway_admin::model::accounts::{AccountModelUsage, AccountUsage};
    use gateway_admin::model::observability::RequestHealth;
    use gateway_admin::model::provider_credentials::{
        ProviderQuota, ProviderQuotaWindow, QuotaLocalUsageAttribution,
    };
    use gateway_core::account::{AccountStatusFacts, CredentialState, QuotaState};

    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let now = Utc::now();
    let facts = AccountStatusFacts {
        quota_paused: false,
        enabled: true,
        credential_state: CredentialState::Ready,
        access_token_expires_at: None,
        quota: QuotaState::unknown(),
        cooldown: None,
        last_error_reason: None,
        last_error_message: None,
    };
    for (authentication_kind, read, write) in [
        ("api_key", Some(13), Some(7)),
        ("api_key", None, None),
        ("oauth", Some(13), Some(7)),
        ("oauth", None, None),
    ] {
        let mut account = account_page_item(facts.clone(), now, authentication_kind);
        if authentication_kind == "oauth" {
            account.account.provider_kind =
                gateway_core::routing::ProviderKind::new("xai").unwrap();
        }
        *fixture.account.lock().unwrap() = Some(account);
        let local_usage = AccountUsage {
            account_id: "acct_cooldown".to_owned(),
            request_count: 1,
            success_count: 1,
            input_tokens: Some(100),
            output_tokens: Some(20),
            cached_input_tokens: read,
            cache_write_input_tokens: write,
            reasoning_output_tokens: Some(3),
            image_input_tokens: None,
            image_output_tokens: None,
            image_request_count: 0,
            image_request_failed_count: 0,
            total_tokens: Some(120),
            cost_coverage: Default::default(),
            costs: Vec::new(),
            last_used_at: Some(now),
            request_buckets: vec![gateway_admin::model::accounts::AccountRequestBucket {
                bucket_start: now,
                request_count: 1,
            }],
            models: [
                (
                    "mixed",
                    RequestHealth::from_counts(1, 2, 3, 1, 1),
                    Some(now),
                ),
                ("failed", RequestHealth::from_counts(0, 1, 0, 0, 0), None),
                ("no-sample", RequestHealth::from_counts(0, 1, 1, 0, 1), None),
            ]
            .into_iter()
            .map(|(model, health, last_used_at)| AccountModelUsage {
                model: model.to_owned(),
                request_count: u64::from(last_used_at.is_some()),
                health,
                input_tokens: None,
                output_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                reasoning_output_tokens: None,
                image_input_tokens: None,
                image_output_tokens: None,
                image_request_count: 0,
                image_request_failed_count: 0,
                total_tokens: None,
                cost_coverage: Default::default(),
                costs: Vec::new(),
                last_used_at,
            })
            .collect(),
        };
        *fixture.account_usage.lock().unwrap() = vec![local_usage.clone()];
        *fixture.provider_quota.lock().unwrap() =
            (authentication_kind == "oauth").then(|| ProviderQuota {
                paused_windows: Vec::new(),
                windows: vec![ProviderQuotaWindow {
                    key: "rolling-day".to_owned(),
                    group: "shortTerm".to_owned(),
                    label: "日限额".to_owned(),
                    limit_id: None,
                    limit_name: None,
                    role: None,
                    local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
                    window_seconds: Some(86_400),
                    used_percent: None,
                    reset_at: None,
                    limit_reached: false,
                    local_usage: Some(local_usage),
                    provider_data: None,
                }],
                ..Default::default()
            });
        let response = admin::accounts::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .uri("/api/admin/accounts")
                    .header("x-request-id", "req_account_usage_contract")
                    .header(header::COOKIE, "cpr_session=valid-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 32768).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status, StatusCode::OK, "{value}");
        let usage = &value["data"]["items"][0]["usage"];
        assert_eq!(
            usage["windowLabelDisplay"],
            if authentication_kind == "oauth" {
                "近 24 小时"
            } else {
                "通用额度"
            }
        );
        assert_eq!(usage["models"][0]["successRateDisplay"], "33.3%");
        assert!((usage["models"][0]["successRate"].as_f64().unwrap() - 100.0 / 3.0).abs() < 0.001);
        assert_eq!(usage["models"][1]["successRate"], 0.0);
        assert_eq!(usage["models"][1]["successRateDisplay"], "0.0%");
        assert!(usage["models"][1]["lastUsedAt"].is_null());
        assert!(usage["models"][2]["successRate"].is_null());
        assert_eq!(usage["models"][2]["successRateDisplay"], "—");
        assert_eq!(usage["requestCount"], 1);
        assert_eq!(usage["totalTokens"], 120);
        if authentication_kind == "oauth" {
            let window = &value["data"]["items"][0]["quota"]["windows"][0];
            assert_eq!(window["group"], "shortTerm");
            assert_eq!(window["localUsage"]["requestCount"], 1);
            assert_eq!(
                window["localUsage"]["requestBuckets"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }
        assert_eq!(usage["cachedInputTokens"], serde_json::json!(read));
        assert_eq!(usage["cacheWriteInputTokens"], serde_json::json!(write));
        assert_eq!(
            usage["cachedInputTokensDisplay"],
            if read.is_some() { "13" } else { "—" }
        );
        assert_eq!(
            usage["cacheWriteInputTokensDisplay"],
            if write.is_some() { "7" } else { "—" }
        );
    }
}

#[tokio::test]
async fn connection_update_requires_admin_and_validates_before_calling_the_service() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let input = serde_json::json!({
        "accountId":"acct_api",
        "enabled":true,
        "concurrencyLimit":null,
        "weight":1,
        "groupIds":[],
        "connection":{"baseUrl":"https://api.example.invalid/v1", "transport":"http"}
    });
    for (authenticated, transport, oauth, expected) in [
        (false, "http", false, StatusCode::UNAUTHORIZED),
        (true, "invalid", false, StatusCode::BAD_REQUEST),
        // 夹具没有凭据 Store，合法输入必须进入服务，不能按普通设置静默保存
        (true, "http", false, StatusCode::SERVICE_UNAVAILABLE),
        (true, "http", true, StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let mut input = input.clone();
        input["connection"]["transport"] = serde_json::json!(transport);
        if oauth {
            input["connection"]
                .as_object_mut()
                .unwrap()
                .remove("baseUrl");
        }
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/admin/accounts/update")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-request-id", "req_connection_update");
        if authenticated {
            request = request.header(header::COOKIE, "cpr_session=valid-session");
        }
        let response = admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(request.body(Body::from(input.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[tokio::test]
async fn personal_info_requires_admin_and_a_valid_account_query() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (query, authenticated, expected) in [
        ("?accountId=acct_test", false, StatusCode::UNAUTHORIZED),
        ("", true, StatusCode::BAD_REQUEST),
        ("?accountId=bad", true, StatusCode::BAD_REQUEST),
        (
            "?accountId=acct_test&refresh=true",
            true,
            StatusCode::BAD_REQUEST,
        ),
        // 此夹具未提供账号 Store，合法查询应透传服务不可用，而非绕过查询
        (
            "?accountId=acct_test",
            true,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let mut request = Request::builder()
            .uri(format!("/api/admin/accounts/personal-info{query}"))
            .header("x-request-id", "req_personal_info");
        if authenticated {
            request = request.header(header::COOKIE, "cpr_session=valid-session");
        }
        let response = admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{query}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[tokio::test]
async fn quota_forecast_requires_admin_and_valid_account_query() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (uri, authenticated, expected) in [
        (
            "/api/admin/accounts/quota-forecast?accountId=acct_test",
            false,
            StatusCode::UNAUTHORIZED,
        ),
        (
            "/api/admin/accounts/quota-forecast",
            true,
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/admin/accounts/quota-forecast?accountId=bad",
            true,
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/admin/accounts/quota-forecast?accountId=acct_test&refresh=true",
            true,
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let mut request = Request::builder()
            .uri(uri)
            .header("x-request-id", "req_forecast");
        if authenticated {
            request = request.header(header::COOKIE, "cpr_session=valid-session");
        }
        let response = admin::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{uri}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(value["data"].is_null());
        assert!(value["message"].is_string());
    }
}

#[tokio::test]
async fn concurrency_details_require_admin_validate_paging_and_report_lease_time() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    *fixture.account.lock().unwrap() = Some(account_page_item(
        gateway_core::account::AccountStatusFacts {
            quota_paused: false,
            enabled: true,
            credential_state: gateway_core::account::CredentialState::Ready,
            access_token_expires_at: None,
            quota: gateway_core::account::QuotaState::unknown(),
            cooldown: None,
            last_error_reason: None,
            last_error_message: None,
        },
        chrono::Utc::now(),
        "api_key",
    ));
    let router = admin::accounts::router::<AdminTestState>().with_state(fixture.state());
    for (authenticated, query, expected) in [
        (false, "accountId=acct_cooldown", StatusCode::UNAUTHORIZED),
        (
            true,
            "accountId=acct_cooldown&page=0",
            StatusCode::BAD_REQUEST,
        ),
        (
            true,
            "accountId=acct_cooldown&pageSize=101",
            StatusCode::BAD_REQUEST,
        ),
        (
            true,
            "accountId=acct_cooldown&page=4294967295&pageSize=100",
            StatusCode::BAD_REQUEST,
        ),
        (true, "accountId=acct_missing", StatusCode::NOT_FOUND),
        (
            true,
            "accountId=acct_cooldown&page=2&pageSize=20",
            StatusCode::OK,
        ),
    ] {
        let mut request = Request::builder()
            .uri(format!("/api/admin/accounts/concurrency?{query}"))
            .header("x-request-id", "req_concurrency");
        if authenticated {
            request = request.header(header::COOKIE, "cpr_session=valid-session");
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{query}");
        if expected == StatusCode::OK {
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let body = to_bytes(response.into_body(), 8192).await.unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["data"]["total"], 21);
            assert_eq!(value["data"]["page"], 2);
            assert_eq!(value["data"]["entries"][0]["occupiedMs"], 5000);
            assert_eq!(value["data"]["entries"][0]["clientIp"], "2001:db8::376");
            assert_eq!(
                value["data"]["entries"][0]["startedAtDisplay"],
                "2026-10-17 07:59:45"
            );
        }
    }
}

#[tokio::test]
async fn native_quota_refresh_waits_for_provider_and_preserves_observation() {
    use super::super::ProviderQuotaGate;
    use chrono::Utc;
    use gateway_admin::model::provider_credentials::ProviderQuota;
    use gateway_core::account::{
        AccountStatusFacts, CredentialState, ProviderAccountId, QuotaState,
    };
    use std::{sync::Arc, time::Duration};

    let fixture = AdminTestFixture::new().await;
    let observed_at = Utc::now();
    *fixture.account.lock().unwrap() = Some(account_page_item(
        AccountStatusFacts {
            quota_paused: false,
            enabled: true,
            credential_state: CredentialState::Ready,
            access_token_expires_at: None,
            quota: QuotaState::unknown(),
            cooldown: None,
            last_error_reason: None,
            last_error_message: None,
        },
        observed_at,
        "oauth",
    ));
    *fixture.provider_quota.lock().unwrap() = Some(ProviderQuota {
        paused_windows: Vec::new(),
        plan_type: Some("plus".to_owned()),
        observed_at: Some(observed_at),
        ..Default::default()
    });
    let gate = Arc::new(ProviderQuotaGate {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    *fixture.provider_quota_gate.lock().unwrap() = Some(gate.clone());
    let services = fixture.services.clone();
    let request = tokio::spawn(async move {
        services
            .accounts()
            .quota(&ProviderAccountId::new("acct_cooldown").unwrap(), true)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), gate.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert!(!request.is_finished());
    gate.release.add_permits(1);
    let result = tokio::time::timeout(Duration::from_secs(2), request)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.quota.observed_at, Some(observed_at));
    assert_eq!(result.account.plan_type.as_deref(), Some("plus"));
}

#[tokio::test]
async fn quota_paused_accounts_expose_local_policy_without_faking_exhaustion() {
    use chrono::Utc;
    use gateway_admin::model::provider_credentials::{
        ProviderQuota, ProviderQuotaWindow, QuotaLocalUsageAttribution,
    };
    use gateway_core::account::{AccountStatusFacts, CredentialState, QuotaState};
    use serde_json::json;

    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let now = Utc::now();
    let mut account = account_page_item(
        AccountStatusFacts {
            quota_paused: true,
            enabled: true,
            credential_state: CredentialState::Ready,
            access_token_expires_at: None,
            quota: QuotaState::allowed(now.into()),
            cooldown: None,
            last_error_reason: None,
            last_error_message: None,
        },
        now,
        "oauth",
    );
    account.account.quota_limits = serde_json::from_value(json!({"week": 80})).unwrap();
    *fixture.account.lock().unwrap() = Some(account);
    *fixture.provider_quota.lock().unwrap() = Some(ProviderQuota {
        observed_at: Some(now),
        paused_windows: vec!["week".to_owned()],
        windows: [
            ("week", QuotaLocalUsageAttribution::AccountWide),
            ("model", QuotaLocalUsageAttribution::Unavailable),
        ]
        .into_iter()
        .map(|(key, local_usage_attribution)| ProviderQuotaWindow {
            key: key.to_owned(),
            group: "shortTerm".to_owned(),
            label: key.to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution,
            window_seconds: Some(604_800),
            used_percent: Some(80.0),
            reset_at: None,
            limit_reached: false,
            local_usage: None,
            provider_data: None,
        })
        .collect(),
        ..Default::default()
    });
    let response = admin::accounts::router::<AdminTestState>()
        .with_state(fixture.state())
        .oneshot(
            Request::builder()
                .uri("/api/admin/accounts?status=quota_paused")
                .header("x-request-id", "req_quota_paused")
                .header(header::COOKIE, "cpr_session=valid-session")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 32_768).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::OK, "{value}");
    let account = &value["data"]["items"][0];
    assert_eq!(account["status"], "quota_paused");
    assert_eq!(account["quotaLimits"], json!({"week": 80}));
    assert_eq!(account["capabilities"]["quotaLimitsSupported"], true);
    assert_eq!(account["allowCredits"], false);
    assert_eq!(account["quota"]["pausedWindows"], json!(["week"]));
    assert_eq!(account["quota"]["limitReached"], false);
    assert_eq!(account["quota"]["windows"][0]["usedPercent"], 80.0);
    assert_eq!(account["quota"]["windows"][0]["accountWide"], true);
    assert_eq!(account["quota"]["windows"][1]["accountWide"], false);
    assert_eq!(value["data"]["summary"]["quotaPaused"], 1);
    assert_eq!(value["data"]["summary"]["quotaExhausted"], 0);
    assert_eq!(value["data"]["summary"]["normal"], 0);
}

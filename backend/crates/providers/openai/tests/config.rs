//! 验证 OpenAI 配置默认值、路径派生与上游地址约束

use std::{path::Path, time::Duration};

use provider_openai::config::{DEFAULT_STREAM_MAX_RETRIES, MAX_STREAM_MAX_RETRIES, OpenAiConfig};

#[test]
fn openai_config_preserves_optional_routing_header_values() {
    for (document, expected) in [
        (r#"{}"#, (None, None)),
        (
            r#"{"residency":null,"account_routing_override":null}"#,
            (None, None),
        ),
        (
            r#"{"residency":"us","account_routing_override":"us"}"#,
            (Some("us"), Some("us")),
        ),
        (
            r#"{"residency":"custom-region","account_routing_override":"custom-route"}"#,
            (Some("custom-region"), Some("custom-route")),
        ),
        (
            r#"{"residency":"","account_routing_override":"NO_CONSTRAINT"}"#,
            (Some(""), Some("NO_CONSTRAINT")),
        ),
    ] {
        let config: OpenAiConfig = serde_json::from_str(document).unwrap();
        assert_eq!(
            (
                config.residency.as_deref(),
                config.account_routing_override.as_deref()
            ),
            expected,
            "{document}",
        );
    }
}

#[test]
fn openai_config_derives_identity_secret_from_runtime_data_dir() {
    let mut config = valid_config();
    let runtime_data_dir = Path::new("/srv/gateway/runtime-data");
    config
        .resolve_and_validate(runtime_data_dir)
        .expect("valid OpenAI config");

    let path = runtime_data_dir.join("identity_hmac_secret");
    assert!(format!("{config:?}").contains(&format!("identity_secret_path: {path:?}")));
}

#[test]
fn openai_config_restricts_upstream_base_url_to_https_or_loopback_http() {
    for base_url in [
        "http://internal.example.com/backend-api",
        "http://10.0.0.7/backend-api",
        "https://chatgpt.com/backend-api?debug=1",
        "https://user:pass@chatgpt.com/backend-api",
        "https://chatgpt.com/backend-api#fragment",
        "ftp://chatgpt.com/backend-api",
    ] {
        let mut config = valid_config();
        config.api.base_url = base_url.to_owned();
        assert!(
            config
                .resolve_and_validate(Path::new("/srv/gateway"))
                .is_err(),
            "expected {base_url} to be rejected"
        );
    }

    for base_url in [
        "https://chatgpt.com/backend-api",
        "http://127.0.0.1:8080/backend-api",
        "http://localhost:8080/backend-api",
        "http://[::1]:8080/backend-api",
    ] {
        let mut config = valid_config();
        config.api.base_url = base_url.to_owned();
        assert!(
            config
                .resolve_and_validate(Path::new("/srv/gateway"))
                .is_ok(),
            "expected {base_url} to be accepted"
        );
    }
}

#[test]
fn openai_config_defaults_to_the_provider_owned_operating_values() {
    let config = OpenAiConfig::default();
    assert_eq!(DEFAULT_STREAM_MAX_RETRIES, 5);

    assert_eq!(
        (
            config.api.base_url.as_str(),
            config.ws_pool.enabled,
            config.ws_pool.max_age_ms,
            config.ws_pool.max_connecting,
            config.ws_pool.stream_idle_timeout_ms,
            config.quota.refresh_interval_minutes,
            config.auth.refresh_enabled,
            config.auth.oauth_client_id.as_str(),
            config.auth.oauth_token_endpoint.as_str(),
            config.stream_max_retries(),
        ),
        (
            "https://chatgpt.com/backend-api",
            true,
            3_300_000,
            8,
            300_000,
            15,
            true,
            "app_EMoamEEZ73f0CkXaXp7hrann",
            "https://auth.openai.com/oauth/token",
            u32::try_from(DEFAULT_STREAM_MAX_RETRIES).expect("default retry budget fits u32"),
        )
    );
}

#[test]
fn openai_stream_retry_budget_uses_the_official_hard_cap() {
    let mut config = OpenAiConfig::default();
    config.stream_max_retries = MAX_STREAM_MAX_RETRIES + 1;

    assert_eq!(
        config.stream_max_retries(),
        u32::try_from(MAX_STREAM_MAX_RETRIES).expect("retry cap fits u32")
    );
}

#[test]
fn websocket_pool_timeouts_accept_defaults_and_individual_overrides() {
    for (settings, idle_ms, fast_ms) in [
        (
            serde_json::json!({
                "enabled": true,
                "max_age_ms": 3_300_000,
                "max_connecting": 8,
                "stream_idle_timeout_ms": 300_000
            }),
            300_000,
            800,
        ),
        (
            serde_json::json!({"idle_timeout_ms": 600_000}),
            600_000,
            800,
        ),
        (serde_json::json!({"fast_path_budget_ms": 1}), 300_000, 1),
        (
            serde_json::json!({"fast_path_budget_ms": 15_000}),
            300_000,
            15_000,
        ),
    ] {
        let mut config: OpenAiConfig =
            serde_json::from_value(serde_json::json!({"ws_pool": settings})).unwrap();
        config
            .resolve_and_validate(Path::new("/srv/gateway"))
            .unwrap();
        let pool = config.websocket_pool_config();
        assert_eq!(pool.idle_timeout, Duration::from_millis(idle_ms));
        assert_eq!(pool.fast_path_budget, Duration::from_millis(fast_ms));
    }
}

#[test]
fn websocket_fast_path_budget_must_fit_the_opening_timeout() {
    for budget_ms in [0, 15_001] {
        let mut config = valid_config();
        config.ws_pool.fast_path_budget_ms = budget_ms;
        let error = config
            .resolve_and_validate(Path::new("/srv/gateway"))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("openai.ws_pool.fast_path_budget_ms")
        );
    }
}

#[test]
fn websocket_pool_idle_timeout_rejects_zero() {
    let mut config = valid_config();
    config.ws_pool.idle_timeout_ms = 0;
    assert!(
        config
            .resolve_and_validate(Path::new("/srv/gateway"))
            .is_err()
    );
}

fn valid_config() -> OpenAiConfig {
    OpenAiConfig::default()
}

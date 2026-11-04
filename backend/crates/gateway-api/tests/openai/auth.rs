//! 验证 OpenAI 入口的 Bearer 认证与插件可见认证信息边界

use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header::AUTHORIZATION};

use gateway_api::openai::auth::{ClientApiKeyAuthError, identify_codex_client};
use gateway_core::policy::CodexClientKind;
use tower::ServiceExt as _;

#[tokio::test]
async fn native_authentication_checks_the_current_envelope_at_the_http_entry() {
    let fixture = crate::admin::AdminTestFixture::new().await;
    let maximum_key = "x".repeat(
        gateway_core::engine::authentication::MAXIMUM_AUTHORIZATION_BYTES - "Bearer ".len(),
    );
    let oversized_key = format!("{maximum_key}x");
    let mut cases = vec![
        ("missing", "sk_client", None, StatusCode::UNAUTHORIZED),
        (
            "opaque",
            "sk_client",
            Some(HeaderValue::from_bytes(&[0xff]).unwrap()),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "lowercase scheme",
            "sk_client",
            Some(HeaderValue::from_static("bearer sk_client")),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "empty token",
            "sk_client",
            Some(HeaderValue::from_static("Bearer    ")),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "whitespace inside token",
            "sk_client",
            Some(HeaderValue::from_static("Bearer key with spaces")),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "trimmed token",
            "sk_client",
            Some(HeaderValue::from_static("Bearer   sk_client   ")),
            StatusCode::OK,
        ),
    ];
    for key in ["q", "sk-old/key+value=:!@", maximum_key.as_str()] {
        cases.push((
            "configured native key",
            key,
            Some(HeaderValue::from_str(&format!("Bearer {key}")).unwrap()),
            StatusCode::OK,
        ));
    }
    cases.push((
        "oversized envelope",
        oversized_key.as_str(),
        Some(HeaderValue::from_str(&format!("Bearer {oversized_key}")).unwrap()),
        StatusCode::UNAUTHORIZED,
    ));

    for (case, key, authorization, expected) in cases {
        let app =
            super::api_router_with_admin_and_client(fixture.services.clone(), key, "key-auth");
        let mut request = crate::support::empty_request(Method::GET, "/v1/models");
        // actor 标记既不能代替认证，也不能干扰已有的原生 Key
        request.headers_mut().insert(
            "x-openai-actor-authorization",
            HeaderValue::from_static("proxy-managed"),
        );
        if let Some(authorization) = authorization {
            request.headers_mut().insert(AUTHORIZATION, authorization);
        }
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected, "{case}");
    }
}

#[test]
fn client_auth_failure_reasons_should_be_stable_and_secret_free() {
    assert_eq!(
        [
            ClientApiKeyAuthError::MissingAuthorization,
            ClientApiKeyAuthError::MalformedAuthorization,
            ClientApiKeyAuthError::InvalidKeyFormat,
            ClientApiKeyAuthError::InvalidKey,
            ClientApiKeyAuthError::RuntimeUnavailable,
        ]
        .map(ClientApiKeyAuthError::reason),
        [
            "missing_authorization",
            "malformed_authorization",
            "invalid_key_format",
            "invalid_key",
            "runtime_unavailable",
        ]
    );
}

#[test]
fn desktop_headers_should_take_precedence_over_embedded_cli_marker() {
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("Codex Desktop"));
    headers.insert("version", HeaderValue::from_static("26.825.6671"));
    headers.insert(
        "user-agent",
        HeaderValue::from_static("codex_cli_rs/0.39.0 terminal (Codex Desktop; 26.1.0)"),
    );

    let identified = identify_codex_client(&headers).expect("recognized desktop");
    assert_eq!(identified.kind(), CodexClientKind::Desktop);
    assert_eq!(
        identified.version().map(ToString::to_string).as_deref(),
        Some("26.825.6671")
    );
}

#[test]
fn official_core_clients_should_use_the_core_version_instead_of_the_app_suffix() {
    for product in [
        "codex_cli_rs",
        "codex-cli",
        "codex-tui",
        "codex_exec",
        "codex_vscode",
        "codex_sdk_ts",
        "codex_python_sdk",
        "codex-app-server",
        "codex-app-server-sdk",
        "codex_mcp_server",
        "CoDeX-TuI",
    ] {
        for (version, expected) in [
            (Some("0.158.0"), Some("0.158.0")),
            (
                Some("0.165.0-alpha.1+build.2"),
                Some("0.165.0-alpha.1+build.2"),
            ),
            (Some("not-a-version"), None),
            (Some(""), None),
            (None, None),
        ] {
            let prefix = version.map_or_else(|| product.to_owned(), |v| format!("{product}/{v}"));
            let mut headers = HeaderMap::new();
            headers.insert(
                "user-agent",
                HeaderValue::from_str(&format!(
                    "{prefix} (Mac OS 26.6.2; arm64) Apple_Terminal ({product}; 0.165.0)"
                ))
                .unwrap(),
            );
            let identified = identify_codex_client(&headers).expect("recognized official client");
            assert_eq!(identified.kind(), CodexClientKind::Cli, "{prefix}");
            assert_eq!(
                identified.version().map(ToString::to_string).as_deref(),
                expected,
                "{prefix}"
            );
        }
    }
}

#[test]
fn unknown_codex_like_products_should_remain_unrestricted() {
    for product in [
        "custom-codex-tui",
        "codex-tui-custom",
        "codex_cli_rs_custom",
        "codex_python_sdk_custom",
        "my_app",
        "deepseek-harness",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(
            "user-agent",
            HeaderValue::from_str(&format!("{product}/0.158.0 (Linux; x86_64)")).unwrap(),
        );
        assert_eq!(identify_codex_client(&headers), None, "{product}");
    }
}

#[test]
fn chatgpt_remote_desktop_without_app_version_should_not_use_a_version_gate() {
    for name in [
        "codex_chatgpt_android_remote",
        "codex_chatgpt_ios_remote",
        "codex_chatgpt_future_os_remote",
    ] {
        for remote_version in ["dev", "1.2.3"] {
            for (product, originator) in [
                ("Codex Desktop", None),
                ("codex_cli_rs", Some("Codex Desktop")),
            ] {
                let mut headers = HeaderMap::new();
                headers.insert(
                    "user-agent",
                    HeaderValue::from_str(&format!(
                        "{product}/0.154.0-alpha.6.2 (Windows 10.0.26200; x86_64) unknown ({name}; {remote_version})"
                    ))
                    .unwrap(),
                );
                if let Some(originator) = originator {
                    headers.insert("originator", HeaderValue::from_static(originator));
                }
                assert_eq!(identify_codex_client(&headers), None, "{headers:?}");
            }
        }
    }
}

#[test]
fn chatgpt_remote_marker_should_require_a_complete_exact_client_suffix() {
    for suffix in [
        "(codex_chatgpt_android_remote_extra; dev)",
        "(unofficial_codex_chatgpt_ios_remote; dev)",
        "(codex_chatgpt__remote; dev)",
        "(codex_chatgpt_remote; dev)",
        "(codex_chatgpt_future os_remote; dev)",
        "(codex_chatgpt_future;os_remote; dev)",
        "codex_chatgpt_android_remote; dev",
        "(codex_chatgpt_android_remote; dev",
        "(codex_chatgpt_android_remote; dev) trailing",
        "(codex_chatgpt_android_remote; dev) (another_client; dev)",
        "(codex_chatgpt_android_remote; )",
        "(another_client; codex_chatgpt_android_remote)",
        "(Codex Desktop; dev)",
        "(Codex Desktop; ) (codex_chatgpt_android_remote; dev)",
        "(Codex Desktop; invalid) (codex_chatgpt_ios_remote; dev)",
        "(Codex Desktop)",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(
            "user-agent",
            HeaderValue::from_str(&format!("Codex Desktop/0.154.0-alpha.6.2 {suffix}")).unwrap(),
        );
        let client = identify_codex_client(&headers).expect("Desktop still requires a version");
        assert_eq!(client.kind(), CodexClientKind::Desktop, "{suffix}");
        assert!(client.version().is_none(), "{suffix}");
    }
}

#[test]
fn chatgpt_remote_desktop_should_preserve_explicit_version_validation() {
    for (value, expected) in [
        (
            HeaderValue::from_static("26.908.70816"),
            Some("26.908.70816"),
        ),
        (HeaderValue::from_static("26.1.0"), Some("26.1.0")),
        (HeaderValue::from_static("dev"), None),
        (HeaderValue::from_static(""), None),
        (HeaderValue::from_bytes(&[0xff]).unwrap(), None),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("version", value);
        headers.insert(
            "user-agent",
            HeaderValue::from_static(
                "Codex Desktop/0.154.0-alpha.6.2 (codex_chatgpt_android_remote; dev)",
            ),
        );
        let client = identify_codex_client(&headers).expect("explicit Desktop version");
        assert_eq!(client.kind(), CodexClientKind::Desktop);
        assert_eq!(
            client.version().map(ToString::to_string).as_deref(),
            expected
        );
    }
}

#[test]
fn chatgpt_remote_marker_should_not_override_an_existing_desktop_version_suffix() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "user-agent",
        HeaderValue::from_static(
            "Codex Desktop/0.154.0-alpha.6.2 (Codex Desktop; 26.1.0) (codex_chatgpt_ios_remote; dev)",
        ),
    );
    let client = identify_codex_client(&headers).expect("Desktop version remains available");
    assert_eq!(client.version().unwrap().to_string(), "26.1.0");
}

#[test]
fn chatgpt_remote_suffix_created_by_header_truncation_should_not_skip_the_gate() {
    let suffix = " (codex_chatgpt_android_remote; dev)";
    let mut user_agent = "Codex Desktop/0.154.0-alpha.6.2 ".to_owned();
    user_agent.push_str(&"x".repeat(4096 - user_agent.len() - suffix.len()));
    user_agent.push_str(suffix);
    user_agent.push_str(" (another_client; dev)");
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", HeaderValue::from_str(&user_agent).unwrap());
    let client =
        identify_codex_client(&headers).expect("incomplete headers still require a version");
    assert_eq!(client.kind(), CodexClientKind::Desktop);
    assert!(client.version().is_none());
}

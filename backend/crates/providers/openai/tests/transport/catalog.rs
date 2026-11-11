//! 验证原生模型目录保留未知字段并生成安全能力投影

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use gateway_core::routing::ModelServiceTier;
use provider_openai::credential::{
    ApiKeyProtocol, CodexCredentialCatalogService, ResponsesTransport,
};
use provider_openai::transport::profile::{CodexWireProfile, CodexWireProfileState};
use provider_openai::transport::{
    CodexBackendClient, CodexCatalogCapabilityEvidence, CodexCatalogVisibility,
    CodexModelCatalogError, CodexRequestContext, build_reqwest_client, parse_codex_model_catalog,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{MemoryAccountStore, catalog_cache};

const OFFICIAL_FIXTURE: &[u8] = include_bytes!("fixtures/official_models_snapshot.json");

#[test]
fn native_document_keeps_unknown_nested_fields_nulls_and_model_instructions() {
    let original = json!({
        "slug": "gpt-native", "display_name": "Native",
        "base_instructions": "original model instructions\nwith line breaks",
        "model_messages": {"future_instructions": {"template": "do not replace"}},
        "future_field": [null, {"new": true}], "explicit_null": null,
        "service_tiers": [{"id":"priority", "name":"Fast", "description":"1.5x speed", "future":true}]
    });
    let body = serde_json::to_vec(&json!({"models":[original]})).expect("body");
    let snapshot = parse_codex_model_catalog(&body, None).expect("catalog");
    let model = &snapshot.models()[0];
    let preserved: Value = serde_json::from_slice(model.document().body()).expect("document");
    assert_eq!(preserved, original);
    assert!(!format!("{model:?}").contains("original model instructions"));
    assert!(!format!("{model:?}").contains("future_instructions"));
    let mut changed = original;
    changed["model_messages"]["future_instructions"]["template"] = json!("new content");
    let changed = parse_codex_model_catalog(
        &serde_json::to_vec(&json!({"models":[changed]})).expect("body"),
        None,
    )
    .expect("catalog");
    assert_ne!(snapshot.models(), changed.models());
}

#[test]
fn official_fixture_should_produce_safe_full_snapshot() {
    let snapshot = parse_codex_model_catalog(OFFICIAL_FIXTURE, Some("W/\"codex-v1\""))
        .expect("official fixture should parse");
    let model = &snapshot.models()[0];

    assert_eq!(
        (
            snapshot.models().len(),
            snapshot.etag(),
            model.request_model().as_str(),
            model.display_name(),
            model
                .limits()
                .context_window_tokens()
                .map(|value| value.get()),
            model
                .limits()
                .max_context_window_tokens()
                .map(|value| value.get()),
            (
                model.capabilities().responses_api(),
                model.capabilities().reasoning(),
                model.capabilities().parallel_tool_calls(),
                model.capabilities().text_input(),
                model.capabilities().image_input(),
                model.capabilities().web_search(),
                model.capabilities().reasoning_efforts(),
            ),
            model.metadata().description(),
            model.metadata().priority(),
        ),
        (
            1,
            Some("W/\"codex-v1\""),
            "gpt-5.4",
            "GPT-5.4",
            Some(272_000),
            Some(272_000),
            (
                CodexCatalogCapabilityEvidence::DeclaredNative,
                CodexCatalogCapabilityEvidence::DeclaredNative,
                CodexCatalogCapabilityEvidence::DeclaredNative,
                CodexCatalogCapabilityEvidence::DeclaredNative,
                CodexCatalogCapabilityEvidence::DeclaredNative,
                CodexCatalogCapabilityEvidence::DeclaredNative,
                &["low".to_owned(), "high".to_owned()][..],
            ),
            Some("Frontier agentic coding model."),
            Some(1),
        )
    );
}

#[test]
fn raw_instructions_and_unknown_fields_should_not_appear_in_snapshot_debug() {
    let snapshot =
        parse_codex_model_catalog(OFFICIAL_FIXTURE, None).expect("official fixture should parse");
    let debug = format!("{snapshot:?}");

    assert!(
        !debug.contains("provider-only instructions")
            && !debug.contains("provider-only template")
            && !debug.contains("available_in_plans")
    );
}

#[test]
fn service_tiers_should_preserve_declared_ids_names_and_descriptions() {
    let body = json!({"models": [{
        "slug": "gpt-future",
        "display_name": "Future",
        "service_tiers": [
            {"id": "priority", "name": "Fast", "description": "Priority processing."},
            {"id": "deferred", "name": "Economy", "description": ""}
        ]
    }]});
    let snapshot = parse_codex_model_catalog(&serde_json::to_vec(&body).expect("JSON"), None)
        .expect("declared service tiers");

    assert_eq!(
        snapshot.models()[0].metadata().service_tiers(),
        [
            ModelServiceTier::new("priority", "Fast", "Priority processing."),
            ModelServiceTier::new("deferred", "Economy", ""),
        ]
    );
}

#[test]
fn missing_or_empty_service_tiers_should_not_infer_fast_from_legacy_metadata() {
    for service_tiers in [None, Some(json!([]))] {
        let mut model = json!({
            "slug": "gpt-5.4",
            "display_name": "GPT-5.4",
            "additional_speed_tiers": ["fast"]
        });
        if let Some(service_tiers) = service_tiers {
            model["service_tiers"] = service_tiers;
        }
        let body = json!({"models": [model]});
        let snapshot = parse_codex_model_catalog(&serde_json::to_vec(&body).expect("JSON"), None)
            .expect("optional service tiers");

        assert!(snapshot.models()[0].metadata().service_tiers().is_empty());
    }
}

#[test]
fn invalid_service_tier_metadata_should_fail_the_entire_snapshot() {
    for (field, value) in [
        ("id", String::new()),
        ("id", "p".repeat(65)),
        ("name", " ".to_owned()),
        ("name", "f".repeat(257)),
        ("name", "Fast\u{1b}".to_owned()),
        ("description", "d".repeat(4097)),
        ("description", "unsafe\u{0}".to_owned()),
    ] {
        let mut tier = json!({"id": "priority", "name": "Fast", "description": "Priority."});
        tier[field] = Value::String(value);
        let body = json!({"models": [{
            "slug": "gpt-5.4", "display_name": "GPT-5.4", "service_tiers": [tier]
        }]});

        assert_eq!(
            parse_codex_model_catalog(&serde_json::to_vec(&body).expect("JSON"), None),
            Err(CodexModelCatalogError::InvalidMetadata),
            "invalid {field}"
        );
    }
}

#[test]
fn malformed_service_tiers_should_fail_the_official_wire_contract() {
    for service_tiers in [
        Value::Null,
        json!("fast"),
        json!([{"id": "priority", "name": "Fast"}]),
        json!([{"id": 1, "name": "Fast", "description": "Priority."}]),
    ] {
        let body = json!({"models": [{
            "slug": "gpt-5.4", "display_name": "GPT-5.4", "service_tiers": service_tiers
        }]});

        assert_eq!(
            parse_codex_model_catalog(&serde_json::to_vec(&body).expect("JSON"), None),
            Err(CodexModelCatalogError::InvalidWire)
        );
    }
}

#[test]
fn invalid_etag_should_fail_the_entire_snapshot() {
    let result = parse_codex_model_catalog(OFFICIAL_FIXTURE, Some("raw-unquoted-etag"));

    assert!(matches!(result, Err(CodexModelCatalogError::InvalidEtag)));
}

#[test]
fn missing_capability_fields_should_remain_unknown() {
    let snapshot = parse_codex_model_catalog(
        br#"{"models":[{"slug":"gpt-unknown","display_name":"Unknown"}]}"#,
        None,
    )
    .expect("identity-only official entry should parse");
    let model = &snapshot.models()[0];

    assert_eq!(
        (
            model.capabilities().responses_api(),
            model.capabilities().reasoning(),
            model.capabilities().parallel_tool_calls(),
            model.capabilities().text_input(),
            model.capabilities().image_input(),
            model.limits().context_window_tokens(),
        ),
        (
            CodexCatalogCapabilityEvidence::Unknown,
            CodexCatalogCapabilityEvidence::Unknown,
            CodexCatalogCapabilityEvidence::Unknown,
            CodexCatalogCapabilityEvidence::Unknown,
            CodexCatalogCapabilityEvidence::Unknown,
            None,
        )
    );
}

#[test]
fn legacy_data_shape_should_fail_the_entire_snapshot() {
    let result = parse_codex_model_catalog(
        br#"{"data":[{"slug":"gpt-5.4","display_name":"GPT-5.4"}]}"#,
        None,
    );

    assert!(matches!(result, Err(CodexModelCatalogError::InvalidWire)));
}

#[test]
fn empty_models_should_fail_the_entire_snapshot() {
    let result = parse_codex_model_catalog(br#"{"models":[]}"#, None);

    assert!(matches!(result, Err(CodexModelCatalogError::EmptySnapshot)));
}

#[test]
fn duplicate_request_slugs_should_fail_the_entire_snapshot() {
    let result = parse_codex_model_catalog(
        br#"{"models":[{"slug":"gpt-5.4","display_name":"A"},{"slug":"gpt-5.4","display_name":"B"}]}"#,
        None,
    );

    assert!(matches!(
        result,
        Err(CodexModelCatalogError::DuplicateModelSlug)
    ));
}

#[test]
fn unknown_top_level_pagination_fields_should_not_reject_the_snapshot() {
    let snapshot = parse_codex_model_catalog(
        br#"{"models":[{"slug":"gpt-5.4","display_name":"GPT-5.4"}],"has_more":true,"cursor":"next"}"#,
        None,
    )
    .expect("unknown pagination fields are not part of the model contract");

    assert_eq!(snapshot.models()[0].request_model().as_str(), "gpt-5.4");
}

#[test]
fn unknown_input_modality_should_degrade_capability_evidence_to_unknown() {
    let snapshot = parse_codex_model_catalog(
        br#"{"models":[{"slug":"gpt-future","display_name":"Future","input_modalities":["audio"]}]}"#,
        None,
    )
    .expect("future modality should not reject the snapshot");
    let capabilities = snapshot.models()[0].capabilities();

    assert_eq!(
        (capabilities.text_input(), capabilities.image_input()),
        (
            CodexCatalogCapabilityEvidence::Unknown,
            CodexCatalogCapabilityEvidence::Unknown
        )
    );
}

#[test]
fn unknown_visibility_should_degrade_without_rejecting_the_snapshot() {
    let snapshot = parse_codex_model_catalog(
        br#"{"models":[{"slug":"gpt-future","display_name":"Future","visibility":"preview_only"}]}"#,
        None,
    )
    .expect("future visibility should not reject the snapshot");

    assert_eq!(
        snapshot.models()[0].metadata().visibility(),
        Some(CodexCatalogVisibility::Unknown)
    );
}

#[test]
fn invalid_request_slug_should_fail_the_entire_snapshot() {
    let result = parse_codex_model_catalog(
        br#"{"models":[{"slug":"gpt-5.4","display_name":"Good"},{"slug":"https://evil.invalid/model","display_name":"Bad"}]}"#,
        None,
    );

    assert!(matches!(
        result,
        Err(CodexModelCatalogError::InvalidModelSlug)
    ));
}

#[tokio::test]
async fn fetch_should_send_official_catalog_headers_and_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .and(query_param("client_version", "0.144.0"))
        .and(header("authorization", "Bearer oauth-access"))
        .and(header("chatgpt-account-id", "acct_123"))
        .and(header("originator", "codex_cli_rs"))
        .and(header(
            "user-agent",
            "codex_cli_rs/0.144.0 (linux 6.8; x86_64) xterm (codex_cli_rs; 1.0.0)",
        ))
        .and(header("accept", "*/*"))
        .and(header("version", "0.144.0"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .insert_header("etag", "\"catalog-v1\"")
                .set_body_raw(OFFICIAL_FIXTURE, "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = CodexBackendClient::new(
        build_reqwest_client().expect("build Codex client"),
        server.uri(),
        profile(),
    );
    let snapshot = client
        .fetch_models_with_context(context(), None)
        .await
        .expect("fetch strict snapshot");

    assert_eq!(snapshot.models()[0].request_model().as_str(), "gpt-5.4");
    let requests = server
        .received_requests()
        .await
        .expect("received model catalog request");
    let headers = &requests[0].headers;
    for forbidden in [
        "content-type",
        "openai-beta",
        "x-client-request-id",
        "session_id",
        "session-id",
        "thread-id",
    ] {
        assert!(
            headers.get(forbidden).is_none(),
            "unexpected {forbidden} header"
        );
    }
}

#[tokio::test]
async fn fetch_preserves_large_model_instructions() {
    let instructions = "model instructions\n".repeat(100_000);
    let body = json!({"models":[{
        "slug":"gpt-native", "display_name":"Native", "base_instructions":instructions,
        "model_messages":{"future":{"template":"preserved"}}
    }]});
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/codex/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .expect(1)
        .mount(&server)
        .await;
    let client = CodexBackendClient::new(
        build_reqwest_client().expect("build Codex client"),
        server.uri(),
        profile(),
    );

    let result = client
        .fetch_models_with_context(context(), Some("0.166.0"))
        .await
        .unwrap();
    let preserved: Value = serde_json::from_slice(result.models()[0].document().body()).unwrap();
    assert!(preserved == body["models"][0]);
}

fn profile() -> CodexWireProfileState {
    CodexWireProfileState::new(CodexWireProfile {
        client_kind: provider_openai::transport::profile::selection::ClientKind::Desktop,
        originator: "codex_cli_rs".to_owned(),
        codex_version: "0.144.0".to_owned(),
        desktop_version: "1.0.0".to_owned(),
        desktop_build: "1".to_owned(),
        os_type: "linux".to_owned(),
        os_version: "6.8".to_owned(),
        arch: "x86_64".to_owned(),
        terminal: "xterm".to_owned(),
        exact_user_agent: None,
        installation_seed: None,
        installation_id: None,
        residency: None,
        account_routing_override: None,
        verified_at: Utc
            .with_ymd_and_hms(2026, 7, 18, 0, 0, 0)
            .single()
            .expect("valid fixture time"),
    })
}

fn context() -> CodexRequestContext<'static> {
    CodexRequestContext {
        response_control: None,
        trace: None,
        authorization: "Bearer oauth-access",
        account_id: Some("acct_123"),
        request_id: "req_catalog",
        turn_state: None,
        turn_metadata: None,
        beta_features: None,
        include_timing_metrics: None,
        version: None,
        codex_window_id: None,
        parent_thread_id: None,
        cookie_header: None,
        installation_id: Some("installation-123"),
        session_id: None,
        thread_id: None,
        client_request_id: None,
        turn_id: None,
        account_selection: Default::default(),
    }
}

async fn api_catalog_fixture(
    server: &MockServer,
    protocol: ApiKeyProtocol,
) -> (Arc<MemoryAccountStore>, CodexCredentialCatalogService) {
    let store = Arc::new(MemoryAccountStore::default());
    store
        .seed_api_key_for_protocol(
            "acct_transport_catalog",
            format!("{}/v1", server.uri()),
            ResponsesTransport::Http,
            protocol,
        )
        .await;
    let service = CodexCredentialCatalogService::new(
        store.repository(),
        profile(),
        reqwest::Client::builder().no_proxy().build().unwrap(),
        "http://unused-oauth.invalid".to_owned(),
        catalog_cache(),
    );
    (store, service)
}

#[tokio::test]
async fn messages_catalog_follows_official_cursors_without_openai_identity_or_version_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("x-api-key", "sk-api-test-only"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(query_param("limit", "1000"))
        .respond_with(|request: &wiremock::Request| {
            let cursor = request
                .url
                .query_pairs()
                .find(|(name, _)| name == "after_id")
                .map(|(_, value)| value.into_owned());
            let body = match cursor.as_deref() {
                None => json!({
                    "data":[{"id":"claude-first","display_name":"First","max_tokens":4096,"capabilities":{"thinking":{"supported":true}}}],
                    "has_more":true,"last_id":"claude-first"
                }),
                Some("claude-first") => json!({
                    "data":[{"id":"claude-second","display_name":"Second"}],
                    "has_more":false,"last_id":"claude-second"
                }),
                other => panic!("unexpected Messages cursor: {other:?}"),
            };
            ResponseTemplate::new(200)
                .insert_header("etag", "\"messages-v1\"")
                .set_body_json(body)
        })
        .expect(2)
        .mount(&server)
        .await;
    let (store, service) = api_catalog_fixture(&server, ApiKeyProtocol::Messages).await;
    let account = store.account("acct_transport_catalog").unwrap();

    let (models, _) = service.account_catalog_documents(&account).await.unwrap();

    assert_eq!(
        models
            .iter()
            .map(|model| (model.request_model().as_str(), model.display_name()))
            .collect::<Vec<_>>(),
        [("claude-first", "First"), ("claude-second", "Second")]
    );
    for model in &models {
        assert_eq!(
            model.capabilities().responses_api(),
            CodexCatalogCapabilityEvidence::Unknown
        );
        assert_eq!(model.document().protocol(), "openai");
    }
    for request in server.received_requests().await.unwrap() {
        assert!(
            !request
                .url
                .query_pairs()
                .any(|(name, _)| name == "client_version")
        );
        for name in [
            "authorization",
            "chatgpt-account-id",
            "cookie",
            "originator",
            "user-agent",
            "version",
            "x-codex-installation-id",
            "session-id",
            "thread-id",
            "openai-beta",
        ] {
            assert!(!request.headers.contains_key(name), "unexpected {name}");
        }
    }
}

#[tokio::test]
async fn messages_catalog_rejects_invalid_pagination_and_duplicate_models_atomically() {
    for (first, second, requests) in [
        (
            json!({"data":[{"id":"claude-a"}],"has_more":true}),
            json!({}),
            1,
        ),
        (
            json!({"data":[{"id":"claude-a"}],"has_more":true,"last_id":""}),
            json!({}),
            1,
        ),
        (
            json!({"data":[{"id":"claude-a"}],"has_more":true,"last_id":"claude-a"}),
            json!({"data":[{"id":"claude-b"}],"has_more":true,"last_id":"claude-a"}),
            2,
        ),
        (
            json!({"data":[{"id":"claude-a"}],"has_more":true,"last_id":"claude-a"}),
            json!({"data":[{"id":"claude-a"}],"has_more":false}),
            2,
        ),
        (
            json!({"data":[{"id":"claude-a"}],"has_more":true,"last_id":"claude-a"}),
            json!({"has_more":false}),
            2,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(move |request: &wiremock::Request| {
                let body = if request
                    .url
                    .query_pairs()
                    .any(|(name, _)| name == "after_id")
                {
                    second.clone()
                } else {
                    first.clone()
                };
                ResponseTemplate::new(200).set_body_json(body)
            })
            .expect(requests)
            .mount(&server)
            .await;
        let (store, service) = api_catalog_fixture(&server, ApiKeyProtocol::Messages).await;
        let account = store.account("acct_transport_catalog").unwrap();

        assert!(service.account_catalog_documents(&account).await.is_err());
        assert_eq!(
            server.received_requests().await.unwrap().len() as u64,
            requests
        );
    }
}

#[tokio::test]
async fn messages_catalog_applies_the_model_limit_across_pages() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(|request: &wiremock::Request| {
            let after = request
                .url
                .query_pairs()
                .any(|(name, _)| name == "after_id");
            let data = if after {
                vec![json!({"id":"claude-overflow"})]
            } else {
                (0..2048)
                    .map(|index| json!({"id":format!("claude-{index}")}))
                    .collect()
            };
            ResponseTemplate::new(200).set_body_json(json!({
                "data":data,"has_more":!after,"last_id":"claude-2047"
            }))
        })
        .expect(2)
        .mount(&server)
        .await;
    let (store, service) = api_catalog_fixture(&server, ApiKeyProtocol::Messages).await;
    let account = store.account("acct_transport_catalog").unwrap();

    assert!(service.account_catalog_documents(&account).await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn api_catalogs_distinguish_native_objects_from_standard_ids_without_protocol_regressions() {
    for (protocol, native, with_data) in [
        (ApiKeyProtocol::Responses, false, true),
        (ApiKeyProtocol::Responses, true, false),
        (ApiKeyProtocol::Responses, true, true),
        (ApiKeyProtocol::Messages, false, true),
        (ApiKeyProtocol::Messages, true, false),
        (ApiKeyProtocol::Messages, true, true),
    ] {
        let server = MockServer::start().await;
        let original = json!({
            "slug":"vendor/model","display_name":"Native","base_instructions":"preserved",
            "supported_in_api":true,"future":{"keep":true}
        });
        let mut body = if native {
            json!({"models":[original]})
        } else {
            json!({"data":[{"id":"vendor/model","future":{"keep":true}}],"models":[{"id":"ignored-extension"}]})
        };
        if native && with_data {
            body["data"] = json!([{"id":"ignored-standard"}]);
            body["has_more"] = json!(true);
        }
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header(
                if protocol == ApiKeyProtocol::Messages {
                    "x-api-key"
                } else {
                    "authorization"
                },
                if protocol == ApiKeyProtocol::Messages {
                    "sk-api-test-only"
                } else {
                    "Bearer sk-api-test-only"
                },
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let (store, service) = api_catalog_fixture(&server, protocol).await;
        let account = store.account("acct_transport_catalog").unwrap();

        let (models, _) = service.account_catalog_documents(&account).await.unwrap();

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].request_model().as_str(), "vendor/model");
        let document: Value = serde_json::from_slice(models[0].document().body()).unwrap();
        assert_eq!(
            document,
            if native {
                original
            } else {
                json!({"id":"vendor/model"})
            }
        );
        let requests = server.received_requests().await.unwrap();
        let request = &requests[0];
        if protocol == ApiKeyProtocol::Messages {
            assert!(
                request
                    .url
                    .query_pairs()
                    .any(|(name, value)| name == "limit" && value == "1000")
            );
            assert!(!request.headers.contains_key("authorization"));
            assert_eq!(request.headers["anthropic-version"], "2023-06-01");
        } else {
            assert!(
                !request
                    .url
                    .query_pairs()
                    .any(|(name, _)| matches!(name.as_ref(), "limit" | "after_id"))
            );
            assert!(!request.headers.contains_key("x-api-key"));
            assert!(!request.headers.contains_key("anthropic-version"));
        }
        assert!(!request.headers.contains_key("chatgpt-account-id"));
    }
}

#[tokio::test]
async fn messages_catalog_does_not_publish_a_partial_snapshot_after_a_later_page_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(|request: &wiremock::Request| {
            if request.url.query_pairs().any(|(name, _)| name == "after_id") {
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "7")
                    .set_body_json(json!({"type":"error","error":{"type":"rate_limit_error","message":"next page limited"}}))
            } else {
                ResponseTemplate::new(200).set_body_json(json!({
                    "data":[{"id":"claude-first"}],"has_more":true,"last_id":"claude-first"
                }))
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let (store, service) = api_catalog_fixture(&server, ApiKeyProtocol::Messages).await;
    let account = store.account("acct_transport_catalog").unwrap();

    assert!(service.account_catalog_documents(&account).await.is_err());
    assert!(
        service
            .read_account_catalog(&account)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn messages_catalog_does_not_downgrade_invalid_native_models_to_standard_data() {
    for models in [
        json!([{"slug":"native-model"}]),
        json!([{"display_name":"Missing slug"}]),
        json!([{"slug":null,"display_name":"Native model"}]),
        json!([{"slug":"vendor/model","display_name":"Native model","context_window":-1}]),
        json!([
            {"slug":"native-model","display_name":"First"},
            {"slug":"native-model","display_name":"Duplicate"}
        ]),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "models":models,"data":[{"id":"valid-standard"}],"has_more":false
            })))
            .expect(1)
            .mount(&server)
            .await;
        let (store, service) = api_catalog_fixture(&server, ApiKeyProtocol::Messages).await;
        let account = store.account("acct_transport_catalog").unwrap();

        assert!(service.account_catalog_documents(&account).await.is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn messages_standard_catalog_ignores_unrelated_models_metadata() {
    for metadata in [
        json!(null),
        json!({"display_name":"top-level metadata"}),
        json!([]),
        json!(["metadata"]),
        json!([{"id":"unrelated-extension"}]),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "models":metadata,"data":[{"id":"vendor/standard"}],"has_more":false
            })))
            .expect(1)
            .mount(&server)
            .await;
        let (store, service) = api_catalog_fixture(&server, ApiKeyProtocol::Messages).await;
        let account = store.account("acct_transport_catalog").unwrap();

        let (models, _) = service.account_catalog_documents(&account).await.unwrap();

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].request_model().as_str(), "vendor/standard");
        assert_eq!(models[0].document().protocol(), "openai");
        let document: Value = serde_json::from_slice(models[0].document().body()).unwrap();
        assert_eq!(document, json!({"id":"vendor/standard"}));
    }
}

//! Messages HTTP 认证、身份隔离、代理、隐私与最终出站字节的真实传输合同

use super::*;
use crate::transport::privacy::{policy, rule};
use gateway_core::engine::middleware::outbound::Target;
use gateway_core::settings::privacy::{PrivacyAction, PrivacyScope};
use provider_openai::credential::{ApiKeyProtocol, ResponsesTransport};

const MESSAGES_JSON: &str = "{ \"id\":\"msg_transport\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-5.4\",\"content\":[{\"type\":\"text\",\"text\":\"upstream result\"}],\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2},\"future\":true }\n";
const MESSAGES_EVENTS: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_transport\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-5.4\",\"content\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"upstream result\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

async fn seed_messages_transport(store: &Arc<MemoryAccountStore>, base_url: String) {
    store
        .seed_api_key_for_protocol(
            "acct_provider_contract",
            base_url,
            ResponsesTransport::Http,
            ApiKeyProtocol::Messages,
        )
        .await;
}

fn native_messages_request(streaming: bool, headers: &[(&str, &str)]) -> Operation {
    let body = json!({
        "model":"client-alias","max_tokens":64,"stream":streaming,
        "messages":[{"role":"user","content":"private prompt"}],
        "future":{"preserve":true}
    });
    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("anthropic", body.as_object().unwrap().clone())
            .unwrap()
            .with_context(Map::from_iter([(
                "opaque_request_headers".to_owned(),
                Value::Array(
                    headers
                        .iter()
                        .map(|(name, value)| json!([name, STANDARD.encode(value)]))
                        .collect(),
                ),
            )])),
    ))
}

#[tokio::test]
async fn messages_http_defaults_keep_credentials_and_protocol_identity_at_the_account_boundary() {
    for streaming in [false, true] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        seed_messages_transport(&store, format!("{}/v1/", server.uri())).await;
        let raw = if streaming {
            MESSAGES_EVENTS
        } else {
            MESSAGES_JSON
        };
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "sk-api-test-only"))
            .and(header("anthropic-version", "2023-06-01"))
            .and(header("content-type", "application/json"))
            .and(wiremock::matchers::headers(
                "accept",
                vec!["application/json", "text/event-stream"],
            ))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                raw,
                if streaming {
                    "Text/Event-Stream; charset=utf-8"
                } else {
                    "application/json"
                },
            ))
            .expect(1)
            .mount(&server)
            .await;
        let operation = native_messages_request(
            streaming,
            &[
                ("authorization", "Bearer downstream-secret"),
                ("x-api-key", "downstream-api-key"),
                ("cookie", "downstream-session=secret"),
                ("chatgpt-account-id", "downstream-account"),
                ("originator", "downstream-codex"),
                ("user-agent", "downstream-codex/secret"),
                ("version", "downstream-version"),
                ("x-openai-future-identity", "downstream-openai"),
                ("x-codex-turn-state", "downstream-turn"),
                ("x-oai-attestation", "downstream-device"),
                ("session-id", "downstream-session"),
                ("session_id", "downstream-session-alias"),
                ("thread-id", "downstream-thread"),
                ("conversation-id", "downstream-conversation"),
                ("turn-id", "downstream-turn"),
                ("anthropic-beta", "first-native-beta"),
                ("anthropic-beta", "second-native-beta"),
                ("x-business-extension", "preserved"),
            ],
        );
        let mut stream = provider(&store)
            .execute(
                planned_request("openai", operation),
                context("req_messages_identity", CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut delivered = Vec::new();
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            if let Some(wire) = event.wire_event()
                && let Some(raw) = wire.raw_json_body().or(wire.raw_sse_frame())
            {
                delivered.extend_from_slice(raw);
            }
        }
        assert_eq!(delivered, raw.as_bytes());
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "gpt-5.4");
        assert_eq!(body["messages"][0]["content"], "private prompt");
        assert_eq!(body["future"], json!({"preserve":true}));
        assert_eq!(request.headers["x-business-extension"], "preserved");
        assert_eq!(
            request
                .headers
                .get_all("anthropic-beta")
                .iter()
                .map(|value| value.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["first-native-beta", "second-native-beta"]
        );
        for name in [
            "authorization",
            "cookie",
            "chatgpt-account-id",
            "originator",
            "user-agent",
            "version",
            "x-openai-future-identity",
            "x-codex-turn-state",
            "x-oai-attestation",
            "x-codex-installation-id",
            "session-id",
            "session_id",
            "thread-id",
            "conversation-id",
            "turn-id",
            "content-encoding",
            "openai-beta",
        ] {
            assert!(
                !request.headers.contains_key(name),
                "unexpected Messages identity: {name}"
            );
        }
    }
}

#[tokio::test]
async fn messages_privacy_rewrites_native_body_and_api_key_before_sending() {
    let server = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    seed_messages_transport(&store, server.uri()).await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(MESSAGES_JSON, "application/json"))
        .expect(1)
        .mount(&server)
        .await;
    let mut body_rule = rule(
        PrivacyScope::RequestBody,
        "$.messages[0].content",
        PrivacyAction::SetValue,
    );
    body_rule.id = "messages-body".to_owned();
    body_rule.steps[0].value = json!("redacted prompt");
    let mut auth_rule = rule(
        PrivacyScope::RequestHeader,
        "x-api-key",
        PrivacyAction::SetValue,
    );
    auth_rule.id = "messages-key".to_owned();
    auth_rule.steps[0].value = json!("privacy-configured-key");
    let config = policy(vec![body_rule, auth_rule]);
    let attempt = AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_messages_privacy").unwrap(),
            ClientApiKeyId::new("key_openai_contract").unwrap(),
        )
        .with_privacy(Some(
            provider_openai::transport::privacy::compile(&config).unwrap(),
        )),
        NonZeroU32::MIN,
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    );
    let mut stream = provider(&store)
        .execute(
            planned_request("openai", native_messages_request(false, &[])),
            attempt,
        )
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers["x-api-key"], "privacy-configured-key");
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["messages"][0]["content"], "redacted prompt");
    assert_eq!(body["future"], json!({"preserve":true}));
}

#[tokio::test]
async fn messages_outbound_middleware_controls_final_target_headers_and_bytes() {
    let original = MockServer::start().await;
    let target = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    seed_messages_transport(&store, format!("{}/v1", original.uri())).await;
    Mock::given(method("PATCH"))
        .and(path("/plugin-messages"))
        .and(header("x-plugin", "messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(MESSAGES_JSON, "application/json"))
        .expect(1)
        .mount(&target)
        .await;
    let uri = format!("{}/plugin-messages", target.uri());
    let mut stream = provider(&store)
        .execute(
            planned_request("openai", native_messages_request(false, &[])),
            outbound_context("req_messages_outbound", move |request| {
                assert!(matches!(&request.target, Target::Http { method, uri } if method == reqwest::Method::POST && uri.path() == "/v1/messages"));
                let headers = request.header_map().unwrap();
                assert_eq!(headers["x-api-key"], "sk-api-test-only");
                assert_eq!(headers["anthropic-version"], "2023-06-01");
                assert!(!headers.contains_key("authorization"));
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(body["messages"][0]["content"], "private prompt");
                request.target = Target::Http { method: reqwest::Method::PATCH, uri: uri.parse().unwrap() };
                request.headers.retain(|header| !matches!(header.name(), "x-api-key" | "anthropic-version" | "content-type" | "accept"));
                request.headers.push(MiddlewareHeader::new("x-plugin", Bytes::from_static(b"messages")));
                request.headers.push(MiddlewareHeader::new("accept", Bytes::from_static(b"application/vnd.plugin-messages")));
                request.body = Bytes::from_static(b"\xff\0plugin-owned-request");
                Ok(())
            }),
        )
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    assert!(original.received_requests().await.unwrap().is_empty());
    let requests = target.received_requests().await.unwrap();
    assert_eq!(requests[0].body, b"\xff\0plugin-owned-request");
    assert_eq!(
        requests[0].headers["accept"],
        "application/vnd.plugin-messages"
    );
    for name in ["x-api-key", "anthropic-version", "content-type"] {
        assert!(
            !requests[0].headers.contains_key(name),
            "restored plugin-removed {name}"
        );
    }
}

#[tokio::test]
async fn messages_uses_account_proxy_without_direct_fallback() {
    for available in [true, false] {
        let direct = MockServer::start().await;
        let proxy = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        seed_messages_transport(&store, format!("{}/v1", direct.uri())).await;
        let proxy_url = if available {
            Mock::given(method("POST"))
                .and(path("/v1/messages"))
                .and(header("x-api-key", "sk-api-test-only"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_raw(MESSAGES_JSON, "application/json"),
                )
                .expect(1)
                .mount(&proxy)
                .await;
            proxy.uri()
        } else {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            drop(listener);
            format!("http://{address}")
        };
        store.set_egress(
            "acct_provider_contract",
            Some(gateway_core::account::OutboundProxy::parse(&proxy_url).unwrap()),
            None,
        );
        let mut stream = provider(&store)
            .execute(
                planned_request("openai", native_messages_request(false, &[])),
                context("req_messages_proxy", CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut failed = false;
        let mut delivered = false;
        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => delivered |= event.wire_event().is_some(),
                Err(_) => failed = true,
            }
        }
        assert_eq!(failed, !available);
        assert_eq!(delivered, available);
        assert!(direct.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn messages_token_billing_uses_configured_standard_rates_without_openai_tiers() {
    for priced in [false, true] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        seed_messages_transport(&store, server.uri()).await;
        let response = json!({
            "id":"msg_cost","type":"message","role":"assistant","model":"gpt-5.4",
            "content":[{"type":"text","text":"priced"}],"stop_reason":"end_turn",
            "usage":{"input_tokens":300000,"cache_read_input_tokens":2,"cache_creation_input_tokens":3,"output_tokens":2}
        });
        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let pricing = if priced {
            serde_json::from_value(json!({"openai":{"gpt-5.4":{
                "multiplierBps":15000,"bands":{
                    "standard":{"input":"2","output":"4","cacheRead":"0.5","cacheWrite":"3"},
                    "long_standard":{"input":"200","output":"400","cacheRead":"50","cacheWrite":"300"},
                    "fast":{"input":"20","output":"40","cacheRead":"5","cacheWrite":"30"}
                }
            }}})).unwrap()
        } else {
            Default::default()
        };
        let mut stream = provider(&store)
            .execute(
                planned_request("openai", native_messages_request(false, &[])),
                context_with_pricing(
                    "req_messages_cost",
                    CancellationToken::new(),
                    FastMode::Default,
                    pricing,
                ),
            )
            .await
            .unwrap();
        let mut cost = None;
        while let Some(event) = stream.next().await {
            for fact in event.unwrap().canonical_facts() {
                if let GatewayEvent::CalculatedCost(value) = fact {
                    cost = Some(value.clone().into_estimate());
                }
            }
        }
        if priced {
            let cost = cost.unwrap();
            assert_eq!(cost.total().unwrap().amount().canonical(), "0.900027");
            let breakdown = cost.breakdown().unwrap();
            assert_eq!(breakdown.service_tier(), Some("default"));
            assert_eq!(breakdown.multiplier_percent(), 100);
            assert_eq!(breakdown.custom_multiplier_bps(), 15000);
            assert!(!breakdown.long_context_billing_applied());
            assert_eq!(
                breakdown.input_price_per_million().amount().canonical(),
                "3"
            );
            assert_eq!(
                breakdown.output_price_per_million().amount().canonical(),
                "6"
            );
            assert_eq!(
                breakdown
                    .cache_read_price_per_million()
                    .amount()
                    .canonical(),
                "0.75"
            );
            assert_eq!(
                breakdown
                    .cache_write_price_per_million()
                    .amount()
                    .canonical(),
                "4.5"
            );
        } else {
            assert!(
                cost.is_none(),
                "Messages must not inherit gpt-5.4 OpenAI pricing"
            );
        }
    }
}

#[tokio::test]
async fn messages_json_and_sse_body_idle_timeouts_use_existing_transport_recovery() {
    for streaming in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            let content_type = if streaming {
                "text/event-stream"
            } else {
                "application/json"
            };
            let prefix = if streaming {
                "event: message_start\n"
            } else {
                "{"
            };
            socket
                .write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n{prefix}"
                ).as_bytes())
                .await
                .unwrap();
            // 保持真实 HTTP 正文未结束，既不发送终态，也不关闭连接
            futures::future::pending::<()>().await;
        });
        let store = Arc::new(MemoryAccountStore::default());
        seed_messages_transport(&store, base_url).await;
        let attempt = AttemptContext::new(
            RequestAttemptContext::new(
                ModelRequestId::new("req_messages_body_idle").unwrap(),
                ClientApiKeyId::new("key_openai_contract").unwrap(),
            ),
            NonZeroU32::MIN,
            SystemTime::now() + Duration::from_secs(600),
            account_policy(),
            AccountAttemptContext::new(BTreeSet::new(), None, None)
                .with_account_scope(contract_account_scope()),
            None,
            CancellationToken::new(),
        );
        let mut stream = provider(&store)
            .execute(
                planned_request("openai", native_messages_request(streaming, &[])),
                attempt,
            )
            .await
            .unwrap();
        loop {
            let event = stream.next().await.unwrap().unwrap();
            assert!(event.wire_event().is_none());
            if event
                .response_observation()
                .is_some_and(|snapshot| snapshot.timings().first_event_ms.is_some())
            {
                break;
            }
        }
        let next = stream.next();
        tokio::pin!(next);
        assert!(futures::poll!(&mut next).is_pending());
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(301)).await;
        let error = match next.await {
            Some(Err(error)) => error,
            _ => panic!("stalled Messages body must return an idle timeout"),
        };
        tokio::time::resume();
        server.abort();
        let _ = server.await;
        assert_eq!(error.kind(), ProviderErrorKind::Timeout);
        assert_eq!(error.send_state(), UpstreamSendState::Sent);
        assert_eq!(error.upstream_status(), None);
        assert!(error.pre_delivery_retry().is_some());
        assert_eq!(
            error.diagnostic().and_then(|diagnostic| diagnostic.stage()),
            Some("receive")
        );
        assert_eq!(
            error.diagnostic().and_then(|diagnostic| diagnostic.code()),
            Some("receive_idle_timeout")
        );
        assert_eq!(
            error.diagnostic().map(|diagnostic| diagnostic.as_str()),
            Some("OpenAI HTTP response event idle timeout after 300s"),
        );
    }
}

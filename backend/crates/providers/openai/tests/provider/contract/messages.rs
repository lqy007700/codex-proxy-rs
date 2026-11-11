//! Messages 原生 wire、跨协议头隔离和本地计数合同

use super::*;
use gateway_core::event::{FinishReason, ProviderResponseTimings};
use gateway_core::operation::TokenCountRequest;
use provider_openai::credential::{ApiKeyProtocol, ResponsesTransport};

const MESSAGES_SSE: &str = concat!(
    ": keepalive\r\n\r\n",
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_native\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-5.4\",\"content\":[],\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":2,\"cache_creation_input_tokens\":3,\"output_tokens\":0}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"},\"future\":true}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":11,\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: { \"type\":\"message_stop\",\"future\":{\"keep\":true} }\n\n",
);

async fn seed(store: &Arc<MemoryAccountStore>, server: &MockServer, protocol: ApiKeyProtocol) {
    store
        .seed_api_key_for_protocol(
            "acct_provider_contract",
            server.uri(),
            ResponsesTransport::Http,
            protocol,
        )
        .await;
}

fn messages_operation(stream: bool) -> Operation {
    let body = json!({
        "model":"gpt-5.4", "max_tokens":128, "stream":stream,
        "messages":[
            {"role":"assistant","content":[{"type":"thinking","thinking":"prior thought","signature":"real-native-signature"},{"type":"text","text":"before"}]},
            {"role":"user","content":"hello"},
            {"role":"system","content":[],"output_config":{"effort":"medium"}}
        ],
        "future_option":{"preserve":true}
    });
    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("anthropic", body.as_object().unwrap().clone())
            .unwrap()
            .with_context(Map::from_iter([(
                "opaque_request_headers".to_owned(),
                json!([
                    ["anthropic-version", STANDARD.encode("2026-10-21")],
                    ["anthropic-beta", STANDARD.encode("native-feature")],
                    ["anthropic-workspace-id", STANDARD.encode("workspace-test")],
                    ["x-business-extension", STANDARD.encode("keep")]
                ]),
            )])),
    ))
}

#[tokio::test]
async fn final_usage_before_truncation_keeps_only_explicitly_priceable_cost() {
    for (priced, long_cache, fast) in [
        (false, false, false),
        (true, false, false),
        (true, true, false),
        (true, false, true),
    ] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        seed(&store, &server, ApiKeyProtocol::Messages).await;
        let mut body = MESSAGES_SSE
            .split("event: message_stop")
            .next()
            .unwrap()
            .to_owned();
        if long_cache {
            body = body.replace(
                "\"cache_creation_input_tokens\":3",
                "\"cache_creation_input_tokens\":3,\"cache_creation\":{\"ephemeral_1h_input_tokens\":3}",
            );
        }
        if fast {
            body = body.replace(
                "\"output_tokens\":2",
                "\"output_tokens\":2,\"speed\":\"fast\"",
            );
        }
        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        let pricing = if priced {
            serde_json::from_value(json!({"openai":{"gpt-5.4":{
                "multiplierBps":10000,"bands":{"standard":{
                    "input":"2","output":"4","cacheRead":"0.5","cacheWrite":"3"
                }}
            }}}))
            .unwrap()
        } else {
            Default::default()
        };
        let mut stream = provider(&store)
            .execute(
                planned_request("openai", messages_operation(true)),
                context_with_pricing(
                    "req_messages_truncated",
                    CancellationToken::new(),
                    FastMode::Default,
                    pricing,
                ),
            )
            .await
            .unwrap();
        let mut usage = Usage::new();
        let mut cost = None;
        let mut failed = false;
        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    for fact in event.canonical_facts() {
                        match fact {
                            GatewayEvent::Usage(value) => usage.merge(value),
                            GatewayEvent::CalculatedCost(value) => {
                                cost = Some(value.clone().into_estimate())
                            }
                            _ => {}
                        }
                    }
                }
                Err(error) => {
                    assert_eq!(error.kind(), ProviderErrorKind::Protocol);
                    failed = true;
                }
            }
        }
        assert!(failed);
        assert_eq!(usage.input_tokens, Some(16));
        assert_eq!(usage.output_tokens, Some(2));
        if priced && !long_cache && !fast {
            assert_eq!(
                cost.unwrap().total().unwrap().amount().canonical(),
                "0.00004"
            );
        } else {
            assert!(cost.is_none());
        }
        server.verify().await;
    }
}

#[tokio::test]
async fn native_messages_preserve_json_sse_headers_and_real_signatures() {
    for streaming in [false, true] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        seed(&store, &server, ApiKeyProtocol::Messages).await;
        let json_response = "{ \"id\":\"msg_native\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-5.4\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}],\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":11,\"cache_read_input_tokens\":2,\"cache_creation_input_tokens\":3,\"output_tokens\":2},\"future\":true }\n";
        let raw = if streaming {
            MESSAGES_SSE
        } else {
            json_response
        };
        Mock::given(method("POST"))
            .and(path("/messages"))
            .and(header("x-api-key", "sk-api-test-only"))
            .and(header("anthropic-version", "2026-10-21"))
            .and(header("anthropic-beta", "native-feature"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                raw,
                if streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            ))
            .expect(1)
            .mount(&server)
            .await;
        let operation = messages_operation(streaming);
        let original = operation.middleware_body().unwrap();
        let mut stream = provider(&store)
            .execute(
                planned_request("openai", operation),
                context("req_messages_native", CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut received = Vec::new();
        let mut usage = Usage::new();
        let mut timings = ProviderResponseTimings::default();
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            if let Some(snapshot) = event.response_observation() {
                timings = snapshot.timings();
            }
            if let Some(wire) = event.wire_event() {
                assert_eq!(wire.protocol(), "anthropic");
                if let Some(raw) = wire.raw_sse_frame().or(wire.raw_json_body()) {
                    received.extend_from_slice(raw);
                }
            }
            for fact in event.canonical_facts() {
                if let GatewayEvent::Usage(newer) = fact {
                    usage.merge(newer);
                }
            }
        }
        assert!(timings.first_event_ms.is_some());
        assert!(timings.first_token_ms.is_some());
        assert!(timings.first_text_ms.is_some());
        assert!(timings.first_reasoning_ms.is_none());
        assert_eq!(received, raw.as_bytes());
        assert_eq!(usage.input_tokens, Some(16));
        assert_eq!(usage.cached_input_tokens, Some(2));
        assert_eq!(usage.cache_write_input_tokens, Some(3));
        assert_eq!(usage.output_tokens, Some(2));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
            serde_json::from_slice::<Value>(&original).unwrap()
        );
        assert!(!requests[0].headers.contains_key("chatgpt-account-id"));
        server.verify().await;
    }
}

#[tokio::test]
async fn messages_and_responses_translate_with_target_header_boundaries() {
    for target in [ApiKeyProtocol::Messages, ApiKeyProtocol::Responses] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        seed(&store, &server, target).await;
        let (endpoint, response) = if target == ApiKeyProtocol::Messages {
            ("/messages", MESSAGES_SSE.to_owned())
        } else {
            (
                "/responses",
                format!(
                    "event: response.created\ndata: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_scope_capture\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\",\"output\":[]}}}}\n\n{CAPTURE_COMPLETED_SSE}"
                ),
            )
        };
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_raw(response, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        let operation = if target == ApiKeyProtocol::Messages {
            generate_with_downstream_headers()
        } else {
            messages_operation(true)
        };
        let expected_protocol = operation.protocol().to_owned();
        let mut stream = provider(&store)
            .execute(
                planned_request("openai", operation),
                context("req_messages_translation", CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut terminal_seen = false;
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let terminal = event
                .canonical_facts()
                .iter()
                .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
            for event in stream.translate_native_response(event, terminal).unwrap() {
                if let Some(wire) = event.wire_event() {
                    assert_eq!(wire.protocol(), expected_protocol);
                }
                terminal_seen |= event
                    .canonical_facts()
                    .iter()
                    .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
            }
        }
        assert!(terminal_seen);
        let requests = server.received_requests().await.unwrap();
        let headers = &requests
            .iter()
            .find(|request| request.method == "POST")
            .unwrap()
            .headers;
        assert_eq!(headers.get("x-business-extension").unwrap(), "keep");
        if target == ApiKeyProtocol::Messages {
            for name in [
                "session-id",
                "thread-id",
                "x-codex-future",
                "x-openai-internal-future",
            ] {
                assert!(!headers.contains_key(name));
            }
        } else {
            for name in [
                "anthropic-version",
                "anthropic-beta",
                "anthropic-workspace-id",
            ] {
                assert!(!headers.contains_key(name));
            }
        }
        server.verify().await;
    }
}

#[tokio::test]
async fn custom_messages_and_responses_count_locally_without_billable_usage() {
    for (target, source) in [
        (ApiKeyProtocol::Messages, "anthropic"),
        (ApiKeyProtocol::Responses, "anthropic"),
        (ApiKeyProtocol::Messages, "token-count"),
        (ApiKeyProtocol::Responses, "token-count"),
    ] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        seed(&store, &server, target).await;
        let body = Bytes::from_static(br#"{"messages":[{"role":"user","content":"hello world"}]}"#);
        let operation = Operation::CountTokens(TokenCountRequest::from_raw_json(
            RawJsonPayload::new(source, body).unwrap(),
        ));
        let mut stream = provider(&store)
            .execute(
                planned_request("openai", operation),
                context("req_messages_count", CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut count = None;
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            assert!(!event.canonical_facts().iter().any(|fact| matches!(
                fact,
                GatewayEvent::Usage(_) | GatewayEvent::CalculatedCost(_)
            )));
            if let Some(wire) = event.wire_event() {
                assert_eq!(wire.protocol(), source);
                let body: Value = serde_json::from_slice(wire.raw_json_body().unwrap()).unwrap();
                count = body.get("input_tokens").and_then(Value::as_u64);
            }
        }
        assert!(count.is_some_and(|count| count > 0));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn cancelled_local_count_returns_no_usage_or_result() {
    let server = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    seed(&store, &server, ApiKeyProtocol::Responses).await;
    let cancel = CancellationToken::new();
    let operation = Operation::CountTokens(TokenCountRequest::from_raw_json(
        RawJsonPayload::new(
            "anthropic",
            Bytes::from_static(br#"{"messages":[{"role":"user","content":"hello"}]}"#),
        )
        .unwrap(),
    ));
    let mut stream = provider(&store)
        .execute(
            planned_request("openai", operation),
            context("req_cancel_local_count", cancel.clone()),
        )
        .await
        .unwrap();
    cancel.cancel();
    assert!(
        matches!(stream.next().await, Some(Err(error)) if error.kind() == ProviderErrorKind::Cancelled)
    );
    assert!(stream.next().await.is_none());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn official_messages_count_preserves_native_result_and_never_estimates_after_failure() {
    use gateway_core::engine::middleware::outbound::Target;

    for source in ["anthropic", "token-count"] {
        for status in [200, 429] {
            let server = MockServer::start().await;
            let store = Arc::new(MemoryAccountStore::default());
            store
                .seed_api_key_for_protocol(
                    "acct_provider_contract",
                    "https://api.anthropic.com/v1".to_owned(),
                    ResponsesTransport::Http,
                    ApiKeyProtocol::Messages,
                )
                .await;
            let expected_body = if status == 200 {
                "{ \"input_tokens\": 42 }\n"
            } else {
                "{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"native count limited\"}}"
            };
            Mock::given(method("POST"))
                .and(path("/v1/messages/count_tokens"))
                .and(header("x-api-key", "sk-api-test-only"))
                .and(header("anthropic-version", "2023-06-01"))
                .respond_with(
                    ResponseTemplate::new(status).set_body_raw(expected_body, "application/json"),
                )
                .expect(1)
                .mount(&server)
                .await;
            let uri = format!("{}/v1/messages/count_tokens", server.uri());
            let attempt = outbound_context("req_native_messages_count", move |request| {
                // 验证官方目标已按原生计数准备，再通过现有出站挂载改向本地合同服务器
                let Target::Http {
                    method,
                    uri: original_uri,
                } = &request.target
                else {
                    panic!("原生计数必须使用 HTTP");
                };
                assert_eq!(method, reqwest::Method::POST);
                assert_eq!(
                    original_uri,
                    "https://api.anthropic.com/v1/messages/count_tokens"
                );
                assert!(!request.header_map().unwrap().contains_key("authorization"));
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(body["model"], "gpt-5.4");
                assert!(body.get("max_tokens").is_none());
                request.target = Target::Http {
                    method: reqwest::Method::POST,
                    uri: uri.parse().unwrap(),
                };
                Ok(())
            });
            let operation = Operation::CountTokens(TokenCountRequest::from_raw_json(
                RawJsonPayload::new(
                    source,
                    Bytes::from_static(br#"{"messages":[{"role":"user","content":"hello"}]}"#),
                )
                .unwrap(),
            ));
            let mut stream = provider(&store)
                .execute(planned_request("openai", operation), attempt)
                .await
                .unwrap();
            let mut raw = Vec::new();
            let mut failed = false;
            while let Some(event) = stream.next().await {
                match event {
                    Ok(event) => {
                        assert!(!event.canonical_facts().iter().any(|fact| matches!(
                            fact,
                            GatewayEvent::Usage(_) | GatewayEvent::CalculatedCost(_)
                        )));
                        if let Some(wire) = event.wire_event() {
                            assert_eq!(wire.protocol(), source);
                            if let Some(bytes) = wire.raw_json_body() {
                                raw.extend_from_slice(bytes);
                            }
                        }
                    }
                    Err(_) => failed = true,
                }
            }
            assert_eq!(failed, status != 200);
            if status == 200 {
                assert_eq!(raw, expected_body.as_bytes());
            } else {
                assert!(raw.is_empty());
            }
            server.verify().await;
        }
    }
}

#[tokio::test]
async fn translated_thinking_uses_selected_account_catalog_and_preserves_fast_betas() {
    let store = Arc::new(MemoryAccountStore::default());
    let adaptive = MockServer::start().await;
    let manual = MockServer::start().await;
    for (account, server, capabilities) in [
        (
            "acct_provider_contract",
            &adaptive,
            json!({"thinking":{"supported":true,"types":{"adaptive":{"supported":true},"enabled":{"supported":false},"disabled":{"supported":false}}},"effort":{"high":{"supported":true}}}),
        ),
        ("acct_manual", &manual, json!({})),
    ] {
        store
            .seed_api_key_for_protocol(
                account,
                server.uri(),
                ResponsesTransport::Http,
                ApiKeyProtocol::Messages,
            )
            .await;
        Mock::given(method("GET")).and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"gpt-5.4","max_tokens":2048,"capabilities":capabilities}],"has_more":false})))
            .expect(1).mount(server).await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(MESSAGES_SSE, "text/event-stream"),
            )
            .expect(2)
            .mount(server)
            .await;
    }
    let provider = provider(&store);
    for _ in 0..2 {
        for account in ["acct_provider_contract", "acct_manual"] {
            let body = json!({"model":"unrelated-client-alias","input":"hello","max_output_tokens":4096,"reasoning":{"effort":"xhigh"},"tools":[{"type":"function","name":"echo"}],"parallel_tool_calls":false,"service_tier":"priority"});
            let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
                    .unwrap()
                    .with_context(Map::from_iter([(
                        "opaque_request_headers".into(),
                        json!([
                            ["anthropic-beta", STANDARD.encode("feature-a")],
                            ["anthropic-beta", STANDARD.encode("feature-b")]
                        ]),
                    )])),
            ));
            let mut stream = Arc::clone(&provider)
                .execute(
                    planned_request("openai", operation),
                    diagnostic_context("req_messages_capabilities", account),
                )
                .await
                .unwrap();
            while let Some(event) = stream.next().await {
                event.unwrap();
            }
        }
    }
    for (server, thinking) in [
        (&adaptive, json!({"type":"adaptive"})),
        (&manual, json!({"type":"enabled","budget_tokens":2047})),
    ] {
        for request in server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.method == "POST")
        {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["model"], "gpt-5.4");
            assert_eq!(body["thinking"], thinking);
            assert_eq!(body["max_tokens"], 2048);
            assert_eq!(body["speed"], "fast");
            assert!(body.get("service_tier").is_none());
            assert_eq!(
                body["tool_choice"],
                json!({"type":"auto","disable_parallel_tool_use":true})
            );
            assert_eq!(
                request.headers.get("anthropic-beta").unwrap(),
                "feature-a,feature-b,fast-mode-2026-02-01"
            );
        }
        server.verify().await;
    }
}

#[tokio::test]
async fn cancelled_capability_lookup_never_sends_generation() {
    let server = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    seed(&store, &server, ApiKeyProtocol::Messages).await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(10))
                .set_body_json(json!({"data":[]})),
        )
        .mount(&server)
        .await;
    let cancellation = CancellationToken::new();
    let cancelled = cancellation.clone();
    let task = tokio::spawn(async move {
        provider(&store)
            .execute(
                planned_request("openai", generate_operation()),
                context("req_cancel_catalog", cancelled),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.received_requests().await.unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result,Err(error) if error.kind()==ProviderErrorKind::Cancelled));
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.method == "GET")
    );
}

#[tokio::test]
async fn messages_json_and_sse_deliver_consistent_responses_terminal_states() {
    for (reason, expected_status, expected_event) in [
        ("end_turn", "completed", "response.completed"),
        ("max_tokens", "incomplete", "response.incomplete"),
        ("refusal", "incomplete", "response.incomplete"),
    ] {
        let mut terminals = Vec::new();
        for upstream_streaming in [false, true] {
            let server = MockServer::start().await;
            let store = Arc::new(MemoryAccountStore::default());
            seed(&store, &server, ApiKeyProtocol::Messages).await;
            let body = json!({"id":"msg_native","type":"message","role":"assistant","model":"gpt-5.4","content":[{"type":"text","text":"hello"}],"stop_reason":reason,"usage":{"input_tokens":11,"cache_read_input_tokens":2,"cache_creation_input_tokens":3,"output_tokens":2}});
            let response = if upstream_streaming {
                ResponseTemplate::new(200).set_body_raw(
                    MESSAGES_SSE.replace("end_turn", reason),
                    "text/event-stream",
                )
            } else {
                ResponseTemplate::new(200).set_body_json(body)
            };
            Mock::given(method("POST"))
                .and(path("/messages"))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            let mut stream = provider(&store)
                .execute(
                    planned_request("openai", generate_with_downstream_headers()),
                    context("req_messages_terminal", CancellationToken::new()),
                )
                .await
                .unwrap();
            let mut terminal_count = 0;
            while let Some(event) = stream.next().await {
                let event = event.unwrap();
                let terminal = event
                    .canonical_facts()
                    .iter()
                    .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
                let translated = stream.translate_native_response(event, terminal).unwrap();
                for event in translated {
                    if event
                        .canonical_facts()
                        .iter()
                        .any(|fact| matches!(fact, GatewayEvent::Started(_)))
                    {
                        assert!(
                            event.wire_event().is_some(),
                            "原生出口的开始事实必须带可交付正文，不能提交空的 SSE 批次"
                        );
                    }
                    if event
                        .canonical_facts()
                        .iter()
                        .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
                    {
                        terminal_count += 1;
                        let wire = event.wire_event().expect("终态附在最后一个转换事件");
                        assert_eq!(wire.data()["type"], expected_event);
                        assert_eq!(wire.data()["response"]["status"], expected_status);
                        terminals.push(wire.data()["response"].clone());
                    }
                }
            }
            assert_eq!(terminal_count, 1);
            server.verify().await;
        }
        assert_eq!(
            terminals[0], terminals[1],
            "同一上游结果的 JSON 与 SSE 语义一致"
        );
    }
}

#[tokio::test]
async fn many_messages_json_blocks_stay_in_one_responses_terminal_event() {
    let server = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    seed(&store, &server, ApiKeyProtocol::Messages).await;
    let content = (0..100)
        .map(|index| json!({"type":"text","text":format!("block {index}")}))
        .collect::<Vec<_>>();
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"msg_many","type":"message","role":"assistant","model":"gpt-5.4","content":content,"stop_reason":"max_tokens","usage":{"input_tokens":10,"output_tokens":100}})))
        .expect(1)
        .mount(&server)
        .await;
    let mut stream = provider(&store)
        .execute(
            planned_request("openai", generate_with_downstream_headers()),
            context("req_messages_many_blocks", CancellationToken::new()),
        )
        .await
        .unwrap();
    let mut received = Vec::new();
    while let Some(event) = stream.next().await {
        let event = event.unwrap();
        let terminal = event
            .canonical_facts()
            .iter()
            .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
        for event in stream.translate_native_response(event, terminal).unwrap() {
            if let Some(wire) = event.wire_event() {
                received.push(wire.data().clone());
            }
        }
    }
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["type"], "response.incomplete");
    assert_eq!(
        received[0]["response"]["output"].as_array().unwrap().len(),
        100
    );
    assert_eq!(
        received[0]["response"]["output"][99]["content"][0]["text"],
        "block 99"
    );
    server.verify().await;
}

#[tokio::test]
async fn oversized_messages_json_is_rejected_before_wire_delivery() {
    let server = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    seed(&store, &server, ApiKeyProtocol::Messages).await;
    let mut body = br#"{"type":"message","id":"msg_big","model":"gpt-5.4","content":[],"stop_reason":"end_turn","usage":{}}"#.to_vec();
    body.resize(16 * 1024 * 1024 + 1, b' ');
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .mount(&server)
        .await;
    let mut stream = provider(&store)
        .execute(
            planned_request("openai", messages_operation(false)),
            context("req_big_json", CancellationToken::new()),
        )
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        match event {
            Err(err) => {
                assert_eq!(err.kind(), ProviderErrorKind::Protocol);
                return;
            }
            Ok(event) => assert!(event.wire_event().is_none()),
        }
    }
    panic!("超限的 JSON 必须失败");
}

#[tokio::test]
async fn slow_catalog_does_not_block_generation_and_populates_next_turn() {
    let server = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    seed(&store, &server, ApiKeyProtocol::Messages).await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_json(
                    json!({"data":[{"id":"gpt-5.4","max_tokens":2048}],"has_more":false}),
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(MESSAGES_SSE, "text/event-stream"))
        .expect(2)
        .mount(&server)
        .await;
    let provider = provider(&store);
    for i in 0..2 {
        let mut stream = provider
            .clone()
            .execute(
                planned_request("openai", generate_operation()),
                context("req_slow_catalog", CancellationToken::new()),
            )
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        if i == 0 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
    let limits = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method == "POST")
        .map(|r| {
            serde_json::from_slice::<Value>(&r.body).unwrap()["max_tokens"]
                .as_u64()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(limits, [32000, 2048]);
    server.verify().await;
}

#[tokio::test]
async fn messages_errors_preserve_structured_facts_and_shared_recovery_budget() {
    use gateway_core::error::{PreDeliveryRetry, TransientRetryPolicy};

    for streaming in [false, true] {
        for (kind, expected) in [
            ("authentication_error", ProviderErrorKind::Unauthorized),
            ("rate_limit_error", ProviderErrorKind::RateLimited),
            ("invalid_request_error", ProviderErrorKind::InvalidRequest),
            ("permission_error", ProviderErrorKind::PermissionDenied),
            (
                "overloaded_error",
                ProviderErrorKind::UpstreamCapacityUnavailable,
            ),
            ("api_error", ProviderErrorKind::Unavailable),
        ] {
            let server = MockServer::start().await;
            let store = Arc::new(MemoryAccountStore::default());
            seed(&store, &server, ApiKeyProtocol::Messages).await;
            let raw = format!(
                "{{ \"type\":\"error\",\"error\":{{\"type\":\"{kind}\",\"message\":\"private messages failure\",\"retry_after\":2}} }}"
            );
            let response = if streaming {
                format!("event: error\ndata: {raw}\n\n")
            } else {
                raw.clone()
            };
            Mock::given(method("POST"))
                .and(path("/messages"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(
                    response,
                    if streaming {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                ))
                .expect(1)
                .mount(&server)
                .await;
            let mut stream = provider(&store)
                .execute(
                    planned_request("openai", messages_operation(streaming)),
                    context("req_messages_error_contract", CancellationToken::new()),
                )
                .await
                .unwrap();
            let error = loop {
                match stream.next().await {
                    Some(Ok(event)) => assert!(event.wire_event().is_none()),
                    Some(Err(error)) => break error,
                    None => panic!("Messages 上游错误不能被吞掉或转成成功"),
                }
            };
            assert_eq!(error.kind(), expected);
            assert_eq!(error.send_state(), UpstreamSendState::Sent);
            assert_eq!(error.upstream_status(), None);
            assert_eq!(error.raw_upstream_error().unwrap().as_str(), raw);
            let fields = error.upstream_failure_fields().unwrap();
            assert_eq!(fields.error_type.as_deref(), Some(kind));
            assert_eq!(fields.status, None);
            assert_eq!(
                error.client_visible_upstream_error().unwrap().error_type(),
                Some(kind)
            );
            if matches!(kind, "overloaded_error" | "api_error") {
                let Some(PreDeliveryRetry::SameAccountTransientRetry {
                    max_retries,
                    policy:
                        TransientRetryPolicy::RetryThenRotateAccountWithBudget {
                            max_total_retries,
                            track_client_retry_failure,
                        },
                    ..
                }) = error.pre_delivery_retry()
                else {
                    panic!("Messages 服务拒绝必须沿用 Core 的共享恢复预算");
                };
                assert_eq!(max_retries.get(), 2);
                assert_eq!(
                    max_total_retries.get(),
                    gateway_core::account::MAX_RECOVERY_ATTEMPTS
                );
                assert_eq!(track_client_retry_failure, kind != "overloaded_error");
                assert!(error.replay_is_safe());
            }
            if kind == "authentication_error" {
                assert_eq!(
                    store
                        .account("acct_provider_contract")
                        .unwrap()
                        .credential_state(),
                    CredentialState::Expired
                );
            }
            assert!(!format!("{error:?} {error}").contains("private messages failure"));
            server.verify().await;
        }
    }
}

#[tokio::test]
async fn late_messages_errors_keep_usage_but_cannot_request_replay() {
    let server = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    seed(&store, &server, ApiKeyProtocol::Messages).await;
    let prefix = MESSAGES_SSE.split("event: message_stop").next().unwrap();
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!("{prefix}event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"overloaded_error\",\"message\":\"late capacity\"}}}}\n\n"),
            "text/event-stream",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let mut stream = provider(&store)
        .execute(
            planned_request("openai", messages_operation(true)),
            context("req_messages_late_error", CancellationToken::new()),
        )
        .await
        .unwrap();
    let mut usage = Usage::new();
    let error = loop {
        match stream.next().await {
            Some(Ok(event)) => {
                for fact in event.canonical_facts() {
                    if let GatewayEvent::Usage(value) = fact {
                        usage.merge(value);
                    }
                    assert!(!matches!(fact, GatewayEvent::Completed(_)));
                }
            }
            Some(Err(error)) => break error,
            None => panic!("流内失败必须保留终止错误"),
        }
    };
    assert_eq!(usage.input_tokens, Some(16));
    assert_eq!(usage.output_tokens, Some(2));
    assert_eq!(error.kind(), ProviderErrorKind::UpstreamCapacityUnavailable);
    assert!(!error.replay_is_safe());
    assert!(error.pre_delivery_retry().is_none());
    server.verify().await;
}

#[tokio::test]
async fn incomplete_messages_usage_never_fabricates_zero_counts_or_cost() {
    for usage in [
        json!({"output_tokens":2}),
        json!({"input_tokens":4}),
        json!({"input_tokens":u64::MAX,"cache_read_input_tokens":1,"output_tokens":2}),
    ] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        seed(&store, &server, ApiKeyProtocol::Messages).await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id":"msg_partial","type":"message","model":"gpt-5.4",
                "role":"assistant","content":[],"stop_reason":"end_turn","usage":usage
            })))
            .expect(1)
            .mount(&server)
            .await;
        let pricing = serde_json::from_value(json!({"openai":{"gpt-5.4":{
            "multiplierBps":10000,"bands":{"standard":{
                "input":"2","output":"4","cacheRead":"0.5","cacheWrite":"3"
            }}
        }}}))
        .unwrap();
        let mut stream = provider(&store)
            .execute(
                planned_request("openai", messages_operation(false)),
                context_with_pricing(
                    "req_partial_messages_usage",
                    CancellationToken::new(),
                    FastMode::Default,
                    pricing,
                ),
            )
            .await
            .unwrap();
        let mut received = Usage::new();
        while let Some(event) = stream.next().await {
            for fact in event.unwrap().canonical_facts() {
                match fact {
                    GatewayEvent::Usage(value) => received.merge(value),
                    GatewayEvent::CalculatedCost(_) => panic!("缺失或溢出的用量不能计费"),
                    _ => {}
                }
            }
        }
        assert_eq!(
            received.input_tokens,
            usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .filter(|_| usage.get("cache_read_input_tokens").is_none())
        );
        assert_eq!(
            received.output_tokens,
            usage.get("output_tokens").and_then(Value::as_u64)
        );
        server.verify().await;
    }
}

#[tokio::test]
async fn rotated_messages_credentials_do_not_reuse_old_model_capabilities() {
    use gateway_core::account::{CredentialCasUpdate, ProviderAccountUpdate};
    use provider_openai::credential::{CodexCredentialCodec, CodexCredentialData};

    let first = MockServer::start().await;
    let next = MockServer::start().await;
    let store = Arc::new(MemoryAccountStore::default());
    seed(&store, &first, ApiKeyProtocol::Messages).await;
    for (server, limit) in [(&first, 2048), (&next, 4096)] {
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data":[{"id":"gpt-5.4","max_tokens":limit}],"has_more":false
            })))
            .expect(1)
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(MESSAGES_SSE, "text/event-stream"),
            )
            .expect(1)
            .mount(server)
            .await;
    }
    let provider = provider(&store);
    for (index, server, limit) in [(0, &first, 2048), (1, &next, 4096)] {
        if index == 1 {
            let id = ProviderAccountId::new("acct_provider_contract").unwrap();
            let loaded = store.load_current_credential(&id).await.unwrap();
            let mut data = CodexCredentialCodec::decode_complete(&loaded.credential).unwrap();
            let CodexCredentialData::ApiKey(config) = &mut data else {
                panic!("API Key 凭据");
            };
            config.base_url = next.uri();
            let update = CredentialCasUpdate::new(
                id.clone(),
                loaded.account.revision(),
                ProviderAccountUpdate {
                    account_id: id,
                    name: "acct_provider_contract".to_owned(),
                    email: None,
                    plan_type: None,
                },
                CodexCredentialCodec::encode_complete(data).unwrap(),
                false,
                None,
                None,
            )
            .unwrap()
            .preserving_profile();
            store.compare_and_swap_credential(update).await.unwrap();
        }
        let body = json!({
            "model":"gpt-5.4","input":"hello","max_output_tokens":8192,
            "reasoning":{"effort":"high"}
        });
        let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object("openai", body.as_object().unwrap().clone()).unwrap(),
        ));
        let mut stream = provider
            .clone()
            .execute(
                planned_request("openai", operation),
                context(
                    "req_messages_rotated_capabilities",
                    CancellationToken::new(),
                ),
            )
            .await
            .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        let requests = server.received_requests().await.unwrap();
        let sent = captured_request_body(
            requests
                .iter()
                .find(|request| request.method == "POST")
                .unwrap(),
        );
        assert_eq!(sent["max_tokens"], limit);
        assert_eq!(sent["thinking"]["budget_tokens"], limit - 1);
        server.verify().await;
    }
}

#[tokio::test]
async fn streamed_thinking_tools_and_usage_survive_native_and_translated_delivery() {
    let values = [
        json!({"type":"message_start","message":{
            "id":"msg_tools","type":"message","role":"assistant","model":"gpt-5.4",
            "content":[],"usage":{"input_tokens":5,"output_tokens":0}
        }}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"check the weather"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"real-native-signature"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_weather","name":"weather","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"city\":\"Paris\"}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
        json!({"type":"message_stop"}),
    ];
    let raw = values
        .iter()
        .map(|value| {
            format!(
                "event: {}\ndata: {value}\n\n",
                value["type"].as_str().unwrap(),
            )
        })
        .collect::<String>();
    for native in [false, true] {
        let server = MockServer::start().await;
        let store = Arc::new(MemoryAccountStore::default());
        seed(&store, &server, ApiKeyProtocol::Messages).await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(raw.clone(), "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        let operation = if native {
            messages_operation(true)
        } else {
            generate_operation()
        };
        let mut stream = provider(&store)
            .execute(
                planned_request("openai", operation),
                context("req_messages_tools", CancellationToken::new()),
            )
            .await
            .unwrap();
        let mut reasoning = String::new();
        let mut tool_name = None;
        let mut arguments = String::new();
        let mut usage = Usage::new();
        let mut native_wire = Vec::new();
        let mut translated_terminal = None;
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let mut terminal = false;
            for fact in event.canonical_facts() {
                match fact {
                    GatewayEvent::ReasoningDelta(delta) => reasoning.push_str(&delta.text),
                    GatewayEvent::ToolCallDelta(delta) => {
                        assert_eq!(delta.call_id, "tool_weather");
                        tool_name = delta.name.clone().or(tool_name);
                        arguments.push_str(&delta.arguments_delta);
                    }
                    GatewayEvent::Usage(value) => usage.merge(value),
                    GatewayEvent::Completed(meta) => {
                        terminal = true;
                        assert_eq!(meta.finish_reason(), Some(FinishReason::ToolCall));
                    }
                    _ => {}
                }
            }
            for translated in stream.translate_native_response(event, terminal).unwrap() {
                if let Some(wire) = translated.wire_event() {
                    if native {
                        native_wire.extend_from_slice(wire.raw_sse_frame().unwrap());
                    } else if wire.data()["type"] == "response.completed" {
                        translated_terminal = Some(wire.data()["response"].clone());
                    }
                }
            }
        }
        assert_eq!(reasoning, "check the weather");
        assert_eq!(tool_name.as_deref(), Some("weather"));
        assert_eq!(arguments, "{\"city\":\"Paris\"}");
        assert_eq!(usage.input_tokens, Some(5));
        assert_eq!(usage.output_tokens, Some(3));
        if native {
            assert_eq!(native_wire, raw.as_bytes());
        } else {
            let terminal = translated_terminal.unwrap();
            let output = terminal["output"].as_array().unwrap();
            let tool = output
                .iter()
                .find(|item| item["type"] == "function_call")
                .unwrap();
            assert_eq!(tool["call_id"], "tool_weather");
            assert_eq!(tool["name"], "weather");
            assert_eq!(tool["arguments"], "{\"city\":\"Paris\"}");
            assert!(output.iter().any(|item| item["type"] == "reasoning"));
        }
        server.verify().await;
    }
}

//! 跨渠道历史在真实 HTTP/WS 出站中的转换、默认保留与发送前拒绝边界

use super::*;

mod rotation;

fn mixed_history() -> Value {
    json!({
        "model":"gpt-5.4", "store":false, "stream":true,
        "instructions":"Keep the visible conversation",
        "input":[
            {"type":"message","role":"user","content":"Remember blue"},
            {"type":"reasoning","id":"rs_channel_a","summary":[],"encrypted_content":"ciphertext-bound-to-different-id"},
            {"type":"message","role":"assistant","id":"msg_original","phase":"commentary","content":[{"type":"output_text","text":"Checking the value"}]},
            {"type":"function_call","id":"fc_original","call_id":"call_original","name":"lookup","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_original","output":"blue"},
            {"type":"reasoning","id":"rs_channel_b","summary":[],"encrypted_content":"second-channel-ciphertext"},
            {"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"The value is blue"}]},
            {"type":"message","role":"user","content":"What was the value?"}
        ],
        "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}],
        "future_field":{"encrypted_content":"ordinary-extension-data"},
        "client_metadata":{"x-codex-turn-state":"old-channel-state","future":"preserved"}
    })
}

fn extended_history() -> Value {
    let mut body = mixed_history();
    let input = body["input"].as_array_mut().unwrap();
    input.insert(0, json!({"type":"additional_tools","role":"developer","tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]}));
    input.push(json!({"type":"agent_message","id":"amsg_original","author":"helper","recipient":"main","content":[{"type":"input_text","text":"Preserve the helper result"}]}));
    input.push(json!({"type":"tool_search_call","id":"tsc_original","call_id":"search_original","arguments":"{}","status":"completed"}));
    input.push(json!({"type":"tool_search_output","call_id":"search_original","tools":[]}));
    input.push(json!({"type":"configuration_update","reasoning":{"effort":"high"}}));
    input.push(json!({"type":"compaction","id":"cmp_original","encrypted_content":"preserve-compacted-context"}));
    input.push(json!({"type":"future_history_item","id":"future_original","encrypted_content":"preserve-opaque-business-context","future":{"unchanged":true}}));
    input.push(json!({"type":"reasoning","id":"rs_plaintext","summary":[{"type":"summary_text","text":"Preserve readable reasoning"}]}));
    input[5]["encrypted_content"] = json!("preserve-encrypted-tool-output");
    body
}

#[tokio::test]
async fn portable_history_recovers_extended_history_and_keeps_later_turns() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &wiremock::Request| {
            let body = captured_request_body(request);
            if body["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["id"] == "rs_channel_a")
            {
                encrypted_failure(400)
            } else {
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
            }
        })
        .expect(4)
        .mount(&server)
        .await;
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_provider_contract").await;
    let cache = Arc::new(HistoryCache::default());
    let provider = with_cache(
        provider_with_base_url(&accounts, server.uri()),
        cache.clone(),
    );
    let mut body = extended_history();
    body["thread_id"] = json!("thread-extended-history");
    let mut expected_inputs = Vec::new();
    for turn in 0..3 {
        if turn > 0 {
            let input = body["input"].as_array_mut().unwrap();
            input.push(json!({"type":"reasoning","id":format!("rs_new_{turn}"),"encrypted_content":format!("new-ciphertext-{turn}"),"summary":[]}));
            input.push(json!({"type":"function_call","id":format!("fc_{turn}"),"call_id":format!("call_{turn}"),"name":"lookup","arguments":"{}"}));
            input.push(json!({"type":"function_call_output","call_id":format!("call_{turn}"),"output":"blue"}));
            input.push(
                json!({"type":"message","role":"user","content":format!("Continue turn {turn}")}),
            );
            input
                .push(json!({"type":format!("new_extension_{turn}"),"content":{"unchanged":true}}));
        }
        let expected = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["id"] != "rs_channel_a" && item["id"] != "rs_channel_b")
            .cloned()
            .collect::<Vec<_>>();
        expected_inputs.push(expected);
        run_history(
            provider.clone(),
            history_operation(body.clone(), false, false),
            1,
            true,
        )
        .await
        .unwrap();
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        captured_request_body(&requests[0])["input"],
        extended_history()["input"]
    );
    for (request, expected) in requests[1..].iter().zip(expected_inputs) {
        assert_eq!(captured_request_body(request)["input"], json!(expected));
    }
    assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn portable_history_preserves_normal_reasoning_before_http_and_ws_send() {
    for api_key in [false, true] {
        for websocket in [false, true] {
            for lite in [false, true] {
                let original = mixed_history();
                let actual = capture_downstream_request_with_portable_history(
                    original.clone(),
                    api_key,
                    websocket,
                    lite,
                    true,
                )
                .await;
                for field in ["input", "tools", "instructions", "future_field"] {
                    assert_eq!(
                        actual[field], original[field],
                        "{field}: api_key={api_key}, websocket={websocket}, lite={lite}"
                    );
                }
                assert_eq!(actual["client_metadata"]["future"], "preserved");
            }
        }
    }
}

#[tokio::test]
async fn portable_history_large_recovery_records_bounded_private_structure() {
    let server = MockServer::start().await;
    let calls = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                encrypted_failure(400)
            } else {
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_provider_contract").await;
    let cache = Arc::new(HistoryCache::default());
    let provider = with_cache(
        provider_with_base_url(&accounts, server.uri()),
        cache.clone(),
    );
    let mut body = extended_history();
    let input = body["input"].as_array_mut().unwrap();
    for index in 0..32 {
        input.push(
            json!({"type":format!("private-extension-{index}"),"content":"private-extension-body"}),
        );
        input.push(json!({"type":"reasoning","id":format!("rs_private_{index}"),"encrypted_content":format!("private-ciphertext-{index}"),"summary":[]}));
    }
    input.push(
        json!({"type":"message","role":"user","content":"private-transcript".repeat(32_000)}),
    );
    let expected = input
        .iter()
        .filter(|item| item["type"] != "reasoning" || item.get("encrypted_content").is_none())
        .cloned()
        .collect::<Vec<_>>();
    let mut session = start_history(
        provider,
        history_operation(body.clone(), false, false),
        1,
        true,
        "key_openai_contract",
    )
    .await
    .unwrap();
    let trace = session.trace();
    session.collect_uncommitted().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(captured_request_body(&requests[0])["input"], body["input"]);
    assert_eq!(
        captured_request_body(&requests[1])["input"],
        json!(expected)
    );
    let snapshot = trace.snapshot().unwrap();
    let evidence = snapshot["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["stage"] == "provider.request.input")
        .map(|event| &event["data"])
        .collect::<Vec<_>>();
    assert!(!evidence.is_empty());
    let structure = &evidence[0]["input"]["structure"];
    assert_eq!(structure["types"].as_array().unwrap().len(), 16);
    assert!(structure["omittedTypeItems"].as_u64().unwrap() > 0);
    assert_eq!(structure["reasoning"].as_array().unwrap().len(), 16);
    assert_eq!(structure["omittedReasoningItems"], 19);
    assert_eq!(
        structure["reasoning"][0]["id"],
        gateway_core::diagnostics::json_fingerprint(&json!("rs_channel_a"))
    );
    assert_eq!(
        structure["reasoning"][0]["encryptedContent"],
        gateway_core::diagnostics::json_fingerprint(&json!("ciphertext-bound-to-different-id"))
    );
    for data in evidence {
        assert!(data.to_string().len() < 16 * 1024);
    }
    let serialized = snapshot.to_string();
    for secret in [
        "private-transcript",
        "private-extension-",
        "private-extension-body",
        "rs_private_",
        "private-ciphertext-",
        "rs_channel_a",
        "ciphertext-bound-to-different-id",
    ] {
        assert!(
            !serialized.contains(secret),
            "unexpected raw evidence: {secret}"
        );
    }
    assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn portable_history_does_not_reject_normal_compaction() {
    let mut original = mixed_history();
    original["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"compaction","encrypted_content":"opaque"}));
    for api_key in [false, true] {
        let actual = capture_downstream_request_with_portable_history(
            original.clone(),
            api_key,
            false,
            false,
            true,
        )
        .await;
        assert_eq!(actual["input"], original["input"]);
    }
}

use gateway_core::engine::provider::{Provider, ProviderRegistry};
use gateway_core::engine::{AttemptCoordinator, EngineError, GatewayEngine};
use gateway_core::event::ProviderEvent;
use gateway_core::provider_ports::{
    ProviderHistoryExclusionPort, ProviderSessionAffinityKey, ProviderStoreError,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct HistoryCache {
    values: Mutex<std::collections::BTreeMap<String, BTreeSet<String>>>,
    reads: AtomicUsize,
    writes: AtomicUsize,
    stalled: bool,
}

impl ProviderHistoryExclusionPort for HistoryCache {
    fn matching<'a>(
        &'a self,
        scope: &'a ProviderSessionAffinityKey,
        candidates: &'a BTreeSet<String>,
        _: Duration,
    ) -> BoxFuture<'a, Result<BTreeSet<String>, ProviderStoreError>> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.stalled {
                return futures::future::pending().await;
            }
            Ok(self
                .values
                .lock()
                .unwrap()
                .get(scope.expose_to_store())
                .map(|values| values.intersection(candidates).cloned().collect())
                .unwrap_or_default())
        })
    }
    fn merge<'a>(
        &'a self,
        scope: &'a ProviderSessionAffinityKey,
        values: &'a BTreeSet<String>,
        _: Duration,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if self.stalled {
                return futures::future::pending().await;
            }
            self.values
                .lock()
                .unwrap()
                .entry(scope.expose_to_store().to_owned())
                .or_default()
                .extend(values.iter().cloned());
            Ok(true)
        })
    }
}

fn history_operation(body: Value, websocket: bool, lite: bool) -> Operation {
    Operation::Generate(GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
            .unwrap()
            .with_context(Map::from_iter([
                ("use_websocket".to_owned(), json!(websocket)),
                (
                    "responses_lite".to_owned(),
                    json!(if lite { "true" } else { "false" }),
                ),
            ])),
    ))
}

async fn run_history(
    provider: Arc<CodexProvider>,
    operation: Operation,
    concurrency: u32,
    enabled: bool,
) -> Result<Vec<ProviderEvent>, EngineError> {
    start_history(
        provider,
        operation,
        concurrency,
        enabled,
        "key_openai_contract",
    )
    .await?
    .collect_uncommitted()
    .await
}

async fn run_history_committed(
    provider: Arc<CodexProvider>,
    operation: Operation,
) -> Result<Vec<ProviderEvent>, EngineError> {
    let mut session = start_history(provider, operation, 1, true, "key_openai_contract").await?;
    let mut events = Vec::new();
    while let Some(batch) = session.next_event().await? {
        if batch.commit_requirement()
            == gateway_core::engine::CommitRequirement::CommitBeforeDelivery
        {
            session.commit_downstream(Some(200)).await?;
        }
        events.extend(batch.into_provider_events());
    }
    Ok(events)
}

async fn start_history(
    provider: Arc<CodexProvider>,
    operation: Operation,
    concurrency: u32,
    enabled: bool,
    client_key: &str,
) -> Result<
    gateway_core::engine::ResponseExecutionSession<super::selection_retry::NoopExecutionStore>,
    EngineError,
> {
    let kind = ProviderKind::new("openai").unwrap();
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        SettingsValues::new(concurrency, "smart", Default::default(), None, None),
        vec![kind.clone()],
        vec![ProviderModel::new(
            kind,
            UpstreamModelId::new("gpt-5.4").unwrap(),
            ModelCapabilities::new(BTreeSet::from([operation.kind()]), Some(32_000))
                .with_upstream_feature_validation(),
        )],
        Vec::new(),
    )
    .unwrap();
    let plan = snapshot
        .plan(
            &PublicModelId::new("gpt-5.4").unwrap(),
            &operation,
            Arc::new(
                contract_account_scope()
                    .as_ref()
                    .clone()
                    .with_portable_history(enabled),
            ),
            &RoutingContext::default(),
        )
        .unwrap();
    let registry = ProviderRegistry::new([provider as Arc<dyn Provider>]).unwrap();
    let coordinator = AttemptCoordinator::new(GatewayEngine::new(
        Arc::new(super::selection_retry::NoopExecutionStore),
        registry,
    ));
    let mut request = super::selection_retry::execution_request(
        "req_history_recovery",
        &operation,
        Duration::from_secs(30),
    );
    request.client_api_key_ref = ClientApiKeyId::new(client_key).unwrap();
    coordinator
        .start(
            request,
            operation,
            plan,
            None,
            None,
            CancellationToken::new(),
        )
        .await
}

#[tokio::test]
async fn newapi_token_affinity_keeps_portable_history_recovery_on_the_selected_account() {
    use gateway_core::provider_ports::ProviderAffinityScope;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &wiremock::Request| {
            if captured_request_body(request)["input"]
                .as_array()
                .is_some_and(|input| input.iter().any(|item| item["id"] == "rs_channel_a"))
            {
                encrypted_failure(400)
            } else {
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
            }
        })
        .expect(3)
        .mount(&server)
        .await;
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_provider_contract").await;
    let affinity = Arc::new(MemorySessionAffinity::default());
    let cache = Arc::new(HistoryCache::default());
    let provider = with_cache(
        provider_with_affinity_and_base_url(&accounts, affinity.clone(), server.uri()),
        cache.clone(),
    );
    let settings = || {
        SettingsValues::new(1, "smart", Default::default(), None, None)
            .with_request_location(global_request_location(), true)
    };
    let scope = || {
        Arc::new(
            contract_account_scope()
                .as_ref()
                .clone()
                .with_portable_history(true),
        )
    };
    super::trusted_execution::start(
        provider.clone(),
        history_operation(json!({"model":"gpt-5.4","input":"hello"}), false, false),
        settings(),
        scope(),
        "key_shared",
    )
    .await
    .collect_uncommitted()
    .await
    .unwrap();
    create_account(&accounts, "acct_scope_new").await;
    super::trusted_execution::start(
        provider,
        history_operation(mixed_history(), false, false),
        settings(),
        scope(),
        "key_rotated",
    )
    .await
    .collect_uncommitted()
    .await
    .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[1].headers.get("authorization"),
        requests[2].headers.get("authorization")
    );
    let replay = captured_request_body(&requests[2]);
    assert!(
        replay["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["type"] != "reasoning")
    );
    assert!(
        replay["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "function_call_output")
    );
    assert_eq!(
        affinity
            .binding_for_scope(ProviderAffinityScope::Token)
            .1
            .account_id()
            .as_str(),
        "acct_provider_contract"
    );
    assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
    server.verify().await;
}

fn with_cache(provider: Arc<CodexProvider>, cache: Arc<HistoryCache>) -> Arc<CodexProvider> {
    Arc::new(
        Arc::try_unwrap(provider)
            .ok()
            .unwrap()
            .with_history_exclusions(cache),
    )
}

fn encrypted_failure(status: u16) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({"error":{
        "code":"invalid_encrypted_content", "type":"invalid_request_error", "message":"fixture encrypted rejection"
    }}))
}

#[tokio::test]
async fn portable_history_recovers_once_without_clearing_normal_input() {
    for api_key in [false, true] {
        for fail_again in [false, true] {
            let server = MockServer::start().await;
            let calls = AtomicUsize::new(0);
            Mock::given(method("POST"))
                .respond_with(move |_: &wiremock::Request| {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 || fail_again {
                        encrypted_failure(400)
                    } else {
                        ResponseTemplate::new(200)
                            .set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
                    }
                })
                .expect(2)
                .mount(&server)
                .await;
            let accounts = Arc::new(MemoryAccountStore::default());
            if api_key {
                accounts
                    .seed_api_key(
                        "acct_provider_contract",
                        server.uri(),
                        provider_openai::credential::ResponsesTransport::Http,
                    )
                    .await;
            } else {
                create_account(&accounts, "acct_provider_contract").await;
            }
            let cache = Arc::new(HistoryCache::default());
            let provider = with_cache(
                provider_with_base_url(&accounts, server.uri()),
                cache.clone(),
            );
            let result = run_history(
                provider,
                history_operation(mixed_history(), false, false),
                1,
                true,
            )
            .await;
            assert_eq!(result.is_ok(), !fail_again, "{result:?}");
            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests.len(), 2);
            let first = captured_request_body(&requests[0]);
            let second = captured_request_body(&requests[1]);
            assert_eq!(first["input"], mixed_history()["input"]);
            assert!(
                !second["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["type"] == "reasoning")
            );
            for field in ["tools", "instructions", "future_field"] {
                assert_eq!(first[field], second[field]);
            }
            assert_eq!(
                cache.writes.load(Ordering::SeqCst),
                usize::from(!fail_again)
            );
        }
    }
}

#[tokio::test]
async fn portable_history_keeps_request_local_conversion_after_recoverable_503() {
    let server = MockServer::start().await;
    let calls = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .respond_with(
            move |_: &wiremock::Request| match calls.fetch_add(1, Ordering::SeqCst) {
                0 => encrypted_failure(400),
                1 => ResponseTemplate::new(503)
                    .insert_header("retry-after", "0")
                    .set_body_json(
                        json!({"error":{"code":"server_error","message":"fixture busy"}}),
                    ),
                _ => ResponseTemplate::new(200)
                    .set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
            },
        )
        .expect(3)
        .mount(&server)
        .await;
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_provider_contract").await;
    let cache = Arc::new(HistoryCache::default());
    let provider = with_cache(
        provider_with_base_url(&accounts, server.uri()),
        cache.clone(),
    );
    run_history(
        provider,
        history_operation(mixed_history(), false, false),
        3,
        true,
    )
    .await
    .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        captured_request_body(&requests[1])["input"],
        captured_request_body(&requests[2])["input"]
    );
    assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn portable_history_filters_only_old_items_and_isolates_threads() {
    let server = MockServer::start().await;
    let calls = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                encrypted_failure(400)
            } else {
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
            }
        })
        .expect(4)
        .mount(&server)
        .await;
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_provider_contract").await;
    let cache = Arc::new(HistoryCache::default());
    let provider = with_cache(provider_with_base_url(&accounts, server.uri()), cache);
    let mut body = mixed_history();
    body["thread_id"] = json!("thread-original");
    run_history(
        provider.clone(),
        history_operation(body.clone(), false, false),
        1,
        true,
    )
    .await
    .unwrap();
    let new =
        json!({"type":"reasoning","id":"rs_new","encrypted_content":"new-ciphertext","summary":[]});
    body["input"].as_array_mut().unwrap().push(new.clone());
    run_history(
        provider.clone(),
        history_operation(body.clone(), false, false),
        1,
        true,
    )
    .await
    .unwrap();
    body["thread_id"] = json!("thread-other");
    run_history(provider, history_operation(body, false, false), 1, true)
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let third = captured_request_body(&requests[2]);
    let reasoning: Vec<_> = third["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "reasoning")
        .collect();
    assert_eq!(reasoning, vec![&new]);
    let fourth = captured_request_body(&requests[3]);
    assert_eq!(
        fourth["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .count(),
        3
    );
}

#[tokio::test]
async fn portable_history_cache_preserves_changed_pairs_and_other_callers() {
    let server = MockServer::start().await;
    let calls = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                encrypted_failure(400)
            } else {
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
            }
        })
        .expect(6)
        .mount(&server)
        .await;
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_provider_contract").await;
    let cache = Arc::new(HistoryCache::default());
    let provider = with_cache(
        provider_with_base_url(&accounts, server.uri()),
        cache.clone(),
    );
    let mut original = extended_history();
    original["thread_id"] = json!("thread-cache-isolation");
    run_history(
        provider.clone(),
        history_operation(original.clone(), false, false),
        1,
        true,
    )
    .await
    .unwrap();
    let mut expected = Vec::new();
    for case in ["ciphertext", "id", "caller", "disabled"] {
        let mut body = original.clone();
        match case {
            "ciphertext" => body["input"][2]["encrypted_content"] = json!("replacement-ciphertext"),
            "id" => body["input"][2]["id"] = json!("rs_replacement"),
            _ => (),
        }
        let mut expected_input = body["input"].clone();
        if matches!(case, "ciphertext" | "id") {
            expected_input
                .as_array_mut()
                .unwrap()
                .retain(|item| item["id"] != "rs_channel_b");
        }
        expected.push(expected_input);
        start_history(
            provider.clone(),
            history_operation(body, false, false),
            1,
            case != "disabled",
            if case == "caller" {
                "key_other_caller"
            } else {
                "key_openai_contract"
            },
        )
        .await
        .unwrap()
        .collect_uncommitted()
        .await
        .unwrap();
    }
    let requests = server.received_requests().await.unwrap();
    for (request, expected) in requests[2..].iter().zip(expected) {
        assert_eq!(captured_request_body(request)["input"], expected);
    }
    assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn portable_history_never_converts_unrelated_errors_or_non_self_contained_history() {
    for case in [
        "disabled",
        "500",
        "409_without_code",
        "missing_call",
        "only_reasoning",
        "item_reference",
        "implicit_reference",
        "conversation",
        "missing_output",
        "duplicate_call",
        "invalid_item",
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(match case {
                "500" => encrypted_failure(500),
                "409_without_code" => ResponseTemplate::new(409)
                    .set_body_json(json!({"error":{"message":"invalid_encrypted_content"}})),
                _ => encrypted_failure(400),
            })
            .expect(if case == "500" { 4 } else { 1 })
            .mount(&server)
            .await;
        let accounts = Arc::new(MemoryAccountStore::default());
        create_account(&accounts, "acct_provider_contract").await;
        let cache = Arc::new(HistoryCache::default());
        let provider = with_cache(
            provider_with_base_url(&accounts, server.uri()),
            cache.clone(),
        );
        let mut body = mixed_history();
        match case {
            "missing_call" => body["input"]
                .as_array_mut()
                .unwrap()
                .retain(|item| item["type"] != "function_call"),
            "only_reasoning" => body["input"]
                .as_array_mut()
                .unwrap()
                .retain(|item| item["type"] == "reasoning"),
            "item_reference" => body["input"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"item_reference","id":"msg_external"})),
            "implicit_reference" => body["input"].as_array_mut().unwrap().push(json!({"id":"msg_external"})),
            "conversation" => body["conversation"] = json!({"id":"conv_external"}),
            "missing_output" => body["input"].as_array_mut().unwrap().retain(|item| item["type"] != "function_call_output"),
            "duplicate_call" => body["input"].as_array_mut().unwrap().insert(4, json!({"type":"function_call","call_id":"call_original","name":"lookup","arguments":"{}"})),
            "invalid_item" => body["input"].as_array_mut().unwrap().push(json!(42)),
            _ => (),
        }
        assert!(
            run_history(
                provider,
                history_operation(body, false, false),
                1,
                case != "disabled"
            )
            .await
            .is_err(),
            "case {case}"
        );
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            if case == "500" { 4 } else { 1 },
            "case {case}"
        );
        let requests = server.received_requests().await.unwrap();
        let initial_input = captured_request_body(&requests[0])["input"].clone();
        assert!(
            requests
                .iter()
                .all(|request| captured_request_body(request)["input"] == initial_input),
            "case {case}: unrelated errors must preserve encrypted history"
        );
        assert_eq!(cache.writes.load(Ordering::SeqCst), 0);
        if case == "disabled" {
            assert_eq!(cache.reads.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn portable_history_cache_timeouts_do_not_block_request_local_recovery() {
    let server = MockServer::start().await;
    let calls = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                encrypted_failure(400)
            } else {
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_provider_contract").await;
    let provider = with_cache(
        provider_with_base_url(&accounts, server.uri()),
        Arc::new(HistoryCache {
            stalled: true,
            ..Default::default()
        }),
    );
    timeout(
        Duration::from_secs(3),
        run_history(
            provider,
            history_operation(mixed_history(), false, false),
            1,
            true,
        ),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn portable_history_http_and_websocket_stream_errors_respect_delivery_barrier() {
    for api_key in [false, true] {
        for websocket in [false, true] {
            for lite in [false, true] {
                for semantic_output in [false, true] {
                    let created = json!({"type":"response.created","response":{"id":"resp_history","model":"gpt-5.4","status":"in_progress","output":[]}});
                    let output = json!({"type":"response.output_text.delta","item_id":"msg_history","output_index":0,"content_index":0,"delta":"visible"});
                    let failure = json!({"type":"error","status":400,"error":{"code":"invalid_encrypted_content","type":"invalid_request_error","message":"fixture rejected"}});
                    let complete = json!({"type":"response.completed","response":{"id":"resp_history","model":"gpt-5.4","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}});
                    let captured = Arc::new(Mutex::new(Vec::new()));
                    let http = MockServer::start().await;
                    let (base, task) = if websocket {
                        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                        let base = format!("http://{}", listener.local_addr().unwrap());
                        let captured = captured.clone();
                        let task = tokio::spawn(async move {
                            let mut connections = tokio::task::JoinSet::new();
                            loop {
                                tokio::select! {
                                    accepted = listener.accept() => {
                                        let (stream, _) = accepted.unwrap();
                                        let captured = captured.clone();
                                        let created = created.clone(); let output = output.clone(); let failure = failure.clone(); let complete = complete.clone();
                                        connections.spawn(async move {
                                            let mut socket = accept_codex_test_websocket(stream).await;
                                            while let Some(Ok(Message::Text(frame))) = socket.next().await {
                                                let value: Value = serde_json::from_str(&frame).unwrap();
                                                let first = { let mut values = captured.lock().unwrap(); let first = values.is_empty(); values.push(value); first };
                                                socket.send(Message::Text(created.to_string().into())).await.unwrap();
                                                if first && semantic_output { socket.send(Message::Text(output.to_string().into())).await.unwrap(); }
                                                socket.send(Message::Text(if first {failure.to_string()} else {complete.to_string()}.into())).await.unwrap();
                                            }
                                        });
                                    }
                                    _ = connections.join_next(), if !connections.is_empty() => {}
                                }
                            }
                        });
                        (base, Some(task))
                    } else {
                        let calls = AtomicUsize::new(0);
                        let captured = captured.clone();
                        Mock::given(method("POST"))
                            .respond_with(move |request: &wiremock::Request| {
                                captured
                                    .lock()
                                    .unwrap()
                                    .push(captured_request_body(request));
                                let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
                                let mut events = vec![created.clone()];
                                if first && semantic_output {
                                    events.push(output.clone());
                                }
                                events.push(if first {
                                    failure.clone()
                                } else {
                                    complete.clone()
                                });
                                let body = events
                                    .into_iter()
                                    .map(|event| {
                                        format!(
                                            "event: {}\ndata: {}\n\n",
                                            event["type"].as_str().unwrap(),
                                            event
                                        )
                                    })
                                    .collect::<String>();
                                ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
                            })
                            .expect(if semantic_output { 1 } else { 2 })
                            .mount(&http)
                            .await;
                        (http.uri(), None)
                    };
                    let accounts = Arc::new(MemoryAccountStore::default());
                    if api_key {
                        accounts
                            .seed_api_key(
                                "acct_provider_contract",
                                base.clone(),
                                if websocket {
                                    provider_openai::credential::ResponsesTransport::PreferWebsocket
                                } else {
                                    provider_openai::credential::ResponsesTransport::Http
                                },
                            )
                            .await;
                    } else {
                        create_account(&accounts, "acct_provider_contract").await;
                    }
                    let cache = Arc::new(HistoryCache::default());
                    let provider =
                        with_cache(provider_with_base_url(&accounts, base), cache.clone());
                    let result = timeout(
                        Duration::from_secs(5),
                        run_history_committed(
                            provider,
                            history_operation(extended_history(), websocket, lite),
                        ),
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        result.is_ok(),
                        !semantic_output,
                        "api_key={api_key} ws={websocket} lite={lite} output={semantic_output}: {result:?}"
                    );
                    {
                        let values = captured.lock().unwrap();
                        assert_eq!(values.len(), if semantic_output { 1 } else { 2 });
                        if !semantic_output {
                            let expected = extended_history()["input"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .filter(|item| {
                                    item["id"] != "rs_channel_a" && item["id"] != "rs_channel_b"
                                })
                                .cloned()
                                .collect::<Vec<_>>();
                            assert_eq!(values[1]["input"], json!(expected));
                        }
                        assert_eq!(
                            cache.writes.load(Ordering::SeqCst),
                            usize::from(!semantic_output)
                        );
                    }
                    if let Some(task) = task {
                        task.abort();
                        let _ = task.await;
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn portable_history_incremental_rejection_requests_full_replay_without_rewriting_upstream_facts()
 {
    for include_reasoning in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(encrypted_failure(400))
            .expect(1)
            .mount(&server)
            .await;
        let accounts = Arc::new(MemoryAccountStore::default());
        accounts
            .seed_api_key(
                "acct_provider_contract",
                server.uri(),
                provider_openai::credential::ResponsesTransport::Http,
            )
            .await;
        let mut body = mixed_history();
        if !include_reasoning {
            body["input"] =
                json!([{"type":"function_call_output","call_id":"call_original","output":"blue"}]);
        }
        body["previous_response_id"] = json!("resp_external");
        let cache = Arc::new(HistoryCache::default());
        let provider = with_cache(
            provider_with_base_url(&accounts, server.uri()),
            cache.clone(),
        );
        // 模拟已有续接归属的请求，不能把缺失 owner 的本地拒绝误当作上游密文拒绝
        let context = context_with_state_owner_location_and_scope(
            "req_incremental_history",
            "acct_provider_contract",
            None,
            Arc::new(
                contract_account_scope()
                    .as_ref()
                    .clone()
                    .with_portable_history(true),
            ),
        );
        let mut stream = provider
            .execute(
                planned_request("openai", history_operation(body, false, false)),
                context,
            )
            .await
            .unwrap();
        let error = loop {
            match stream.next().await {
                Some(Err(error)) => break error,
                Some(Ok(_)) => (),
                None => panic!("expected upstream rejection"),
            }
        };
        assert_eq!(error.upstream_status(), Some(400), "{error:?}");
        assert_eq!(
            error.upstream_code().unwrap().as_str(),
            "invalid_encrypted_content"
        );
        assert_eq!(error.send_state(), UpstreamSendState::Sent);
        assert_eq!(
            error.client_visible_upstream_error().unwrap().code(),
            Some("previous_response_not_found")
        );
        assert_eq!(
            error.continuation_recovery_disposition(),
            Some(ContinuationRecoveryDisposition::ClientReplayRequired)
        );
        assert!(error.request_recovery().is_none());
        assert_eq!(cache.reads.load(Ordering::SeqCst), 0);
        assert_eq!(cache.writes.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn portable_history_large_normal_history_preserves_payload_and_queries_only_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_provider_contract").await;
    let cache = Arc::new(HistoryCache::default());
    let provider = with_cache(
        provider_with_base_url(&accounts, server.uri()),
        cache.clone(),
    );
    let mut body = mixed_history();
    let input = body["input"].as_array_mut().unwrap();
    for i in 0..1024 {
        input.push(json!({"type":"reasoning","id":format!("rs_large_{i}"),"summary":[],"encrypted_content":"x".repeat(2048)}));
    }
    let start = std::time::Instant::now();
    run_history(
        provider,
        history_operation(body.clone(), false, false),
        1,
        true,
    )
    .await
    .unwrap();
    eprintln!(
        "history normal path: 1026 reasoning items / >2 MiB, debug loopback elapsed {:?}",
        start.elapsed()
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(captured_request_body(&requests[0])["input"], body["input"]);
    assert_eq!(cache.reads.load(Ordering::SeqCst), 1);
    assert_eq!(cache.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn portable_history_approved_recovery_keeps_new_chain_boundary_when_input_is_already_filtered()
 {
    let server = MockServer::start().await;
    let calls = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                encrypted_failure(400)
            } else {
                ResponseTemplate::new(200).set_body_raw(CAPTURE_COMPLETED_SSE, "text/event-stream")
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let accounts = Arc::new(MemoryAccountStore::default());
    create_account(&accounts, "acct_provider_contract").await;
    let cache = Arc::new(HistoryCache::default());
    let provider = with_cache(
        provider_with_base_url(&accounts, server.uri()),
        cache.clone(),
    );
    let context = context_with_state_owner_location_and_scope(
        "req_already_filtered",
        "acct_provider_contract",
        None,
        Arc::new(
            contract_account_scope()
                .as_ref()
                .clone()
                .with_portable_history(true),
        ),
    );
    let mut stream = provider
        .clone()
        .execute(
            planned_request("openai", history_operation(mixed_history(), false, false)),
            context.clone(),
        )
        .await
        .unwrap();
    let error = loop {
        match stream.next().await {
            Some(Err(error)) => break error,
            Some(Ok(_)) => (),
            None => panic!("expected encrypted rejection"),
        }
    };
    let data = error.request_recovery().cloned().unwrap();
    drop(stream);
    // 上游适配或中间件可已移除候选项，批准的恢复仍须清理原轮次状态并提交排除集
    let mut filtered = mixed_history();
    filtered["input"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item["type"] != "reasoning");
    let mut stream = provider
        .execute(
            planned_request("openai", history_operation(filtered, false, false)),
            context.with_request_recovery(Some(data)),
        )
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let requests = server.received_requests().await.unwrap();
    assert!(
        captured_request_body(&requests[1])["client_metadata"]
            .get("x-codex-turn-state")
            .is_none()
    );
    assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
}

//! 跨渠道历史在真实 HTTP/WS 出站中的转换、默认保留与发送前拒绝边界

use super::*;

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

#[tokio::test]
async fn portable_history_removes_mixed_channel_reasoning_before_the_first_http_and_ws_send() {
    for api_key in [false, true] {
        for websocket in [false, true] {
            for lite in [false, true] {
                let original = mixed_history();
                let mut expected = original.clone();
                expected["input"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|item| item["type"] != "reasoning");
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
                        actual[field], expected[field],
                        "{field}: api_key={api_key}, websocket={websocket}, lite={lite}"
                    );
                }
                assert!(
                    actual["client_metadata"]
                        .get("x-codex-turn-state")
                        .is_none()
                );
                assert_eq!(actual["client_metadata"]["future"], "preserved");
                // 客户端再次提交包含旧密文的历史仍可转换，不依赖换号或首轮缓存
                let repeated = capture_downstream_request_with_portable_history(
                    original, api_key, websocket, lite, true,
                )
                .await;
                assert_eq!(repeated["input"], expected["input"]);
            }
        }
    }
}

#[tokio::test]
async fn native_history_preserves_reasoning_ids_and_ciphertext() {
    for api_key in [false, true] {
        let original = mixed_history();
        let actual = capture_downstream_request(original.clone(), api_key, false, false).await;
        assert_eq!(actual["input"], original["input"]);
    }
}

#[tokio::test]
async fn portable_history_rejects_opaque_history_before_selecting_or_sending() {
    let store = Arc::new(MemoryAccountStore::default());
    let provider = provider(&store);
    let mut cases = vec![
        (
            "previous_response_id",
            json!("resp_channel_a"),
            "previous_response_not_found",
        ),
        (
            "conversation",
            json!("conv_channel_a"),
            "portable_history_unavailable",
        ),
    ];
    for item in [
        json!({"type":"compaction","encrypted_content":"opaque"}),
        json!({"type":"context_compaction","encrypted_content":"opaque"}),
        json!({"type":"item_reference","id":"msg_missing"}),
        json!({"type":"function_call_output","call_id":"call_original","output":[{"type":"encrypted_content","encrypted_content":"opaque"}]}),
        json!({"type":"function_call","call_id":"call_original","name":"lookup","arguments":"{}","encrypted_function_args":"opaque"}),
    ] {
        let mut input = mixed_history()["input"].as_array().unwrap().clone();
        input.push(item);
        cases.push(("input", json!(input), "portable_history_unavailable"));
    }
    cases.push((
        "input",
        json!([{"type":"reasoning","encrypted_content":"only-state"}]),
        "portable_history_unavailable",
    ));
    for (field, value, code) in cases {
        let mut body = mixed_history();
        body[field] = value;
        let operation = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object("openai", body.as_object().unwrap().clone()).unwrap(),
        ));
        let result = Arc::clone(&provider)
            .execute(
                planned_request("openai", operation),
                context_with_state_owner_location_and_scope(
                    "req_portable_rejected",
                    "acct_provider_contract",
                    None,
                    Arc::new(
                        contract_account_scope()
                            .as_ref()
                            .clone()
                            .with_portable_history(true),
                    ),
                ),
            )
            .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("opaque history must fail before account selection"),
        };
        assert_eq!(error.send_state(), UpstreamSendState::NotSent);
        assert_eq!(
            error
                .client_visible_upstream_error()
                .and_then(|error| error.code()),
            Some(code)
        );
        if code == "previous_response_not_found" {
            assert_eq!(
                error.continuation_recovery_disposition(),
                Some(ContinuationRecoveryDisposition::ClientReplayRequired)
            );
        }
    }
}

//! 验证智谱 Responses 错误、事件身份、完成快照与失败用量的真实 Provider 流

use super::provider::{context, provider, request, scope, setup, stream};
use futures::StreamExt;
use gateway_core::{
    engine::provider::Provider,
    error::{ProviderError, ProviderErrorKind},
    event::{GatewayEvent, ProviderEvent},
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use wiremock::{Mock, ResponseTemplate, matchers::method};

fn created() -> Value {
    json!({"type":"response.created","response":{"id":"resp_glm","status":"in_progress","model":"glm-5.3"}})
}

fn message(id: &str, parts: Vec<Value>) -> Value {
    json!({"id":id,"type":"message","status":"completed","role":"assistant","content":parts})
}

fn completed(output: Vec<Value>) -> Value {
    json!({"type":"response.completed","response":{"id":"resp_glm","status":"completed","model":"glm-5.3","output":output}})
}

async fn collect(events: Vec<Value>) -> (Vec<ProviderEvent>, Option<ProviderError>) {
    let (server, store) = setup().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(stream(events), "text/event-stream"))
        .mount(&server)
        .await;
    let scope = scope(&store);
    let mut response = provider(store.clone())
        .execute(
            request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
            context(scope),
        )
        .await
        .unwrap();
    let mut received = Vec::new();
    let mut failure = None;
    while let Some(event) = response.next().await {
        match event {
            Ok(event) => received.push(event),
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    drop(response);
    assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
    (received, failure)
}

fn text_parts(events: &[ProviderEvent]) -> Vec<(u32, String)> {
    events
        .iter()
        .flat_map(ProviderEvent::canonical_facts)
        .filter_map(|fact| match fact {
            GatewayEvent::TextDelta(text) => Some((text.content_index, text.text.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn responses_string_errors_keep_their_category_and_hide_untrusted_messages() {
    for (code, kind, replay_safe) in [
        (
            "authentication_error",
            ProviderErrorKind::Unauthorized,
            true,
        ),
        ("invalid_api_key", ProviderErrorKind::Unauthorized, true),
        ("expired", ProviderErrorKind::Unauthorized, true),
        (
            "permission_denied",
            ProviderErrorKind::PermissionDenied,
            true,
        ),
        ("rate_limit_exceeded", ProviderErrorKind::RateLimited, true),
        (
            "insufficient_quota",
            ProviderErrorKind::QuotaExhausted,
            true,
        ),
        ("quota_exceeded", ProviderErrorKind::QuotaExhausted, true),
        (
            "usage_limit_reached",
            ProviderErrorKind::QuotaExhausted,
            true,
        ),
        (
            "usage_not_included",
            ProviderErrorKind::QuotaExhausted,
            true,
        ),
        (
            "server_is_overloaded",
            ProviderErrorKind::UpstreamCapacityUnavailable,
            true,
        ),
        (
            "overloaded",
            ProviderErrorKind::UpstreamCapacityUnavailable,
            true,
        ),
        (
            "slow_down",
            ProviderErrorKind::UpstreamCapacityUnavailable,
            true,
        ),
        ("server_error", ProviderErrorKind::Unavailable, false),
        ("invalid_request", ProviderErrorKind::InvalidRequest, false),
        ("model_not_found", ProviderErrorKind::InvalidRequest, false),
        (
            "context_length_exceeded",
            ProviderErrorKind::InvalidRequest,
            false,
        ),
        ("not_implemented", ProviderErrorKind::Unsupported, false),
        ("request_too_large", ProviderErrorKind::MessageTooBig, false),
        ("content_filter", ProviderErrorKind::PermissionDenied, false),
        ("cyber_policy", ProviderErrorKind::PermissionDenied, false),
    ] {
        let (_, failure) = collect(vec![json!({
            "type":"error","code":code,"message":"secret-upstream-key"
        })])
        .await;
        let mut failure = failure.unwrap();
        assert_eq!(failure.kind(), kind, "{code}");
        assert_eq!(failure.replay_is_safe(), replay_safe, "{code}");
        assert_eq!(failure.upstream_code().unwrap().as_str(), code);
        let public = failure.client_visible_upstream_error().unwrap();
        assert!(!public.message().contains("secret-upstream-key"));
        let terminal = failure.take_atomic_client_events();
        assert_eq!(terminal.len(), 1);
        assert!(
            !terminal[0]
                .wire_event()
                .unwrap()
                .data()
                .to_string()
                .contains("secret-upstream-key")
        );
        if matches!(code, "content_filter" | "cyber_policy") {
            assert!(failure.retry_is_prohibited());
        }
    }
}

#[tokio::test]
async fn unknown_error_codes_are_not_exposed_or_assumed_replay_safe() {
    let (_, failure) = collect(vec![json!({
        "type":"error","code":"secret-upstream-key","message":"secret-upstream-key"
    })])
    .await;
    let mut failure = failure.unwrap();
    assert_eq!(failure.kind(), ProviderErrorKind::Protocol);
    assert_eq!(
        failure.upstream_code().unwrap().as_str(),
        "glm_upstream_error"
    );
    assert!(!failure.replay_is_safe());
    assert!(!format!("{failure:?}").contains("secret-upstream-key"));
    let terminal = failure.take_atomic_client_events();
    assert!(
        !terminal[0]
            .wire_event()
            .unwrap()
            .data()
            .to_string()
            .contains("secret-upstream-key")
    );
}

#[tokio::test]
async fn failed_response_usage_is_emitted_before_failure_without_a_success_or_raw_terminal() {
    let (events, failure) = collect(vec![created(), json!({
        "type":"response.failed",
        "response":{"id":"resp_glm","status":"failed","error":{"code":"insufficient_quota", "message":"secret-upstream-key"}, "debug":"secret-upstream-key",
            "usage":{"input_tokens":10,"input_tokens_details":{"cached_tokens":2},"output_tokens":3}}
    })]).await;
    let mut failure = failure.unwrap();
    assert_eq!(failure.kind(), ProviderErrorKind::QuotaExhausted);
    assert!(
        !events
            .iter()
            .flat_map(ProviderEvent::canonical_facts)
            .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
    );
    let usage = events
        .iter()
        .find(|event| {
            event
                .canonical_facts()
                .iter()
                .any(|fact| matches!(fact, GatewayEvent::Usage(_)))
        })
        .unwrap();
    assert!(usage.wire_event().is_none());
    assert!(usage.is_replayable_prelude());
    assert!(usage.canonical_facts().iter().any(|fact| matches!(fact,
        GatewayEvent::Usage(usage) if usage.input_tokens == Some(10)
            && usage.cached_input_tokens == Some(2) && usage.output_tokens == Some(3))));
    assert!(
        events
            .iter()
            .filter_map(ProviderEvent::wire_event)
            .all(|wire| wire.event_type() != Some("response.failed"))
    );
    let atomic = failure.take_atomic_client_events();
    assert_eq!(atomic.len(), 1);
    assert!(
        !atomic[0]
            .wire_event()
            .unwrap()
            .data()
            .to_string()
            .contains("secret-upstream-key")
    );
    assert_eq!(
        atomic[0].wire_event().unwrap().event_type(),
        Some("response.failed")
    );
}

#[tokio::test]
async fn completed_snapshot_recovers_done_only_text_and_keeps_original_wire() {
    let expected = vec![
        created(),
        completed(vec![message(
            "msg_glm",
            vec![json!({"type":"output_text","text":"done only"})],
        )]),
    ];
    let original = stream(expected.clone());
    let (events, failure) = collect(expected).await;
    assert!(failure.is_none());
    assert_eq!(text_parts(&events), [(0, "done only".to_owned())]);
    let wire: Vec<_> = events
        .iter()
        .filter_map(ProviderEvent::wire_event)
        .filter_map(|wire| wire.raw_sse_frame())
        .flat_map(|bytes| bytes.iter().copied())
        .collect();
    assert_eq!(wire, original.as_bytes());
}

#[tokio::test]
async fn content_indices_and_completion_snapshots_do_not_merge_or_duplicate_text() {
    let (events, failure) = collect(vec![
        created(),
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"msg_glm","delta":"hello"}),
        json!({"type":"response.output_text.done","output_index":0,"content_index":0,"item_id":"msg_glm","text":"hello world"}),
        json!({"type":"response.output_text.done","output_index":0,"content_index":1,"item_id":"msg_glm","text":"second"}),
        completed(vec![message("msg_glm", vec![
            json!({"type":"output_text","text":"hello world"}),
            json!({"type":"output_text","text":"second"}),
        ])]),
    ]).await;
    assert!(failure.is_none());
    assert_eq!(
        text_parts(&events),
        [
            (0, "hello".to_owned()),
            (0, " world".to_owned()),
            (1, "second".to_owned())
        ]
    );
}

#[tokio::test]
async fn refusal_and_reasoning_done_only_content_is_observed_once() {
    let (events, failure) = collect(vec![
        created(),
        json!({"type":"response.reasoning_text.done","output_index":0,"item_id":"rs_glm","text":"reason"}),
        json!({"type":"response.refusal.done","output_index":1,"content_index":0,"item_id":"msg_glm","refusal":"no"}),
        completed(vec![
            json!({"id":"rs_glm","type":"reasoning","content":[{"type":"reasoning_text","text":"reason"}]}),
            message("msg_glm", vec![json!({"type":"refusal","refusal":"no"})]),
        ]),
    ]).await;
    assert!(failure.is_none());
    assert_eq!(text_parts(&events), [(1, "no".to_owned())]);
    let reasoning: Vec<_> = events
        .iter()
        .flat_map(ProviderEvent::canonical_facts)
        .filter_map(|fact| match fact {
            GatewayEvent::ReasoningDelta(delta) => Some(delta.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning, ["reason"]);
}

#[tokio::test]
async fn tool_completion_recovers_arguments_without_repeating_deltas_or_metadata() {
    for custom in [false, true] {
        let item_type = if custom {
            "custom_tool_call"
        } else {
            "function_call"
        };
        let event_type = if custom {
            "response.custom_tool_call_input.delta"
        } else {
            "response.function_call_arguments.delta"
        };
        let field = if custom { "input" } else { "arguments" };
        let mut initial =
            json!({"type":item_type,"id":"tool_glm","call_id":"call_glm","name":"lookup"});
        initial[field] = json!("");
        let mut final_item = initial.clone();
        final_item[field] = json!("payload");
        let (events, failure) = collect(vec![
            created(),
            json!({"type":"response.output_item.added","output_index":0,"item":initial}),
            json!({"type":event_type,"output_index":0,"item_id":"tool_glm","delta":"pay"}),
            json!({"type":"response.output_item.done","output_index":0,"item":final_item.clone()}),
            completed(vec![final_item]),
        ])
        .await;
        assert!(failure.is_none());
        let calls: Vec<_> = events
            .iter()
            .flat_map(ProviderEvent::canonical_facts)
            .filter_map(|fact| match fact {
                GatewayEvent::ToolCallDelta(call) => Some(call),
                _ => None,
            })
            .collect();
        assert_eq!(calls.iter().filter(|call| call.name.is_some()).count(), 1);
        assert!(calls.iter().all(|call| call.call_id == "call_glm"));
        assert_eq!(
            calls
                .iter()
                .map(|call| call.arguments_delta.as_str())
                .collect::<String>(),
            "payload"
        );
    }
}

#[tokio::test]
async fn conflicting_response_item_content_and_terminal_identities_fail_closed() {
    for events in [
        vec![
            created(),
            json!({"type":"response.in_progress","response":{"id":"other"}}),
        ],
        vec![
            created(),
            json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_a","delta":"a"}),
            json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_b","delta":"b"}),
        ],
        vec![
            created(),
            json!({"type":"response.output_text.delta","output_index":0,"delta":"a"}),
            completed(vec![message(
                "msg_glm",
                vec![json!({"type":"output_text","text":"b"})],
            )]),
        ],
        vec![
            created(),
            json!({"type":"response.completed","response":{"id":"resp_glm","status":"incomplete","output":[]}}),
        ],
        vec![
            created(),
            json!({"type":"response.failed","response":{"id":"resp_glm","status":"completed","error":{"code":"server_error"}}}),
        ],
    ] {
        let (_, failure) = collect(events).await;
        assert_eq!(failure.unwrap().kind(), ProviderErrorKind::Protocol);
    }
}

#[tokio::test]
async fn sse_event_name_cannot_disagree_with_the_json_event_type() {
    let (server, store) = setup().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "event: response.completed\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_glm\"}}\n\n",
            "text/event-stream",
        ))
        .mount(&server)
        .await;
    let scope = scope(&store);
    let mut response = provider(store.clone())
        .execute(
            request(scope.clone(), json!({"model":"glm-5.3","input":"test"})),
            context(scope),
        )
        .await
        .unwrap();
    let mut failure = None;
    while let Some(event) = response.next().await {
        if let Err(error) = event {
            failure = Some(error);
            break;
        }
    }
    assert_eq!(failure.unwrap().kind(), ProviderErrorKind::Protocol);
    drop(response);
    assert_eq!(store.in_flight.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_failure_after_text_keeps_output_and_forbids_replay() {
    let (events, failure) = collect(vec![
        created(),
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"visible"}),
        json!({"type":"error","code":"overloaded"}),
        completed(vec![message("msg_glm", vec![json!({"type":"output_text","text":"ignored"})])]),
    ]).await;
    let failure = failure.unwrap();
    assert_eq!(text_parts(&events), [(0, "visible".to_owned())]);
    assert!(failure.retry_is_prohibited());
    assert!(
        !events
            .iter()
            .flat_map(ProviderEvent::canonical_facts)
            .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
    );
}

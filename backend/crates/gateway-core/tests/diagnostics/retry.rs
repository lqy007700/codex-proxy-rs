//! 验证失败后的第二次出站正文采集、预算、脱敏和请求隔离

use gateway_core::diagnostics::{
    MAX_RETRY_REQUEST_BODY_BYTES, MAX_TRACE_SNAPSHOT_BYTES, TraceContext,
};
use serde_json::json;

#[test]
fn only_second_attempt_after_first_failure_retains_one_outbound_body() {
    let trace = TraceContext::new("req_retry_body");
    let first = trace.attempt(1);
    first.capture("upstream.request.body", br#"{"input":"first"}"#);
    let second = trace.attempt(2);
    second.capture("upstream.request.body", br#"{"input":"before failure"}"#);
    assert!(trace.snapshot().unwrap().get("retryRequestBody").is_none());
    first.record("attempt.failed", json!({"kind":"invalid_request"}));
    second.capture("client.request.body", br#"{"input":"client body"}"#);
    assert!(trace.snapshot().unwrap().get("retryRequestBody").is_none());
    let exchange = second.exchange("http_sse");
    exchange.capture("upstream.request.body", br#"{"input":"actual outbound"}"#);
    second
        .exchange("http_sse")
        .capture("upstream.request.body", br#"{"input":"another exchange"}"#);
    trace
        .attempt(3)
        .capture("upstream.request.body", br#"{"input":"third"}"#);
    let snapshot = trace.snapshot().unwrap();
    let body = &snapshot["retryRequestBody"];
    assert_eq!(body["json"]["input"], "actual outbound");
    assert_eq!(body["attemptIndex"], 2);
    assert_eq!(body["exchangeId"], 1);
    let other = TraceContext::new("req_other").attempt(2);
    other.capture("upstream.request.body", br#"{"input":"unrelated"}"#);
    assert!(other.snapshot().unwrap().get("retryRequestBody").is_none());
}

#[test]
fn large_retry_body_survives_event_pressure_and_removes_credentials() {
    let trace = TraceContext::new("req_retry_large");
    trace.attempt(1).record("attempt.failed", json!({}));
    let prompt = "正常任务内容".repeat(40_000);
    let nested = json!({"access_token":"fixture-nested", "task":"保留任务"}).to_string();
    let plain_output = " { \"task\": \"keep spacing\" } \n";
    let body = json!({
        "input": prompt, "authorization":"fixture-auth", "plain_output": plain_output,
        "tool_output": nested, "note":"Bearer fixture-bearer",
        "proxy":"https://fixture-user:fixture-pass@example.invalid",
    });
    trace
        .attempt(2)
        .capture("upstream.request.body", &serde_json::to_vec(&body).unwrap());
    for index in 0..500 {
        trace.record("sample", json!({"index":index,"detail":"x".repeat(2000)}));
    }
    let snapshot = trace.snapshot().unwrap();
    let captured = &snapshot["retryRequestBody"]["json"];
    assert_eq!(captured["input"], prompt);
    assert_eq!(captured["plain_output"], plain_output);
    assert!(captured["tool_output"].is_string());
    assert!(
        captured["tool_output"]
            .as_str()
            .unwrap()
            .contains("保留任务")
    );
    assert!(!snapshot.to_string().contains("fixture-"));
    assert_eq!(snapshot["retryRequestBody"]["truncated"], false);
    assert!(snapshot.to_string().len() <= MAX_TRACE_SNAPSHOT_BYTES);
    assert!(snapshot["droppedEvents"].as_u64().unwrap() > 0);
    let mut events_only = snapshot;
    events_only
        .as_object_mut()
        .unwrap()
        .remove("retryRequestBody");
    assert!(events_only.to_string().len() <= 128 * 1024);
}

#[test]
fn oversized_and_non_json_retry_bodies_report_the_missing_content() {
    for (bytes, reason) in [
        (
            vec![b'x'; MAX_RETRY_REQUEST_BODY_BYTES + 1],
            "request_body_size_limit",
        ),
        (b"not json".to_vec(), "request_body_not_json"),
    ] {
        let trace = TraceContext::new("req_retry_gap");
        trace.attempt(1).record("attempt.failed", json!({}));
        trace.attempt(2).capture("upstream.request.body", &bytes);
        let snapshot = trace.snapshot().unwrap();
        assert_eq!(snapshot["retryRequestBody"]["omitted"], reason);
        assert!(snapshot["retryRequestBody"]["json"].is_null());
    }
}

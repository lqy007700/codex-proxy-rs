//! 验证第二次 HTTP 出站正文与实际压缩报文一致

use super::super::*;

#[tokio::test]
async fn second_attempt_diagnostics_capture_actual_zstd_http_request_body() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/codex/responses"))
        .respond_with(
            wiremock::ResponseTemplate::new(400)
                .set_body_json(json!({"error":{"message":"test rejection"}})),
        )
        .mount(&server)
        .await;
    let client = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        server.uri(),
        test_wire_profile(),
    );
    let trace = gateway_core::diagnostics::TraceContext::new("req_retry_compressed");
    let mut request = codex_request("gpt-5.5", "检查这个正常任务", Vec::new());
    request.force_http_sse = true;
    for index in 1..=3 {
        let attempt = trace.attempt(index);
        assert!(
            client
                .create_response(
                    &request,
                    request_context("req_retry_compressed", Some("acct")).with_trace(&attempt)
                )
                .await
                .is_err()
        );
        if index == 1 {
            assert!(trace.snapshot().unwrap().get("retryRequestBody").is_none());
            attempt.record("attempt.failed", json!({"kind":"upstream_unavailable"}));
        }
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1].headers.get("content-encoding").unwrap(), "zstd");
    let sent: Value =
        serde_json::from_slice(&zstd::stream::decode_all(requests[1].body.as_slice()).unwrap())
            .unwrap();
    let snapshot = trace.snapshot().unwrap();
    assert_eq!(snapshot["retryRequestBody"]["json"], sent);
    assert_eq!(snapshot["retryRequestBody"]["attemptIndex"], 2);
    assert_eq!(snapshot["retryRequestBody"]["exchangeId"], 2);
}

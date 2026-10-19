//! Responses 首帧保活后的 HTTP 错误投影

use axum::{
    body::{Body, to_bytes},
    http::header,
    response::Response,
};
use gateway_core::engine::execution::ResponseCommit;
use gateway_protocol::openai::sse::{
    DONE_SSE_FRAME, encode_sse_event, response_failed_sse_data_from_error_event,
    response_failed_sse_data_with_id,
};
use serde_json::Value;
use std::future::Future;
use tokio::sync::oneshot;

pub(super) async fn response(
    response: impl Future<Output = Response> + Send + 'static,
    admitted: oneshot::Receiver<ResponseCommit>,
) -> Response {
    crate::openai::middleware::keepalive_response(response, admitted, |response| {
        Box::pin(stream_failure(response))
    })
    .await
}

async fn stream_failure(response: Response) -> Body {
    let status = response.status();
    let retry_after = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = serde_json::from_slice::<Value>(&bytes).ok();
    let raw_message = std::str::from_utf8(&bytes)
        .ok()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    // 已完成客户端错误投影的对象原样保留，包括重试建议和策略错误的扩展字段
    let mut data = json
        .as_ref()
        .and_then(|json| response_failed_sse_data_from_error_event(None, None, json))
        .unwrap_or_else(|| {
            response_failed_sse_data_with_id(
                None,
                "server_error",
                "upstream_error",
                raw_message.unwrap_or_else(|| {
                    status
                        .canonical_reason()
                        .unwrap_or("Response stream failed")
                }),
            )
        });
    // HTTP 头已提交，按官方 SSE 错误合同保留允许下发的重试建议
    if let Some(retry_after) = retry_after
        && let Some(error) = data["response"]["error"].as_object_mut()
        && let Some(headers) = error
            .entry("headers")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
    {
        headers
            .entry("retry-after")
            .or_insert(Value::String(retry_after));
    }
    let mut event = encode_sse_event("response.failed", &data.to_string());
    event.push_str(DONE_SSE_FRAME);
    Body::from(event)
}

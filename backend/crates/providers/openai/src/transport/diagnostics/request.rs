//! 从最终 HTTP 请求采集正文，压缩报文只在第二次尝试时有界还原

use std::io::Read;

use gateway_core::diagnostics::{MAX_RETRY_REQUEST_BODY_BYTES, TraceContext};
use reqwest::{Request, header::CONTENT_ENCODING};

pub(in crate::transport) fn capture_body(trace: &TraceContext, request: &Request) {
    let bytes = request
        .body()
        .and_then(reqwest::Body::as_bytes)
        .unwrap_or_default();
    if trace.needs_retry_request_body()
        && request
            .headers()
            .get(CONTENT_ENCODING)
            .is_some_and(|value| value == "zstd")
    {
        let decoded = zstd::stream::read::Decoder::new(bytes).and_then(|decoder| {
            let mut body = Vec::new();
            decoder
                .take((MAX_RETRY_REQUEST_BODY_BYTES + 1) as u64)
                .read_to_end(&mut body)?;
            Ok(body)
        });
        match decoded {
            Ok(body) if body.len() <= MAX_RETRY_REQUEST_BODY_BYTES => {
                trace.capture_retry_request_body(&body);
            }
            Ok(_) => trace.omit_retry_request_body(bytes, "decoded_request_body_size_limit"),
            Err(_) => trace.omit_retry_request_body(bytes, "request_body_decode_failed"),
        }
    }
    trace.capture("upstream.request.body", bytes);
}

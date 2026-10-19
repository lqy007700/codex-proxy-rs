//! 准入成功后的首帧保活；协议入口负责已提交 HTTP 错误的流内表达

use std::future::Future;

use axum::{
    body::{Body, BodyDataStream, Bytes},
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use futures::{StreamExt as _, future::BoxFuture, stream};
use gateway_core::engine::execution::ResponseCommit;
use tokio::sync::oneshot;

use crate::openai::middleware::{SSE_KEEPALIVE_FRAME, SSE_KEEPALIVE_INTERVAL};

pub(crate) async fn response(
    response: impl Future<Output = Response> + Send + 'static,
    admitted: oneshot::Receiver<ResponseCommit>,
    failure: fn(Response) -> BoxFuture<'static, Body>,
) -> Response {
    let mut response = Box::pin(response);
    // 鉴权、模型门禁和准入失败仍保留原 HTTP 状态，不提前建立 SSE
    let commit = tokio::select! {
        biased;
        response = &mut response => return response,
        admitted = admitted => match admitted {
            Ok(commit) => commit,
            Err(_) => return response.await,
        },
    };
    tokio::select! {
        biased;
        response = &mut response => return response,
        () = tokio::time::sleep(SSE_KEEPALIVE_INTERVAL) => {}
    }
    if !commit.record(Some(StatusCode::OK.as_u16())) {
        return response.await;
    }
    let pending = PendingResponse::Waiting(response, failure);
    let stream = stream::once(async { Ok::<_, axum::Error>(SSE_KEEPALIVE_FRAME) })
        .chain(stream::try_unfold(pending, PendingResponse::next));
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

enum PendingResponse {
    Waiting(
        BoxFuture<'static, Response>,
        fn(Response) -> BoxFuture<'static, Body>,
    ),
    Streaming(BodyDataStream),
}

impl PendingResponse {
    async fn next(mut self) -> Result<Option<(Bytes, Self)>, axum::Error> {
        loop {
            match &mut self {
                Self::Waiting(response, failure) => {
                    // 心跳只轮询同一个 future，不取消重建首帧等待或插件调用
                    let response = tokio::select! {
                        biased;
                        response = response => response,
                        () = tokio::time::sleep(SSE_KEEPALIVE_INTERVAL) => return Ok(Some((SSE_KEEPALIVE_FRAME, self))),
                    };
                    let sse = response.status().is_success()
                        && response
                            .headers()
                            .get(header::CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok())
                            .and_then(|value| value.split(';').next())
                            .is_some_and(|value| {
                                value.trim().eq_ignore_ascii_case("text/event-stream")
                            });
                    let body = if sse {
                        response.into_body()
                    } else {
                        failure(response).await
                    };
                    // 正式响应的正文已经带有交付期心跳，后续只转交，不叠加定时器
                    self = Self::Streaming(body.into_data_stream());
                }
                Self::Streaming(body) => {
                    return body
                        .next()
                        .await
                        .transpose()
                        .map(|chunk| chunk.map(|chunk| (chunk, self)));
                }
            }
        }
    }
}

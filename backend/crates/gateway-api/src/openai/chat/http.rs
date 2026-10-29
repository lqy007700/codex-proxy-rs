//! Chat 入口复用现有认证、中间件、执行与计量链路

use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
};

use axum::{
    body::Bytes,
    extract::{ConnectInfo, Extension, State},
    http::{
        HeaderMap, StatusCode,
        header::{CONTENT_ENCODING, CONTENT_LENGTH},
    },
    response::Response,
};
use gateway_core::{
    engine::{execution::ClientTransport, middleware::MiddlewareError},
    operation::OperationKind,
};
use gateway_protocol::openai::chat::decode_chat_request;

use crate::openai::{
    auth::{authenticate_client, client_access_error_response},
    error::{gateway_error_response, protocol_error_response},
    middleware::{
        ExpectedBody, RequestInput, error_response, into_http_response, invoke_request,
        request_parts,
    },
    responses::{
        ProtocolError, ProtocolErrorBody, decode_request_with_headers, decompress_request_body,
        execution_response,
    },
    router::CHAT_COMPLETIONS_PATH,
};
use crate::{
    ApiState, middleware::headers::encode_headers, request_context::request_client_context,
};

use super::{
    delivery::chat_response,
    validation::{ChatValidationFacts, validate_response},
};

pub(in crate::openai) async fn chat_completions(
    State(state): State<ApiState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    ingress_id: Option<Extension<tower_http::request_id::RequestId>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let service = state.openai().clone();
    let client = match authenticate_client(&service, &headers).await {
        Ok(client) => client,
        Err(error) => return client_access_error_response(error),
    };
    let body = match decompress_request_body(
        &body,
        &headers,
        client.snapshot().responses_max_decompressed_body_bytes(),
    ) {
        Ok(body) => Bytes::copy_from_slice(&body),
        Err(error) => {
            return protocol_error_response(StatusCode::BAD_REQUEST, error.protocol_body());
        }
    };
    let initial = match decode_chat_request(&body) {
        Ok(request) => request,
        Err(error) => {
            return protocol_error_response(StatusCode::BAD_REQUEST, chat_request_error(error));
        }
    };
    let stream = initial.stream;
    let model = initial.responses["model"].as_str().map(str::to_owned);
    drop(initial);
    let execution = service.execution();
    let prepared = match execution.prepare_execution(client).await {
        Ok(prepared) => prepared,
        Err(error) => return gateway_error_response(&error),
    };
    let request_id = prepared.request_id().clone();
    let started_id = Arc::new(OnceLock::new());
    let terminal_id = Arc::clone(&started_id);
    let (client_ip, user_agent) = request_client_context(
        &headers,
        connect_info.map(|Extension(ConnectInfo(address))| address),
    );
    let mut headers = headers;
    headers.remove(CONTENT_ENCODING);
    headers.remove(CONTENT_LENGTH);
    let transport = if stream {
        ClientTransport::HttpSse
    } else {
        ClientTransport::HttpJson
    };
    let input = RequestInput {
        endpoint: CHAT_COMPLETIONS_PATH.to_owned(),
        protocol: "openai".to_owned(),
        operation: Some(OperationKind::Generate),
        transport,
        model_hint: model,
        headers: encode_headers(&headers),
        body,
    };
    let facts = ChatValidationFacts::default();
    let terminal_facts = facts.clone();
    let (admitted, keepalive) = tokio::sync::oneshot::channel();
    let response = async move {
        let response = invoke_request(
            execution,
            prepared,
            input,
            Box::new(move |prepared, request| {
                Box::pin(async move {
                    let (protocol, headers, body) = request_parts(request.clone())?;
                    if protocol != "openai" {
                        return Err(MiddlewareError::Rejected);
                    }
                    let chat = match decode_chat_request(&body) {
                        Ok(chat) => chat,
                        Err(error) => {
                            return crate::openai::middleware::buffered_response(
                                "openai",
                                protocol_error_response(
                                    StatusCode::BAD_REQUEST,
                                    chat_request_error(error),
                                ),
                            )
                            .await;
                        }
                    };
                    if chat.stream != stream {
                        return Err(MiddlewareError::Rejected);
                    }
                    let model = chat.responses["model"]
                        .as_str()
                        .ok_or(MiddlewareError::InvalidState)?
                        .to_owned();
                    let responses_body = serde_json::to_vec(&chat.responses)
                        .map_err(|_| MiddlewareError::InvalidState)?;
                    let decoded = decode_request_with_headers(
                        &responses_body,
                        &headers,
                        prepared
                            .client()
                            .snapshot()
                            .responses_max_decompressed_body_bytes(),
                    )
                    .map_err(|_| MiddlewareError::Rejected)?
                    .with_client_context(client_ip, user_agent)
                    .with_middleware_capabilities(&request)?;
                    let guard = if stream {
                        Some(
                            service
                                .try_register_connection()
                                .map_err(|_| MiddlewareError::Fault)?,
                        )
                    } else {
                        None
                    };
                    let started = service
                        .start_prepared_response(
                            prepared,
                            decoded,
                            transport,
                            CHAT_COMPLETIONS_PATH,
                        )
                        .await
                        .map_err(MiddlewareError::Gateway)?;
                    let _ = terminal_id.set(started.request_id.clone());
                    started.session.trace().headers(
                        "client.request",
                        serde_json::json!({
                            "ingressRequestId": ingress_id.as_ref().and_then(|Extension(id)| id.header_value().to_str().ok()),
                        }),
                        headers
                            .iter()
                            .map(|(name, value)| (name.as_str(), value.as_bytes())),
                    );
                    started
                        .session
                        .trace()
                        .capture("client.request.body", &body);
                    let _ = admitted.send(started.session.response_commit());
                    let response = execution_response(started, guard).await?;
                    Ok(chat_response(
                        response,
                        model,
                        stream,
                        chat.include_usage,
                        terminal_facts,
                    ))
                })
            }),
        )
        .await;
        match response {
            Ok(response) => {
                into_http_response(
                    validate_response(response, facts, stream),
                    if stream {
                        ExpectedBody::Sse
                    } else {
                        ExpectedBody::SingleJson
                    },
                )
                .await
            }
            Err(error) => error_response(error),
        }
    };
    let response = if stream {
        crate::openai::middleware::keepalive_response(response, keepalive, |response| {
            Box::pin(super::delivery::stream_failure(response))
        })
        .await
    } else {
        response.await
    };
    crate::openai::with_model_request_id(response, started_id.get().unwrap_or(&request_id))
}

fn chat_request_error(
    error: gateway_protocol::openai::chat::ChatRequestError,
) -> ProtocolErrorBody {
    ProtocolErrorBody {
        error: ProtocolError {
            kind: "invalid_request_error",
            code: "unsupported_or_invalid_parameter",
            message: error.message.to_owned(),
            param: Some(error.param),
        },
    }
}

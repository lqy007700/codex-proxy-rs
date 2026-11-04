//! OpenAI Responses 的透明 wire adapter 与 Core canonical facts 投影

mod error;
mod http;
mod keepalive;
mod request;
mod response;
mod validation;
pub mod websocket;

pub use crate::client_error::{ProtocolError, ProtocolErrorBody};
pub(crate) use crate::request_context::request_client_context;
pub use error::{RequestDecodeError, ResponseEncodeError};
pub use http::{collect_execution_response, stream_execution_response};
pub(crate) use http::{execution_response, responses};
pub(crate) use request::decompress_request_body;
pub(crate) use request::passthrough_request_headers;
pub use request::{
    ContinuationIntent, DecodedResponsesRequest, OpenAiRequestHeaders, ResponsesRequestMetadata,
    decode_request_with_headers,
};
pub use response::OpenAiResponsesEncoder;
pub(crate) use websocket::responses_websocket;
pub use websocket::{ResponseCreateFrameError, decode_response_create_with_context};

use gateway_core::event::ProviderResponseHeader;
pub(super) use gateway_protocol::openai::response_header_is_forwardable;
pub(super) fn response_connection_options(headers: &[ProviderResponseHeader]) -> Vec<String> {
    headers
        .iter()
        .filter(|header| header.name().trim().eq_ignore_ascii_case("connection"))
        .filter_map(|header| std::str::from_utf8(header.value()).ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

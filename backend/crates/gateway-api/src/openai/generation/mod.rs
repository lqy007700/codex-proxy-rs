//! 生成与计数协议共用的解码事实和 HTTP 执行边界

mod http;
pub use http::{collect_execution_response, stream_execution_response};
mod request;
mod response;
mod validation;

pub(crate) use http::{GenerationHttpRequest, execute_prepared_generation, execution_response};
pub use request::{ContinuationIntent, DecodedGenerationRequest, GenerationRequestMetadata};

use axum::http::HeaderMap;
use gateway_core::{
    error::{GatewayError, GatewayErrorKind},
    operation::OperationKind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationProtocol {
    Responses,
    Messages,
    MessageTokenCount,
    GenAi,
    GenAiTokenCount,
}

impl GenerationProtocol {
    pub(crate) fn from_request(request: &DecodedGenerationRequest) -> Self {
        let protocol = request.operation().protocol();
        if request.operation().kind() == OperationKind::CountTokens {
            if protocol == gateway_protocol::genai::GENAI_PROTOCOL {
                Self::GenAiTokenCount
            } else {
                Self::MessageTokenCount
            }
        } else if protocol == gateway_protocol::genai::GENAI_PROTOCOL {
            Self::GenAi
        } else if protocol == "anthropic" {
            Self::Messages
        } else {
            Self::Responses
        }
    }

    pub(crate) const fn wire(self) -> &'static str {
        match self {
            Self::Responses => "openai",
            Self::Messages | Self::MessageTokenCount => "anthropic",
            Self::GenAi | Self::GenAiTokenCount => gateway_protocol::genai::GENAI_PROTOCOL,
        }
    }

    pub(crate) const fn endpoint(self) -> &'static str {
        match self {
            Self::Responses => super::router::RESPONSES_PATH,
            Self::Messages => super::router::MESSAGES_PATH,
            Self::MessageTokenCount => super::router::MESSAGE_TOKENS_PATH,
            Self::GenAi | Self::GenAiTokenCount => super::genai::MODEL_ACTION_PATH,
        }
    }

    pub(crate) const fn operation(self) -> OperationKind {
        match self {
            Self::Responses | Self::Messages | Self::GenAi => OperationKind::Generate,
            Self::MessageTokenCount | Self::GenAiTokenCount => OperationKind::CountTokens,
        }
    }

    pub(crate) fn decode(
        self,
        body: &[u8],
        headers: &HeaderMap,
        maximum: usize,
        model: &str,
        stream: bool,
    ) -> Result<DecodedGenerationRequest, GatewayError> {
        match self {
            Self::Responses => super::responses::decode_request_with_headers(
                body, headers, maximum,
            )
            .map_err(|_| {
                GatewayError::new(
                    GatewayErrorKind::InvalidRequest,
                    "Request middleware produced an invalid Responses request.",
                )
            }),
            Self::Messages | Self::MessageTokenCount => {
                super::messages::decode_request(body, headers, maximum, self)
            }
            Self::GenAi | Self::GenAiTokenCount => {
                super::genai::decode_request(model, stream, self, body, headers, maximum)
            }
        }
    }
}

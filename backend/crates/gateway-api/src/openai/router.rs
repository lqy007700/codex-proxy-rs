//! OpenAI 客户端协议路由

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};

use super::{
    chat::chat_completions,
    images::{image_edits, image_generations},
    live,
    messages::{count_tokens, messages},
    models::{model_detail, models},
    responses::{responses, responses_websocket},
    search::standalone_search,
    usage,
};

use crate::ApiState;

// Core 使用操作类型，不根据这些协议路径分派业务
pub(super) use gateway_protocol::openai::RESPONSES_PATH;
pub(super) const MESSAGES_PATH: &str = "/v1/messages";
pub(super) const MESSAGE_TOKENS_PATH: &str = "/v1/messages/count_tokens";
pub(super) const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";
pub(super) const IMAGE_GENERATIONS_PATH: &str = "/v1/images/generations";
pub(super) const IMAGE_EDITS_PATH: &str = "/v1/images/edits";
pub(super) const SEARCH_PATH: &str = "/v1/alpha/search";
pub(super) const MODELS_PATH: &str = "/v1/models";
pub(super) const MODEL_DETAIL_PATH: &str = "/v1/models/{model_id}";
pub(super) const USAGE_PATH: &str = "/v1/usage";

/// 构造 OpenAI 客户端协议路由
pub(crate) fn router() -> Router<ApiState> {
    let messages_router = Router::new()
        .route(MESSAGES_PATH, post(messages))
        .route(MESSAGE_TOKENS_PATH, post(count_tokens))
        .method_not_allowed_fallback(super::messages::method_not_allowed);
    Router::new()
        .merge(messages_router)
        .merge(super::genai::router())
        .route(CHAT_COMPLETIONS_PATH, post(chat_completions))
        .route(IMAGE_GENERATIONS_PATH, post(image_generations))
        .route(IMAGE_EDITS_PATH, post(image_edits))
        .route(SEARCH_PATH, post(standalone_search))
        .route(RESPONSES_PATH, get(responses_websocket).post(responses))
        .route(MODELS_PATH, get(models))
        // 官方 OpenAI 模型详情合同使用 path ID；它不属于 Admin API 约束
        .route(MODEL_DETAIL_PATH, get(model_detail))
        .merge(usage::router())
        .merge(live::router())
        // OpenAI 数据面正文属于客户端/上游协议；代理不能用私有大小上限提前拒绝
        // 上游本可接受的未来 payload
        .layer(DefaultBodyLimit::disable())
}

#[derive(Clone, Copy)]
pub(crate) enum DataPlaneNamespace {
    V1,
    GenAi,
}

impl DataPlaneNamespace {
    pub(crate) fn for_path(path: &str) -> Option<Self> {
        if path == "/v1" || path.starts_with("/v1/") {
            Some(Self::V1)
        } else if path == "/v1beta" || path.starts_with("/v1beta/") || path.starts_with("/v1beta:")
        {
            Some(Self::GenAi)
        } else {
            None
        }
    }

    pub(crate) const fn unmatched_path(self) -> &'static str {
        match self {
            Self::V1 => "/v1/<unmatched>",
            Self::GenAi => "/v1beta/<unmatched>",
        }
    }

    pub(crate) async fn unmatched_response(self) -> axum::response::Response {
        use crate::client_error::{ProtocolError, ProtocolErrorBody};
        let response = super::error::protocol_error_response(
            axum::http::StatusCode::NOT_FOUND,
            ProtocolErrorBody {
                error: ProtocolError {
                    kind: "invalid_request_error",
                    code: "unknown_endpoint",
                    message: "The requested API endpoint does not exist.".into(),
                    param: None,
                },
            },
        );
        match self {
            Self::V1 => response,
            Self::GenAi => super::genai::project_error_response(response).await,
        }
    }
}

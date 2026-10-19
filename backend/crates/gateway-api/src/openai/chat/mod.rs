//! Chat Completions 的 HTTP 入口与 Responses 交付适配

mod delivery;
mod http;
mod validation;

pub(super) use http::chat_completions;

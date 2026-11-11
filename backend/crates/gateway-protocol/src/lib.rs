//! 网关协议编解码器
//!
//! 本 crate 只处理 wire contract，不包含 Provider 认证、路由、重试或传输策略

/// Anthropic Messages 与 Responses 的协议转换
pub mod anthropic;
/// 从请求正文提取协议能力事实
pub mod request;

/// OpenAI wire contract
pub mod openai;

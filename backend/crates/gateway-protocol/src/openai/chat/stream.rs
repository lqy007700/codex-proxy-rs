//! Responses 事件到单候选 Chat 增量的转换，执行及终结生命周期由 API 持有

use super::response::{ChatIdentity, ChatResponseError, ProjectionStreamEncoder};
use serde_json::Value;

/// 单次流的输出身份与工具索引
pub struct ChatStreamEncoder {
    projection: ProjectionStreamEncoder,
}

impl ChatStreamEncoder {
    #[must_use]
    pub fn new(model: String, created: u64, include_usage: bool) -> Self {
        Self {
            projection: ProjectionStreamEncoder::new(
                ChatIdentity {
                    id: String::new(),
                    model,
                    created,
                },
                include_usage,
            ),
        }
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.projection.is_terminal()
    }

    /// 保留原生 response 身份，拒绝终结后的额外事件，不生成虚假的成功终态
    pub fn push(&mut self, kind: &str, data: &Value) -> Result<Vec<Value>, ChatResponseError> {
        if self.is_terminal() {
            return Err(ChatResponseError);
        }
        if let Some(response) = data.get("response")
            && let Some(id) = response.get("id").and_then(Value::as_str)
        {
            self.projection
                .bind_identity(id, response.get("created_at").and_then(Value::as_u64))?;
        }
        self.projection.push_event(Some(kind), data)
    }
}

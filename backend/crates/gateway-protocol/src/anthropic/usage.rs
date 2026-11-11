//! Messages 累计用量的字段合并与 Responses 投影

use serde_json::{Value, json};

/// 原生 Messages 用量，缺失字段保留未知状态
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MessageUsage {
    /// 未命中缓存的输入量
    pub input_tokens: Option<u64>,
    /// 输出量
    pub output_tokens: Option<u64>,
    /// 缓存读取量
    pub cache_read_input_tokens: Option<u64>,
    /// 缓存写入量
    pub cache_creation_input_tokens: Option<u64>,
    /// 输出量中的推理量，仅在上游明确提供时可用
    pub thinking_tokens: Option<u64>,
}

impl MessageUsage {
    /// 从原生 usage 对象解析字段
    #[must_use]
    pub fn from_value(value: &Value) -> Self {
        Self {
            input_tokens: value.get("input_tokens").and_then(Value::as_u64),
            output_tokens: value.get("output_tokens").and_then(Value::as_u64),
            cache_read_input_tokens: value.get("cache_read_input_tokens").and_then(Value::as_u64),
            cache_creation_input_tokens: value
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64),
            thinking_tokens: value
                .pointer("/output_tokens_details/thinking_tokens")
                .and_then(Value::as_u64),
        }
    }

    /// Messages 增量上报的是累计值，仅覆盖本次实际出现的字段
    pub fn merge(&mut self, value: &Value) {
        let newer = Self::from_value(value);
        self.input_tokens = newer.input_tokens.or(self.input_tokens);
        self.output_tokens = newer.output_tokens.or(self.output_tokens);
        self.cache_read_input_tokens = newer
            .cache_read_input_tokens
            .or(self.cache_read_input_tokens);
        self.cache_creation_input_tokens = newer
            .cache_creation_input_tokens
            .or(self.cache_creation_input_tokens);
        self.thinking_tokens = newer.thinking_tokens.or(self.thinking_tokens);
    }

    /// 包含缓存读取与写入的总输入量
    #[must_use]
    pub fn total_input_tokens(self) -> Option<u64> {
        self.input_tokens.and_then(|input| {
            input
                .checked_add(self.cache_read_input_tokens.unwrap_or_default())?
                .checked_add(self.cache_creation_input_tokens.unwrap_or_default())
        })
    }

    pub(super) fn responses(self) -> Value {
        let input = self.total_input_tokens();
        let mut usage = json!({"input_tokens_details": {"cached_tokens": self.cache_read_input_tokens.unwrap_or_default(), "cache_write_tokens": self.cache_creation_input_tokens.unwrap_or_default()}});
        // 缺失计数不能补成计价事实，保留本地账本对完整用量的判定
        if let Some(input) = input {
            usage["input_tokens"] = json!(input);
        }
        if let Some(output) = self.output_tokens {
            usage["output_tokens"] = json!(output);
            if let Some(total) = input.and_then(|input| input.checked_add(output)) {
                usage["total_tokens"] = json!(total);
            }
        }
        if let Some(thinking) = self.thinking_tokens {
            usage["output_tokens_details"] = json!({"reasoning_tokens": thinking});
        }
        usage
    }
}

pub(super) fn responses_usage(value: &Value) -> Value {
    let input = value.get("input_tokens").and_then(Value::as_u64);
    let cached = value
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let written = value
        .pointer("/input_tokens_details/cache_write_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let mut usage =
        json!({"cache_read_input_tokens": cached, "cache_creation_input_tokens": written});
    if let Some(input) = input
        .and_then(|input| input.checked_sub(cached))
        .and_then(|input| input.checked_sub(written))
    {
        usage["input_tokens"] = json!(input);
    }
    if let Some(output) = value.get("output_tokens").and_then(Value::as_u64) {
        usage["output_tokens"] = json!(output);
    }
    if let Some(thinking) = value
        .pointer("/output_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64)
    {
        usage["output_tokens_details"] = json!({"thinking_tokens": thinking});
    }
    usage
}

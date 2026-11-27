//! 按请求冻结的 OpenAI 路由头与连接池覆盖配置

use serde::{Deserialize, Deserializer, Serialize};

/// 缺省字段沿用部署配置，显式 null 省略对应请求头
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenAiRequestSettings {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub residency: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub account_routing_override: Option<Option<String>>,
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl OpenAiRequestSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        for (field, value) in [
            ("openaiRequestSettings.residency", self.residency.as_ref()),
            (
                "openaiRequestSettings.accountRoutingOverride",
                self.account_routing_override.as_ref(),
            ),
        ] {
            if let Some(Some(value)) = value
                && !http::HeaderValue::from_str(value).is_ok_and(|header| header.to_str().is_ok())
            {
                return Err(field);
            }
        }
        Ok(())
    }
}

/// 每个缺省参数沿用现有连接池的启动配置
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenAiWebSocketPoolSettings {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub max_age_ms: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub max_connecting: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub fast_path_budget_ms: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub idle_timeout_ms: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub stream_idle_timeout_ms: Option<u64>,
}

impl OpenAiWebSocketPoolSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.max_age_ms == Some(0) {
            return Err("openaiWebsocketPool.maxAgeMs");
        }
        if self.max_connecting == Some(0) {
            return Err("openaiWebsocketPool.maxConnecting");
        }
        if self
            .fast_path_budget_ms
            .is_some_and(|ms| !(1..=15_000).contains(&ms))
        {
            return Err("openaiWebsocketPool.fastPathBudgetMs");
        }
        if self.idle_timeout_ms == Some(0) {
            return Err("openaiWebsocketPool.idleTimeoutMs");
        }
        Ok(())
    }
}

//! 智谱标准 API 凭据校验与可公开的连接设置

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use zeroize::Zeroize;

pub(crate) const DEFAULT_BASE_URL: &str = "https://open.bigmodel.cn/api/v1";
pub(crate) const DEFAULT_MODEL: &str = "glm-5.3";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Credential {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub api_key: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "http_transport")]
    pub transport: String,
    #[serde(default = "default_models")]
    pub models: Vec<String>,
}

impl Drop for Credential {
    fn drop(&mut self) {
        self.api_key.zeroize();
    }
}

fn schema_version() -> u32 {
    1
}
fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_owned()
}
fn http_transport() -> String {
    "http".to_owned()
}
fn default_models() -> Vec<String> {
    vec![DEFAULT_MODEL.to_owned()]
}

impl Credential {
    pub fn parse(value: &Map<String, Value>) -> Result<Self, ()> {
        let credential: Self =
            serde_json::from_value(Value::Object(value.clone())).map_err(|_| ())?;
        credential.validate()?;
        Ok(credential)
    }

    pub fn validate(&self) -> Result<(), ()> {
        let url = url::Url::parse(&self.base_url).map_err(|_| ())?;
        if self.schema_version != 1
            || self.transport != "http"
            || self.api_key.is_empty()
            || self.api_key.len() > 16 * 1024
            || !self.api_key.bytes().all(|b| b.is_ascii_graphic())
            || self.base_url.len() > 2048
            || self.base_url.chars().any(char::is_control)
            || !matches!(url.scheme(), "https" | "http")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || self.models.is_empty()
            || self.models.len() > 128
            || self.models.iter().collect::<BTreeSet<_>>().len() != self.models.len()
            || self.models.iter().any(|model| {
                !model.starts_with("glm-")
                    || gateway_core::routing::UpstreamModelId::new(model.clone()).is_err()
            })
        {
            return Err(());
        }
        // 编程套餐的 Chat 地址不能当成标准 Responses 前缀，避免错误端点和计费模式混用
        if url.path().contains("/coding/")
            || url.path().contains("/paas/")
            || url.path().contains("/anthropic")
        {
            return Err(());
        }
        Ok(())
    }

    pub fn document(&self) -> Result<Map<String, Value>, ()> {
        serde_json::to_value(self)
            .map_err(|_| ())?
            .as_object()
            .cloned()
            .ok_or(())
    }

    pub fn configuration(&self) -> Map<String, Value> {
        serde_json::json!({"base_url": self.base_url, "transport": "http", "models": self.models})
            .as_object()
            .cloned()
            .unwrap_or_default()
    }

    pub fn responses_url(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }
}

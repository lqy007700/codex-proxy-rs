//! 可信代理身份配置及独立于认证 Key 的会话所有权

use super::ClientApiKeyId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum CallerIdentityPolicy {
    #[default]
    Direct,
    NewApi {
        #[serde(rename = "sourceId")]
        source_id: String,
        #[serde(default, rename = "allowUnidentified")]
        allow_unidentified: bool,
    },
}

impl CallerIdentityPolicy {
    pub fn validate(&self) -> Result<(), CallerIdentityError> {
        if let Self::NewApi { source_id, .. } = self {
            uuid::Uuid::parse_str(source_id).map_err(|_| CallerIdentityError)?;
        }
        Ok(())
    }

    /// 只采信入口冻结且完整合法的双 ID，识别失败沿用 Key 的无身份处理方式
    pub fn resolve(
        &self,
        key: &ClientApiKeyId,
        headers: &ForwardedCallerHeaders,
    ) -> Result<CallerIdentity, CallerIdentityError> {
        match self {
            Self::Direct => Ok(CallerIdentity::Direct {
                key_id: key.as_str().to_owned(),
            }),
            Self::NewApi {
                source_id,
                allow_unidentified,
            } => {
                self.validate()?;
                match (headers.users.as_slice(), headers.tokens.as_slice()) {
                    ([user], [token]) if valid_id(user) && valid_id(token) => {
                        Ok(CallerIdentity::NewApi {
                            source_id: source_id.clone(),
                            user_id: user.clone(),
                            token_id: token.clone(),
                        })
                    }
                    // 身份透传配置不能阻断原有 Key 请求，也不能用部分 ID 猜测用户归属
                    _ if *allow_unidentified => Ok(CallerIdentity::Unidentified {
                        request_id: uuid::Uuid::new_v4().to_string(),
                    }),
                    _ => Ok(CallerIdentity::direct(key)),
                }
            }
        }
    }
}

// 使用规范十进制字符串，避免 01 与 1 成为两份身份，也不经过浮点数或 JS 整数
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && !value.starts_with('0')
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok()
}

#[derive(Debug, Clone, Default)]
pub struct ForwardedCallerHeaders {
    pub users: Vec<String>,
    pub tokens: Vec<String>,
    pub token_names: Vec<Vec<u8>>,
}

/// 名称只作展示，非法输入等同未提供，不推断编码或身份
#[must_use]
pub fn normalize_caller_token_name(value: &[u8]) -> Option<&str> {
    if value.len() > 256 {
        return None;
    }
    let value = std::str::from_utf8(value).ok()?;
    if value.chars().any(char::is_control) {
        return None;
    }
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

impl ForwardedCallerHeaders {
    #[must_use]
    pub fn token_name(&self, caller: &CallerIdentity) -> Option<String> {
        if !matches!(caller, CallerIdentity::NewApi { .. }) {
            return None;
        }
        let [value] = self.token_names.as_slice() else {
            return None;
        };
        normalize_caller_token_name(value).map(str::to_owned)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum CallerIdentity {
    Direct {
        #[serde(rename = "keyId")]
        key_id: String,
    },
    NewApi {
        #[serde(rename = "sourceId")]
        source_id: String,
        #[serde(rename = "userId")]
        user_id: String,
        #[serde(rename = "tokenId")]
        token_id: String,
    },
    Unidentified {
        #[serde(rename = "requestId")]
        request_id: String,
    },
}

impl CallerIdentity {
    #[must_use]
    pub fn direct(key: &ClientApiKeyId) -> Self {
        Self::Direct {
            key_id: key.as_str().to_owned(),
        }
    }

    /// 直连保持原有字节；代理使用带域隔离及长度分隔的所有权，不充当认证 Key
    #[must_use]
    pub fn session_namespace(&self) -> String {
        match self {
            Self::Direct { key_id } => key_id.clone(),
            Self::NewApi {
                source_id,
                user_id,
                token_id,
            } => format!(
                "\0cpr-proxy:{}:{source_id}:{}:{user_id}:{}:{token_id}",
                source_id.len(),
                user_id.len(),
                token_id.len()
            ),
            Self::Unidentified { request_id } => format!("\0cpr-unidentified:{request_id}"),
        }
    }

    #[must_use]
    pub const fn is_unidentified(&self) -> bool {
        matches!(self, Self::Unidentified { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("trusted caller identity is missing or invalid")]
pub struct CallerIdentityError;

//! Provider 会话亲和、绑定和排除事实

use super::{ProviderStoreError, ProviderStoreErrorKind};
use crate::{account::ProviderAccountId, identity::ProviderKind};
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fmt, time::Duration};

/// Provider 从原始会话锚点派生的不可逆亲和键
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderSessionAffinityKey(String);

impl ProviderSessionAffinityKey {
    /// 可信 New API Token 的稳定主绑定引用，供数据面与管理配置共用
    #[must_use]
    pub fn newapi_token_primary(caller: &crate::policy::CallerIdentity) -> Option<Self> {
        if !matches!(caller, crate::policy::CallerIdentity::NewApi { .. }) {
            return None;
        }
        let mut hasher = Sha256::new();
        hasher.update(b"codex-session-affinity-v1\0newapi-token-primary\0");
        hasher.update(caller.session_namespace().as_bytes());
        let hash = hasher.finalize();
        let mut value = String::with_capacity(hash.len() * 2);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in hash {
            value.push(char::from(HEX[usize::from(byte >> 4)]));
            value.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Some(Self(value))
    }

    pub fn try_new(value: impl Into<String>) -> Result<Self, ProviderStoreError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            return Err(ProviderStoreError::new(
                ProviderStoreErrorKind::InvalidData,
                "validate provider session affinity key",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose_to_store(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderSessionAffinityKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderSessionAffinityKey([OPAQUE])")
    }
}

/// 会话绑定快照；版本区分同一账号的不同认领，防止过期和 A → B → A 后的旧写入
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum SessionBindingDirective {
    Clear,
    Reassign,
    Target {
        #[serde(rename = "accountId")]
        account_id: String,
    },
}

/// 管理索引中的绑定层级，缺省值兼容已有会话观测
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderAffinityScope {
    #[default]
    Session,
    Token,
}

/// 一次发送前观测，用于管理索引；不保存原始会话 ID 或凭据
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSessionObservation {
    // 展示名称独立存储，不能进入旧版严格解析的索引或操作快照 JSON
    #[serde(skip)]
    pub token_name: Option<String>,
    #[serde(default)]
    pub binding_scope: ProviderAffinityScope,
    pub caller: crate::policy::CallerIdentity,
    pub client_key_id: String,
    pub request_id: String,
    pub actual_account_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSessionBinding {
    account_id: ProviderAccountId,
    revision: String,
    directive: Option<SessionBindingDirective>,
}

impl ProviderSessionBinding {
    #[must_use]
    pub fn with_directive(mut self, directive: Option<SessionBindingDirective>) -> Self {
        self.directive = directive;
        self
    }

    #[must_use]
    pub const fn directive(&self) -> Option<&SessionBindingDirective> {
        self.directive.as_ref()
    }

    pub fn new(
        account_id: ProviderAccountId,
        revision: String,
    ) -> Result<Self, ProviderStoreError> {
        if revision.len() != 32 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ProviderStoreError::new(
                ProviderStoreErrorKind::InvalidData,
                "decode provider session binding revision",
            ));
        }
        Ok(Self {
            account_id,
            revision,
            directive: None,
        })
    }

    #[must_use]
    pub const fn account_id(&self) -> &ProviderAccountId {
        &self.account_id
    }

    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }
}

/// 已观测请求关联及其账号迁移权限，由 Provider 解释协议后写入
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSessionAlias {
    pub session_key: ProviderSessionAffinityKey,
    pub root_session_key: Option<ProviderSessionAffinityKey>,
    pub follow_only: bool,
}

/// 发送前需要核对完整版本的绑定；首次认领的 expected 为空
#[derive(Clone, Copy)]
pub struct ProviderSessionClaim<'a> {
    pub key: &'a ProviderSessionAffinityKey,
    pub expected: Option<&'a ProviderSessionBinding>,
    pub account: &'a ProviderAccountId,
    pub ttl: Duration,
}

/// 新请求的轮询位置与全部首绑定共同提交，不能留下部分认领
#[derive(Clone, Copy)]
pub struct ProviderRoundRobinAdmission<'a> {
    pub client_key: &'a crate::policy::ClientApiKeyId,
    pub cursor: u64,
    pub claims: &'a [ProviderSessionClaim<'a>],
}

/// 账号选择后的原子认领请求，按是否推进轮询游标分派
#[derive(Clone, Copy)]
pub enum ProviderAccountAdmission<'a> {
    Session(ProviderSessionClaim<'a>),
    RoundRobin {
        client_key: &'a crate::policy::ClientApiKeyId,
        cursor: u64,
        session: Option<ProviderSessionClaim<'a>>,
    },
}

impl<'a> ProviderAccountAdmission<'a> {
    #[must_use]
    pub const fn session(self) -> Option<ProviderSessionClaim<'a>> {
        match self {
            Self::Session(claim) => Some(claim),
            Self::RoundRobin { session, .. } => session,
        }
    }
}

/// Token 次数和本次逻辑请求的准入结果，空计数兼容旧绑定
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderTokenRequestState {
    pub count: Option<u32>,
    pub latched: bool,
    pub switched: Option<bool>,
}

/// 同时认领 Token、会话、计数和可选轮询位置
pub struct ProviderTokenRequestAdmission<'a> {
    pub token_key: &'a ProviderSessionAffinityKey,
    pub request_id: &'a str,
    pub expected: ProviderTokenRequestState,
    pub next_count: u32,
    pub latched: bool,
    pub switched: bool,
    pub preserve_token_directive: bool,
    pub ttl: Duration,
    pub claims: &'a [ProviderSessionClaim<'a>],
    pub cursor: Option<(&'a crate::policy::ClientApiKeyId, u64)>,
}

/// 客户端作用域内的会话账号绑定，原始会话身份由 Provider 哈希后传入
pub trait ProviderSessionAffinityPort: Send + Sync {
    fn observe<'a>(
        &'a self,
        _provider: &'a ProviderKind,
        _key: &'a ProviderSessionAffinityKey,
        _observation: ProviderSessionObservation,
        _ttl: Duration,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async { Ok(()) })
    }

    fn load<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
    ) -> BoxFuture<'a, Result<Option<ProviderSessionBinding>, ProviderStoreError>>;

    /// 在发送前原子认领、续期或迁移；None 表示快照已变化，调用方必须释放租约重新选择
    /// expected 为 None 只允许首次认领，已有快照必须连同版本匹配
    fn compare_and_bind<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        expected: Option<&'a ProviderSessionBinding>,
        account_id: &'a ProviderAccountId,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<Option<ProviderSessionBinding>, ProviderStoreError>>;

    /// 读取下一次成功分配的位置，不因亲和命中、等待或失败读取而推进
    fn round_robin_cursor<'a>(
        &'a self,
        provider: &'a ProviderKind,
        client_key: &'a crate::policy::ClientApiKeyId,
    ) -> BoxFuture<'a, Result<u64, ProviderStoreError>>;

    /// 同时比较轮询位置与全部绑定版本；冲突不写入、不续期
    fn admit_round_robin<'a>(
        &'a self,
        provider: &'a ProviderKind,
        admission: ProviderRoundRobinAdmission<'a>,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>>;

    /// 统一会话认领与轮询提交入口，冲突返回 false 供调用方释放租约重选
    fn admit_account<'a>(
        &'a self,
        provider: &'a ProviderKind,
        admission: ProviderAccountAdmission<'a>,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            match admission {
                ProviderAccountAdmission::Session(claim) => Ok(self
                    .compare_and_bind(
                        provider,
                        claim.key,
                        claim.expected,
                        claim.account,
                        claim.ttl,
                    )
                    .await?
                    .is_some()),
                ProviderAccountAdmission::RoundRobin {
                    client_key,
                    cursor,
                    session,
                } => {
                    let claims: Vec<_> = session.into_iter().collect();
                    self.admit_round_robin(
                        provider,
                        ProviderRoundRobinAdmission {
                            client_key,
                            cursor,
                            claims: &claims,
                        },
                    )
                    .await
                }
            }
        })
    }

    /// 不支持计数的存储必须关闭此功能
    fn load_token_request_state<'a>(
        &'a self,
        _provider: &'a ProviderKind,
        _token_key: &'a ProviderSessionAffinityKey,
        _request_id: &'a str,
    ) -> BoxFuture<'a, Result<ProviderTokenRequestState, ProviderStoreError>> {
        Box::pin(async {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "load token request state",
            ))
        })
    }

    /// 只有所有快照一致时才共同写入绑定、计数、请求标记和游标
    fn admit_token_request<'a>(
        &'a self,
        _provider: &'a ProviderKind,
        _admission: ProviderTokenRequestAdmission<'a>,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "admit token request",
            ))
        })
    }

    /// 显式观测到的请求关联只指向会话键，不缓存账号，迁移后仍读取当前绑定
    fn load_alias<'a>(
        &'a self,
        provider: &'a ProviderKind,
        alias: &'a ProviderSessionAffinityKey,
    ) -> BoxFuture<'a, Result<Option<ProviderSessionAlias>, ProviderStoreError>>;

    /// 只允许首次关联或同目标续期，冲突时禁止把同一轮次改指其他会话
    fn bind_alias<'a>(
        &'a self,
        provider: &'a ProviderKind,
        alias: &'a ProviderSessionAffinityKey,
        session: &'a ProviderSessionAlias,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>>;
}

/// Provider 会话内已失败账号的可丢失排除集
///
/// Provider 自行派生会话键并决定何时写入或清理；Core 只承载调度所需的账号 ID
/// 与 compare-and-swap revision，不解释任一 Provider 协议字段
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSessionExclusions {
    excluded_accounts: BTreeSet<ProviderAccountId>,
    revision: String,
}

impl ProviderSessionExclusions {
    #[must_use]
    pub const fn new(excluded_accounts: BTreeSet<ProviderAccountId>, revision: String) -> Self {
        Self {
            excluded_accounts,
            revision,
        }
    }

    #[must_use]
    pub const fn excluded_accounts(&self) -> &BTreeSet<ProviderAccountId> {
        &self.excluded_accounts
    }

    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }
}

/// 可丢失的 Provider 会话级账号排除状态
///
/// 该端口不接收协议正文；Provider 只能以不可逆会话键、账号 ID 和固定 TTL 操作
pub trait ProviderSessionExclusionPort: Send + Sync {
    fn load<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
    ) -> BoxFuture<'a, Result<Option<ProviderSessionExclusions>, ProviderStoreError>>;

    fn record_failure<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        account_id: &'a ProviderAccountId,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<ProviderSessionExclusions, ProviderStoreError>>;

    fn clear<'a>(
        &'a self,
        provider_kind: &'a ProviderKind,
        key: &'a ProviderSessionAffinityKey,
        expected_revision: &'a str,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>>;
}

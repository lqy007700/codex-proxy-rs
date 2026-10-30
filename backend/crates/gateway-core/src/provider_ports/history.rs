//! Provider 历史排除指纹的可丢失缓存合同，不接收聊天正文或密文

use std::{collections::BTreeSet, time::Duration};

use futures::future::BoxFuture;

use super::{ProviderSessionAffinityKey, ProviderStoreError};

pub const MAX_HISTORY_EXCLUSIONS: usize = 4096;

/// scope 与指纹由 Provider 单向派生，Store 只负责有界集合与原子过期
pub trait ProviderHistoryExclusionPort: Send + Sync {
    /// 只查询本次输入中的候选项，命中后续期，不为未发生恢复的请求创建记录
    fn matching<'a>(
        &'a self,
        scope: &'a ProviderSessionAffinityKey,
        candidates: &'a BTreeSet<String>,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<BTreeSet<String>, ProviderStoreError>>;

    /// 原子合并整批；超过容量时整批拒绝，返回是否已接受
    fn merge<'a>(
        &'a self,
        scope: &'a ProviderSessionAffinityKey,
        fingerprints: &'a BTreeSet<String>,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>>;
}

/// 未装配缓存的宿主仍可进行请求内恢复，不宣称已持久化排除集
pub struct NoopProviderHistoryExclusions;

impl ProviderHistoryExclusionPort for NoopProviderHistoryExclusions {
    fn matching<'a>(
        &'a self,
        _: &'a ProviderSessionAffinityKey,
        _: &'a BTreeSet<String>,
        _: Duration,
    ) -> BoxFuture<'a, Result<BTreeSet<String>, ProviderStoreError>> {
        Box::pin(async { Ok(BTreeSet::new()) })
    }

    fn merge<'a>(
        &'a self,
        _: &'a ProviderSessionAffinityKey,
        _: &'a BTreeSet<String>,
        _: Duration,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async { Ok(false) })
    }
}

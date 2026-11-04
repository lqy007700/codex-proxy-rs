//! 亲和操作的精确预览、重复执行与逐项审计协调

use super::publish_committed;
use crate::{
    model::{AdminError, MutationContext, affinity::*},
    ports::affinity::AffinityStorePorts,
};
use chrono::{Duration, Utc};
use gateway_core::{
    account::ProviderAccountId,
    provider_ports::{ProviderSessionAffinityKey, SessionBindingDirective},
    routing::ProviderKind,
    runtime::SnapshotControl,
};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone)]
pub struct AffinityService {
    stores: AffinityStorePorts,
    snapshot: Arc<dyn SnapshotControl>,
}

impl AffinityService {
    pub fn new(stores: AffinityStorePorts, snapshot: Arc<dyn SnapshotControl>) -> Self {
        Self { stores, snapshot }
    }

    pub async fn list(&self, query: &AffinityQuery) -> Result<AffinityPage, AdminError> {
        for value in [
            &query.cursor,
            &query.mode,
            &query.source_id,
            &query.client_key_id,
            &query.user_id,
            &query.token_id,
            &query.provider,
            &query.account_id,
            &query.reference,
            &query.request_id,
            &query.status,
        ]
        .into_iter()
        .flatten()
        {
            if value.len() > 256 || value.chars().any(char::is_control) {
                return Err(AdminError::invalid("亲和筛选参数无效"));
            }
        }
        if query
            .mode
            .as_deref()
            .is_some_and(|value| !["direct", "newApi"].contains(&value))
            || query
                .status
                .as_deref()
                .is_some_and(|value| !["bound", "pending", "primary", "spillover"].contains(&value))
        {
            return Err(AdminError::invalid("亲和筛选类型无效"));
        }
        if let Some(provider) = &query.provider {
            ProviderKind::new(provider.clone()).map_err(|_| AdminError::invalid("渠道无效"))?;
        }
        if let Some(reference) = &query.reference {
            ProviderSessionAffinityKey::try_new(reference.clone())
                .map_err(|_| AdminError::invalid("绑定引用无效"))?;
        }
        self.stores.bindings.list(query).await
    }

    pub async fn sources(&self) -> Result<Vec<IdentitySource>, AdminError> {
        self.stores.journal.sources().await
    }

    pub async fn save_source(
        &self,
        context: &MutationContext,
        mut source: IdentitySource,
    ) -> Result<(), AdminError> {
        uuid::Uuid::parse_str(&source.id).map_err(|_| AdminError::invalid("来源 ID 无效"))?;
        source.name = source.name.trim().to_owned();
        if source.name.is_empty()
            || source.name.len() > 128
            || source.name.chars().any(char::is_control)
        {
            return Err(AdminError::invalid("来源名称无效"));
        }
        let revision = self.stores.journal.save_source(context, source).await?;
        publish_committed(self.snapshot.as_ref(), revision).await?;
        Ok(())
    }

    pub async fn preview(
        &self,
        context: &MutationContext,
        command: AffinityPreviewRequest,
    ) -> Result<AffinityOperation, AdminError> {
        uuid::Uuid::parse_str(&command.operation_id)
            .map_err(|_| AdminError::invalid("操作 ID 无效"))?;
        if command.selection.is_empty() || command.selection.len() > 200 {
            return Err(AdminError::invalid("每次请选择 1 至 200 个绑定"));
        }
        if let SessionBindingDirective::Target { account_id } = &command.directive {
            ProviderAccountId::new(account_id.clone())
                .map_err(|_| AdminError::invalid("目标账号无效"))?;
        }
        let mut unique = BTreeSet::new();
        let mut items = Vec::with_capacity(command.selection.len());
        for selection in command.selection {
            ProviderKind::new(selection.provider.clone())
                .map_err(|_| AdminError::invalid("渠道无效"))?;
            ProviderSessionAffinityKey::try_new(selection.reference.clone())
                .map_err(|_| AdminError::invalid("绑定引用无效"))?;
            if selection.revision.len() != 32
                || !selection
                    .revision
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(AdminError::invalid("绑定版本无效"));
            }
            if !unique.insert(format!("{}:{}", selection.provider, selection.reference)) {
                return Err(AdminError::invalid("绑定不能重复选择"));
            }
            let session = self
                .stores
                .bindings
                .session(&selection.provider, &selection.reference)
                .await?
                .ok_or_else(|| AdminError::conflict("绑定已过期，请刷新后重新选择"))?;
            if session.revision != selection.revision {
                return Err(AdminError::conflict("绑定已变化，请刷新后重新预览"));
            }
            let rejection = self.rejection(&session, &command.directive).await?;
            items.push(AffinityPlanItem {
                session,
                next_revision: uuid::Uuid::new_v4().simple().to_string(),
                rejection,
            });
        }
        let now = Utc::now();
        self.stores
            .journal
            .create_plan(
                context,
                AffinityPlan {
                    operation_id: command.operation_id,
                    directive: command.directive,
                    items,
                    created_at: now,
                    expires_at: now + Duration::minutes(15),
                },
            )
            .await
    }

    async fn rejection(
        &self,
        session: &AffinitySession,
        directive: &SessionBindingDirective,
    ) -> Result<Option<String>, AdminError> {
        // New API 仅管理 Token 主偏好，不能通过逐会话操作制造与主绑定冲突的迁移
        if matches!(
            session.caller(),
            gateway_core::policy::CallerIdentity::NewApi { .. }
        ) && session.observation.binding_scope
            == gateway_core::provider_ports::ProviderAffinityScope::Session
        {
            return Ok(Some(
                "New API 会话仅查看归属，请操作对应 Key 的主绑定".to_owned(),
            ));
        }
        if matches!(directive, SessionBindingDirective::Clear) {
            return Ok(None);
        }
        let target = match directive {
            SessionBindingDirective::Target { account_id } => Some(account_id.as_str()),
            _ => None,
        };
        if self.stores.journal.eligible_target(session, target).await? {
            Ok(None)
        } else {
            Ok(Some("目标账号不在当前 Key 的可调度范围内".to_owned()))
        }
    }

    pub async fn apply(
        &self,
        context: &MutationContext,
        id: &str,
    ) -> Result<AffinityOperation, AdminError> {
        uuid::Uuid::parse_str(id).map_err(|_| AdminError::invalid("操作 ID 无效"))?;
        let mut operation = self
            .stores
            .journal
            .operation(id)
            .await?
            .ok_or_else(|| AdminError::not_found("亲和操作不存在"))?;
        for item in &operation.plan.items {
            if operation.results.get(&item.key()).is_some_and(|result| {
                ["applied", "conflict", "rejected"].contains(&result.status.as_str())
                    && result.audit_recorded
            }) {
                continue;
            }
            // 先恢复 Redis 的原子回执，避免日志故障后因权限变化把已生效结果改写为拒绝
            let mutation = async {
                if let Some(receipt) = self.stores.bindings.receipt(&operation.plan, item).await? {
                    return Ok(receipt);
                }
                // 过期判定仍经过 Redis 原子回执，不能与刚完成的并发操作争写终态
                let rejection = if operation.plan.expires_at <= Utc::now() {
                    None
                } else {
                    match &item.rejection {
                        Some(reason) => Some(reason.clone()),
                        None => {
                            self.rejection(&item.session, &operation.plan.directive)
                                .await?
                        }
                    }
                };
                self.stores
                    .bindings
                    .apply(&operation.plan, item, rejection.as_deref())
                    .await
            }
            .await;
            let result = mutation.unwrap_or_else(|_| AffinityItemResult {
                provider: item.session.provider.clone(),
                reference: item.session.reference.clone(),
                status: "failed".to_owned(),
                message: "亲和存储不可用，可重试此操作".to_owned(),
                audit_recorded: false,
            });
            let result = self
                .stores
                .journal
                .record_result(context, id, &result)
                .await
                .unwrap_or(result);
            operation.results.insert(result.key(), result);
        }
        Ok(operation)
    }

    pub async fn history(&self) -> Result<Vec<AffinityOperation>, AdminError> {
        self.stores.journal.history().await
    }
}

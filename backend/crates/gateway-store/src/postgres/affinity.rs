//! 可信身份来源配置与亲和操作日志的 PostgreSQL 适配

use async_trait::async_trait;
use gateway_admin::{
    model::{AdminError, MutationContext, Revision, affinity::*, audit::MutationAuditOperation},
    ports::affinity::AffinityJournalStore,
};
use gateway_core::policy::CallerIdentity;
use sqlx::{PgPool, Row, types::Json};

#[derive(Clone)]
pub struct PgAffinityJournal {
    pool: PgPool,
}

impl PgAffinityJournal {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn unavailable(error: impl std::fmt::Display) -> AdminError {
    tracing::warn!(%error, "Affinity journal operation failed");
    AdminError::new(
        gateway_admin::model::AdminErrorKind::Unavailable,
        "亲和操作日志暂不可用",
    )
}

#[async_trait]
impl AffinityJournalStore for PgAffinityJournal {
    async fn newapi_affinity(
        &self,
        identity: Option<&gateway_core::policy::NewApiTokenIdentity>,
    ) -> Result<NewApiAffinityView, AdminError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("set transaction isolation level repeatable read read only")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let row = sqlx::query("select enabled, request_limit, target_account from newapi_token_affinity_default where singleton")
            .fetch_one(&mut *tx).await.map_err(unavailable)?;
        let default = decode_newapi_config(&row)?;
        let override_config = if let Some(identity) = identity {
            sqlx::query("select enabled, request_limit, target_account from newapi_token_affinity_overrides where source_id=$1 and user_id=$2 and token_id=$3")
                .bind(&identity.source_id).bind(&identity.user_id).bind(&identity.token_id)
                .fetch_optional(&mut *tx).await.map_err(unavailable)?
                .as_ref().map(decode_newapi_config).transpose()?
        } else {
            None
        };
        tx.commit().await.map_err(unavailable)?;
        Ok(NewApiAffinityView {
            default,
            override_config,
        })
    }

    async fn save_newapi_affinity(
        &self,
        context: &MutationContext,
        update: &NewApiAffinityUpdate,
    ) -> Result<Revision, AdminError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        if let Some(identity) = &update.identity {
            let exists: bool = sqlx::query_scalar(
                "select exists(select 1 from caller_identity_sources where id=$1)",
            )
            .bind(&identity.source_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
            if !exists {
                return Err(AdminError::invalid("身份来源不存在"));
            }
        }
        if let Some(target) = update
            .config
            .as_ref()
            .and_then(|config| config.target_account.as_deref())
        {
            let eligible: bool = if let Some(identity) = &update.identity {
                sqlx::query_scalar("select exists(select 1 from client_api_keys k join caller_identity_sources s on s.id=k.identity_source_id and s.enabled cross join provider_accounts a where k.id=$1 and k.enabled and k.identity_policy_json->>'mode'='newApi' and s.id=$2 and a.id=$3 and a.provider_kind='openai' and a.enabled and a.scheduling_role='regular' and a.credential_state not in ('invalid','banned') and (not exists(select 1 from client_api_key_groups kg where kg.client_api_key_id=k.id) or exists(select 1 from client_api_key_groups kg join account_groups g on g.id=kg.account_group_id and g.enabled join account_group_accounts ga on ga.account_group_id=g.id where kg.client_api_key_id=k.id and ga.provider_account_id=a.id)))")
                    .bind(update.client_key_id.as_deref()).bind(&identity.source_id).bind(target).fetch_one(&mut *tx).await.map_err(unavailable)?
            } else {
                sqlx::query_scalar("select exists(select 1 from provider_accounts where id=$1 and provider_kind='openai' and enabled and scheduling_role='regular' and credential_state not in ('invalid','banned'))")
                    .bind(target).fetch_one(&mut *tx).await.map_err(unavailable)?
            };
            if !eligible {
                return Err(AdminError::invalid("目标账号不在当前可调度范围内"));
            }
        }
        match (&update.identity, &update.config) {
            (None, Some(config)) => {
                sqlx::query("update newapi_token_affinity_default set enabled=$1, request_limit=$2, target_account=$3 where singleton")
                    .bind(config.enabled).bind(i64::from(config.request_limit)).bind(&config.target_account).execute(&mut *tx).await.map_err(unavailable)?;
            }
            (Some(identity), Some(config)) => {
                sqlx::query("insert into newapi_token_affinity_overrides(source_id,user_id,token_id,enabled,request_limit,target_account) values($1,$2,$3,$4,$5,$6) on conflict(source_id,user_id,token_id) do update set enabled=excluded.enabled,request_limit=excluded.request_limit,target_account=excluded.target_account")
                    .bind(&identity.source_id).bind(&identity.user_id).bind(&identity.token_id).bind(config.enabled).bind(i64::from(config.request_limit)).bind(&config.target_account).execute(&mut *tx).await.map_err(unavailable)?;
            }
            (Some(identity), None) => {
                sqlx::query("delete from newapi_token_affinity_overrides where source_id=$1 and user_id=$2 and token_id=$3")
                    .bind(&identity.source_id).bind(&identity.user_id).bind(&identity.token_id).execute(&mut *tx).await.map_err(unavailable)?;
            }
            (None, None) => return Err(AdminError::invalid("全局规则不能为空")),
        }
        let revision = super::bump_config_revision_in_transaction(&mut tx)
            .await
            .map_err(unavailable)?;
        let entity = update.identity.as_ref().map_or_else(
            || "default".to_owned(),
            |identity| {
                format!(
                    "{}:{}:{}",
                    identity.source_id, identity.user_id, identity.token_id
                )
            },
        );
        let event = super::admin_security_audit::mutation_audit(
            context,
            MutationAuditOperation::NewApiTokenAffinitySave,
            &entity,
            vec!["config".to_owned()],
        );
        super::admin_security_audit::append_admin_audit_event_in_transaction(
            &mut tx,
            event,
            Some(revision),
        )
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        crate::admin_revision(revision).map_err(unavailable)
    }
    async fn sources(&self) -> Result<Vec<IdentitySource>, AdminError> {
        sqlx::query("select id, name, enabled from caller_identity_sources order by name, id")
            .fetch_all(&self.pool)
            .await
            .map_err(unavailable)?
            .into_iter()
            .map(|row| {
                Ok(IdentitySource {
                    id: row.try_get("id").map_err(unavailable)?,
                    name: row.try_get("name").map_err(unavailable)?,
                    enabled: row.try_get("enabled").map_err(unavailable)?,
                })
            })
            .collect()
    }

    async fn save_source(
        &self,
        context: &MutationContext,
        source: IdentitySource,
    ) -> Result<Revision, AdminError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("insert into caller_identity_sources(id,name,enabled) values($1,$2,$3) on conflict(id) do update set name=excluded.name,enabled=excluded.enabled,updated_at=now()")
            .bind(&source.id).bind(&source.name).bind(source.enabled).execute(&mut *tx).await.map_err(unavailable)?;
        let revision = super::bump_config_revision_in_transaction(&mut tx)
            .await
            .map_err(unavailable)?;
        let event = super::admin_security_audit::mutation_audit(
            context,
            MutationAuditOperation::CallerIdentitySourceSave,
            &source.id,
            vec!["name".to_owned(), "enabled".to_owned()],
        );
        super::admin_security_audit::append_admin_audit_event_in_transaction(
            &mut tx,
            event,
            Some(revision),
        )
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        crate::admin_revision(revision).map_err(unavailable)
    }

    async fn eligible_target(
        &self,
        session: &AffinitySession,
        target: Option<&str>,
    ) -> Result<bool, AdminError> {
        let (mode, source) = match session.caller() {
            CallerIdentity::Direct { .. } => ("direct", None),
            CallerIdentity::NewApi { source_id, .. } => ("newApi", Some(source_id.as_str())),
            CallerIdentity::Unidentified { .. } => return Ok(false),
        };
        sqlx::query_scalar::<_, bool>("select exists(select 1 from client_api_keys k cross join provider_accounts a where k.id=$1 and k.enabled and a.provider_kind=$2 and a.enabled and a.scheduling_role='regular' and a.credential_state not in ('invalid','banned') and k.identity_policy_json->>'mode'=$3 and ($4::text is null or (k.identity_policy_json->>'sourceId'=$4 and exists(select 1 from caller_identity_sources s where s.id=$4 and s.enabled))) and (($5::text is not null and a.id=$5) or ($5::text is null and a.id<>$6)) and (not exists(select 1 from client_api_key_groups kg where kg.client_api_key_id=k.id) or exists(select 1 from client_api_key_groups kg join account_groups g on g.id=kg.account_group_id and g.enabled join account_group_accounts ga on ga.account_group_id=g.id where kg.client_api_key_id=k.id and ga.provider_account_id=a.id)))")
            .bind(&session.observation.client_key_id).bind(&session.provider).bind(mode).bind(source).bind(target).bind(&session.account_id).fetch_one(&self.pool).await.map_err(unavailable)
    }

    async fn create_plan(
        &self,
        context: &MutationContext,
        plan: AffinityPlan,
    ) -> Result<AffinityOperation, AdminError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        // 回执保留 30 天，按过期索引分批回收；不等待正在补写审计的操作
        sqlx::query("delete from affinity_operations where id in (select id from affinity_operations where expires_at < now() - interval '30 days' order by expires_at limit 200 for update skip locked)")
            .execute(&mut *tx).await.map_err(unavailable)?;
        let inserted = sqlx::query("insert into affinity_operations(id,plan_json,actor_json,expires_at) values($1,$2,$3,$4) on conflict(id) do nothing").bind(&plan.operation_id).bind(Json(&plan)).bind(Json(context)).bind(plan.expires_at).execute(&mut *tx).await.map_err(unavailable)?.rows_affected();
        if inserted > 0 {
            let event = super::admin_security_audit::mutation_audit(
                context,
                MutationAuditOperation::AffinityPreview,
                &plan.operation_id,
                vec!["selection".to_owned(), "directive".to_owned()],
            );
            super::admin_security_audit::append_admin_audit_event_in_transaction(
                &mut tx, event, None,
            )
            .await
            .map_err(unavailable)?;
        }
        tx.commit().await.map_err(unavailable)?;
        let current = self
            .operation(&plan.operation_id)
            .await?
            .ok_or_else(|| AdminError::internal("亲和预览未保存"))?;
        let keys = |plan: &AffinityPlan| {
            plan.items
                .iter()
                .map(|item| {
                    (
                        item.session.provider.clone(),
                        item.session.reference.clone(),
                        item.session.revision.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        if current.plan.directive != plan.directive || keys(&current.plan) != keys(&plan) {
            return Err(AdminError::conflict("操作 ID 已用于其他预览"));
        }
        Ok(current)
    }

    async fn operation(&self, id: &str) -> Result<Option<AffinityOperation>, AdminError> {
        sqlx::query("select plan_json, results_json from affinity_operations where id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(unavailable)?
            .map(decode_operation)
            .transpose()
    }

    async fn history(&self) -> Result<Vec<AffinityOperation>, AdminError> {
        sqlx::query("select plan_json, results_json from affinity_operations order by created_at desc,id desc limit 50").fetch_all(&self.pool).await.map_err(unavailable)?.into_iter().map(decode_operation).collect()
    }

    async fn record_result(
        &self,
        context: &MutationContext,
        operation_id: &str,
        result: &AffinityItemResult,
    ) -> Result<AffinityItemResult, AdminError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let mut recorded = result.clone();
        recorded.audit_recorded = true;
        let changed = sqlx::query("update affinity_operations set results_json=jsonb_set(results_json,array[$2::text],$3::jsonb),updated_at=now() where id=$1 and coalesce(results_json->$2->>'status','') not in ('applied','conflict','rejected')")
            .bind(operation_id).bind(result.key()).bind(Json(&recorded)).execute(&mut *tx).await.map_err(unavailable)?.rows_affected();
        if changed > 0 {
            let event = super::admin_security_audit::mutation_audit(
                context,
                MutationAuditOperation::AffinityApply,
                operation_id,
                vec![format!("{}:{}", result.key(), result.status)],
            );
            super::admin_security_audit::append_admin_audit_event_in_transaction(
                &mut tx, event, None,
            )
            .await
            .map_err(unavailable)?;
        }
        tx.commit().await.map_err(unavailable)?;
        self.operation(operation_id)
            .await?
            .and_then(|operation| operation.results.get(&result.key()).cloned())
            .ok_or_else(|| AdminError::internal("亲和结果未保存"))
    }
}

fn decode_newapi_config(
    row: &sqlx::postgres::PgRow,
) -> Result<gateway_core::policy::NewApiTokenAffinityConfig, AdminError> {
    let limit: i64 = row.try_get("request_limit").map_err(unavailable)?;
    let config = gateway_core::policy::NewApiTokenAffinityConfig {
        enabled: row.try_get("enabled").map_err(unavailable)?,
        request_limit: u32::try_from(limit).map_err(unavailable)?,
        target_account: row.try_get("target_account").map_err(unavailable)?,
    };
    if !config.is_valid() {
        return Err(AdminError::internal("New API 规则数据无效"));
    }
    Ok(config)
}

fn decode_operation(row: sqlx::postgres::PgRow) -> Result<AffinityOperation, AdminError> {
    Ok(AffinityOperation {
        plan: row
            .try_get::<Json<AffinityPlan>, _>("plan_json")
            .map_err(unavailable)?
            .0,
        results: row
            .try_get::<Json<std::collections::BTreeMap<String, AffinityItemResult>>, _>(
                "results_json",
            )
            .map_err(unavailable)?
            .0,
    })
}

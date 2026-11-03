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

//! 只读额度候选、独立检查进度及周期去重与邮件的原子提交

use super::*;
use gateway_admin::model::provider_credentials::ProviderDocument;
use gateway_core::account::OpaqueProviderData;

impl PgAlertRepository {
    pub(super) async fn load_quota_accounts(
        &self,
        after: Option<String>,
    ) -> AdminStoreResult<Vec<QuotaWarningAccount>> {
        self.transaction("read quota warning observations", move |tx| Box::pin(async move {
            let rows = sqlx::query("select id,name,email,plan_type,credential_revision,quota_observed_at,provider_quota_json from provider_accounts where enabled and provider_kind='openai' and authentication_kind='oauth' and ($1::text is null or id>$1) order by id limit 200")
                .bind(after).fetch_all(&mut **tx).await.map_err(db_error)?;
            rows.into_iter().map(|row| {
                let document: Option<serde_json::Value> = row.try_get("provider_quota_json").map_err(db_error)?;
                // Store 只校验文档容器，窗口和比例由 Provider 解释
                let document = document.and_then(|value| value.as_object().cloned()).map(|value| ProviderDocument::new(OpaqueProviderData::new(value)));
                Ok(QuotaWarningAccount {
                    id: row.try_get("id").map_err(db_error)?, name: row.try_get("name").map_err(db_error)?,
                    email: row.try_get("email").map_err(db_error)?, plan_type: row.try_get("plan_type").map_err(db_error)?,
                    credential_revision: unsigned(row.try_get("credential_revision").map_err(db_error)?)?,
                    observed_at: row.try_get("quota_observed_at").map_err(db_error)?, document,
                })
            }).collect()
        })).await
    }

    pub(super) async fn load_quota_states(&self) -> AdminStoreResult<Vec<QuotaWarningState>> {
        self.transaction("read quota warning cycles", |tx| Box::pin(async move {
            let rows = sqlx::query("select account_id,window_key,cycle_reset_at,notified from quota_alert_windows order by account_id,window_key").fetch_all(&mut **tx).await.map_err(db_error)?;
            rows.into_iter().map(|row| Ok(QuotaWarningState {
                account_id: row.try_get("account_id").map_err(db_error)?, window_key: row.try_get("window_key").map_err(db_error)?,
                cycle_reset_at: row.try_get("cycle_reset_at").map_err(db_error)?, notified: row.try_get("notified").map_err(db_error)?,
            })).collect()
        })).await
    }

    pub(super) async fn load_quota_status(&self) -> AdminStoreResult<QuotaWarningStatus> {
        self.transaction("read quota warning status", |tx| {
            Box::pin(async move { quota_status(tx).await })
        })
        .await
    }

    pub(super) async fn save_quota_evaluation(
        &self,
        revision: u64,
        previous_check: Option<DateTime<Utc>>,
        mut evaluation: QuotaWarningEvaluation,
    ) -> AdminStoreResult<bool> {
        self.transaction("commit quota warning", move |tx| Box::pin(async move {
            let settings = load_settings(tx, true).await?;
            let previous = quota_status(tx).await?;
            let checked = evaluation.status.checked_at.ok_or_else(|| invalid("quota evaluation time missing"))?;
            if settings.revision != revision || !settings.values.quota_warning().enabled || previous.checked_at != previous_check
                || previous_check.is_some_and(|last| checked >= last && checked.signed_duration_since(last).num_seconds() < QUOTA_EVALUATION_INTERVAL_SECONDS) {
                return Ok(false);
            }
            // 账号在扫描后被关闭或删除时不消费通知资格，下一轮重新读取
            let ids: Vec<_> = evaluation.states.iter().map(|state| state.account_id.clone()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            let count: i64 = sqlx::query_scalar("select count(*) from provider_accounts where id=any($1::text[]) and enabled and provider_kind='openai' and authentication_kind='oauth'")
                .bind(&ids).fetch_one(&mut **tx).await.map_err(db_error)?;
            if usize::try_from(count).ok() != Some(ids.len()) { return Ok(false); }
            if !evaluation.items.is_empty() {
                let (subject, body) = gateway_admin::alerts::quota::notification_text(&evaluation);
                let key = format!("quota:{}", uuid::Uuid::now_v7().simple());
                // 队列满会回滚整笔事务，不能把未入队窗口记作已提醒
                enqueue(tx, &settings, None, AlertNotificationKind::QuotaWarning, &key, &subject, &body).await?;
            }
            let account_ids: Vec<_> = evaluation.states.iter().map(|state| state.account_id.clone()).collect();
            let keys: Vec<_> = evaluation.states.iter().map(|state| state.window_key.clone()).collect();
            let resets: Vec<_> = evaluation.states.iter().map(|state| state.cycle_reset_at).collect();
            let notified: Vec<_> = evaluation.states.iter().map(|state| state.notified).collect();
            sqlx::query("insert into quota_alert_windows(account_id,window_key,cycle_reset_at,notified) select * from unnest($1::text[],$2::text[],$3::timestamptz[],$4::boolean[]) on conflict (account_id,window_key) do update set cycle_reset_at=excluded.cycle_reset_at,notified=excluded.notified")
                .bind(account_ids).bind(keys).bind(resets).bind(notified).execute(&mut **tx).await.map_err(db_error)?;
            evaluation.status.notified_windows += evaluation.status.pending_windows;
            sqlx::query("update alert_settings set quota_runtime_json=$1 where id=1")
                .bind(sqlx::types::Json(evaluation.status)).execute(&mut **tx).await.map_err(db_error)?;
            prune(tx).await?;
            Ok(true)
        })).await
    }
}

pub(super) async fn quota_status(
    tx: &mut Transaction<'_, Postgres>,
) -> StoreResult<QuotaWarningStatus> {
    Ok(
        sqlx::query_scalar::<_, sqlx::types::Json<QuotaWarningStatus>>(
            "select quota_runtime_json from alert_settings where id=1",
        )
        .fetch_one(&mut **tx)
        .await
        .map_err(db_error)?
        .0,
    )
}

//! 告警设置、评估事件与邮件 outbox 的 PostgreSQL 事务 owner

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use gateway_admin::{
    model::{MutationContext, alerts::*, audit::MutationAuditOperation},
    ports::{
        alerts::AlertRepository,
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};
use secrecy::{ExposeSecret as _, SecretString};
use sqlx::{Acquire as _, PgPool, Postgres, Row as _, Transaction, postgres::PgRow};
use tokio::sync::Semaphore;

mod quota;

use super::{ObservabilityQueryBudget, append_admin_audit_event_in_transaction};
use crate::{
    ConflictKind, StoreBackend, StoreError, StoreResult, admin_store_error, mutation_audit,
    postgres_unavailable,
};

#[derive(Clone)]
pub struct PgAlertRepository {
    pool: PgPool,
    budget: ObservabilityQueryBudget,
    slots: Arc<Semaphore>,
}

impl PgAlertRepository {
    #[must_use]
    pub fn new(pool: PgPool, budget: ObservabilityQueryBudget) -> Self {
        Self {
            pool,
            budget,
            slots: Arc::new(Semaphore::new(1)),
        }
    }

    /// 告警 SQL 不排队占满观测连接，网络投递不在此事务范围内
    async fn transaction<T: Send, F>(&self, operation: &'static str, run: F) -> AdminStoreResult<T>
    where
        F: for<'a, 'b> FnOnce(&'a mut Transaction<'b, Postgres>) -> BoxFuture<'a, StoreResult<T>>
            + Send,
    {
        let slot = self.slots.clone().try_acquire_owned().map_err(|_| {
            AdminStoreError::new(
                AdminStoreErrorKind::Unavailable,
                "email alerts",
                "alert store is busy",
            )
        })?;
        self.budget.run(&self.pool, operation, |mut connection| async move {
            let _slot = slot;
            tokio::time::timeout(Duration::from_secs(3), async {
                let mut tx = connection.begin().await.map_err(db_error)?;
                let result = async {
                    sqlx::query("set transaction isolation level repeatable read")
                        .execute(&mut *tx).await.map_err(db_error)?;
                    sqlx::query("select set_config('statement_timeout','2s',true),set_config('lock_timeout','100ms',true)")
                        .execute(&mut *tx).await.map_err(db_error)?;
                    run(&mut tx).await
                }.await;
                match result {
                    Ok(value) => { tx.commit().await.map_err(db_error)?; Ok(value) }
                    Err(error) => match tx.rollback().await {
                        Ok(()) => Err(error),
                        Err(cleanup) => Err(error.with_cleanup(cleanup)),
                    },
                }
            }).await.map_err(|_| StoreError::Unavailable {
                backend: StoreBackend::PostgreSql,
                message: "alert operation exceeded its budget".to_owned(), source: None,
            })?
        }).await.map_err(|error| admin_store_error("email alerts", error))
    }
}

#[async_trait]
impl AlertRepository for PgAlertRepository {
    async fn quota_accounts(
        &self,
        after: Option<String>,
    ) -> AdminStoreResult<Vec<QuotaWarningAccount>> {
        self.load_quota_accounts(after).await
    }
    async fn quota_states(&self) -> AdminStoreResult<Vec<QuotaWarningState>> {
        self.load_quota_states().await
    }
    async fn quota_failed(
        &self,
        revision: u64,
        previous_check: Option<DateTime<Utc>>,
    ) -> AdminStoreResult<()> {
        self.transaction("mark quota warning failure", move |tx| Box::pin(async move {
            sqlx::query("update alert_settings set quota_runtime_json=jsonb_set(quota_runtime_json,'{error}','true'::jsonb) where id=1 and revision=$1 and coalesce(quota_runtime_json->'checkedAt','null'::jsonb)=$2")
                .bind(signed(revision)?).bind(sqlx::types::Json(previous_check)).execute(&mut **tx).await.map_err(db_error)?;
            Ok(())
        })).await
    }
    async fn quota_status(&self) -> AdminStoreResult<QuotaWarningStatus> {
        self.load_quota_status().await
    }
    async fn commit_quota_evaluation(
        &self,
        revision: u64,
        previous_check: Option<DateTime<Utc>>,
        evaluation: QuotaWarningEvaluation,
    ) -> AdminStoreResult<bool> {
        self.save_quota_evaluation(revision, previous_check, evaluation)
            .await
    }

    async fn settings(&self) -> AdminStoreResult<AlertSettings> {
        self.transaction("load alert settings", |tx| {
            Box::pin(async move { load_settings(tx, false).await })
        })
        .await
    }

    async fn update_settings(
        &self,
        command: UpdateAlertSettings,
        context: &MutationContext,
    ) -> AdminStoreResult<AlertSettings> {
        let context = context.clone();
        self.transaction("update alert settings", move |tx| Box::pin(async move {
            let current = load_settings(tx, true).await?;
            if current.revision != command.expected_revision { return Err(conflict()); }
            let password = if command.clear_password { None } else { command.password.or(current.password.clone()) };
            if current.values == command.values
                && current.password.as_ref().map(|value| value.expose_secret()) == password.as_ref().map(|value| value.expose_secret())
            { return Ok(current); }
            let failure_changed = failure_rule(&current.values) != failure_rule(&command.values);
            let mail_changed = mail_config(&current.values) != mail_config(&command.values)
                || current.password.as_ref().map(|v| v.expose_secret()) != password.as_ref().map(|v| v.expose_secret());
            let quota_changed = current.values.quota_warning() != command.values.quota_warning();
            let previous = load_runtime(tx).await?;
            let runtime = if failure_changed || mail_changed { AlertRuntime {
                generation: previous.generation.saturating_add(1),
                evaluation_status: if command.values.enabled { AlertEvaluationStatus::Waiting } else { AlertEvaluationStatus::Disabled },
                ..AlertRuntime::default()
            } } else { previous };
            if failure_changed || mail_changed {
            sqlx::query("update alert_events set status='cancelled',closed_at=now() where status='firing'")
                .execute(&mut **tx).await.map_err(db_error)?;
            }
            sqlx::query("update alert_notifications set status='cancelled',claimed_until=null where status in ('pending','sending') and ($1 or (kind in ('triggered','reminder','recovered') and $2) or (kind='quota_warning' and $3))")
                .bind(mail_changed).bind(failure_changed).bind(quota_changed).execute(&mut **tx).await.map_err(db_error)?;
            // 无关规则变更不取消另一规则已入队的邮件，仅推进它们的配置版本
            sqlx::query("update alert_notifications set settings_revision=$1 where status in ('pending','sending')")
                .bind(signed(current.revision+1)?).execute(&mut **tx).await.map_err(db_error)?;
            if quota_changed {
                sqlx::query("update alert_settings set quota_runtime_json='{}'::jsonb where id=1").execute(&mut **tx).await.map_err(db_error)?;
            }
            sqlx::query("update alert_settings set revision=revision+1,values_json=$1,smtp_password=$2,runtime_json=$3,updated_at=now() where id=1")
                .bind(sqlx::types::Json(&command.values))
                .bind(password.as_ref().map(|value| value.expose_secret()))
                .bind(sqlx::types::Json(runtime))
                .execute(&mut **tx).await.map_err(db_error)?;
            let mut changed = Vec::new();
            let before = serde_json::to_value(&current.values).map_err(json_error)?;
            let after = serde_json::to_value(&command.values).map_err(json_error)?;
            if let (Some(before), Some(after)) = (before.as_object(), after.as_object()) {
                changed.extend(after.iter().filter(|(key, value)| before.get(*key) != Some(*value)).map(|(key, _)| key.clone()));
            }
            if current.password.as_ref().map(|value| value.expose_secret()) != password.as_ref().map(|value| value.expose_secret()) {
                changed.push("smtpPassword".to_owned());
            }
            append_admin_audit_event_in_transaction(tx, mutation_audit(&context, MutationAuditOperation::AlertSettingsUpdate, "1", changed), None).await?;
            load_settings(tx, false).await
        })).await
    }

    async fn runtime(&self) -> AdminStoreResult<AlertRuntime> {
        self.transaction("load alert runtime", |tx| {
            Box::pin(async move { load_runtime(tx).await })
        })
        .await
    }

    async fn status(&self) -> AdminStoreResult<AlertStatus> {
        self.transaction("load alert status", |tx| Box::pin(async move {
            // 状态与最近记录在同一快照中读取，不把相邻事务的触发与投递混成一次结果
            let runtime = load_runtime(tx).await?;
            let rows = sqlx::query("select id,status,opened_at,closed_at,sample_json from alert_events order by opened_at desc,id desc limit 10")
                .fetch_all(&mut **tx).await.map_err(db_error)?;
            let events = rows.iter().map(|row| Ok(AlertEvent {
                id: row.try_get("id").map_err(db_error)?, status: row.try_get("status").map_err(db_error)?,
                opened_at: row.try_get("opened_at").map_err(db_error)?, closed_at: row.try_get("closed_at").map_err(db_error)?,
                sample: row.try_get::<sqlx::types::Json<AlertSample>, _>("sample_json").map_err(db_error)?.0,
            })).collect::<StoreResult<Vec<_>>>()?;
            let rows = sqlx::query("select id,kind,recipient,status,attempts,created_at,sent_at,next_attempt_at,error_message from alert_notifications order by created_at desc,id desc limit 10")
                .fetch_all(&mut **tx).await.map_err(db_error)?;
            let deliveries = rows.iter().map(delivery_record).collect::<StoreResult<_>>()?;
            Ok(AlertStatus { runtime, events, deliveries, quota_warning: quota::quota_status(tx).await? })
        })).await
    }

    async fn sample(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> AdminStoreResult<AlertSample> {
        self.transaction("sample alert failure rate", move |tx| Box::pin(async move {
            if start >= end || end.signed_duration_since(start) > chrono::TimeDelta::hours(1) {
                return Err(invalid("invalid alert window"));
            }
            // 按完成时间读取原始终态，避免小时投影或开始时间桶改变滑动窗口含义
            // 复用已有 completed_at 索引，不引入请求侧计数、触发器或写入分支
            let row = sqlx::query(
                "select count(*)::bigint as requests,count(*) filter (where outcome in ('failed','incomplete'))::bigint as failures
                 from model_request_observations
                 where outcome in ('succeeded','failed','incomplete')
                   and completed_at >= $1 and completed_at < $2
                   and request_kind is distinct from 'prewarm'
                   and client_transport not in ('internal','internal_plugin')
                   and operation in ('generate','generate_image')"
            ).bind(start).bind(end).fetch_one(&mut **tx).await.map_err(db_error)?;
            Ok(AlertSample {
                window_start: start, window_end: end,
                requests: unsigned(row.try_get("requests").map_err(db_error)?)?,
                failures: unsigned(row.try_get("failures").map_err(db_error)?)?,
            })
        })).await
    }

    async fn commit_evaluation(
        &self,
        expected_revision: u64,
        expected_generation: u64,
        evaluation: AlertEvaluation,
        subject: String,
        body: String,
    ) -> AdminStoreResult<bool> {
        self.transaction("commit alert evaluation", move |tx| Box::pin(async move {
            let settings = load_settings(tx, true).await?;
            let previous = load_runtime(tx).await?;
            if settings.revision != expected_revision || previous.generation != expected_generation || !settings.values.enabled {
                return Ok(false);
            }
            let now = evaluation.runtime.checked_at.ok_or_else(|| invalid("evaluation time missing"))?;
            if evaluation.notification == Some(AlertNotificationKind::Triggered) {
                let event_id = evaluation.event_id.as_deref().ok_or_else(|| invalid("event missing"))?;
                let sample = evaluation.runtime.sample.as_ref().ok_or_else(|| invalid("sample missing"))?;
                sqlx::query("insert into alert_events (id,status,opened_at,sample_json) values ($1,'firing',$2,$3)")
                    .bind(event_id).bind(now).bind(sqlx::types::Json(sample))
                    .execute(&mut **tx).await.map_err(db_error)?;
            }
            if evaluation.notification == Some(AlertNotificationKind::Recovered) {
                sqlx::query("update alert_events set status='recovered',closed_at=$2 where id=$1 and status='firing'")
                    .bind(&evaluation.event_id).bind(now).execute(&mut **tx).await.map_err(db_error)?;
                // 恢复后取消尚未开始的异常提醒，避免恢复邮件之后再收到过期异常
                sqlx::query("update alert_notifications set status='cancelled' where event_id=$1 and status='pending' and kind in ('triggered','reminder')")
                    .bind(&evaluation.event_id).execute(&mut **tx).await.map_err(db_error)?;
            }
            if let Some(kind) = evaluation.notification.filter(|kind| *kind != AlertNotificationKind::Recovered || settings.values.recovery_notification) {
                let key = format!("{}:{}", evaluation.event_id.as_deref().unwrap_or_default(), evaluation.runtime.generation);
                enqueue(tx, &settings, evaluation.event_id.as_deref(), kind, &key, &subject, &body).await?;
            }
            sqlx::query("update alert_settings set runtime_json=$1 where id=1")
                .bind(sqlx::types::Json(&evaluation.runtime)).execute(&mut **tx).await.map_err(db_error)?;
            prune(tx).await?;
            Ok(true)
        })).await
    }

    async fn queue_test(
        &self,
        expected_revision: u64,
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        let context = context.clone();
        self.transaction("queue alert test", move |tx| Box::pin(async move {
            let settings = load_settings(tx, true).await?;
            if settings.revision != expected_revision { return Err(conflict()); }
            let limited: bool = sqlx::query_scalar::<_, Option<bool>>("select last_test_at > now()-interval '1 minute' from alert_settings where id=1")
                .fetch_one(&mut **tx).await.map_err(db_error)?.unwrap_or(false);
            if limited { return Err(conflict()); }
            let key = format!("test_{}", uuid::Uuid::now_v7().simple());
            enqueue(tx, &settings, None, AlertNotificationKind::Test, &key,
                "[Codex Proxy][测试] 告警邮件配置测试",
                "这是一封管理员主动发起的测试邮件\n收到此邮件表示当前 SMTP 配置可以向此地址投递\n测试不代表模型请求发生异常\n").await?;
            sqlx::query("update alert_settings set last_test_at=now() where id=1").execute(&mut **tx).await.map_err(db_error)?;
            append_admin_audit_event_in_transaction(tx, mutation_audit(&context, MutationAuditOperation::AlertEmailTest, "1", Vec::new()), None).await?;
            prune(tx).await
        })).await
    }

    async fn claim_delivery(&self) -> AdminStoreResult<Option<AlertDelivery>> {
        self.transaction("claim alert email", |tx| Box::pin(async move {
            // 与配置保存使用相同锁序，防止关闭或修改后继续领取旧版本消息
            let settings = load_settings(tx, true).await?;
            // 在途发送保留实际结果，只有待发送或租约已到期的旧异常通知可以取消
            sqlx::query("update alert_notifications n set status='cancelled',claimed_until=null,error_message=null where kind in ('triggered','reminder') and (status='pending' or (status='sending' and claimed_until<=now())) and exists (select 1 from alert_events e where e.id=n.event_id and e.status<>'firing')")
                .execute(&mut **tx).await.map_err(db_error)?;
            sqlx::query("update alert_notifications set status='failed',error_message='邮件投递未能确认，已达到重试上限',claimed_until=null where status='sending' and claimed_until<=now() and attempts>=3")
                .execute(&mut **tx).await.map_err(db_error)?;
            let row = sqlx::query(
                "with candidate as (
                   select id from alert_notifications
                   where settings_revision=$1 and (kind='test' or (kind='quota_warning' and $3) or (kind in ('triggered','reminder','recovered') and $2))
                     and attempts<3 and next_attempt_at<=now()
                     and (status='pending' or (status='sending' and claimed_until<=now()))
                   order by created_at,id for update skip locked limit 1
                 ) update alert_notifications n set status='sending',attempts=attempts+1,claimed_until=now()+interval '60 seconds'
                   from candidate c where n.id=c.id returning n.id,n.attempts,n.recipient,n.subject,n.body"
            ).bind(signed(settings.revision)?).bind(settings.values.enabled).bind(settings.values.quota_warning().enabled)
                .fetch_optional(&mut **tx).await.map_err(db_error)?;
            row.map(|row| Ok(AlertDelivery {
                id: row.try_get("id").map_err(db_error)?,
                attempt: u32::try_from(row.try_get::<i32,_>("attempts").map_err(db_error)?).map_err(|_| invalid("invalid attempt"))?,
                recipient: row.try_get("recipient").map_err(db_error)?,
                subject: row.try_get("subject").map_err(db_error)?, body: row.try_get("body").map_err(db_error)?, settings,
            })).transpose()
        })).await
    }

    async fn complete_delivery(
        &self,
        id: &str,
        attempt: u32,
        succeeded: bool,
    ) -> AdminStoreResult<()> {
        let id = id.to_owned();
        self.transaction("complete alert email", move |tx| Box::pin(async move {
            // 与恢复及领取串行化，避免恢复清理与失败重排交错后重新发送旧异常
            sqlx::query("select id from alert_settings where id=1 for update")
                .execute(&mut **tx).await.map_err(db_error)?;
            sqlx::query(
                "with result as (
                   select n.id,kind in ('triggered','reminder') and exists (
                     select 1 from alert_events e where e.id=n.event_id and e.status<>'firing'
                   ) as obsolete
                   from alert_notifications n where id=$1 and attempts=$2 and status='sending'
                 ) update alert_notifications n set
                   status=case when $3 then 'sent' when r.obsolete then 'cancelled' when attempts>=3 then 'failed' else 'pending' end,
                   sent_at=case when $3 then now() else null end,
                   error_message=case when $3 or r.obsolete then null else '邮件发送失败，请检查 SMTP 配置或网络连接' end,
                   next_attempt_at=now()+make_interval(secs=>case when attempts=1 then 60 else 300 end),claimed_until=null
                 from result r where n.id=r.id"
            ).bind(id).bind(i32::try_from(attempt).map_err(|_| invalid("invalid attempt"))?).bind(succeeded)
                .execute(&mut **tx).await.map_err(db_error)?;
            Ok(())
        })).await
    }
}

async fn load_settings(
    tx: &mut Transaction<'_, Postgres>,
    lock: bool,
) -> StoreResult<AlertSettings> {
    let sql = if lock {
        "select revision,values_json,smtp_password,updated_at from alert_settings where id=1 for update"
    } else {
        "select revision,values_json,smtp_password,updated_at from alert_settings where id=1"
    };
    let row = sqlx::query(sql)
        .fetch_one(&mut **tx)
        .await
        .map_err(db_error)?;
    Ok(AlertSettings {
        revision: unsigned(row.try_get("revision").map_err(db_error)?)?,
        values: row
            .try_get::<sqlx::types::Json<AlertSettingsValues>, _>("values_json")
            .map_err(db_error)?
            .0,
        password: row
            .try_get::<Option<String>, _>("smtp_password")
            .map_err(db_error)?
            .map(SecretString::from),
        updated_at: row.try_get("updated_at").map_err(db_error)?,
    })
}

async fn load_runtime(tx: &mut Transaction<'_, Postgres>) -> StoreResult<AlertRuntime> {
    Ok(sqlx::query_scalar::<_, sqlx::types::Json<AlertRuntime>>(
        "select runtime_json from alert_settings where id=1",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(db_error)?
    .0)
}

async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    settings: &AlertSettings,
    event_id: Option<&str>,
    kind: AlertNotificationKind,
    key: &str,
    subject: &str,
    body: &str,
) -> StoreResult<()> {
    let pending: i64 = sqlx::query_scalar(
        "select count(*) from alert_notifications where status in ('pending','sending')",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(db_error)?;
    if pending + i64::try_from(settings.values.recipients.len()).unwrap_or(i64::MAX)
        > MAX_PENDING_DELIVERIES
    {
        return Err(StoreError::Unavailable {
            backend: StoreBackend::PostgreSql,
            message: "alert email queue is full".to_owned(),
            source: None,
        });
    }
    for recipient in &settings.values.recipients {
        sqlx::query("insert into alert_notifications (id,event_id,kind,settings_revision,notification_key,recipient,subject,body) values ($1,$2,$3,$4,$5,$6,$7,$8) on conflict (notification_key,recipient) do nothing")
            .bind(format!("mail_{}",uuid::Uuid::now_v7().simple())).bind(event_id).bind(kind.as_str())
            .bind(signed(settings.revision)?).bind(key).bind(recipient).bind(subject).bind(body)
            .execute(&mut **tx).await.map_err(db_error)?;
    }
    Ok(())
}

async fn prune(tx: &mut Transaction<'_, Postgres>) -> StoreResult<()> {
    sqlx::query("delete from alert_notifications where id in (select id from alert_notifications where status in ('sent','failed','cancelled') and created_at<now()-interval '30 days' order by created_at limit 100)")
        .execute(&mut **tx).await.map_err(db_error)?;
    sqlx::query("delete from alert_events where id in (select e.id from alert_events e where e.closed_at<now()-interval '30 days' and not exists (select 1 from alert_notifications n where n.event_id=e.id) order by e.closed_at limit 20)")
        .execute(&mut **tx).await.map_err(db_error)?;
    Ok(())
}

fn delivery_record(row: &PgRow) -> StoreResult<AlertDeliveryRecord> {
    let kind: String = row.try_get("kind").map_err(db_error)?;
    let kind = match kind.as_str() {
        "triggered" => AlertNotificationKind::Triggered,
        "reminder" => AlertNotificationKind::Reminder,
        "recovered" => AlertNotificationKind::Recovered,
        "test" => AlertNotificationKind::Test,
        "quota_warning" => AlertNotificationKind::QuotaWarning,
        _ => return Err(invalid("invalid notification kind")),
    };
    Ok(AlertDeliveryRecord {
        id: row.try_get("id").map_err(db_error)?,
        kind,
        recipient: row.try_get("recipient").map_err(db_error)?,
        status: row.try_get("status").map_err(db_error)?,
        attempts: u32::try_from(row.try_get::<i32, _>("attempts").map_err(db_error)?)
            .map_err(|_| invalid("invalid attempt"))?,
        created_at: row.try_get("created_at").map_err(db_error)?,
        sent_at: row.try_get("sent_at").map_err(db_error)?,
        next_attempt_at: row.try_get("next_attempt_at").map_err(db_error)?,
        error_message: row.try_get("error_message").map_err(db_error)?,
    })
}

fn signed(value: u64) -> StoreResult<i64> {
    i64::try_from(value).map_err(|_| invalid("value exceeds bigint"))
}
fn unsigned(value: i64) -> StoreResult<u64> {
    u64::try_from(value).map_err(|_| invalid("negative count or revision"))
}
fn db_error(error: sqlx::Error) -> StoreError {
    postgres_unavailable("email alert storage", error)
}
fn json_error(error: serde_json::Error) -> StoreError {
    invalid("invalid alert json").with_source(error)
}
fn invalid(message: &str) -> StoreError {
    StoreError::InvalidData {
        entity: "email alerts",
        message: message.to_owned(),
        source: None,
    }
}
fn conflict() -> StoreError {
    StoreError::Conflict {
        entity: "email alerts",
        id: "1".to_owned(),
        kind: ConflictKind::StaleRevision,
        source: None,
    }
}

fn failure_rule(
    values: &AlertSettingsValues,
) -> (bool, u32, u32, u32, u32, u32, Option<u32>, bool) {
    (
        values.enabled,
        values.window_minutes,
        values.minimum_requests,
        values.failure_threshold_percent,
        values.consecutive_evaluations,
        values.recovery_threshold_percent,
        values.reminder_minutes,
        values.recovery_notification,
    )
}

fn mail_config(
    values: &AlertSettingsValues,
) -> (&str, u16, SmtpSecurity, &str, &str, &str, &[String]) {
    (
        &values.smtp_host,
        values.smtp_port,
        values.smtp_security,
        &values.smtp_username,
        &values.sender_address,
        &values.sender_name,
        &values.recipients,
    )
}

//! 额度快照的凭据版本与连接校验，不解释额度正文

use super::*;
use gateway_core::account::UpstreamConnection;

pub(super) async fn write(
    repository: &PgProviderAccountRepository,
    observation: QuotaObservation,
) -> Result<QuotaWriteOutcome, CoreStoreError> {
    let updated = if let Some(expected) = &observation.expected_connection {
        if expected.account_id != observation.account_id
            || expected.credential_revision != observation.expected_revision
        {
            return Ok(QuotaWriteOutcome::Conflict);
        }
        let mut transaction = repository.pool.begin().await.map_err(database_error)?;
        // 先等待账号锁，再以新语句读取代理，避免使用等待前的连接投影
        sqlx::query("select id from provider_accounts where id = $1 for update")
            .bind(observation.account_id.as_str())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?;
        let row = sqlx::query(
            "select account.enabled, account.credential_revision,
                    case when coalesce(proxy.enabled, true) then account.outbound_proxy_url end as outbound_proxy_url
             from provider_accounts account
             left join outbound_proxies proxy on proxy.id = account.outbound_proxy_id
             where account.id = $1",
        )
        .bind(observation.account_id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let Some(row) = row else {
            return Ok(QuotaWriteOutcome::Conflict);
        };
        let revision = get::<i64>(&row, "credential_revision")
            .and_then(to_u64)
            .map_err(crate::core_store_error)?;
        let revision = CoreCredentialRevision::new(revision)
            .map_err(|source| CoreStoreError::caused_by(CoreStoreErrorKind::InvalidData, source))?;
        let proxy = get::<Option<String>>(&row, "outbound_proxy_url")
            .map_err(crate::core_store_error)?
            .map(|url| gateway_core::account::OutboundProxy::parse(&url))
            .transpose()
            .map_err(|source| CoreStoreError::caused_by(CoreStoreErrorKind::InvalidData, source))?;
        let connection =
            UpstreamConnection::new(observation.account_id.clone(), revision, proxy.as_ref());
        if !get::<bool>(&row, "enabled").map_err(crate::core_store_error)?
            || connection != *expected
        {
            return Ok(QuotaWriteOutcome::Conflict);
        }
        let updated = super::repository::write_quota(&mut *transaction, observation)
            .await
            .map_err(crate::core_store_error)?;
        transaction.commit().await.map_err(database_error)?;
        updated
    } else {
        super::repository::write_quota(&repository.pool, observation)
            .await
            .map_err(crate::core_store_error)?
    };
    Ok(if updated {
        QuotaWriteOutcome::Updated
    } else {
        QuotaWriteOutcome::Conflict
    })
}

fn database_error(error: sqlx::Error) -> CoreStoreError {
    CoreStoreError::caused_by(CoreStoreErrorKind::Unavailable, error)
}

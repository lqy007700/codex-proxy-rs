//! 额度快照的凭据版本校验与写入，不解释额度正文

use super::*;

pub(super) async fn write(
    repository: &PgProviderAccountRepository,
    observation: QuotaObservation,
) -> Result<QuotaWriteOutcome, CoreStoreError> {
    let quota = JsonObject::try_from_value(
        "provider_quota_json",
        serde_json::Value::Object(observation.quota.into_inner()),
        QUOTA_MAX_BYTES,
    )
    .map_err(crate::core_store_error)?;
    let revision =
        Revision::new(observation.expected_revision.get()).map_err(crate::core_store_error)?;
    let updated = repository
        .compare_and_swap_provider_quota(
            observation.account_id.as_str(),
            revision,
            quota,
            observation.observed_at.into(),
            observation.state,
            observation.plan_type.as_deref(),
        )
        .await
        .map_err(crate::core_store_error)?;
    Ok(if updated {
        QuotaWriteOutcome::Updated
    } else {
        QuotaWriteOutcome::Conflict
    })
}

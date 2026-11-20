//! `gateway-admin` 账号端口的 PostgreSQL adapter

use gateway_admin::model::{
    accounts::{
        AccountModelTest, AccountModelTestDetail, AccountModelTestKind, AccountModelTestOutcome,
    },
    audit::MutationAuditOperation,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

use gateway_core::provider_ports::ProviderCooldownPort;

use super::*;
use crate::postgres::ObservabilityQueryBudget;

fn account_capacity(
    account: &AccountRecord,
    default_concurrency: u64,
    in_flight: Option<&BTreeMap<String, u64>>,
) -> gateway_admin::model::accounts::AccountCapacity {
    gateway_admin::model::accounts::AccountCapacity {
        used_slots: in_flight.map(|counts| counts.get(&account.id).copied().unwrap_or(0)),
        total_slots: account.concurrency_limit.map_or_else(
            || (default_concurrency > 0).then_some(default_concurrency),
            |limit| Some(u64::from(limit.get())),
        ),
    }
}

/// Admin 账号用例所需的公共账号、留存观测与 revision 事务能力
///
/// 三个 PostgreSQL adapter 都保持私有，调用方只能取得 [`AccountStore`] 暴露的领域能力
#[derive(Clone)]
pub struct PgAdminAccountStore {
    pub(super) pool: PgPool,
    pub(super) accounts: PgProviderAccountRepository,
    observability: PgObservabilityRepository,
    control_plane: PgControlPlaneRepository,
    cooldowns: Option<Arc<dyn ProviderCooldownPort>>,
    query_budget: ObservabilityQueryBudget,
}

impl PgAdminAccountStore {
    #[must_use]
    pub fn new(
        pool: PgPool,
        cooldowns: Option<Arc<dyn ProviderCooldownPort>>,
        query_budget: ObservabilityQueryBudget,
    ) -> Self {
        let accounts = PgProviderAccountRepository::new(pool.clone());
        let accounts = match &cooldowns {
            Some(port) => accounts.with_account_error_protection(port.clone()),
            None => accounts,
        };
        Self {
            pool: pool.clone(),
            accounts,
            observability: PgObservabilityRepository::new(
                pool.clone(),
                None,
                query_budget.clone(),
                None,
            ),
            control_plane: PgControlPlaneRepository::new(pool),
            cooldowns,
            query_budget,
        }
    }

    async fn usage_observations(
        &self,
        range: ObservabilityRange,
        account_ids: &[String],
    ) -> AdminStoreResult<Vec<ProviderAccountUsageObservation>> {
        if account_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut observations = Vec::with_capacity(account_ids.len());
        for account_ids in account_ids.chunks(ADMIN_USAGE_CHUNK_SIZE) {
            let query = ProviderAccountUsageQuery::for_accounts(range, account_ids.to_vec())
                .and_then(ProviderAccountUsageQuery::with_hourly_request_buckets)
                .map_err(|error| admin_store_error(ENTITY, error))?;
            observations.extend(
                self.observability
                    .provider_account_usage(query)
                    .await
                    .map_err(|error| admin_store_error(ENTITY, error))?,
            );
        }
        Ok(observations)
    }

    async fn usage_by_windows(
        &self,
        windows: &[AccountUsageWindowQuery],
    ) -> AdminStoreResult<Vec<AccountUsageWindowResult>> {
        if windows.is_empty() {
            return Ok(Vec::new());
        }
        let account_ids = windows
            .iter()
            .map(|window| window.account_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        validate_admin_account_ids(&account_ids)
            .map_err(|error| admin_store_error(ENTITY, error))?;
        for window in windows {
            require_nonempty(ENTITY, "quota window key", &window.key)
                .map_err(|error| admin_store_error(ENTITY, error))?;
            ObservabilityRange::new(window.range.start, window.range.end)
                .map_err(|error| admin_store_error(ENTITY, error))?;
        }
        self.query_budget
            .run(
                &self.pool,
                "load account usage windows",
                |mut connection| async move {
                    crate::postgres::observability::account_usage_windows(&mut connection, windows)
                        .await
                },
            )
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
    }

    async fn required_scope(
        &self,
        account_id: &str,
    ) -> AdminStoreResult<ProviderAccountAdminScope> {
        let record = self
            .accounts
            .load_provider_account(account_id)
            .await
            .map_err(|error| admin_store_error(ENTITY, error))?
            .ok_or_else(|| {
                admin_store_error(
                    ENTITY,
                    StoreError::NotFound {
                        source: None,
                        entity: ENTITY,
                        id: account_id.to_owned(),
                    },
                )
            })?;
        Ok(ProviderAccountAdminScope {
            provider_kind: record.summary.provider_kind,
        })
    }

    async fn commit_prepared_import(
        &self,
        prepared: PreparedCredentialImport,
        settings: Option<AccountImportSettings>,
        context: &MutationContext,
        action: MutationAuditOperation,
        outbound_proxy: Option<gateway_admin::model::proxies::ImportProxyBinding>,
    ) -> AdminStoreResult<CredentialImportResult> {
        let command = prepare_import(prepared, settings, context, action, outbound_proxy)?;
        let imported = self
            .accounts
            .import_provider_accounts(command)
            .await
            .map_err(|error| admin_store_error(ENTITY, error))?;
        import_result(imported)
    }

    async fn commit_prepared_rotation(
        &self,
        prepared: PreparedCredentialRotationFacts,
        settings: Option<UpdateAccount>,
        context: &MutationContext,
        action: MutationAuditOperation,
    ) -> AdminStoreResult<CredentialMutationResult> {
        let account_id = prepared.account_id.clone();
        let command = prepare_rotation(prepared, settings, context, action)?;
        let rotation = self
            .accounts
            .rotate_provider_account(command)
            .await
            .map_err(|error| admin_store_error(ENTITY, error))?;
        rotation_result(rotation, account_id)
    }

    async fn account_groups_by_account(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<BTreeMap<String, Vec<AccountGroupRef>>> {
        if account_ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let rows = sqlx::query_as::<_, (String, String, String, String, bool)>(
            "select m.provider_account_id, g.id, g.name, g.color, g.enabled
             from account_group_accounts m
             join account_groups g on g.id = m.account_group_id
             where m.provider_account_id = any($1::text[])
             order by m.provider_account_id, g.id",
        )
        .bind(account_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|source| {
            admin_store_error(
                ENTITY,
                postgres_unavailable("load account group references", source),
            )
        })?;
        let mut groups = BTreeMap::<String, Vec<AccountGroupRef>>::new();
        for (account_id, group_id, name, color, enabled) in rows {
            let id = AccountGroupId::new(group_id).map_err(|_| {
                AdminStoreError::new(
                    AdminStoreErrorKind::Invalid,
                    ENTITY,
                    "persisted account group ID is invalid",
                )
            })?;
            groups.entry(account_id).or_default().push(AccountGroupRef {
                id,
                name,
                color: gateway_admin::model::account_groups::AccountGroupColor::parse(&color)
                    .ok_or_else(|| {
                        AdminStoreError::new(
                            AdminStoreErrorKind::Invalid,
                            ENTITY,
                            "persisted account group color is invalid",
                        )
                    })?,
                enabled,
            });
        }
        Ok(groups)
    }
}

#[async_trait]
impl AccountStore for PgAdminAccountStore {
    async fn begin_model_test(&self, detail: &AccountModelTestDetail) -> AdminStoreResult<()> {
        let test = &detail.summary;
        let result = sqlx::query(
            "insert into account_model_tests
             (id, account_id, account_name, model_id, kind, prompt, started_at)
             values ($1, $2, $3, $4, $5, $6, $7)
             on conflict (account_id) where finished_at is null do nothing",
        )
        .bind(test.id)
        .bind(&test.account_id)
        .bind(&test.account_name)
        .bind(&test.model_id)
        .bind(test.kind.as_str())
        .bind(&detail.prompt)
        .bind(test.started_at)
        .execute(&self.pool)
        .await
        .map_err(model_test_sql_error)?;
        if result.rows_affected() == 0 {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Conflict,
                "model tests",
                "account already has a running test",
            ));
        }
        Ok(())
    }

    async fn finish_model_test(
        &self,
        id: Uuid,
        account_id: &str,
        outcome: AccountModelTestOutcome,
        keep: i64,
    ) -> AdminStoreResult<()> {
        let (error, text, image) = match outcome {
            AccountModelTestOutcome::Text(text) => (None, Some(text.into_bytes()), None),
            AccountModelTestOutcome::Image(image) => (None, None, Some(image)),
            AccountModelTestOutcome::Failed(error) => (Some(error), None, None),
        };
        let (image_type, image_bytes) = image.map_or((None, None), |image| {
            (Some(image.content_type), Some(image.bytes))
        });
        let mut tx = self.pool.begin().await.map_err(model_test_sql_error)?;
        // 同一账号的终态写入和淘汰串行化，记录与图片由同一个事务提交
        sqlx::query("select id from provider_accounts where id = $1 for no key update")
            .bind(account_id)
            .execute(&mut *tx)
            .await
            .map_err(model_test_sql_error)?;
        let update = sqlx::query(
            "update account_model_tests
             set finished_at = greatest(started_at, clock_timestamp()), error = $3,
                 result_text = $4, image_type = $5, image_bytes = $6
             where id = $1 and account_id = $2 and finished_at is null",
        )
        .bind(id)
        .bind(account_id)
        .bind(error)
        .bind(text)
        .bind(image_type)
        .bind(image_bytes)
        .execute(&mut *tx)
        .await
        .map_err(model_test_sql_error)?;
        if update.rows_affected() != 0 {
            sqlx::query(
                "delete from account_model_tests where id in (
                   select id from account_model_tests where account_id = $1 and finished_at is not null
                   order by started_at desc, id desc offset $2
                 )",
            )
            .bind(account_id)
            .bind(keep)
            .execute(&mut *tx)
            .await
            .map_err(model_test_sql_error)?;
        }
        tx.commit().await.map_err(model_test_sql_error)
    }

    async fn expired_model_tests(
        &self,
        account_id: Option<&str>,
        before: DateTime<Utc>,
    ) -> AdminStoreResult<Vec<(Uuid, String)>> {
        sqlx::query_as(
            "select id, account_id from account_model_tests
             where finished_at is null and started_at < $1 and ($2::text is null or account_id = $2)",
        )
        .bind(before)
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
        .map_err(model_test_sql_error)
    }

    async fn list_model_tests(
        &self,
        account_id: Option<&str>,
        offset: i64,
        limit: i64,
    ) -> AdminStoreResult<(Vec<AccountModelTest>, u64)> {
        let total: i64 = sqlx::query_scalar(
            "select count(*) from account_model_tests where $1::text is null or account_id = $1",
        )
        .bind(account_id)
        .fetch_one(&self.pool)
        .await
        .map_err(model_test_sql_error)?;
        let rows = sqlx::query(
            "select id, account_id, account_name, model_id, kind, started_at, finished_at,
                    error is not null as failed
             from account_model_tests where $1::text is null or account_id = $1
             order by started_at desc, id desc limit $2 offset $3",
        )
        .bind(account_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(model_test_sql_error)?;
        let tests = rows
            .iter()
            .map(model_test_summary)
            .collect::<AdminStoreResult<_>>()?;
        Ok((tests, total.unsigned_abs()))
    }

    async fn model_test_detail(
        &self,
        id: Uuid,
    ) -> AdminStoreResult<Option<AccountModelTestDetail>> {
        let row = sqlx::query(
            "select id, account_id, account_name, model_id, kind, started_at, finished_at,
                    error is not null as failed, error, prompt, result_text,
                    image_type is not null as has_image
             from account_model_tests where id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(model_test_sql_error)?;
        row.map(|row| {
            let summary = model_test_summary(&row)?;
            let text: Option<Vec<u8>> = row.try_get("result_text").map_err(model_test_sql_error)?;
            Ok(AccountModelTestDetail {
                summary,
                prompt: row.try_get("prompt").map_err(model_test_sql_error)?,
                error: row.try_get("error").map_err(model_test_sql_error)?,
                text: text.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
                has_image: row.try_get("has_image").map_err(model_test_sql_error)?,
            })
        })
        .transpose()
    }

    async fn model_test_image(&self, id: Uuid) -> AdminStoreResult<Option<(String, Vec<u8>)>> {
        sqlx::query_as(
            "select image_type, image_bytes from account_model_tests
             where id = $1 and image_type is not null",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(model_test_sql_error)
    }

    async fn load_account_record(
        &self,
        account_id: &CoreProviderAccountId,
    ) -> AdminStoreResult<Option<AccountRecord>> {
        self.accounts
            .load_provider_account(account_id.as_str())
            .await
            .map_err(|error| admin_store_error(ENTITY, error))?
            .map(|record| admin_account_record(record.summary))
            .transpose()
    }

    async fn account_exists(&self, account_id: &str) -> AdminStoreResult<bool> {
        sqlx::query_scalar("select exists(select 1 from provider_accounts where id = $1)")
            .bind(account_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|source| {
                admin_store_error(
                    ENTITY,
                    postgres_unavailable("check account existence", source),
                )
            })
    }

    async fn list_accounts(
        &self,
        query: AdminAccountListQuery,
        runtime: gateway_admin::model::accounts::AccountRuntimeSnapshot,
    ) -> AdminStoreResult<AccountPage> {
        if query.page == 0 {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                ENTITY,
                "page number must be positive",
            ));
        }
        let now = Utc::now();
        let cooldown = runtime.cooldown;
        let now_system_time = std::time::SystemTime::from(now);
        let active_rate_limited_ids = cooldown
            .iter()
            .filter(|(_, cooldown)| cooldown.is_active(now_system_time))
            .map(|(account_id, _)| account_id.clone())
            .collect::<Vec<_>>();
        let page = load_admin_account_page(
            &self.pool,
            &self.query_budget,
            &query,
            now,
            active_rate_limited_ids,
        )
        .await?;
        let item_ids = page
            .accounts
            .iter()
            .map(|account| account.id.clone())
            .collect::<Vec<_>>();
        let mut groups_by_account = self.account_groups_by_account(&item_ids).await?;
        let items = page
            .accounts
            .into_iter()
            .map(|summary| {
                let account_id = summary.id.clone();
                let projection = account_status_projection(
                    &summary,
                    now.into(),
                    cooldown.get(&account_id).copied(),
                );
                let mut account = admin_account_record(summary)?;
                account.groups = groups_by_account.remove(&account_id).unwrap_or_default();
                Ok(AccountPageItem {
                    protection_rules: runtime
                        .protection_rules
                        .get(&account.id)
                        .cloned()
                        .unwrap_or_default(),
                    capacity: account_capacity(
                        &account,
                        page.default_concurrency,
                        runtime.in_flight.as_ref(),
                    ),
                    account,
                    projection,
                })
            })
            .collect::<AdminStoreResult<Vec<_>>>()?;
        Ok(AccountPage {
            config_revision: page.config_revision,
            items,
            total: page.total,
            summary: page.summary,
        })
    }

    async fn load_account(
        &self,
        account_id: &str,
        runtime: gateway_admin::model::accounts::AccountRuntimeSnapshot,
    ) -> AdminStoreResult<Option<AccountPageItem>> {
        let record = self
            .accounts
            .load_provider_account(account_id)
            .await
            .map_err(|error| admin_store_error(ENTITY, error))?;
        let Some(record) = record else {
            return Ok(None);
        };
        let now = Utc::now();
        let cooldown = runtime.cooldown.get(account_id).copied();
        let projection = account_status_projection(&record.summary, now.into(), cooldown);
        let account_id = record.summary.id.clone();
        let mut groups = self
            .account_groups_by_account(std::slice::from_ref(&account_id))
            .await?;
        let mut account = admin_account_record(record.summary)?;
        account.groups = groups.remove(&account_id).unwrap_or_default();
        let default_concurrency: i64 = sqlx::query_scalar(
            "select max_concurrent_per_account from runtime_settings where id = 1",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|source| {
            admin_store_error(
                ENTITY,
                postgres_unavailable("load account concurrency", source),
            )
        })?;
        let default_concurrency = u64::try_from(default_concurrency).map_err(|_| {
            AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                ENTITY,
                "invalid default account concurrency",
            )
        })?;
        Ok(Some(AccountPageItem {
            protection_rules: runtime
                .protection_rules
                .get(&account_id)
                .cloned()
                .unwrap_or_default(),
            capacity: account_capacity(&account, default_concurrency, runtime.in_flight.as_ref()),
            account,
            projection,
        }))
    }

    async fn load_account_usage(
        &self,
        range: TimeRange,
        account_ids: &[String],
    ) -> AdminStoreResult<Vec<AccountUsage>> {
        let range = ObservabilityRange::new(range.start, range.end)
            .map_err(|error| admin_store_error(ENTITY, error))?;
        Ok(self
            .usage_observations(range, account_ids)
            .await?
            .into_iter()
            .map(admin_account_usage)
            .collect())
    }

    async fn load_account_usage_by_windows(
        &self,
        windows: &[AccountUsageWindowQuery],
    ) -> AdminStoreResult<Vec<AccountUsageWindowResult>> {
        self.usage_by_windows(windows).await
    }

    async fn load_quota_forecast_history(
        &self,
        window: &AccountUsageWindowQuery,
    ) -> AdminStoreResult<gateway_admin::model::quota_forecast_sampling::QuotaForecastHistory> {
        super::quota_forecast::load_history(&self.pool, &self.query_budget, window).await
    }

    async fn credential_details(
        &self,
        provider_kind: &ProviderKind,
        account_id: &CoreProviderAccountId,
    ) -> AdminStoreResult<Option<CredentialDetails>> {
        let (control_plane, account) = futures::try_join!(
            self.control_plane.load_control_plane(),
            self.accounts.load_provider_account(account_id.as_str()),
        )
        .map_err(|error| admin_store_error(ENTITY, error))?;
        account
            .filter(|record| record.summary.provider_kind == provider_kind.as_str())
            .map(|record| {
                Ok(CredentialDetails {
                    config_revision: admin_revision(control_plane.settings.config_revision)?,
                    credential: admin_account_record(record.summary)?,
                })
            })
            .transpose()
    }

    async fn credential_details_by_id(
        &self,
        account_id: &CoreProviderAccountId,
    ) -> AdminStoreResult<Option<CredentialDetails>> {
        let (control_plane, account) = futures::try_join!(
            self.control_plane.load_control_plane(),
            self.accounts.load_provider_account(account_id.as_str()),
        )
        .map_err(|error| admin_store_error(ENTITY, error))?;
        account
            .map(|record| {
                Ok(CredentialDetails {
                    config_revision: admin_revision(control_plane.settings.config_revision)?,
                    credential: admin_account_record(record.summary)?,
                })
            })
            .transpose()
    }

    async fn load_credentials_for_export(
        &self,
        provider_kind: &ProviderKind,
        account_ids: &[CoreProviderAccountId],
    ) -> AdminStoreResult<Vec<ProviderExportCredentialInput>> {
        let ids = account_ids
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>();
        validate_admin_account_ids(&ids).map_err(|error| admin_store_error(ENTITY, error))?;
        let mut credentials = Vec::with_capacity(account_ids.len());
        for account_id in account_ids {
            let record = self
                .accounts
                .load_provider_account(account_id.as_str())
                .await
                .map_err(|error| admin_store_error(ENTITY, error))?
                .ok_or_else(|| {
                    AdminStoreError::new(
                        AdminStoreErrorKind::NotFound,
                        ENTITY,
                        "one or more exported credentials do not exist",
                    )
                })?;
            if record.summary.provider_kind != provider_kind.as_str() {
                return Err(AdminStoreError::new(
                    AdminStoreErrorKind::NotFound,
                    ENTITY,
                    "one or more exported credentials belong to another Provider",
                ));
            }
            credentials.push(ProviderExportCredentialInput {
                account: admin_account_record(record.summary)?,
                provider_material: ProviderDocument::new(OpaqueProviderData::new(
                    record.provider_credentials_json.fields().clone(),
                )),
            });
        }
        Ok(credentials)
    }

    async fn commit_credential_import(
        &self,
        command: CredentialImportCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialImportResult> {
        let result = self
            .commit_prepared_import(
                command.prepared,
                command.settings,
                context,
                MutationAuditOperation::ProviderAccountImportDocument,
                command.outbound_proxy,
            )
            .await?;
        Ok(result)
    }

    async fn commit_authorization(
        &self,
        command: AuthorizationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<gateway_admin::model::provider_credentials::AuthorizationCommitResult>
    {
        self.commit_authorization_once(command, context).await
    }

    async fn authorization_receipt(
        &self,
        key: &gateway_admin::model::provider_credentials::AuthorizationReceiptKey,
    ) -> AdminStoreResult<Option<CredentialMutationResult>> {
        self.load_authorization_receipt(key).await
    }

    async fn commit_credential_rotation(
        &self,
        command: CredentialRotationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult> {
        self.commit_prepared_rotation(
            command.prepared,
            command.settings,
            context,
            MutationAuditOperation::ProviderAccountRotateCredential,
        )
        .await
    }

    async fn commit_credential_refresh(
        &self,
        command: CredentialRotationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult> {
        if command.settings.is_some() {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                ENTITY,
                "credential refresh cannot change account settings",
            ));
        }
        self.commit_prepared_rotation(
            command.prepared,
            None,
            context,
            MutationAuditOperation::ProviderAccountRefreshCredential,
        )
        .await
    }

    async fn update_account(
        &self,
        command: UpdateAccount,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountUpdateResult> {
        let account_id = CoreProviderAccountId::new(command.account_id.clone()).map_err(|_| {
            AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                ENTITY,
                "invalid provider account ID",
            )
        })?;
        let mut changed_fields = vec![
            "enabled".to_owned(),
            "concurrency_limit".to_owned(),
            "weight".to_owned(),
            "groups".to_owned(),
        ];
        if command.scheduling_role.is_some() {
            changed_fields.push("scheduling_role".to_owned());
        }
        if command.model_mappings.is_some() {
            changed_fields.push("model_mappings".to_owned());
        }
        if command.model_access.is_some() {
            changed_fields.push("model_access".to_owned());
        }
        if command.outbound_proxy.is_some() {
            changed_fields.push("outbound_proxy".to_owned());
        }
        if command.fast_mode.is_some() {
            changed_fields.push("fast_mode".to_owned());
        }
        if command.display_name.is_some() {
            changed_fields.push("display_name".to_owned());
        }
        if command.notes.is_some() {
            changed_fields.push("notes".to_owned());
        }
        let config_revision = self
            .accounts
            .batch_update_provider_accounts_admin(BatchUpdateProviderAccountsAdmin {
                account_ids: vec![command.account_id.clone()],
                pauses: Default::default(),
                quota_limits: None,
                display_name: command.display_name,
                fast_mode: command.fast_mode,
                notes: command.notes,
                allow_credits: None,
                enabled: Some(command.enabled),
                concurrency_limit: Some(command.concurrency_limit),
                weight: Some(command.weight),
                scheduling_role: command.scheduling_role,
                model_access: command.model_access,
                model_mappings: command.model_mappings,
                group_ids: Some(command.group_ids),
                outbound_proxy: command.outbound_proxy,
                audit: mutation_audit(
                    context,
                    MutationAuditOperation::ProviderAccountUpdate,
                    &command.account_id,
                    changed_fields,
                ),
            })
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
            .and_then(admin_revision)?;
        Ok(AccountUpdateResult {
            config_revision,
            account_id,
        })
    }

    async fn recover_account(
        &self,
        account_id: &CoreProviderAccountId,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountUpdateResult> {
        if let Some(cooldowns) = self.cooldowns.as_deref() {
            cooldowns.clear_all(account_id).await.map_err(|_| {
                AdminStoreError::new(
                    AdminStoreErrorKind::Unavailable,
                    ENTITY,
                    "provider account cooldown cleanup failed",
                )
            })?;
        }
        let config_revision = self
            .accounts
            .recover_provider_account_admin(RecoverProviderAccount {
                account_id: account_id.as_str().to_owned(),
                audit: mutation_audit(
                    context,
                    MutationAuditOperation::ProviderAccountRecover,
                    account_id.as_str(),
                    vec!["status".to_owned(), "quota".to_owned()],
                ),
            })
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
            .and_then(admin_revision)?;
        Ok(AccountUpdateResult {
            config_revision,
            account_id: account_id.clone(),
        })
    }

    async fn batch_update_accounts(
        &self,
        command: BatchUpdateAccounts,
        pauses: gateway_admin::model::accounts::PreparedQuotaPauses,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountsUpdateResult> {
        let account_ids = command
            .account_ids
            .iter()
            .map(|id| {
                CoreProviderAccountId::new(id.clone()).map_err(|_| {
                    AdminStoreError::new(
                        AdminStoreErrorKind::Invalid,
                        ENTITY,
                        "invalid provider account ID",
                    )
                })
            })
            .collect::<AdminStoreResult<Vec<_>>>()?;
        let audit_target = if command.account_ids.len() == 1 {
            command.account_ids[0].clone()
        } else {
            "provider_accounts".to_owned()
        };
        let mut changed_fields = Vec::new();
        for (changed, field) in [
            (command.quota_limits.is_some(), "quota_limits"),
            (command.enabled.is_some(), "enabled"),
            (command.allow_credits.is_some(), "allow_credits"),
            (command.concurrency_limit.is_some(), "concurrency_limit"),
            (command.weight.is_some(), "weight"),
            (command.group_ids.is_some(), "groups"),
        ] {
            if changed {
                changed_fields.push(field.to_owned());
            }
        }
        if command.scheduling_role.is_some() {
            changed_fields.push("scheduling_role".to_owned());
        }
        if command.model_mappings.is_some() {
            changed_fields.push("model_mappings".to_owned());
        }
        if command.model_access.is_some() {
            changed_fields.push("model_access".to_owned());
        }
        if command.outbound_proxy.is_some() {
            changed_fields.push("outbound_proxy".to_owned());
        }
        let config_revision = self
            .accounts
            .batch_update_provider_accounts_admin(BatchUpdateProviderAccountsAdmin {
                account_ids: command.account_ids,
                pauses,
                quota_limits: command.quota_limits,
                display_name: None,
                fast_mode: None,
                notes: None,
                allow_credits: command.allow_credits,
                enabled: command.enabled,
                concurrency_limit: command.concurrency_limit,
                weight: command.weight,
                scheduling_role: command.scheduling_role,
                model_access: command.model_access,
                model_mappings: command.model_mappings,
                group_ids: command.group_ids,
                outbound_proxy: command.outbound_proxy,
                audit: mutation_audit(
                    context,
                    MutationAuditOperation::ProviderAccountBatchUpdate,
                    &audit_target,
                    changed_fields,
                ),
            })
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
            .and_then(admin_revision)?;
        Ok(AccountsUpdateResult {
            config_revision,
            account_ids,
        })
    }

    async fn delete_accounts(
        &self,
        command: DeleteAccounts,
        context: &MutationContext,
    ) -> AdminStoreResult<AdminRevision> {
        let first_account_id = command.account_ids.first().ok_or_else(|| {
            AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                ENTITY,
                "account deletion requires at least one account ID",
            )
        })?;
        let scope = self.required_scope(first_account_id).await?;
        let audit_target = if command.account_ids.len() == 1 {
            first_account_id.clone()
        } else {
            "provider_accounts".to_owned()
        };
        self.accounts
            .delete_provider_accounts_admin(DeleteProviderAccounts {
                scope,
                account_ids: command.account_ids,
                audit: mutation_audit(
                    context,
                    MutationAuditOperation::ProviderAccountDelete,
                    &audit_target,
                    Vec::new(),
                ),
            })
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
            .and_then(admin_revision)
    }

    async fn record_credential_export(
        &self,
        account_ids: &[CoreProviderAccountId],
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        let ids = account_ids
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>();
        validate_admin_account_ids(&ids).map_err(|error| admin_store_error(ENTITY, error))?;
        for account_id in &ids {
            if self
                .accounts
                .load_provider_account(account_id)
                .await
                .map_err(|error| admin_store_error(ENTITY, error))?
                .is_none()
            {
                return Err(AdminStoreError::new(
                    AdminStoreErrorKind::NotFound,
                    ENTITY,
                    "one or more exported credentials do not exist",
                ));
            }
        }
        let control_plane = self
            .control_plane
            .load_control_plane()
            .await
            .map_err(|error| admin_store_error(ENTITY, error))?;
        let revision = control_plane.settings.config_revision;
        let mut transaction = self.accounts.pool.begin().await.map_err(|source| {
            admin_store_error(
                ENTITY,
                postgres_unavailable("begin credential export audit", source),
            )
        })?;
        let result = async {
            for account_id in &ids {
                append_admin_audit_event_in_transaction(
                    &mut transaction,
                    mutation_audit(
                        context,
                        MutationAuditOperation::ProviderAccountExportCredentials,
                        account_id,
                        Vec::new(),
                    ),
                    revision,
                )
                .await?;
            }
            Ok(())
        }
        .await;
        finish_admin_transaction(transaction, result, "credential export audit")
            .await
            .map_err(|error| admin_store_error(ENTITY, error))
    }
}

pub(super) fn prepare_import(
    prepared: PreparedCredentialImport,
    settings: Option<AccountImportSettings>,
    context: &MutationContext,
    action: MutationAuditOperation,
    outbound_proxy: Option<gateway_admin::model::proxies::ImportProxyBinding>,
) -> AdminStoreResult<ImportProviderAccounts> {
    let provider_kind = prepared.provider_kind.as_str().to_owned();
    let accounts = prepared
        .credentials
        .into_iter()
        .map(prepared_account)
        .collect::<StoreResult<Vec<_>>>()
        .map_err(|error| admin_store_error(ENTITY, error))?;
    let mut changed_fields = vec!["credentials".to_owned()];
    if settings
        .as_ref()
        .is_some_and(|settings| settings.model_access.is_some())
        || accounts
            .iter()
            .any(|account| account.model_access.is_some())
    {
        changed_fields.push("model_access".to_owned());
    }
    if let Some(settings) = &settings {
        changed_fields
            .extend(["enabled", "concurrency_limit", "weight", "group_ids"].map(str::to_owned));
        if settings.notes.is_some() {
            changed_fields.push("notes".to_owned());
        }
    }
    Ok(ImportProviderAccounts {
        settings,
        outbound_proxy,
        scope: ProviderAccountAdminScope {
            provider_kind: provider_kind.clone(),
        },
        accounts,
        audit: mutation_audit(context, action, &provider_kind, changed_fields),
    })
}

pub(super) fn import_result(
    imported: ProviderAccountAdminImport,
) -> AdminStoreResult<CredentialImportResult> {
    Ok(CredentialImportResult {
        config_revision: admin_revision(imported.config_revision)?,
        credential_ids: imported
            .account_ids
            .into_iter()
            .map(|id| {
                CoreProviderAccountId::new(id).map_err(|_| {
                    AdminStoreError::new(
                        AdminStoreErrorKind::Unavailable,
                        ENTITY,
                        "provider account import returned an invalid account ID",
                    )
                })
            })
            .collect::<AdminStoreResult<_>>()?,
    })
}

pub(super) fn authorization_import_result(
    imported: ProviderAccountAdminImport,
) -> AdminStoreResult<CredentialMutationResult> {
    let [id]: [String; 1] = imported.account_ids.try_into().map_err(|_| {
        AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            ENTITY,
            "authorization must return one account",
        )
    })?;
    let revision = imported.credential_revisions.get(&id).ok_or_else(|| {
        AdminStoreError::new(
            AdminStoreErrorKind::Unavailable,
            ENTITY,
            "authorization credential revision is missing",
        )
    })?;
    Ok(CredentialMutationResult {
        config_revision: admin_revision(imported.config_revision)?,
        account_id: CoreProviderAccountId::new(id).map_err(|_| {
            AdminStoreError::new(
                AdminStoreErrorKind::Unavailable,
                ENTITY,
                "authorization returned an invalid account ID",
            )
        })?,
        credential_revision: Some(admin_revision(*revision)?),
    })
}

pub(super) fn prepare_rotation(
    prepared: PreparedCredentialRotationFacts,
    settings: Option<UpdateAccount>,
    context: &MutationContext,
    action: MutationAuditOperation,
) -> AdminStoreResult<RotateProviderAccount> {
    let account_id = prepared.account_id.clone();
    let scope = ProviderAccountAdminScope {
        provider_kind: prepared.provider_kind.as_str().to_owned(),
    };
    let mut changed_fields = vec!["credentials".to_owned()];
    if let Some(settings) = &settings {
        changed_fields
            .extend(["enabled", "concurrency_limit", "weight", "groups"].map(str::to_owned));
        if settings.model_access.is_some() {
            changed_fields.push("model_access".to_owned());
        }
        if settings.outbound_proxy.is_some() {
            changed_fields.push("outbound_proxy".to_owned());
        }
        if settings.display_name.is_some() {
            changed_fields.push("display_name".to_owned());
        }
        if settings.notes.is_some() {
            changed_fields.push("notes".to_owned());
        }
    }
    Ok(RotateProviderAccount {
        settings,
        scope,
        profile: UpdateProviderAccount {
            id: account_id.as_str().to_owned(),
            name: prepared.name,
            email: prepared.email,
            plan_type: prepared.plan_type,
        },
        replacement_identity: prepared.replacement_identity,
        credential: ProviderCredentialUpdate {
            account_id: account_id.as_str().to_owned(),
            expected_revision: store_revision(prepared.expected_credential_revision)?,
            provider_credentials_json: provider_document_json(prepared.provider_material)
                .map_err(|error| admin_store_error(ENTITY, error))?,
            has_refresh_token: prepared.has_refresh_token,
            access_token_expires_at: prepared.access_token_expires_at,
            next_refresh_at: prepared.next_refresh_at,
            preserve_profile: prepared.preserve_profile,
            preserve_credential_state: prepared.preserve_credential_state,
        },
        audit: mutation_audit(context, action, account_id.as_str(), changed_fields),
    })
}

pub(super) fn rotation_result(
    rotation: ProviderAccountAdminRotation,
    account_id: CoreProviderAccountId,
) -> AdminStoreResult<CredentialMutationResult> {
    Ok(CredentialMutationResult {
        config_revision: admin_revision(rotation.config_revision)?,
        account_id,
        credential_revision: Some(admin_revision(rotation.credential_revision)?),
    })
}

fn model_test_sql_error(error: sqlx::Error) -> AdminStoreError {
    if error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_foreign_key_violation)
    {
        return AdminStoreError::new(
            AdminStoreErrorKind::NotFound,
            "model tests",
            "account removed",
        );
    }
    admin_store_error(
        "model tests",
        postgres_unavailable("account model tests", error),
    )
}

fn model_test_summary(row: &sqlx::postgres::PgRow) -> AdminStoreResult<AccountModelTest> {
    let kind = match row.try_get("kind").map_err(model_test_sql_error)? {
        "text" => AccountModelTestKind::Text,
        "image" => AccountModelTestKind::Image,
        "animation" => AccountModelTestKind::Animation,
        _ => {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Invalid,
                "model tests",
                "unknown test kind",
            ));
        }
    };
    Ok(AccountModelTest {
        id: row.try_get("id").map_err(model_test_sql_error)?,
        account_id: row.try_get("account_id").map_err(model_test_sql_error)?,
        account_name: row.try_get("account_name").map_err(model_test_sql_error)?,
        model_id: row.try_get("model_id").map_err(model_test_sql_error)?,
        kind,
        started_at: row.try_get("started_at").map_err(model_test_sql_error)?,
        finished_at: row.try_get("finished_at").map_err(model_test_sql_error)?,
        failed: row.try_get("failed").map_err(model_test_sql_error)?,
    })
}

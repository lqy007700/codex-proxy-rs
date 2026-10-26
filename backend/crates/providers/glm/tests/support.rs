//! 智谱协议测试使用的内存账号、租约、冷却与冻结请求范围

use async_trait::async_trait;
use futures::future::BoxFuture;
use gateway_core::policy::ClientApiKeyId;
use gateway_core::{
    account::*,
    error::{StoreError, StoreErrorKind},
    provider_ports::*,
    routing::ProviderKind,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
pub struct Store {
    pub rows: Mutex<BTreeMap<ProviderAccountId, LoadedCredential>>,
    pub in_flight: Arc<AtomicUsize>,
    pub busy: AtomicBool,
    pub cooldowns: Mutex<BTreeMap<ProviderAccountId, ProviderCooldown>>,
}
impl Store {
    pub fn add(&self, id: &str, provider: &str, base: &str, model: &str) {
        let account = ProviderAccount::new(
            ProviderAccountId::new(id).unwrap(),
            ProviderKind::new(provider).unwrap(),
            id.to_owned(),
            None,
            "api_key".into(),
            CredentialRevision::new(1).unwrap(),
            None,
        )
        .with_account_facts(
            true,
            CredentialState::Ready,
            QuotaState::unknown(),
            None,
            None,
        );
        let credential=PlaintextCredential::new(serde_json::json!({"api_key":"fixture-key","base_url":base,"transport":"http","models":[model]}).as_object().unwrap().clone());
        self.rows.lock().unwrap().insert(
            account.id().clone(),
            LoadedCredential {
                account,
                credential,
            },
        );
    }
}

#[async_trait]
impl ProviderAccountStore for Store {
    async fn create_account(&self, _account: NewProviderAccount) -> Result<(), StoreError> {
        Err(StoreError::new(StoreErrorKind::Unavailable))
    }
    async fn get_account(
        &self,
        account: &ProviderAccountId,
    ) -> Result<Option<ProviderAccount>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .get(account)
            .map(|r| r.account.clone()))
    }
    async fn list_accounts(&self) -> Result<Vec<ProviderAccount>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .values()
            .map(|r| r.account.clone())
            .collect())
    }
    async fn list_for_provider(
        &self,
        provider: &ProviderKind,
    ) -> Result<Vec<ProviderAccount>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.account.provider() == provider)
            .map(|r| r.account.clone())
            .collect())
    }
    async fn list_refresh_candidates(
        &self,
        _query: ProviderRefreshQuery,
    ) -> Result<Vec<LoadedCredential>, StoreError> {
        Err(StoreError::new(StoreErrorKind::Unavailable))
    }
    async fn load_credential(
        &self,
        account: &ProviderAccountId,
        expected_revision: CredentialRevision,
    ) -> Result<LoadedCredential, StoreError> {
        self.rows
            .lock()
            .unwrap()
            .get(account)
            .filter(|r| r.account.revision() == expected_revision)
            .cloned()
            .ok_or_else(|| StoreError::new(StoreErrorKind::Conflict))
    }
    async fn load_current_credential(
        &self,
        account: &ProviderAccountId,
    ) -> Result<LoadedCredential, StoreError> {
        self.rows
            .lock()
            .unwrap()
            .get(account)
            .cloned()
            .ok_or_else(|| StoreError::new(StoreErrorKind::InvalidData))
    }
    async fn compare_and_swap_credential(
        &self,
        _update: CredentialCasUpdate,
    ) -> Result<CredentialCasOutcome, StoreError> {
        Err(StoreError::new(StoreErrorKind::Unavailable))
    }
    async fn get_quotas(
        &self,
        _accounts: &[ProviderAccountId],
    ) -> Result<Vec<QuotaObservation>, StoreError> {
        Err(StoreError::new(StoreErrorKind::Unavailable))
    }
    async fn compare_and_swap_quota(
        &self,
        _observation: QuotaObservation,
    ) -> Result<QuotaWriteOutcome, StoreError> {
        Err(StoreError::new(StoreErrorKind::Unavailable))
    }
    async fn touch_quota_observation(
        &self,
        _touch: QuotaObservationTouch,
    ) -> Result<QuotaWriteOutcome, StoreError> {
        Err(StoreError::new(StoreErrorKind::Unavailable))
    }
    async fn apply_quota_access(
        &self,
        _change: QuotaAccessChange,
    ) -> Result<QuotaWriteOutcome, StoreError> {
        Err(StoreError::new(StoreErrorKind::Unavailable))
    }
    async fn apply_state_change(&self, change: AccountStateChange) -> Result<(), StoreError> {
        let mut rows = self.rows.lock().unwrap();
        let row = rows
            .get_mut(&change.account_id)
            .filter(|row| row.account.revision() == change.expected_revision)
            .ok_or_else(|| StoreError::new(StoreErrorKind::Conflict))?;
        row.account = row.account.clone().with_account_facts(
            row.account.enabled(),
            change.credential_state,
            row.account.quota(),
            change.error_reason,
            change.message,
        );
        Ok(())
    }
    async fn update_account(&self, _update: ProviderAccountUpdate) -> Result<(), StoreError> {
        Err(StoreError::new(StoreErrorKind::Unavailable))
    }
    async fn set_enabled(
        &self,
        _account: &ProviderAccountId,
        _enabled: bool,
    ) -> Result<(), StoreError> {
        Err(StoreError::new(StoreErrorKind::Unavailable))
    }
    async fn delete_account(&self, _account: &ProviderAccountId) -> Result<(), StoreError> {
        Err(StoreError::new(StoreErrorKind::Unavailable))
    }
}
struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl ProviderLeasePort for Store {
    fn load_state<'a>(
        &'a self,
        _client_api_key_id: &'a ClientApiKeyId,
        _provider_kind: &'a ProviderKind,
        accounts: &'a [ProviderAccountId],
        _pool: ProviderConcurrencyPool,
    ) -> BoxFuture<'a, Result<ProviderSchedulingState, ProviderStoreError>> {
        Box::pin(async move {
            Ok(ProviderSchedulingState::new(
                accounts
                    .iter()
                    .map(|id| {
                        (
                            id.clone(),
                            AccountRuntimeSignals {
                                in_flight: 0,
                                last_started_at: None,
                                quota_reset_at: None,
                                quota_remaining_rank: None,
                                cooldown: None,
                                failure_rate_basis_points: None,
                                first_output_latency_ms: None,
                            },
                        )
                    })
                    .collect(),
                0,
            ))
        })
    }
    fn try_acquire(
        &self,
        _request: ProviderLeaseRequest,
    ) -> BoxFuture<'_, Result<ProviderLeaseAcquisition, ProviderStoreError>> {
        Box::pin(async move {
            if self.busy.load(Ordering::SeqCst) {
                Ok(ProviderLeaseAcquisition::Busy { retry_after: None })
            } else {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                Ok(ProviderLeaseAcquisition::Acquired(Box::new(Guard(
                    self.in_flight.clone(),
                ))))
            }
        })
    }
}
impl ProviderCooldownPort for Store {
    fn put_if_later(
        &self,
        cooldown: ProviderCooldown,
    ) -> BoxFuture<'_, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            self.cooldowns
                .lock()
                .unwrap()
                .insert(cooldown.account_id().clone(), cooldown);
            Ok(true)
        })
    }
    fn read<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
    ) -> BoxFuture<'a, Result<Option<ProviderCooldown>, ProviderStoreError>> {
        Box::pin(async move { Ok(self.cooldowns.lock().unwrap().get(account_id).cloned()) })
    }
    fn clear<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
        _through_revision: CredentialRevision,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move { Ok(self.cooldowns.lock().unwrap().remove(account_id).is_some()) })
    }
    fn put_scoped_if_later(
        &self,
        _cooldown: ProviderScopedCooldown,
    ) -> BoxFuture<'_, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "fixture",
            ))
        })
    }
    fn read_scoped<'a>(
        &'a self,
        _account_id: &'a ProviderAccountId,
        _scope: &'a ProviderCooldownScope,
    ) -> BoxFuture<'a, Result<Option<ProviderScopedCooldown>, ProviderStoreError>> {
        Box::pin(async move {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "fixture",
            ))
        })
    }
    fn clear_scoped<'a>(
        &'a self,
        _account_id: &'a ProviderAccountId,
        _scope: &'a ProviderCooldownScope,
        _through_revision: CredentialRevision,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "fixture",
            ))
        })
    }
    fn clear_all<'a>(
        &'a self,
        account_id: &'a ProviderAccountId,
    ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
        Box::pin(async move { Ok(self.cooldowns.lock().unwrap().remove(account_id).is_some()) })
    }
    fn record_capacity_failure<'a>(
        &'a self,
        _account_id: &'a ProviderAccountId,
        _window: Duration,
        _in_flight: u32,
    ) -> BoxFuture<'a, Result<u32, ProviderStoreError>> {
        Box::pin(async move {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "fixture",
            ))
        })
    }
    fn clear_after_success<'a>(
        &'a self,
        _account_id: &'a ProviderAccountId,
        _through_revision: CredentialRevision,
    ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
        Box::pin(async move {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "fixture",
            ))
        })
    }
    fn capacity_peak_in_flight<'a>(
        &'a self,
        _account_id: &'a ProviderAccountId,
    ) -> BoxFuture<'a, Result<Option<u32>, ProviderStoreError>> {
        Box::pin(async move {
            Err(ProviderStoreError::new(
                ProviderStoreErrorKind::Unavailable,
                "fixture",
            ))
        })
    }
}

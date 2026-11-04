//! 验证亲和管理精确预览、部分结果、幂等重试与审计故障恢复

use async_trait::async_trait;
use chrono::{Duration, Utc};
use gateway_admin::{
    AffinityService,
    model::{AdminError, MutationActor, MutationContext, Revision, affinity::*},
    ports::affinity::*,
};
use gateway_core::{
    policy::CallerIdentity,
    provider_ports::{ProviderSessionObservation, SessionBindingDirective},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[derive(Default)]
struct Stores {
    sessions: Mutex<BTreeMap<String, AffinitySession>>,
    operations: Mutex<BTreeMap<String, AffinityOperation>>,
    receipts: Mutex<BTreeMap<String, AffinityItemResult>>,
    unavailable: AtomicBool,
    eligible: AtomicBool,
    fail_audit: AtomicBool,
    mutations: AtomicUsize,
}
fn unavailable() -> AdminError {
    AdminError::internal("synthetic unavailable")
}
#[async_trait]
impl AffinityBindingStore for Stores {
    async fn list(&self, _: &AffinityQuery) -> Result<AffinityPage, AdminError> {
        Ok(AffinityPage {
            items: self.sessions.lock().unwrap().values().cloned().collect(),
            next_cursor: None,
            coverage: "observedOnly",
        })
    }
    async fn session(
        &self,
        provider: &str,
        reference: &str,
    ) -> Result<Option<AffinitySession>, AdminError> {
        Ok(self
            .sessions
            .lock()
            .unwrap()
            .get(&format!("{provider}:{reference}"))
            .cloned())
    }
    async fn receipt(
        &self,
        plan: &AffinityPlan,
        item: &AffinityPlanItem,
    ) -> Result<Option<AffinityItemResult>, AdminError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(unavailable());
        }
        Ok(self
            .receipts
            .lock()
            .unwrap()
            .get(&format!("{}:{}", plan.operation_id, item.key()))
            .cloned())
    }
    async fn apply(
        &self,
        plan: &AffinityPlan,
        item: &AffinityPlanItem,
        rejection: Option<&str>,
    ) -> Result<AffinityItemResult, AdminError> {
        if let Some(receipt) = self.receipt(plan, item).await? {
            return Ok(receipt);
        }
        let status = if plan.expires_at <= Utc::now() || rejection.is_some() {
            "rejected"
        } else if item.session.reference == "conflict" {
            "conflict"
        } else {
            self.mutations.fetch_add(1, Ordering::SeqCst);
            "applied"
        };
        let result = AffinityItemResult {
            provider: item.session.provider.clone(),
            reference: item.session.reference.clone(),
            status: status.into(),
            message: status.into(),
            audit_recorded: false,
        };
        self.receipts.lock().unwrap().insert(
            format!("{}:{}", plan.operation_id, item.key()),
            result.clone(),
        );
        Ok(result)
    }
}
#[async_trait]
impl AffinityJournalStore for Stores {
    async fn sources(&self) -> Result<Vec<IdentitySource>, AdminError> {
        Ok(vec![])
    }
    async fn save_source(
        &self,
        _: &MutationContext,
        _: IdentitySource,
    ) -> Result<Revision, AdminError> {
        Revision::new(1).map_err(|_| unavailable())
    }
    async fn eligible_target(
        &self,
        _: &AffinitySession,
        _: Option<&str>,
    ) -> Result<bool, AdminError> {
        Ok(self.eligible.load(Ordering::SeqCst))
    }
    async fn create_plan(
        &self,
        _: &MutationContext,
        plan: AffinityPlan,
    ) -> Result<AffinityOperation, AdminError> {
        let operation = AffinityOperation {
            plan,
            results: BTreeMap::new(),
        };
        self.operations
            .lock()
            .unwrap()
            .insert(operation.plan.operation_id.clone(), operation.clone());
        Ok(operation)
    }
    async fn operation(&self, id: &str) -> Result<Option<AffinityOperation>, AdminError> {
        Ok(self.operations.lock().unwrap().get(id).cloned())
    }
    async fn history(&self) -> Result<Vec<AffinityOperation>, AdminError> {
        Ok(self.operations.lock().unwrap().values().cloned().collect())
    }
    async fn record_result(
        &self,
        _: &MutationContext,
        id: &str,
        result: &AffinityItemResult,
    ) -> Result<AffinityItemResult, AdminError> {
        if self.fail_audit.load(Ordering::SeqCst) {
            return Err(unavailable());
        }
        let mut operations = self.operations.lock().unwrap();
        let operation = operations.get_mut(id).unwrap();
        let key = result.key();
        let mut result = operation
            .results
            .get(&key)
            .filter(|existing| existing.status != "failed")
            .cloned()
            .unwrap_or_else(|| result.clone());
        result.audit_recorded = true;
        operation.results.insert(key, result.clone());
        Ok(result)
    }
}
fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "affinity-test".into(),
    }
}
fn session(provider: &str, reference: &str) -> AffinitySession {
    AffinitySession {
        reference: reference.into(),
        provider: provider.into(),
        observation: ProviderSessionObservation {
            binding_scope: Default::default(),
            caller: CallerIdentity::Direct {
                key_id: "key_direct".into(),
            },
            client_key_id: "key_direct".into(),
            request_id: "req_a".into(),
            actual_account_id: "acct_a".into(),
        },
        account_id: "acct_a".into(),
        revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        directive: None,
        first_observed_at: Utc::now(),
        last_admission_at: Utc::now(),
        expires_at: Utc::now() + Duration::hours(1),
    }
}
fn fixture() -> (Arc<Stores>, AffinityService) {
    let stores = Arc::new(Stores::default());
    stores.eligible.store(true, Ordering::SeqCst);
    for session in [
        session("openai", "same"),
        session("other", "same"),
        session("openai", "conflict"),
    ] {
        stores.sessions.lock().unwrap().insert(
            format!("{}:{}", session.provider, session.reference),
            session,
        );
    }
    let service = AffinityService::new(
        AffinityStorePorts {
            bindings: stores.clone(),
            journal: stores.clone(),
        },
        Arc::new(super::NoopSnapshot),
    );
    (stores, service)
}
async fn preview(
    service: &AffinityService,
    references: &[(&str, &str)],
    directive: SessionBindingDirective,
) -> AffinityOperation {
    service
        .preview(
            &context(),
            AffinityPreviewRequest {
                operation_id: uuid::Uuid::new_v4().to_string(),
                directive,
                selection: references
                    .iter()
                    .map(|(provider, reference)| AffinitySelection {
                        provider: (*provider).into(),
                        reference: (*reference).into(),
                        revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                    })
                    .collect(),
            },
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn exact_preview_partial_results_and_retries_keep_provider_namespaces_separate() {
    let (stores, service) = fixture();
    let operation = preview(
        &service,
        &[
            ("openai", "same"),
            ("other", "same"),
            ("openai", "conflict"),
        ],
        SessionBindingDirective::Clear,
    )
    .await;
    assert_eq!(stores.mutations.load(Ordering::SeqCst), 0);
    let result = service
        .apply(&context(), &operation.plan.operation_id)
        .await
        .unwrap();
    assert_eq!(result.results.len(), 3);
    assert_eq!(result.results["openai:same"].status, "applied");
    assert_eq!(result.results["other:same"].status, "applied");
    assert_eq!(result.results["openai:conflict"].status, "conflict");
    service
        .apply(&context(), &operation.plan.operation_id)
        .await
        .unwrap();
    assert_eq!(stores.mutations.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn receipt_recovers_applied_change_after_audit_failure_expiry_and_eligibility_revocation() {
    let (stores, service) = fixture();
    let operation = preview(
        &service,
        &[("openai", "same")],
        SessionBindingDirective::Reassign,
    )
    .await;
    stores.fail_audit.store(true, Ordering::SeqCst);
    let first = service
        .apply(&context(), &operation.plan.operation_id)
        .await
        .unwrap();
    assert_eq!(first.results["openai:same"].status, "applied");
    assert!(!first.results["openai:same"].audit_recorded);
    stores.eligible.store(false, Ordering::SeqCst);
    stores.fail_audit.store(false, Ordering::SeqCst);
    stores
        .operations
        .lock()
        .unwrap()
        .get_mut(&operation.plan.operation_id)
        .unwrap()
        .plan
        .expires_at = Utc::now() - Duration::seconds(1);
    let recovered = service
        .apply(&context(), &operation.plan.operation_id)
        .await
        .unwrap();
    assert_eq!(recovered.results["openai:same"].status, "applied");
    assert!(recovered.results["openai:same"].audit_recorded);
    assert_eq!(stores.mutations.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn expired_preview_never_mutates_and_redis_outage_is_a_retryable_item_result() {
    let (stores, service) = fixture();
    let operation = preview(
        &service,
        &[("openai", "same")],
        SessionBindingDirective::Clear,
    )
    .await;
    stores.unavailable.store(true, Ordering::SeqCst);
    let result = service
        .apply(&context(), &operation.plan.operation_id)
        .await
        .unwrap();
    assert_eq!(result.results["openai:same"].status, "failed");
    stores.unavailable.store(false, Ordering::SeqCst);
    stores
        .operations
        .lock()
        .unwrap()
        .get_mut(&operation.plan.operation_id)
        .unwrap()
        .plan
        .expires_at = Utc::now() - Duration::seconds(1);
    // 清除故障结果模拟持久层允许 failed 被后续终态更新
    stores
        .operations
        .lock()
        .unwrap()
        .get_mut(&operation.plan.operation_id)
        .unwrap()
        .results
        .clear();
    let result = service
        .apply(&context(), &operation.plan.operation_id)
        .await
        .unwrap();
    assert_eq!(result.results["openai:same"].status, "rejected");
    assert_eq!(stores.mutations.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn preview_rejects_stale_and_duplicate_selections_and_validates_target() {
    let (stores, service) = fixture();
    for selection in [
        vec![AffinitySelection {
            provider: "openai".into(),
            reference: "same".into(),
            revision: "old".into(),
        }],
        vec![
            AffinitySelection {
                provider: "openai".into(),
                reference: "same".into(),
                revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()
            };
            2
        ],
        vec![],
    ] {
        assert!(
            service
                .preview(
                    &context(),
                    AffinityPreviewRequest {
                        operation_id: uuid::Uuid::new_v4().to_string(),
                        selection,
                        directive: SessionBindingDirective::Clear
                    }
                )
                .await
                .is_err()
        );
    }
    stores.eligible.store(false, Ordering::SeqCst);
    let operation = preview(
        &service,
        &[("openai", "same")],
        SessionBindingDirective::Target {
            account_id: "acct_outside_scope".into(),
        },
    )
    .await;
    assert!(operation.plan.items[0].rejection.is_some());
    assert_eq!(
        service
            .apply(&context(), &operation.plan.operation_id)
            .await
            .unwrap()
            .results["openai:same"]
            .status,
        "rejected"
    );
    assert_eq!(stores.mutations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_management_inputs_fail_before_store_access() {
    let (_, service) = fixture();
    for query in [
        AffinityQuery {
            mode: Some("unknown".into()),
            ..Default::default()
        },
        AffinityQuery {
            status: Some("unknown".into()),
            ..Default::default()
        },
        AffinityQuery {
            reference: Some("invalid:reference".into()),
            ..Default::default()
        },
    ] {
        assert!(service.list(&query).await.is_err());
    }
    assert!(
        service
            .apply(&context(), "invalid-operation")
            .await
            .is_err()
    );
    let selection = vec![AffinitySelection {
        provider: "openai".into(),
        reference: "same".into(),
        revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
    }];
    assert!(
        service
            .preview(
                &context(),
                AffinityPreviewRequest {
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    selection,
                    directive: SessionBindingDirective::Target {
                        account_id: "".into()
                    }
                }
            )
            .await
            .is_err()
    );
}

#[test]
fn scope_filter_distinguishes_token_primary_and_legacy_session_observations() {
    use gateway_core::provider_ports::ProviderAffinityScope;
    let mut item = session("openai", "scope-filter");
    let mut legacy = serde_json::to_value(&item.observation).unwrap();
    legacy.as_object_mut().unwrap().remove("bindingScope");
    item.observation = serde_json::from_value(legacy).unwrap();
    let query = |scope| AffinityQuery {
        binding_scope: Some(scope),
        ..Default::default()
    };
    assert!(query(ProviderAffinityScope::Session).matches(&item));
    assert!(!query(ProviderAffinityScope::Token).matches(&item));
    item.observation.binding_scope = ProviderAffinityScope::Token;
    assert!(query(ProviderAffinityScope::Token).matches(&item));
    assert!(!query(ProviderAffinityScope::Session).matches(&item));
    assert!(AffinityQuery::default().matches(&item));
}

#[tokio::test]
async fn newapi_session_operations_are_rejected_but_token_primary_can_be_managed() {
    use gateway_core::provider_ports::ProviderAffinityScope;
    let (stores, service) = fixture();
    for scope in [ProviderAffinityScope::Session, ProviderAffinityScope::Token] {
        {
            let mut sessions = stores.sessions.lock().unwrap();
            let item = sessions.get_mut("openai:same").unwrap();
            item.observation.caller = CallerIdentity::NewApi {
                source_id: "source".into(),
                user_id: "1".into(),
                token_id: "2".into(),
            };
            item.observation.binding_scope = scope;
        }
        let op = preview(
            &service,
            &[("openai", "same")],
            SessionBindingDirective::Clear,
        )
        .await;
        assert_eq!(
            op.plan.items[0].rejection.is_some(),
            scope == ProviderAffinityScope::Session
        );
        let result = service
            .apply(&context(), &op.plan.operation_id)
            .await
            .unwrap();
        assert_eq!(
            result.results["openai:same"].status,
            if scope == ProviderAffinityScope::Session {
                "rejected"
            } else {
                "applied"
            }
        );
    }
    assert_eq!(stores.mutations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn token_status_filters_separate_recent_routing_from_pending_changes() {
    use gateway_core::provider_ports::ProviderAffinityScope;
    let (_, service) = fixture();
    let query = |value: &str| AffinityQuery {
        status: Some(value.into()),
        ..Default::default()
    };
    let mut item = session("openai", "routing-status");
    item.observation.binding_scope = ProviderAffinityScope::Token;
    assert!(query("primary").matches(&item));
    assert!(!query("spillover").matches(&item));
    service.list(&query("primary")).await.unwrap();
    service.list(&query("spillover")).await.unwrap();
    item.observation.actual_account_id = "acct_b".into();
    assert!(query("spillover").matches(&item));
    assert!(query("bound").matches(&item));
    assert!(!query("primary").matches(&item));
    item.directive = Some(SessionBindingDirective::Reassign);
    assert!(query("pending").matches(&item));
    assert!(!query("spillover").matches(&item));
    item.directive = None;
    item.observation.binding_scope = ProviderAffinityScope::Session;
    assert!(!query("spillover").matches(&item));
}

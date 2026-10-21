//! 验证插件调度策略对软亲和的覆盖与宿主候选范围限制

use std::{collections::BTreeSet, num::NonZeroU32, sync::Arc, time::SystemTime};

use futures::{executor::block_on, future::BoxFuture};
use gateway_core::{
    account::{
        AccountCandidate, AccountEligibilityPolicy, AccountRuntimeSignals, AccountSelectionContext,
        AccountSelectionPolicy, AccountWeight, CredentialRevision, CredentialState,
        PreferredAccountSelection, ProviderAccount, ProviderAccountId, QuotaState,
        RotationStrategy,
    },
    engine::{
        ModelRequestId,
        policy::{
            AccountPolicyError, AccountScheduleDecision, AccountScheduleInput, ModelRouteDecision,
            ModelRouteInput, RequestPolicyContext, RequestPolicyFault, RequestPolicyPlan,
        },
    },
    policy::ClientApiKeyId,
    routing::ProviderKind,
    routing::extensions::{ExtensionSetId, ExtensionSetLease, ExtensionSetReference},
};

#[derive(Debug)]
struct FixedScheduler(AccountScheduleDecision);

impl RequestPolicyPlan for FixedScheduler {
    fn route_model(
        &self,
        _: ModelRouteInput,
    ) -> BoxFuture<'static, Result<ModelRouteDecision, RequestPolicyFault>> {
        Box::pin(async { Ok(ModelRouteDecision::Unhandled) })
    }

    fn schedule_account(
        &self,
        _: AccountScheduleInput,
    ) -> BoxFuture<'static, Result<AccountScheduleDecision, RequestPolicyFault>> {
        let decision = self.0.clone();
        Box::pin(async move { Ok(decision) })
    }
}

struct Lease;
impl ExtensionSetLease for Lease {
    fn is_ready(&self) -> bool {
        true
    }
}

fn account_id(id: &str) -> ProviderAccountId {
    ProviderAccountId::new(id.to_owned()).unwrap()
}

fn candidate(id: &str) -> AccountCandidate {
    AccountCandidate {
        account: ProviderAccount::new(
            account_id(id),
            ProviderKind::new("openai").unwrap(),
            id.into(),
            None,
            "test".into(),
            CredentialRevision::new(1).unwrap(),
            None,
        )
        .with_account_facts(
            true,
            CredentialState::Ready,
            QuotaState::unknown(),
            None,
            None,
        )
        .with_scheduling(None, AccountWeight::new(100).unwrap()),
        signals: AccountRuntimeSignals {
            in_flight: 0,
            last_started_at: None,
            quota_reset_at: None,
            quota_remaining_rank: None,
            cooldown: None,
            failure_rate_basis_points: None,
            first_output_latency_ms: None,
        },
    }
}

fn policy(choice: Option<&'static str>) -> RequestPolicyContext {
    policy_decision(choice.map_or(AccountScheduleDecision::Delegate, |id| {
        AccountScheduleDecision::Pick(account_id(id))
    }))
}

fn policy_decision(decision: AccountScheduleDecision) -> RequestPolicyContext {
    RequestPolicyContext::new(
        Arc::new(FixedScheduler(decision)),
        ExtensionSetReference::new(
            ExtensionSetId::new("scheduler-test".into()).unwrap(),
            Arc::new(Lease),
        ),
        ModelRequestId::new("req_scheduler").unwrap(),
        ClientApiKeyId::new("scheduler-key").unwrap(),
        vec![],
    )
}

fn context() -> AccountSelectionContext {
    AccountSelectionContext {
        policy: AccountSelectionPolicy::new(RotationStrategy::Sticky, NonZeroU32::new(2).unwrap()),
        now: SystemTime::now(),
        excluded_accounts: BTreeSet::new(),
        busy_accounts: Default::default(),
        preferred_account: Some(account_id("acct_a")),
        preferred_account_overrides_weight: true,
        round_robin_cursor: 0,
        eligibility: AccountEligibilityPolicy::Enforce,
        account_scope: None,
        reserved_concurrency: 0,
    }
}

#[test]
fn scheduler_can_override_soft_affinity_and_reports_the_override() {
    block_on(async {
        let candidates = [candidate("acct_a"), candidate("acct_b")];
        let selected = policy(Some("acct_b"))
            .select_account(
                NonZeroU32::new(1).unwrap(),
                &ProviderKind::new("openai").unwrap(),
                Some("demo-model"),
                &candidates,
                &mut context(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                selected.candidate().account.id().as_str(),
                selected.preferred()
            ),
            ("acct_b", PreferredAccountSelection::OverriddenByPolicy)
        );
    });
}

#[test]
fn delegated_scheduler_keeps_builtin_affinity() {
    block_on(async {
        let candidates = [candidate("acct_a"), candidate("acct_b")];
        let selected = policy(None)
            .select_account(
                NonZeroU32::new(1).unwrap(),
                &ProviderKind::new("openai").unwrap(),
                None,
                &candidates,
                &mut context(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                selected.candidate().account.id().as_str(),
                selected.preferred()
            ),
            ("acct_a", PreferredAccountSelection::Hit)
        );
    });
}

#[test]
fn explicit_plugin_choices_override_native_soft_affinity() {
    block_on(async {
        let mut candidates = [candidate("acct_a"), candidate("acct_b")];
        candidates[0].account = candidates[0]
            .account
            .clone()
            .with_scheduling(None, AccountWeight::new(10).unwrap());
        let mut selection_context = context();
        selection_context.policy =
            AccountSelectionPolicy::new(RotationStrategy::Smart, NonZeroU32::new(2).unwrap())
                .with_smart_scheduling(
                    gateway_core::account::SmartSchedulingConfig::new([
                        1.0, 0.8, 1.0, 0.5, 0.0, 0.0,
                    ])
                    .unwrap(),
                );
        for chosen in [Some("acct_a"), Some("acct_b"), None] {
            let selected = policy(chosen)
                .select_account(
                    NonZeroU32::new(1).unwrap(),
                    &ProviderKind::new("openai").unwrap(),
                    None,
                    &candidates,
                    &mut selection_context,
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                selected.candidate().account.id().as_str(),
                chosen.unwrap_or("acct_a")
            );
            let expected = match chosen {
                Some("acct_a") => PreferredAccountSelection::Hit,
                Some(_) => PreferredAccountSelection::OverriddenByPolicy,
                None => PreferredAccountSelection::Hit,
            };
            assert_eq!(selected.preferred(), expected);
        }
    });
}

#[test]
fn scheduler_cannot_select_outside_the_hard_bound_candidate_pool() {
    block_on(async {
        let candidates = [candidate("acct_a"), candidate("acct_b")];
        let mut context = context();
        context.excluded_accounts.insert(account_id("acct_b"));
        let result = policy(Some("acct_b"))
            .select_account(
                NonZeroU32::new(1).unwrap(),
                &ProviderKind::new("openai").unwrap(),
                None,
                &candidates,
                &mut context,
            )
            .await;
        assert!(matches!(result, Err(AccountPolicyError::Fault)));
    });
}

#[test]
fn filtered_delegation_preserves_native_weights_and_affinity_on_remaining_accounts() {
    block_on(async {
        let mut candidates = [
            candidate("acct_a"),
            candidate("acct_b"),
            candidate("acct_c"),
        ];
        candidates[1].account = candidates[1]
            .account
            .clone()
            .with_scheduling(None, AccountWeight::new(10).unwrap());
        let policy = policy_decision(AccountScheduleDecision::DelegateFiltered(BTreeSet::from([
            account_id("acct_a"),
        ])));
        for strategy in [RotationStrategy::Smart, RotationStrategy::RoundRobin] {
            let mut context = context();
            context.policy = AccountSelectionPolicy::new(strategy, NonZeroU32::new(2).unwrap());
            let selected = policy
                .select_account(
                    NonZeroU32::MIN,
                    &ProviderKind::new("openai").unwrap(),
                    None,
                    &candidates,
                    &mut context,
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(selected.candidate().account.id(), &account_id("acct_c"));
            assert!(!selected.is_policy_choice());
            assert_eq!(
                gateway_core::account::AccountSelector.wait_candidates(&candidates, &context),
                [account_id("acct_c"), account_id("acct_b")]
            );
            assert_eq!(
                gateway_core::account::AccountSelector
                    .select(&candidates, &context)
                    .unwrap()
                    .candidate()
                    .account
                    .id(),
                &account_id("acct_c")
            );
        }
    });
}

#[test]
fn filtered_delegation_covers_busy_candidates_and_does_not_persist_between_selections() {
    block_on(async {
        let mut candidates = [candidate("acct_a"), candidate("acct_b")];
        candidates[0].signals.in_flight = 2;
        let mut selection = context();
        selection.busy_accounts.insert(account_id("acct_b"));
        let filtered =
            policy_decision(AccountScheduleDecision::DelegateFiltered(BTreeSet::from([
                account_id("acct_a"),
            ])));
        assert!(
            filtered
                .select_account(
                    NonZeroU32::MIN,
                    &ProviderKind::new("openai").unwrap(),
                    None,
                    &candidates,
                    &mut selection
                )
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            gateway_core::account::AccountSelector.wait_candidates(&candidates, &selection),
            [account_id("acct_b")]
        );
        candidates[0].signals.in_flight = 0;
        let recovered = policy(None)
            .select_account(
                NonZeroU32::MIN,
                &ProviderKind::new("openai").unwrap(),
                None,
                &candidates,
                &mut context(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered.candidate().account.id(), &account_id("acct_a"));
    });
}

#[test]
fn filtering_all_candidates_is_terminal_even_when_the_only_candidate_is_busy_or_pinned() {
    block_on(async {
        for busy in [false, true] {
            let candidates = [candidate("acct_a"), candidate("acct_b")];
            let mut context = context();
            context.excluded_accounts.insert(account_id("acct_b"));
            if busy {
                context.busy_accounts.insert(account_id("acct_a"));
            }
            let result =
                policy_decision(AccountScheduleDecision::DelegateFiltered(BTreeSet::from([
                    account_id("acct_a"),
                ])))
                .select_account(
                    NonZeroU32::MIN,
                    &ProviderKind::new("openai").unwrap(),
                    None,
                    &candidates,
                    &mut context,
                )
                .await;
            assert!(matches!(result, Err(AccountPolicyError::Filtered)));
        }
    });
}

#[test]
fn filtering_ids_outside_the_host_candidate_pool_is_a_policy_fault() {
    block_on(async {
        let candidates = [candidate("acct_a"), candidate("acct_b")];
        for id in ["acct_b", "acct_unknown"] {
            let mut context = context();
            context.excluded_accounts.insert(account_id("acct_b"));
            let result =
                policy_decision(AccountScheduleDecision::DelegateFiltered(BTreeSet::from([
                    account_id(id),
                ])))
                .select_account(
                    NonZeroU32::MIN,
                    &ProviderKind::new("openai").unwrap(),
                    None,
                    &candidates,
                    &mut context,
                )
                .await;
            assert!(matches!(result, Err(AccountPolicyError::Fault)));
        }
    });
}

#[test]
fn empty_filter_preserves_native_affinity_and_busy_candidates_cannot_be_picked() {
    block_on(async {
        let candidates = [candidate("acct_a"), candidate("acct_b")];
        let selected = policy_decision(AccountScheduleDecision::DelegateFiltered(BTreeSet::new()))
            .select_account(
                NonZeroU32::MIN,
                &ProviderKind::new("openai").unwrap(),
                None,
                &candidates,
                &mut context(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(selected.preferred(), PreferredAccountSelection::Hit);
        let mut context = context();
        context.busy_accounts.insert(account_id("acct_a"));
        let result = policy(Some("acct_a"))
            .select_account(
                NonZeroU32::MIN,
                &ProviderKind::new("openai").unwrap(),
                None,
                &candidates,
                &mut context,
            )
            .await;
        assert!(matches!(result, Err(AccountPolicyError::Fault)));
    });
}

#[test]
fn recovery_selection_isolates_roles_and_round_robins_without_moving_daily_preferences() {
    use gateway_core::{
        account::{AccountSchedulingRole, AccountSelector},
        engine::recovery::AccountRecoveryStage,
    };
    let mut candidates = [
        candidate("acct_a"),
        candidate("acct_b"),
        candidate("acct_c"),
        candidate("acct_d"),
    ];
    candidates[2].account = candidates[2]
        .account
        .clone()
        .with_scheduling(None, AccountWeight::new(1).unwrap());
    candidates[3].account = candidates[3]
        .account
        .clone()
        .with_scheduling_role(AccountSchedulingRole::Fallback);
    let initial = AccountRecoveryStage::Initial;
    let mut normal = context();
    normal.preferred_account = Some(account_id("acct_d"));
    initial.prepare_selection(&candidates, &mut normal, None);
    assert_eq!(
        AccountSelector
            .select(&candidates, &normal)
            .unwrap()
            .candidate()
            .account
            .id(),
        &account_id("acct_a")
    );
    // 显式插件选号同样不能让保底账号进入日常池
    assert!(matches!(
        block_on(policy(Some("acct_d")).select_account(
            NonZeroU32::MIN,
            &ProviderKind::new("openai").unwrap(),
            None,
            &candidates,
            &mut normal
        )),
        Err(AccountPolicyError::Fault)
    ));
    let mut after = None;
    for expected in ["acct_b", "acct_c", "acct_b"] {
        let mut selection = context();
        AccountRecoveryStage::OtherAccounts {
            initial: account_id("acct_a"),
            after: after.clone(),
        }
        .prepare_selection(&candidates, &mut selection, None);
        let selected = AccountSelector.select(&candidates, &selection).unwrap();
        assert_eq!(selected.candidate().account.id(), &account_id(expected));
        after = Some(selected.candidate().account.id().clone());
    }
    let mut fallback = context();
    AccountRecoveryStage::Fallback.prepare_selection(&candidates, &mut fallback, None);
    assert_eq!(
        AccountSelector
            .select(&candidates, &fallback)
            .unwrap()
            .candidate()
            .account
            .id(),
        &account_id("acct_d")
    );
    let mut native = context();
    native.preferred_account = Some(account_id("acct_d"));
    initial.prepare_selection(&candidates, &mut native, Some(&account_id("acct_d")));
    assert_eq!(
        AccountSelector
            .select(&candidates, &native)
            .unwrap()
            .candidate()
            .account
            .id(),
        &account_id("acct_d")
    );
    let mut diagnostic = context();
    diagnostic.eligibility = AccountEligibilityPolicy::BypassForDiagnostic;
    diagnostic.preferred_account = Some(account_id("acct_d"));
    initial.prepare_selection(&candidates, &mut diagnostic, None);
    assert_eq!(
        AccountSelector
            .select(&candidates, &diagnostic)
            .unwrap()
            .candidate()
            .account
            .id(),
        &account_id("acct_d")
    );
}

#[test]
fn recovery_roles_follow_the_frozen_scope_and_preserve_hard_eligibility() {
    use gateway_core::{
        account::{
            AccountSchedulingRole, AccountSelector,
            scope::{
                ClientRoutingScope, FrozenAccountScope, RuntimeAccount, RuntimeAccountDirectory,
            },
        },
        engine::recovery::AccountRecoveryStage,
    };
    use std::collections::BTreeMap;
    let provider = ProviderKind::new("openai").unwrap();
    let candidates = [candidate("acct_a"), candidate("acct_d")];
    let scope = Arc::new(FrozenAccountScope::new(
        Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
            (
                account_id("acct_a"),
                RuntimeAccount::new(provider.clone(), Default::default()),
            ),
            (
                account_id("acct_d"),
                RuntimeAccount::new(provider, Default::default())
                    .with_scheduling_role(AccountSchedulingRole::Fallback),
            ),
        ]))),
        ClientRoutingScope::all_accounts(),
    ));
    let mut selection = context();
    selection.account_scope = Some(scope);
    AccountRecoveryStage::Initial.prepare_selection(&candidates, &mut selection, None);
    assert!(selection.excluded_accounts.contains(&account_id("acct_d")));
    for blocked in [false, true] {
        let mut fallback = context();
        fallback.account_scope = selection.account_scope.clone();
        if blocked {
            fallback.busy_accounts.insert(account_id("acct_d"));
        }
        AccountRecoveryStage::Fallback.prepare_selection(&candidates, &mut fallback, None);
        assert_eq!(
            AccountSelector.select(&candidates, &fallback).is_none(),
            blocked
        );
    }
}

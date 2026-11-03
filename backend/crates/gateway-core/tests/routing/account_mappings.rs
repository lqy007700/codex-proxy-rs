//! 验证账号映射的优先级、候选合并、权限、能力和冻结边界

use super::*;
use gateway_core::account::{
    AccountModelAccess, AccountModelAccessMode, AccountModelMappings, AccountSchedulingRole,
};
use gateway_core::routing::RoutingPlan;

fn account(id: &str) -> ProviderAccountId {
    ProviderAccountId::new(id).unwrap()
}

fn mapped_snapshot() -> RuntimeSnapshot {
    let provider = ProviderKind::new("openai").unwrap();
    let mappings = |target: &str| {
        AccountModelMappings::new(BTreeMap::from([("alias".to_owned(), target.to_owned())]))
            .unwrap()
    };
    let directory = Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
        (
            account("acct_a"),
            RuntimeAccount::new(provider.clone(), BTreeSet::new())
                .with_model_mappings(mappings("target-a")),
        ),
        (
            account("acct_b"),
            RuntimeAccount::new(provider.clone(), BTreeSet::new())
                .with_model_mappings(mappings("target-b"))
                .with_scheduling_role(AccountSchedulingRole::Fallback),
        ),
        (
            account("acct_c"),
            RuntimeAccount::new(provider.clone(), BTreeSet::new()),
        ),
    ])));
    RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        settings().with_model_mappings(BTreeMap::from([
            ("alias".to_owned(), "target-c".to_owned()),
            ("target-a".to_owned(), "target-c".to_owned()),
        ])),
        vec![provider],
        vec![
            model("openai", "target-a", capabilities()),
            model("openai", "target-b", capabilities()),
            model("openai", "target-c", capabilities()),
        ],
        Vec::new(),
    )
    .unwrap()
    .with_account_directory(directory)
}

fn plan(snapshot: &RuntimeSnapshot) -> RoutingPlan {
    snapshot
        .plan(
            &PublicModelId::new("alias").unwrap(),
            &operation(),
            snapshot.all_account_scope(),
            &Default::default(),
        )
        .unwrap()
}

#[test]
fn account_mapping_overrides_global_without_chaining_or_splitting_provider_scheduling() {
    let snapshot = mapped_snapshot();
    let plan = plan(&snapshot);
    assert_eq!(plan.candidates().len(), 1);
    let candidate = &plan.candidates()[0];
    for (id, target) in [
        ("acct_a", "target-a"),
        ("acct_b", "target-b"),
        ("acct_c", "target-c"),
    ] {
        assert!(candidate.account_scope().allows(&account(id)));
        assert_eq!(
            candidate
                .upstream_model_for_account(&account(id))
                .unwrap()
                .as_str(),
            target
        );
        assert_eq!(
            candidate
                .for_account(&account(id))
                .upstream_model()
                .unwrap()
                .as_str(),
            target
        );
    }
    assert!(candidate.has_scheduling_role(AccountSchedulingRole::Fallback));
}

#[test]
fn account_mapping_policy_checks_the_requested_name_and_freezes_existing_plans() {
    let snapshot = mapped_snapshot();
    let old_plan = plan(&snapshot);
    let mut accounts = snapshot
        .all_account_scope()
        .directory()
        .accounts()
        .map(|(id, account)| (id.clone(), account.clone()))
        .collect::<BTreeMap<_, _>>();
    let blocked = accounts
        .remove(&account("acct_a"))
        .unwrap()
        .with_model_access(
            AccountModelAccess::new(AccountModelAccessMode::Denylist, vec!["alias".to_owned()])
                .unwrap(),
        );
    accounts.insert(account("acct_a"), blocked);
    let changed = snapshot.with_account_directory(Arc::new(RuntimeAccountDirectory::new(accounts)));
    assert!(
        !plan(&changed).candidates()[0]
            .account_scope()
            .allows(&account("acct_a"))
    );
    assert!(
        old_plan.candidates()[0]
            .account_scope()
            .allows(&account("acct_a"))
    );
    assert_eq!(
        old_plan.candidates()[0]
            .upstream_model_for_account(&account("acct_a"))
            .unwrap()
            .as_str(),
        "target-a"
    );
}

#[test]
fn selected_request_name_can_route_to_an_unselected_target_without_exposing_it_directly() {
    let original = mapped_snapshot();
    let id = account("acct_a");
    let configured = original
        .all_account_scope()
        .directory()
        .account(&id)
        .unwrap()
        .clone()
        .with_model_access(
            AccountModelAccess::new(AccountModelAccessMode::Allowlist, vec!["alias".to_owned()])
                .unwrap(),
        );
    let snapshot =
        original.with_account_directory(Arc::new(RuntimeAccountDirectory::new(BTreeMap::from([
            (id.clone(), configured),
        ]))));
    let plan = plan(&snapshot);
    let candidate = &plan.candidates()[0];
    assert!(candidate.account_scope().allows(&id));
    assert_eq!(
        candidate.upstream_model_for_account(&id).unwrap().as_str(),
        "target-a"
    );
    assert!(
        snapshot
            .public_models_for_scope(&snapshot.all_account_scope())
            .contains(&PublicModelId::new("alias").unwrap())
    );
    assert!(
        !snapshot
            .public_models_for_scope(&snapshot.all_account_scope())
            .contains(&PublicModelId::new("target-a").unwrap())
    );
    assert!(
        snapshot
            .plan(
                &PublicModelId::new("target-a").unwrap(),
                &operation(),
                snapshot.all_account_scope(),
                &Default::default()
            )
            .is_err()
    );
}

#[test]
fn mapped_alias_is_hidden_outside_its_account_scope() {
    let snapshot = mapped_snapshot();
    let only_c = Arc::new(
        snapshot
            .all_account_scope()
            .restricted_to(&Default::default(), &BTreeSet::from([account("acct_c")])),
    );
    let alias = PublicModelId::new("local-only").unwrap();
    let mut accounts = snapshot
        .all_account_scope()
        .directory()
        .accounts()
        .map(|(id, account)| (id.clone(), account.clone()))
        .collect::<BTreeMap<_, _>>();
    let mapped = accounts
        .remove(&account("acct_a"))
        .unwrap()
        .with_model_mappings(
            AccountModelMappings::new(BTreeMap::from([(
                "local-only".to_owned(),
                "target-a".to_owned(),
            )]))
            .unwrap(),
        );
    accounts.insert(account("acct_a"), mapped);
    let snapshot =
        snapshot.with_account_directory(Arc::new(RuntimeAccountDirectory::new(accounts)));
    assert!(
        snapshot
            .public_models_for_scope(&snapshot.all_account_scope())
            .contains(&alias)
    );
    assert!(!snapshot.public_models_for_scope(&only_c).contains(&alias));
    assert!(!snapshot.allows_public_model_for_account(&alias, &only_c, &account("acct_c")));
}

#[test]
fn invalid_account_target_is_not_silently_replaced_by_global_mapping() {
    let snapshot = mapped_snapshot();
    let mut accounts = snapshot
        .all_account_scope()
        .directory()
        .accounts()
        .map(|(id, account)| (id.clone(), account.clone()))
        .collect::<BTreeMap<_, _>>();
    let mapped = accounts
        .remove(&account("acct_a"))
        .unwrap()
        .with_model_mappings(
            AccountModelMappings::new(BTreeMap::from([(
                "alias".to_owned(),
                "missing-target".to_owned(),
            )]))
            .unwrap(),
        );
    accounts.insert(account("acct_a"), mapped);
    let snapshot =
        snapshot.with_account_directory(Arc::new(RuntimeAccountDirectory::new(accounts)));
    assert!(
        !plan(&snapshot).candidates()[0]
            .account_scope()
            .allows(&account("acct_a"))
    );
}

#[test]
fn metadata_must_confirm_the_selected_accounts_target() {
    use gateway_core::engine::provider::ProviderCallMetadata;
    use gateway_core::upstream::UpstreamTransport;
    let snapshot = mapped_snapshot();
    let plan = plan(&snapshot);
    let metadata = ProviderCallMetadata::new(
        ProviderKind::new("openai").unwrap(),
        UpstreamModelId::new("target-b").unwrap(),
        account("acct_b"),
        UpstreamTransport::new("http").unwrap(),
    );
    assert!(metadata.confirms(&plan.candidates()[0]));
    let forged = ProviderCallMetadata::new(
        ProviderKind::new("openai").unwrap(),
        UpstreamModelId::new("target-a").unwrap(),
        account("acct_b"),
        UpstreamTransport::new("http").unwrap(),
    );
    assert!(!forged.confirms(&plan.candidates()[0]));
}

#[test]
fn account_mapping_filters_capabilities_before_merging_and_does_not_restore_ineligible_fallbacks() {
    let original = mapped_snapshot();
    let directory = Arc::clone(original.all_account_scope().directory());
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(2).unwrap(),
        original.settings().clone(),
        vec![ProviderKind::new("openai").unwrap()],
        vec![
            model("openai", "target-a", capabilities()),
            model(
                "openai",
                "target-b",
                ModelCapabilities::new(BTreeSet::from([OperationKind::CountTokens]), Some(16_000)),
            ),
            model("openai", "target-c", capabilities()),
        ],
        Vec::new(),
    )
    .unwrap()
    .with_account_directory(directory);
    let plan = plan(&snapshot);
    assert_eq!(plan.candidates().len(), 1);
    let candidate = &plan.candidates()[0];
    assert!(candidate.account_scope().allows(&account("acct_a")));
    assert!(candidate.account_scope().allows(&account("acct_c")));
    assert!(!candidate.account_scope().allows(&account("acct_b")));
    assert!(!candidate.has_scheduling_role(AccountSchedulingRole::Fallback));
}

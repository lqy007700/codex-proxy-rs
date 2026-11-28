//! 可信 Token 请求计数、固定切换目标与原生续接安全

use super::*;
use gateway_core::policy::{CallerIdentity, NewApiTokenAffinityRule};
use gateway_core::provider_ports::{
    ProviderAffinityScope, ProviderSessionAffinityPort, SessionBindingDirective,
};

fn counted_attempt(id: &str, attempt: u32, owner: Option<&str>, limit: u32) -> AttemptContext {
    let key = ClientApiKeyId::new("key_codex_contract").unwrap();
    let caller = CallerIdentity::NewApi {
        source_id: "source".into(),
        user_id: "user".into(),
        token_id: "token".into(),
    };
    let continuation = owner.map(|owner| {
        ContinuationBinding::Pinned(NativeContinuationPin::new(
            PreviousResponseId::new("client-response"),
            PreviousResponseId::new("upstream-response"),
            key.clone(),
            ProviderKind::new("openai").unwrap(),
            ProviderAccountId::new(owner).unwrap(),
        ))
    });
    AttemptContext::new(
        RequestAttemptContext::new(ModelRequestId::new(format!("req_{id}")).unwrap(), key)
            .with_caller_identity(caller)
            .with_newapi_token_affinity_rule(Some(NewApiTokenAffinityRule {
                request_limit: NonZeroU32::new(limit).unwrap(),
                target_account: ProviderAccountId::new("acct_second").unwrap(),
            })),
        NonZeroU32::new(attempt).unwrap(),
        SystemTime::now() + Duration::from_secs(5),
        account_policy().with_openai_account_affinity(AccountAffinity::Preferred),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        continuation,
        CancellationToken::new(),
    )
}

async fn select_for(
    selector: &CodexCredentialSelector,
    attempt: &AttemptContext,
    key: &ProviderSessionAffinityKey,
) -> Result<provider_openai::credential::CodexCredentialLease, CredentialSelectionError> {
    let url = Url::parse(OFFICIAL_CODEX_BASE_URL).unwrap();
    selector
        .select(&SelectCodexCredential {
            upstream_model: "gpt-5.4",
            request_url: &url,
            attempt,
            session_affinity_key: Some(key),
        })
        .await
}

#[tokio::test]
async fn repeated_attempt_does_not_count_again_and_old_native_owner_must_replay() {
    let store = Arc::new(MemoryAccountStore::default());
    for id in ["acct_first", "acct_second", "acct_other"] {
        create_account(&store, id, "token");
    }
    store.set_scheduling("acct_first", None, AccountWeight::new(100).unwrap());
    let affinity = Arc::new(MemorySessionAffinity::default());
    let selector =
        selector_with_affinity(&store, Arc::new(TestLeaseCoordinator::default()), affinity);
    let session = ProviderSessionAffinityKey::try_new("counted-session").unwrap();
    for (id, index) in [("request-one", 1), ("request-one", 2), ("request-two", 1)] {
        let lease = select_for(&selector, &counted_attempt(id, index, None, 2), &session)
            .await
            .unwrap();
        assert_eq!(lease.account_id().as_str(), "acct_first");
    }
    let error = select_for(
        &selector,
        &counted_attempt("request-three", 1, Some("acct_first"), 2),
        &session,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        CredentialSelectionError::ContinuationOwnerChanged
    ));
    for id in ["request-three-replay", "request-four"] {
        let lease = select_for(&selector, &counted_attempt(id, 1, None, 2), &session)
            .await
            .unwrap();
        assert_eq!(lease.account_id().as_str(), "acct_second");
    }
    let lease = select_for(
        &selector,
        &counted_attempt("higher-limit", 1, None, 100),
        &session,
    )
    .await
    .unwrap();
    assert_eq!(lease.account_id().as_str(), "acct_second");
}

#[tokio::test]
async fn concurrent_requests_cross_threshold_once() {
    let store = Arc::new(MemoryAccountStore::default());
    for id in ["acct_first", "acct_second", "acct_other"] {
        create_account(&store, id, "token");
    }
    store.set_scheduling("acct_first", None, AccountWeight::new(100).unwrap());
    let affinity = Arc::new(MemorySessionAffinity::default());
    let selector =
        selector_with_affinity(&store, Arc::new(TestLeaseCoordinator::default()), affinity);
    let session = ProviderSessionAffinityKey::try_new("concurrent-counted-session").unwrap();
    let first = counted_attempt("concurrent-first", 1, None, 1);
    let second = counted_attempt("concurrent-second", 1, None, 1);
    let (first, second) = tokio::join!(
        select_for(&selector, &first, &session),
        select_for(&selector, &second, &session)
    );
    let mut selected = [
        first.unwrap().account_id().as_str().to_owned(),
        second.unwrap().account_id().as_str().to_owned(),
    ];
    selected.sort();
    assert_eq!(selected, ["acct_first", "acct_second"]);
}

#[tokio::test]
async fn fixed_target_unavailable_never_uses_another_account() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_first", "token");
    create_account(&store, "acct_other", "token");
    store.set_scheduling("acct_first", None, AccountWeight::new(100).unwrap());
    let affinity = Arc::new(MemorySessionAffinity::default());
    let selector =
        selector_with_affinity(&store, Arc::new(TestLeaseCoordinator::default()), affinity);
    let session = ProviderSessionAffinityKey::try_new("fixed-target-session").unwrap();
    let first = select_for(
        &selector,
        &counted_attempt("missing-first", 1, None, 1),
        &session,
    )
    .await
    .unwrap();
    assert_eq!(first.account_id().as_str(), "acct_first");
    let error = select_for(
        &selector,
        &counted_attempt("missing-second", 1, None, 1),
        &session,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        CredentialSelectionError::SessionBound(inner)
            if matches!(*inner, CredentialSelectionError::NoEligibleCredential)
    ));
    create_account(&store, "acct_second", "token");
    let next = select_for(
        &selector,
        &counted_attempt("missing-third", 1, None, 1),
        &session,
    )
    .await
    .unwrap();
    assert_eq!(next.account_id().as_str(), "acct_second");
}

#[tokio::test]
async fn manual_directive_has_priority_and_its_admission_is_counted() {
    for defer in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        for id in ["acct_first", "acct_second", "acct_other"] {
            create_account(&store, id, "token");
        }
        store.set_scheduling("acct_first", None, AccountWeight::new(100).unwrap());
        let affinity = Arc::new(MemorySessionAffinity::default());
        let selector = selector_with_affinity(
            &store,
            Arc::new(TestLeaseCoordinator::default()),
            affinity.clone(),
        );
        let session = ProviderSessionAffinityKey::try_new("manual-token-session").unwrap();
        let first = select_for(
            &selector,
            &counted_attempt("manual-first", 1, None, 1),
            &session,
        )
        .await
        .unwrap();
        assert_eq!(first.account_id().as_str(), "acct_first");
        let (token, _) = affinity.binding_for_scope(ProviderAffinityScope::Token);
        affinity.set_directive(
            &token,
            SessionBindingDirective::Target {
                account_id: if defer { "acct_missing" } else { "acct_other" }.into(),
            },
        );
        let next = select_for(
            &selector,
            &counted_attempt("manual-second", 1, None, 1),
            &session,
        )
        .await
        .unwrap();
        assert_eq!(
            next.account_id().as_str(),
            if defer { "acct_first" } else { "acct_other" }
        );
        let token_key = ProviderSessionAffinityKey::try_new(token.1).unwrap();
        let state = affinity
            .load_token_request_state(
                &ProviderKind::new("openai").unwrap(),
                &token_key,
                "req_manual-second",
            )
            .await
            .unwrap();
        assert_eq!(state.count, Some(2));
        assert!(!state.latched);
        assert_eq!(state.switched, Some(false));
        if !defer {
            let switched = select_for(
                &selector,
                &counted_attempt("manual-third", 1, None, 1),
                &session,
            )
            .await
            .unwrap();
            assert_eq!(switched.account_id().as_str(), "acct_second");
        }
    }
}

#[tokio::test]
async fn retry_after_expiry_does_not_recreate_or_claim_a_binding() {
    let store = Arc::new(MemoryAccountStore::default());
    for id in ["acct_first", "acct_second"] {
        create_account(&store, id, "token");
    }
    store.set_scheduling("acct_first", None, AccountWeight::new(100).unwrap());
    let affinity = Arc::new(MemorySessionAffinity::default());
    let selector = selector_with_affinity(
        &store,
        Arc::new(TestLeaseCoordinator::default()),
        affinity.clone(),
    );
    let session = ProviderSessionAffinityKey::try_new("expired-in-flight").unwrap();
    let first = counted_attempt("expiry-first", 1, None, 1);
    select_for(&selector, &first, &session).await.unwrap();
    let switched = counted_attempt("expiry-second", 1, None, 1);
    let lease = select_for(&selector, &switched, &session).await.unwrap();
    assert_eq!(lease.account_id().as_str(), "acct_second");
    affinity.expire_all_bindings();
    let lease = select_for(&selector, &switched, &session).await.unwrap();
    assert_eq!(lease.account_id().as_str(), "acct_second");
    assert_eq!(affinity.binding_count(), 0);
    let next = select_for(
        &selector,
        &counted_attempt("expiry-third", 1, None, 1),
        &session,
    )
    .await
    .unwrap();
    assert_eq!(next.account_id().as_str(), "acct_first");
    let switched_again = select_for(
        &selector,
        &counted_attempt("expiry-fourth", 1, None, 1),
        &session,
    )
    .await
    .unwrap();
    assert_eq!(switched_again.account_id().as_str(), "acct_second");
}

#[tokio::test]
async fn admitted_old_owner_retry_does_not_undo_a_later_switch() {
    let store = Arc::new(MemoryAccountStore::default());
    for id in ["acct_first", "acct_second"] {
        create_account(&store, id, "token");
    }
    store.set_scheduling("acct_first", None, AccountWeight::new(100).unwrap());
    let affinity = Arc::new(MemorySessionAffinity::default());
    let selector = selector_with_affinity(
        &store,
        Arc::new(TestLeaseCoordinator::default()),
        affinity.clone(),
    );
    let session = ProviderSessionAffinityKey::try_new("old-in-flight").unwrap();
    let old = counted_attempt("old-in-flight", 1, Some("acct_first"), 1);
    assert_eq!(
        select_for(&selector, &old, &session)
            .await
            .unwrap()
            .account_id()
            .as_str(),
        "acct_first"
    );
    assert_eq!(
        select_for(
            &selector,
            &counted_attempt("switch-after-old", 1, None, 1),
            &session
        )
        .await
        .unwrap()
        .account_id()
        .as_str(),
        "acct_second"
    );
    assert_eq!(
        select_for(&selector, &old, &session)
            .await
            .unwrap()
            .account_id()
            .as_str(),
        "acct_first"
    );
    let (_, token) = affinity.binding_for_scope(ProviderAffinityScope::Token);
    assert_eq!(token.account_id().as_str(), "acct_second");
    assert_eq!(
        affinity
            .load(&ProviderKind::new("openai").unwrap(), &session)
            .await
            .unwrap()
            .unwrap()
            .as_str(),
        "acct_second"
    );
}

//! 执行引擎各阶段与端口合同的测试入口

pub(crate) mod account_protection;
mod admission;
mod connection;
mod continuation;
mod coordinator;
mod execution;
mod extensions;
mod middleware;
mod policy;
mod provider;
mod response_control;

use gateway_core::engine::AttemptTrigger;

#[test]
fn attempt_trigger_names_should_match_ops_event_contract() {
    assert_eq!(AttemptTrigger::AccountRetry.as_str(), "account_retry");
}

#[test]
fn newapi_admission_is_shared_by_clones_and_is_write_once() {
    use std::num::NonZeroU32;

    use gateway_core::account::{AccountSelectionPolicy, ProviderAccountId, RotationStrategy};
    use gateway_core::engine::{
        AccountAttemptContext, AttemptContext, ModelRequestId, RequestAttemptContext,
    };
    use gateway_core::lifecycle::{CancellationToken, Deadline};
    use gateway_core::policy::{ClientApiKeyId, NewApiTokenAffinityRule};

    let rule = NewApiTokenAffinityRule {
        request_limit: NonZeroU32::MIN,
        target_account: ProviderAccountId::new("acct_target").unwrap(),
    };
    let request = || {
        RequestAttemptContext::new(
            ModelRequestId::new("req_affinity").unwrap(),
            ClientApiKeyId::new("key_affinity").unwrap(),
        )
    };
    let attempt = |request| {
        AttemptContext::new(
            request,
            NonZeroU32::MIN,
            Deadline::default(),
            AccountSelectionPolicy::new(RotationStrategy::Smart, NonZeroU32::MIN),
            AccountAttemptContext::new(Default::default(), None, None),
            None,
            CancellationToken::new(),
        )
    };

    for first in [false, true] {
        let shared = request().with_newapi_token_affinity_rule(Some(rule.clone()));
        let first_attempt = attempt(shared.clone());
        let second_attempt = attempt(shared);
        let other_request = attempt(request().with_newapi_token_affinity_rule(Some(rule.clone())));
        assert_eq!(first_attempt.newapi_token_affinity_rule(), Some(&rule));
        assert_eq!(first_attempt.newapi_token_affinity_admission(), None);
        first_attempt.record_newapi_token_affinity_admission(first);
        assert_eq!(
            second_attempt.newapi_token_affinity_admission(),
            Some(first)
        );
        second_attempt.record_newapi_token_affinity_admission(!first);
        assert_eq!(first_attempt.newapi_token_affinity_admission(), Some(first));
        assert_eq!(other_request.newapi_token_affinity_admission(), None);
    }

    let disabled = attempt(request());
    disabled.record_newapi_token_affinity_admission(true);
    assert_eq!(disabled.newapi_token_affinity_rule(), None);
    assert_eq!(disabled.newapi_token_affinity_admission(), None);
}

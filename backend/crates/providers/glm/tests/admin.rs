//! 验证智谱凭据输入、导出合同和不支持的接入模式

use super::support::Store;
use gateway_admin::{
    model::provider_credentials::{PrepareCredentialImport, ProviderDocument},
    ports::provider::ProviderAdmin,
};
use gateway_core::{account::OpaqueProviderData, routing::ProviderKind};
use provider_glm::GlmAdmin;
use serde_json::{Value, json};
use std::sync::Arc;

fn command(value: Value) -> PrepareCredentialImport {
    PrepareCredentialImport {
        default_outbound_proxy: None,
        document: ProviderDocument::new(OpaqueProviderData::new(
            value.as_object().unwrap().clone(),
        )),
    }
}
#[tokio::test]
async fn imports_only_standard_responses_credentials() {
    let admin = GlmAdmin::new(
        ProviderKind::new("glm").unwrap(),
        Arc::new(Store::default()),
    );
    let imported = admin
        .prepare_import(command(json!({"name":"test","api_key":"fixture-key"})))
        .await
        .unwrap();
    let account = &imported.credentials[0];
    assert_eq!(account.authentication_kind, "api_key");
    assert!(!account.has_refresh_token);
    assert_eq!(account.plan_type.as_deref(), Some("payg"));
    let material = account
        .provider_material
        .expose_to_provider()
        .expose_to_provider();
    assert_eq!(material["base_url"], "https://open.bigmodel.cn/api/v1");
    assert!(!format!("{account:?}").contains("fixture-key"));
    for extra in [
        json!({"base_url":"https://open.bigmodel.cn/api/coding/paas/v4"}),
        json!({"transport":"prefer_websocket"}),
        json!({"provider":"openai"}),
        json!({"account_mode":"coding"}),
        json!({"base_url":"https://user:password@example.com/v1"}),
    ] {
        let mut value = json!({"name":"test","api_key":"fixture-key"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(admin.prepare_import(command(value)).await.is_err());
    }
}

#[tokio::test]
async fn prices_only_complete_usage_with_explicit_usd_override() {
    use gateway_admin::ports::provider::ProviderUsagePricingInput;
    use gateway_core::metering::{ModelPriceOverride, Usage};
    let admin = GlmAdmin::new(
        ProviderKind::new("glm").unwrap(),
        Arc::new(Store::default()),
    );
    let price:ModelPriceOverride=serde_json::from_value(json!({"multiplierBps":20000,"bands":{"standard":{"input":"2","output":"4","cacheRead":"1","cacheWrite":"0"}}})).unwrap();
    let mut usage = Usage::new();
    usage.input_tokens = Some(10);
    usage.cached_input_tokens = Some(3);
    usage.output_tokens = Some(4);
    usage.reasoning_output_tokens = Some(2);
    let mut input = ProviderUsagePricingInput {
        upstream_model_id: "glm-5.3".into(),
        service_tier: None,
        usage,
    };
    let result = admin
        .evaluate_usage_pricing(&input, Some(&price))
        .unwrap()
        .unwrap();
    assert_eq!(result.calculated_cost().total().amount().scaled(), 660_000);
    input.usage.cached_input_tokens = None;
    assert!(
        admin
            .evaluate_usage_pricing(&input, Some(&price))
            .unwrap()
            .is_none()
    );
    assert!(
        admin
            .evaluate_usage_pricing(&input, None)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn ordinary_configuration_never_returns_the_upstream_key() {
    use gateway_core::account::ProviderAccountId;
    let store = Arc::new(Store::default());
    store.add(
        "acct_glm",
        "glm",
        "https://open.bigmodel.cn/api/v1",
        "glm-5.3",
    );
    let admin = GlmAdmin::new(ProviderKind::new("glm").unwrap(), store);
    let document = admin
        .account_configuration(&ProviderAccountId::new("acct_glm").unwrap())
        .await
        .unwrap()
        .unwrap();
    let object = document.expose_to_provider().expose_to_provider();
    assert!(!object.contains_key("api_key"));
    assert_eq!(object["base_url"], "https://open.bigmodel.cn/api/v1");
}

#[tokio::test]
async fn model_catalog_updates_preserve_key_and_health_and_reject_invalid_lists() {
    use chrono::Utc;
    use gateway_admin::model::{
        Revision,
        accounts::{AccountRecord, AccountWeight, CredentialState, QuotaState},
        provider_credentials::PrepareCredentialRotation,
    };
    let store = Arc::new(Store::default());
    store.add(
        "acct_glm",
        "glm",
        "https://open.bigmodel.cn/api/v1",
        "glm-5.3",
    );
    let admin = GlmAdmin::new(ProviderKind::new("glm").unwrap(), store);
    let now = Utc::now();
    let account = AccountRecord {
        id: "acct_glm".to_owned(),
        provider_kind: ProviderKind::new("glm").unwrap(),
        groups: vec![],
        name: "fixture".to_owned(),
        notes: None,
        email: None,
        upstream_user_id: None,
        upstream_account_id: None,
        plan_type: Some("payg".to_owned()),
        authentication_kind: "api_key".to_owned(),
        credential_revision: Revision::new(1).unwrap(),
        has_refresh_token: false,
        access_token_expires_at: None,
        next_refresh_at: None,
        enabled: true,
        concurrency_limit: None,
        weight: AccountWeight::new(1).unwrap(),
        model_access: Default::default(),
        allow_credits: false,
        outbound_proxy_id: None,
        outbound_proxy: None,
        outbound_proxy_enabled: true,
        credential_state: CredentialState::Invalid,
        credential_observed_at: now,
        quota: QuotaState::unknown(),
        last_error_reason: None,
        last_error_message: None,
        created_at: now,
        updated_at: now,
        scheduling_role: Default::default(),
    };
    let maximum_models = (0..128)
        .map(|index| format!("glm-fixture-{index}"))
        .collect::<Vec<_>>();
    for (changes, expected_models, preserves_health) in [
        (
            json!({"models":["glm-5.3", "glm-fixture"]}),
            json!(["glm-5.3", "glm-fixture"]),
            true,
        ),
        (
            json!({"models":maximum_models}),
            json!(maximum_models),
            true,
        ),
        (
            json!({"base_url":"https://example.invalid/api/v1"}),
            json!(["glm-5.3"]),
            true,
        ),
        (
            json!({"api_key":"replacement-fixture-key"}),
            json!(["glm-5.3"]),
            false,
        ),
    ] {
        let rotated = admin
            .prepare_rotation(PrepareCredentialRotation {
                account: account.clone(),
                provider_material: ProviderDocument::new(OpaqueProviderData::new(
                    changes.as_object().unwrap().clone(),
                )),
            })
            .await
            .unwrap();
        let facts = rotated.facts();
        assert_eq!(facts.preserve_credential_state, preserves_health);
        assert_eq!(facts.expected_credential_revision.get(), 1);
        let material = facts
            .provider_material
            .expose_to_provider()
            .expose_to_provider();
        assert_eq!(material["models"], expected_models);
        assert_eq!(
            material["api_key"],
            if preserves_health {
                "fixture-key"
            } else {
                "replacement-fixture-key"
            }
        );
    }
    for models in [
        json!([]),
        json!(["gpt-example"]),
        json!(["glm-5.3", "glm-5.3"]),
        json!(
            (0..129)
                .map(|index| format!("glm-fixture-{index}"))
                .collect::<Vec<_>>()
        ),
    ] {
        assert!(
            admin
                .prepare_rotation(PrepareCredentialRotation {
                    account: account.clone(),
                    provider_material: ProviderDocument::new(OpaqueProviderData::new(
                        json!({"models":models}).as_object().unwrap().clone()
                    )),
                })
                .await
                .is_err()
        );
    }
}

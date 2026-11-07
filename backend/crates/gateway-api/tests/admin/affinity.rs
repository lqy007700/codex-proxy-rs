//! 验证亲和管理的管理员认证、服务未装配边界与列表展示投影

use super::AdminTestFixture;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt as _;

#[tokio::test]
async fn every_affinity_route_requires_admin_and_reports_unavailable_optional_service() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    for (method, path, body) in [
        ("GET", "sessions", ""),
        ("GET", "sources", ""),
        ("GET", "operations", ""),
        (
            "POST",
            "sources/save",
            r#"{"id":"b7bb7de9-7b06-4caa-8070-352af86a8a4a","name":"fixture","enabled":true}"#,
        ),
        (
            "POST",
            "preview",
            r#"{"operationId":"b7bb7de9-7b06-4caa-8070-352af86a8a4a","selection":[],"directive":{"action":"clear"}}"#,
        ),
        (
            "POST",
            "apply",
            r#"{"operationId":"b7bb7de9-7b06-4caa-8070-352af86a8a4a"}"#,
        ),
    ] {
        for (cookie, expected) in [
            ("", StatusCode::UNAUTHORIZED),
            ("cpr_session=valid-session", StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let response = crate::openai::api_router_with_admin(fixture.services.clone())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(format!("/api/admin/affinity/{path}"))
                        .header(header::COOKIE, cookie)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{method} {path}");
        }
    }
}

use async_trait::async_trait;
use gateway_admin::{
    model::{AdminError, MutationContext, Revision, affinity::*},
    ports::affinity::*,
};
use std::sync::Arc;

struct NamedBindings;
#[async_trait]
impl AffinityBindingStore for NamedBindings {
    async fn list(&self, _: &AffinityQuery) -> Result<AffinityPage, AdminError> {
        let now = chrono::Utc::now();
        Ok(AffinityPage {
            items: [Some("开发用 Key".to_owned()), None]
                .into_iter()
                .enumerate()
                .map(|(index, token_name)| AffinitySession {
                    reference: format!("binding-{index}"),
                    provider: "openai".into(),
                    observation: gateway_core::provider_ports::ProviderSessionObservation {
                        token_name,
                        binding_scope: gateway_core::provider_ports::ProviderAffinityScope::Token,
                        caller: gateway_core::policy::CallerIdentity::NewApi {
                            source_id: "source".into(),
                            user_id: "1".into(),
                            token_id: index.to_string(),
                        },
                        client_key_id: "shared-key".into(),
                        request_id: "req_fixture".into(),
                        actual_account_id: "acct_recent".into(),
                    },
                    account_id: "acct_primary".into(),
                    revision: "revision".into(),
                    directive: None,
                    first_observed_at: now,
                    last_admission_at: now,
                    expires_at: now + chrono::Duration::hours(1),
                    active: false,
                })
                .collect(),
            next_cursor: Some("next-page".into()),
            coverage: "observedOnly",
        })
    }
    async fn session(&self, _: &str, _: &str) -> Result<Option<AffinitySession>, AdminError> {
        panic!("unused session lookup")
    }
    async fn receipt(
        &self,
        _: &AffinityPlan,
        _: &AffinityPlanItem,
    ) -> Result<Option<AffinityItemResult>, AdminError> {
        panic!("unused receipt")
    }
    async fn apply(
        &self,
        _: &AffinityPlan,
        _: &AffinityPlanItem,
        _: Option<&str>,
    ) -> Result<AffinityItemResult, AdminError> {
        panic!("unused mutation")
    }
}
#[async_trait]
impl AffinityJournalStore for NamedBindings {
    async fn sources(&self) -> Result<Vec<IdentitySource>, AdminError> {
        panic!("unused sources")
    }
    async fn save_source(
        &self,
        _: &MutationContext,
        _: IdentitySource,
    ) -> Result<Revision, AdminError> {
        panic!("unused source mutation")
    }
    async fn eligible_target(
        &self,
        _: &AffinitySession,
        _: Option<&str>,
    ) -> Result<bool, AdminError> {
        panic!("unused target lookup")
    }
    async fn create_plan(
        &self,
        _: &MutationContext,
        _: AffinityPlan,
    ) -> Result<AffinityOperation, AdminError> {
        panic!("unused plan mutation")
    }
    async fn operation(&self, _: &str) -> Result<Option<AffinityOperation>, AdminError> {
        panic!("unused operation lookup")
    }
    async fn history(&self) -> Result<Vec<AffinityOperation>, AdminError> {
        panic!("unused history")
    }
    async fn record_result(
        &self,
        _: &MutationContext,
        _: &str,
        _: &AffinityItemResult,
    ) -> Result<AffinityItemResult, AdminError> {
        panic!("unused audit mutation")
    }
}

#[tokio::test]
async fn affinity_list_projects_optional_name_outside_strict_observation_and_keeps_time_and_cursor()
{
    let store = Arc::new(NamedBindings);
    let fixture = AdminTestFixture::with_affinity(AffinityStorePorts {
        bindings: store.clone(),
        journal: store,
    })
    .await;
    fixture.auth.insert_session("valid-session");
    let response = crate::openai::api_router_with_admin(fixture.services)
        .oneshot(
            Request::builder()
                .uri("/api/admin/affinity/sessions?mode=newApi&bindingScope=token")
                .header(header::COOKIE, "cpr_session=valid-session")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let page = &body["data"];
    assert_eq!(page["coverage"], "observedOnly");
    assert_eq!(page["nextCursor"], "next-page");
    assert_eq!(page["items"][0]["tokenName"], "开发用 Key");
    assert!(page["items"][1]["tokenName"].is_null());
    for item in page["items"].as_array().unwrap() {
        assert!(item["observation"].get("tokenName").is_none());
        assert_eq!(item["accountId"], "acct_primary");
        assert_eq!(item["observation"]["actualAccountId"], "acct_recent");
        assert!(item["lastAdmissionAtDisplay"].is_string());
        assert!(item["expiresAtDisplay"].is_string());
    }
}

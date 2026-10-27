//! 告警管理接口的鉴权、配置版本、秘密隔离与只读预览测试

use std::sync::Mutex;

use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{DateTime, Utc};
use gateway_admin::{
    model::{AdminError, MutationContext, alerts::*},
    ports::{
        alerts::{AlertRepository, EmailSender},
        store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
    },
};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::{AdminTestFixture, AdminTestState};

pub(super) struct MemoryAlerts {
    settings: Mutex<AlertSettings>,
}

impl Default for MemoryAlerts {
    fn default() -> Self {
        Self {
            settings: Mutex::new(AlertSettings {
                revision: 1,
                values: AlertSettingsValues::default(),
                password: None,
                updated_at: Utc::now(),
            }),
        }
    }
}

fn unused() -> AdminStoreError {
    AdminStoreError::new(AdminStoreErrorKind::Unavailable, "alerts", "not used")
}

#[async_trait]
impl AlertRepository for MemoryAlerts {
    async fn settings(&self) -> AdminStoreResult<AlertSettings> {
        Ok(self.settings.lock().unwrap().clone())
    }
    async fn update_settings(
        &self,
        command: UpdateAlertSettings,
        _: &MutationContext,
    ) -> AdminStoreResult<AlertSettings> {
        let mut settings = self.settings.lock().unwrap();
        if settings.revision != command.expected_revision {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::StaleRevision,
                "alerts",
                "stale revision",
            ));
        }
        settings.revision += 1;
        settings.values = command.values;
        if command.clear_password {
            settings.password = None;
        } else if let Some(password) = command.password {
            settings.password = Some(password);
        }
        Ok(settings.clone())
    }
    async fn runtime(&self) -> AdminStoreResult<AlertRuntime> {
        Ok(AlertRuntime::default())
    }
    async fn status(&self) -> AdminStoreResult<AlertStatus> {
        Ok(AlertStatus {
            runtime: AlertRuntime::default(),
            events: Vec::new(),
            deliveries: Vec::new(),
        })
    }
    async fn sample(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> AdminStoreResult<AlertSample> {
        Ok(AlertSample {
            window_start: start,
            window_end: end,
            requests: 100,
            failures: 25,
        })
    }
    async fn commit_evaluation(
        &self,
        _: u64,
        _: u64,
        _: AlertEvaluation,
        _: String,
        _: String,
    ) -> AdminStoreResult<bool> {
        Err(unused())
    }
    async fn queue_test(&self, _: u64, _: &MutationContext) -> AdminStoreResult<()> {
        Ok(())
    }
    async fn claim_delivery(&self) -> AdminStoreResult<Option<AlertDelivery>> {
        Err(unused())
    }
    async fn complete_delivery(&self, _: &str, _: u32, _: bool) -> AdminStoreResult<()> {
        Err(unused())
    }
}

pub(super) struct TestEmailSender;

#[async_trait]
impl EmailSender for TestEmailSender {
    fn validate_addresses(&self, _: &AlertSettingsValues) -> Result<(), AdminError> {
        Ok(())
    }
    async fn send(&self, _: &AlertDelivery) -> Result<(), EmailSendError> {
        Err(EmailSendError)
    }
}

fn app(fixture: &AdminTestFixture) -> Router {
    gateway_api::admin::alerts::router::<AdminTestState>().with_state(fixture.state())
}

fn request(method: Method, suffix: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("/api/admin/settings/alerts{suffix}"))
        .header(header::COOKIE, "cpr_session=valid-session")
        .header("x-request-id", "req_alerts_test");
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

fn update_body() -> Value {
    let mut body = serde_json::to_value(AlertSettingsValues {
        smtp_host: "smtp.example.com".to_owned(),
        smtp_username: "ops@example.com".to_owned(),
        sender_address: "ops@example.com".to_owned(),
        recipients: vec!["admin@example.com".to_owned()],
        ..AlertSettingsValues::default()
    })
    .unwrap();
    body["revision"] = json!(1);
    body["smtpPassword"] = json!("PRIVATE_SMTP_PASSWORD");
    body
}

#[tokio::test]
async fn all_alert_routes_require_admin_authentication() {
    let fixture = AdminTestFixture::new().await;
    for (method, suffix, body) in [
        (Method::GET, "", None),
        (Method::GET, "/status", None),
        (Method::GET, "/preview", None),
        (Method::POST, "/update", Some(update_body())),
        (Method::POST, "/test", Some(json!({"revision":1}))),
    ] {
        let response = app(&fixture)
            .oneshot(request(method, suffix, body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn alert_settings_default_to_disabled_and_never_return_the_password() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let response = app(&fixture)
        .oneshot(request(Method::GET, "", None))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert_eq!(body["data"]["enabled"], false);
    assert_eq!(body["data"]["passwordConfigured"], false);
    let response = app(&fixture)
        .oneshot(request(Method::POST, "/update", Some(update_body())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["data"]["passwordConfigured"], true);
    assert_eq!(body["data"]["revision"], 2);
    assert!(!body.to_string().contains("PRIVATE_SMTP_PASSWORD"));
    assert!(body["data"].get("smtpPassword").is_none());
    let response = app(&fixture)
        .oneshot(request(Method::GET, "", None))
        .await
        .unwrap();
    assert!(
        !json_body(response)
            .await
            .to_string()
            .contains("PRIVATE_SMTP_PASSWORD")
    );
}

#[tokio::test]
async fn alert_configuration_rejects_stale_revision_and_unsafe_recovery_threshold() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let mut body = update_body();
    body["revision"] = json!(2);
    let response = app(&fixture)
        .oneshot(request(Method::POST, "/update", Some(body)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let mut body = update_body();
    body["recoveryThresholdPercent"] = json!(20);
    let response = app(&fixture)
        .oneshot(request(Method::POST, "/update", Some(body)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preview_is_read_only_and_test_requires_saved_configuration() {
    let fixture = AdminTestFixture::new().await;
    fixture.auth.insert_session("valid-session");
    let response = app(&fixture)
        .oneshot(request(Method::GET, "/preview", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["data"]["requests"], 100);
    assert_eq!(body["data"]["failureRatePercent"], 25.0);
    assert!(body["data"]["windowEndDisplay"].is_string());
    let response = app(&fixture)
        .oneshot(request(Method::POST, "/test", Some(json!({"revision":1}))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app(&fixture)
        .oneshot(request(Method::POST, "/update", Some(update_body())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app(&fixture)
        .oneshot(request(Method::POST, "/test", Some(json!({"revision":1}))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = app(&fixture)
        .oneshot(request(Method::POST, "/test", Some(json!({"revision":2}))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(
        json_body(response).await["data"]
            .as_str()
            .unwrap()
            .contains("排队")
    );
}

#[test]
fn update_contract_rejects_unknown_fields_without_exposing_secrets() {
    let mut body = update_body();
    body["unknown"] = json!(true);
    assert!(
        serde_json::from_value::<gateway_api::admin::alerts::UpdateAlertSettingsRequest>(body)
            .is_err()
    );
}

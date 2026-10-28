//! 系统设置中的邮件告警 HTTP 合同，秘密不进入普通读取投影

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use gateway_admin::model::alerts::*;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminResponse, wire::map_admin_service_error,
};
use crate::{auth::SessionState, time::TimePresenter};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertSettingsView {
    pub revision: u64,
    #[serde(flatten)]
    pub values: AlertSettingsValues,
    pub password_configured: bool,
    pub updated_at: DateTime<Utc>,
    pub updated_at_display: String,
}

impl AlertSettingsView {
    fn new(mut settings: AlertSettings, time: TimePresenter) -> Self {
        settings.values.quota_warning = Some(settings.values.quota_warning());
        Self {
            revision: settings.revision,
            values: settings.values,
            password_configured: settings.password.is_some(),
            updated_at: settings.updated_at,
            updated_at_display: time.datetime(&settings.updated_at),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateAlertSettingsRequest {
    pub revision: u64,
    #[serde(flatten)]
    pub values: AlertSettingsValues,
    #[serde(default)]
    pub smtp_password: Option<String>,
    #[serde(default)]
    pub clear_password: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestAlertEmailRequest {
    pub revision: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertSampleView {
    #[serde(flatten)]
    pub sample: AlertSample,
    pub failure_rate_percent: Option<f64>,
    pub window_start_display: String,
    pub window_end_display: String,
}

impl AlertSampleView {
    fn new(sample: AlertSample, time: TimePresenter) -> Self {
        Self {
            failure_rate_percent: sample.failure_rate_percent(),
            window_start_display: time.datetime(&sample.window_start),
            window_end_display: time.datetime(&sample.window_end),
            sample,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertRuntimeView {
    pub phase: AlertPhase,
    pub evaluation_status: AlertEvaluationStatus,
    pub abnormal_count: u32,
    pub recovery_count: u32,
    pub checked_at_display: Option<String>,
    pub stale: bool,
    pub sample: Option<AlertSampleView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertEventView {
    pub id: String,
    pub status: String,
    pub opened_at_display: String,
    pub closed_at_display: Option<String>,
    pub sample: AlertSampleView,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertDeliveryView {
    pub id: String,
    pub kind: AlertNotificationKind,
    pub recipient: String,
    pub status: String,
    pub attempts: u32,
    pub created_at_display: String,
    pub sent_at_display: Option<String>,
    pub next_attempt_at_display: String,
    pub error_message: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertStatusView {
    pub quota_warning: QuotaWarningStatusView,
    pub runtime: AlertRuntimeView,
    pub events: Vec<AlertEventView>,
    pub deliveries: Vec<AlertDeliveryView>,
}

impl AlertStatusView {
    fn new(status: AlertStatus, time: TimePresenter) -> Self {
        let runtime = status.runtime;
        Self {
            quota_warning: QuotaWarningStatusView::new(status.quota_warning, time),
            runtime: AlertRuntimeView {
                phase: runtime.phase,
                evaluation_status: runtime.evaluation_status,
                abnormal_count: runtime.abnormal_count,
                recovery_count: runtime.recovery_count,
                checked_at_display: runtime
                    .checked_at
                    .as_ref()
                    .map(|value| time.datetime(value)),
                stale: runtime.evaluation_status != AlertEvaluationStatus::Disabled
                    && runtime.checked_at.is_some_and(|last| {
                        Utc::now().signed_duration_since(last).num_seconds() > 130
                    }),
                sample: runtime
                    .sample
                    .map(|sample| AlertSampleView::new(sample, time)),
            },
            events: status
                .events
                .into_iter()
                .map(|event| AlertEventView {
                    id: event.id,
                    status: event.status,
                    opened_at_display: time.datetime(&event.opened_at),
                    closed_at_display: event.closed_at.as_ref().map(|value| time.datetime(value)),
                    sample: AlertSampleView::new(event.sample, time),
                })
                .collect(),
            deliveries: status
                .deliveries
                .into_iter()
                .map(|record| AlertDeliveryView {
                    id: record.id,
                    kind: record.kind,
                    recipient: record.recipient,
                    status: record.status,
                    attempts: record.attempts,
                    created_at_display: time.datetime(&record.created_at),
                    sent_at_display: record.sent_at.as_ref().map(|value| time.datetime(value)),
                    next_attempt_at_display: time.datetime(&record.next_attempt_at),
                    error_message: record.error_message,
                })
                .collect(),
        }
    }
}

pub fn router<S>() -> Router<S>
where
    S: SessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/settings/alerts", get(settings::<S>))
        .route("/api/admin/settings/alerts/update", post(update::<S>))
        .route("/api/admin/settings/alerts/status", get(status::<S>))
        .route("/api/admin/settings/alerts/preview", get(preview::<S>))
        .route("/api/admin/settings/alerts/test", post(test::<S>))
        .route(
            "/api/admin/settings/alerts/quota-preview",
            get(quota_preview::<S>),
        )
}

async fn settings<S>(
    _auth: AdminAuth,
    State(state): State<S>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let services = state.admin_services();
    let settings = services
        .alerts()
        .settings()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(AlertSettingsView::new(
            settings,
            TimePresenter::new(services.timezone()),
        )),
    ))
}

async fn update<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<UpdateAlertSettingsRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let services = state.admin_services();
    let settings = services
        .alerts()
        .update(
            UpdateAlertSettings {
                expected_revision: request.revision,
                values: request.values,
                password: request.smtp_password.map(SecretString::from),
                clear_password: request.clear_password,
            },
            &auth.context().mutation_context(),
        )
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(AlertSettingsView::new(
            settings,
            TimePresenter::new(services.timezone()),
        )),
    ))
}

async fn status<S>(
    _auth: AdminAuth,
    State(state): State<S>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let services = state.admin_services();
    let status = services
        .alerts()
        .status()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(AlertStatusView::new(
            status,
            TimePresenter::new(services.timezone()),
        )),
    ))
}

async fn preview<S>(
    _auth: AdminAuth,
    State(state): State<S>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let services = state.admin_services();
    let sample = services
        .alerts()
        .preview()
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(AlertSampleView::new(
            sample,
            TimePresenter::new(services.timezone()),
        )),
    ))
}

async fn test<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<TestAlertEmailRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    state
        .admin_services()
        .alerts()
        .test_email(request.revision, &auth.context().mutation_context())
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::ACCEPTED,
        AdminEnvelope::ok("测试邮件已排队，请查看投递结果"),
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWarningStatusView {
    #[serde(flatten)]
    pub status: QuotaWarningStatus,
    pub checked_at_display: Option<String>,
    pub stale: bool,
}

impl QuotaWarningStatusView {
    fn new(status: QuotaWarningStatus, time: TimePresenter) -> Self {
        Self {
            checked_at_display: status.checked_at.as_ref().map(|at| time.datetime(at)),
            stale: status.checked_at.is_some_and(|at| {
                Utc::now().signed_duration_since(at).num_seconds()
                    > QUOTA_EVALUATION_INTERVAL_SECONDS + 130
            }),
            status,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWarningPreviewView {
    pub checked_at_display: Option<String>,
    pub status: QuotaWarningStatus,
    pub items: Vec<QuotaWarningItemView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWarningItemView {
    pub window_key: String,
    pub account_id: String,
    pub account_label: String,
    pub plan_type: Option<String>,
    pub period: QuotaWarningPeriod,
    pub used_percent: Option<f64>,
    pub threshold: u32,
    pub reset_at_display: Option<String>,
    pub observed_at_display: Option<String>,
}

async fn quota_preview<S>(
    _auth: AdminAuth,
    State(state): State<S>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let services = state.admin_services();
    let evaluation = services
        .alerts()
        .quota_preview()
        .await
        .map_err(map_admin_service_error)?;
    let time = TimePresenter::new(services.timezone());
    let items = evaluation
        .items
        .into_iter()
        .map(|item| QuotaWarningItemView {
            account_label: item
                .account
                .email
                .filter(|email| !email.is_empty())
                .unwrap_or_else(|| format!("{} ({})", item.account.name, item.account.id)),
            account_id: item.account.id,
            window_key: item.window.key,
            plan_type: item.account.plan_type,
            period: item.window.period,
            used_percent: item.window.used_percent,
            threshold: item.threshold,
            reset_at_display: item.window.reset_at.as_ref().map(|at| time.datetime(at)),
            observed_at_display: item
                .account
                .observed_at
                .as_ref()
                .map(|at| time.datetime(at)),
        })
        .collect();
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(QuotaWarningPreviewView {
            checked_at_display: evaluation
                .status
                .checked_at
                .as_ref()
                .map(|at| time.datetime(at)),
            status: evaluation.status,
            items,
        }),
    ))
}

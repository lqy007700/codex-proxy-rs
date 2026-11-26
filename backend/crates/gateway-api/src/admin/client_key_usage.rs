//! 管理员查看单个密钥的 7 天与 30 天用量和模型统计接口

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use chrono::Utc;
use gateway_admin::model::observability::{KeyStatisticsOverview, KeyUsageMetric, TimeRange};
use gateway_core::policy::ClientApiKeyId;
use serde::{Deserialize, Serialize};

use super::{
    AdminAuth, AdminEnvelope, AdminError, AdminQuery, AdminResponse, client_keys::ClientBudgetView,
    wire::map_admin_service_error,
};
use crate::{auth::SessionState, time::TimePresenter};

pub(super) fn router<S: SessionState + Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/admin/client-keys/statistics", get(statistics::<S>))
        .route("/api/admin/client-keys/statistics/models", get(models::<S>))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StatisticsQuery {
    id: String,
    period: Option<String>,
    #[serde(default)]
    metric: KeyUsageMetric,
    as_of: Option<i64>,
}

fn key_id(value: String) -> Result<ClientApiKeyId, AdminError> {
    ClientApiKeyId::new(value).map_err(|_| AdminError::bad_request("API Key ID 无效"))
}

#[derive(Serialize)]
struct Statistics {
    budget: ClientBudgetView,
    dates: Vec<chrono::NaiveDate>,
    #[serde(rename = "asOf")]
    as_of: i64,
    #[serde(flatten)]
    overview: KeyStatisticsOverview,
}

async fn statistics<S: SessionState>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<StatisticsQuery>,
) -> Result<impl IntoResponse, AdminError> {
    let services = state.admin_services();
    let id = key_id(query.id.clone())?;
    let tz = services.timezone();
    let range = statistics_range(&query, tz)?;
    let budget = services
        .client_keys()
        .budget(&id)
        .await
        .map_err(map_admin_service_error)?;
    let overview = services
        .observability()
        .client_key_statistics(range, &id, query.metric)
        .await
        .map_err(map_admin_service_error)?;
    let end = tz.local(range.end).date_naive();
    let dates = tz
        .local(range.start)
        .date_naive()
        .iter_days()
        .take_while(|date| *date <= end)
        .collect();
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(Statistics {
            budget: ClientBudgetView::from((budget, TimePresenter::new(tz))),
            dates,
            as_of: range.end.timestamp_millis(),
            overview,
        }),
    ))
}

fn statistics_range(
    query: &StatisticsQuery,
    tz: gateway_core::time::DeploymentTimeZone,
) -> Result<TimeRange, AdminError> {
    let period = query.period.as_deref().unwrap_or("7d");
    if !matches!(period, "7d" | "30d") {
        return Err(AdminError::bad_request("统计周期须为 7 天或 30 天"));
    }
    crate::time::query_range(
        None,
        None,
        Some(period),
        Some(query.as_of.unwrap_or_else(|| Utc::now().timestamp_millis())),
        tz,
        "7d",
    )
    .map_err(|_| AdminError::bad_request("统计时间范围无效"))
}

async fn models<S: SessionState>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<StatisticsQuery>,
) -> Result<impl IntoResponse, AdminError> {
    let services = state.admin_services();
    let range = statistics_range(&query, services.timezone())?;
    let id = key_id(query.id)?;
    services
        .client_keys()
        .budget(&id)
        .await
        .map_err(map_admin_service_error)?;
    let items = services
        .observability()
        .client_key_models(range, &id)
        .await
        .map_err(map_admin_service_error)?;
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(items)))
}

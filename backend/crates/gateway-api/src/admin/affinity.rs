//! 亲和会话查询、身份来源及精确操作的管理 API

use super::{AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminQuery};
use crate::auth::SessionState;
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use gateway_admin::model::affinity::{AffinityPreviewRequest, AffinityQuery, IdentitySource};
use serde::{Deserialize, Serialize};

fn service<S: SessionState>(state: &S) -> Result<&gateway_admin::AffinityService, AdminError> {
    state
        .admin_services()
        .affinity()
        .ok_or_else(AdminError::service_unavailable)
}

fn response<T>(data: T) -> Json<AdminEnvelope<T>> {
    Json(AdminEnvelope::ok(data))
}

fn present<T: Serialize, S: SessionState>(
    state: &S,
    data: T,
) -> Result<Json<AdminEnvelope<serde_json::Value>>, AdminError> {
    let mut value = serde_json::to_value(data).map_err(|_| AdminError::service_unavailable())?;
    let time = crate::time::TimePresenter::new(state.admin_services().timezone());
    fn visit(value: &mut serde_json::Value, time: crate::time::TimePresenter) {
        match value {
            serde_json::Value::Array(items) => items.iter_mut().for_each(|item| visit(item, time)),
            serde_json::Value::Object(object) => {
                for key in [
                    "firstObservedAt",
                    "lastAdmissionAt",
                    "expiresAt",
                    "createdAt",
                ] {
                    if let Some(display) = object
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .and_then(|value| time.rfc_display(Some(value)))
                    {
                        object.insert(format!("{key}Display"), display.into());
                    }
                }
                object.values_mut().for_each(|item| visit(item, time));
            }
            _ => {}
        }
    }
    visit(&mut value, time);
    Ok(response(value))
}

fn map(error: gateway_admin::model::AdminError) -> AdminError {
    super::wire::map_admin_service_error(error)
}

pub fn router<S: SessionState + Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/admin/affinity/sessions", get(list::<S>))
        .route("/api/admin/affinity/sources", get(sources::<S>))
        .route("/api/admin/affinity/sources/save", post(save_source::<S>))
        .route("/api/admin/affinity/preview", post(preview::<S>))
        .route("/api/admin/affinity/apply", post(apply::<S>))
        .route("/api/admin/affinity/operations", get(history::<S>))
}

async fn list<S: SessionState + Send + Sync>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<AffinityQuery>,
) -> Result<Json<AdminEnvelope<serde_json::Value>>, AdminError> {
    let page = service(&state)?.list(&query).await.map_err(map)?;
    // 列表投影携带名称，审计计划仍使用原持久化合同
    let items: Vec<_> = page
        .items
        .into_iter()
        .map(|session| AffinityListItem {
            token_name: session.observation.token_name.clone(),
            session,
        })
        .collect();
    present(
        &state,
        AffinityListPage {
            items,
            next_cursor: page.next_cursor,
            coverage: page.coverage,
        },
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AffinityListItem {
    #[serde(flatten)]
    session: gateway_admin::model::affinity::AffinitySession,
    token_name: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AffinityListPage {
    items: Vec<AffinityListItem>,
    next_cursor: Option<String>,
    coverage: &'static str,
}

async fn sources<S: SessionState + Send + Sync>(
    _auth: AdminAuth,
    State(state): State<S>,
) -> Result<Json<AdminEnvelope<Vec<IdentitySource>>>, AdminError> {
    Ok(response(service(&state)?.sources().await.map_err(map)?))
}

async fn save_source<S: SessionState + Send + Sync>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(source): AdminJson<IdentitySource>,
) -> Result<Json<AdminEnvelope<Saved>>, AdminError> {
    service(&state)?
        .save_source(&auth.context().mutation_context(), source)
        .await
        .map_err(map)?;
    Ok(response(Saved { saved: true }))
}

#[derive(Serialize)]
struct Saved {
    saved: bool,
}

async fn preview<S: SessionState + Send + Sync>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<AffinityPreviewRequest>,
) -> Result<Json<AdminEnvelope<serde_json::Value>>, AdminError> {
    present(
        &state,
        service(&state)?
            .preview(&auth.context().mutation_context(), command)
            .await
            .map_err(map)?,
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyRequest {
    operation_id: String,
}

async fn apply<S: SessionState + Send + Sync>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(command): AdminJson<ApplyRequest>,
) -> Result<Json<AdminEnvelope<serde_json::Value>>, AdminError> {
    present(
        &state,
        service(&state)?
            .apply(&auth.context().mutation_context(), &command.operation_id)
            .await
            .map_err(map)?,
    )
}

async fn history<S: SessionState + Send + Sync>(
    _auth: AdminAuth,
    State(state): State<S>,
) -> Result<Json<AdminEnvelope<serde_json::Value>>, AdminError> {
    present(&state, service(&state)?.history().await.map_err(map)?)
}

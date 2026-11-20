//! 账号降智测试的提交、历史查询与认证图片交付

use gateway_admin::model::accounts::{AccountModelTest, AccountModelTestKind};

use super::*;
use crate::{admin::AdminPageData, auth::SessionState, time::TimePresenter};

const PAGE_SIZE: u32 = 20;

pub(super) fn router<S>() -> Router<S>
where
    S: SessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/admin/accounts/model-tests",
            get(list::<S>).post(start::<S>),
        )
        .route("/api/admin/accounts/model-tests/detail", get(detail::<S>))
        .route("/api/admin/accounts/model-tests/image", get(image::<S>))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartRequest {
    account_id: String,
    model_id: String,
    kind: AccountModelTestKind,
    prompt: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HistoryQuery {
    account_id: Option<String>,
    page: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TestQuery {
    test_id: String,
}

impl TestQuery {
    fn into_id(self) -> Result<Uuid, AdminError> {
        Uuid::parse_str(&self.test_id)
            .map_err(|_| map_wire_error(WireValidationError::new("testId")))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Started {
    test_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    id: String,
    account_id: String,
    account_name: String,
    model_id: String,
    kind: AccountModelTestKind,
    started_at_display: String,
    status: &'static str,
    duration_ms: Option<i64>,
}

impl Summary {
    fn new(row: AccountModelTest, time: TimePresenter) -> Self {
        Self {
            id: row.id.to_string(),
            account_id: row.account_id,
            account_name: row.account_name,
            model_id: row.model_id,
            kind: row.kind,
            started_at_display: time.datetime(&row.started_at),
            status: match (row.finished_at, row.failed) {
                (None, _) => "running",
                (Some(_), true) => "failed",
                (Some(_), false) => "succeeded",
            },
            duration_ms: row
                .finished_at
                .map(|at| (at - row.started_at).num_milliseconds()),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Detail {
    #[serde(flatten)]
    summary: Summary,
    prompt: String,
    error: Option<String>,
    text: Option<String>,
    has_image: bool,
}

async fn start<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<StartRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let StartRequest {
        account_id,
        model_id,
        kind,
        prompt,
    } = request;
    let (account_id, model) = AccountTestQuery {
        account_id,
        model_id,
    }
    .into_command()
    .map_err(map_wire_error)?;
    let test_id = state
        .admin_services()
        .accounts()
        .start_model_test(account_id, model, kind, prompt)
        .await
        .map_err(map_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::ACCEPTED,
        AdminEnvelope::ok(Started {
            test_id: test_id.to_string(),
        }),
    ))
}

async fn list<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<HistoryQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let account_id = query
        .account_id
        .map(|account_id| {
            require_account_id(&account_id, "accountId")?;
            ProviderAccountId::new(account_id).map_err(|_| WireValidationError::new("accountId"))
        })
        .transpose()
        .map_err(map_wire_error)?;
    let page = query.page.unwrap_or(1).max(1);
    let services = state.admin_services();
    let (rows, total) = services
        .accounts()
        .list_model_tests(account_id, page, PAGE_SIZE)
        .await
        .map_err(map_service_error)?;
    let time = TimePresenter::new(services.timezone());
    let data = AdminPageData::new(
        rows.into_iter()
            .map(|row| Summary::new(row, time))
            .collect(),
        PageMeta::new(
            page,
            PAGE_SIZE,
            total,
            u32::try_from(total.div_ceil(u64::from(PAGE_SIZE))).unwrap_or(u32::MAX),
        ),
    );
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}

async fn detail<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<TestQuery>,
) -> Result<impl IntoResponse, AdminError>
where
    S: SessionState + Send + Sync,
{
    let services = state.admin_services();
    let row = services
        .accounts()
        .model_test_detail(query.into_id()?)
        .await
        .map_err(map_service_error)?;
    let data = Detail {
        summary: Summary::new(row.summary, TimePresenter::new(services.timezone())),
        prompt: row.prompt,
        error: row.error,
        text: row.text,
        has_image: row.has_image,
    };
    Ok(AdminResponse::new(StatusCode::OK, AdminEnvelope::ok(data)))
}

async fn image<S>(
    _auth: AdminAuth,
    State(state): State<S>,
    AdminQuery(query): AdminQuery<TestQuery>,
) -> Result<Response, AdminError>
where
    S: SessionState + Send + Sync,
{
    let (media_type, bytes) = state
        .admin_services()
        .accounts()
        .model_test_image(query.into_id()?)
        .await
        .map_err(map_service_error)?;
    let content_type = match media_type.as_str() {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => media_type,
        _ => return Err(AdminError::bad_request("测试图片格式不受支持")),
    };
    Ok((
        [
            ("content-type", content_type.as_str()),
            ("x-content-type-options", "nosniff"),
            ("content-security-policy", "default-src 'none'"),
        ],
        bytes,
    )
        .into_response())
}

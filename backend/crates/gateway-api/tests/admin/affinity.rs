//! 验证亲和管理全部路由的管理员认证与服务未装配边界

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

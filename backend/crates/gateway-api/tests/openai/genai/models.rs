//! 原生模型目录的分页、详情、鉴权与能力投影

use super::*;
use gateway_core::{
    operation::OperationKind,
    routing::{
        ConfigRevision, ModelCapabilities, ProviderKind, ProviderModel, RuntimeSnapshot,
        UpstreamModelId,
    },
};

fn client() -> AuthenticatedClient {
    let source = authenticated_client(KEY);
    let caps = ModelCapabilities::new(
        [OperationKind::Generate, OperationKind::CountTokens].into(),
        None,
    )
    .with_generation_protocol(GENAI_PROTOCOL);
    let provider = ProviderKind::new("openai").unwrap();
    let models = ["model-a", "model-b", "model-c", "unreachable/name"].map(|id| {
        ProviderModel::new(
            provider.clone(),
            UpstreamModelId::new(id).unwrap(),
            caps.clone(),
        )
    });
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        source.snapshot().settings().clone(),
        vec![provider],
        models.into(),
        vec![source.policy().clone()],
    )
    .unwrap();
    super::super::authenticated_client_from_snapshot(KEY, snapshot)
}

async fn query(router: &axum::Router, path: &str, authorized: bool) -> (StatusCode, Value) {
    let mut request = Request::get(path);
    if authorized {
        request = request.header("authorization", format!("Bearer {KEY}"));
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    (status, value)
}

#[tokio::test]
async fn list_pages_and_details_expose_only_supported_native_models() {
    let execution = Arc::new(Execution {
        client: Some(client()),
        ..Default::default()
    });
    let router = api_router(execution.clone()).await;
    let (status, first) = query(&router, "/v1beta/models?pageSize=2", true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["models"].as_array().unwrap().len(), 2);
    assert_eq!(first["models"][0]["name"], "models/model-a");
    assert_eq!(
        first["models"][0]["supportedGenerationMethods"],
        json!(["generateContent", "streamGenerateContent", "countTokens"])
    );
    assert!(first["models"][0].get("inputTokenLimit").is_none());
    let token = first["nextPageToken"].as_str().unwrap();
    let (_, second) = query(
        &router,
        &format!("/v1beta/models?pageSize=2&pageToken={token}"),
        true,
    )
    .await;
    assert_eq!(
        second["models"],
        json!([{"name":"models/model-c","displayName":"model-c","supportedGenerationMethods":["generateContent","streamGenerateContent","countTokens"]}])
    );
    assert!(second.get("nextPageToken").is_none());
    let (status, _) = query(
        &router,
        &format!("/v1beta/models?pageSize=1&pageToken={token}"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, detail) = query(&router, "/v1beta/models/model-a", true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail, first["models"][0]);
    assert!(execution.seen.lock().unwrap().is_none());
    let (_, empty) = query(
        &api_router(Arc::new(Execution::default())).await,
        "/v1beta/models",
        true,
    )
    .await;
    assert_eq!(empty, json!({"models":[]}));
}

#[tokio::test]
async fn directory_errors_follow_google_rpc_and_require_authentication() {
    let router = api_router(Arc::new(Execution {
        client: Some(client()),
        ..Default::default()
    }))
    .await;
    for (path, authorized, status) in [
        ("/v1beta/models", false, StatusCode::UNAUTHORIZED),
        ("/v1beta/models/model-a", false, StatusCode::UNAUTHORIZED),
        ("/v1beta/models/unknown", true, StatusCode::NOT_FOUND),
        ("/v1beta/models?pageSize=-1", true, StatusCode::BAD_REQUEST),
        (
            "/v1beta/models?pageToken=bad",
            true,
            StatusCode::BAD_REQUEST,
        ),
        ("/v1beta/models?pageSize=abc", true, StatusCode::BAD_REQUEST),
    ] {
        let (actual, body) = query(&router, path, authorized).await;
        assert_eq!(actual, status, "{path}");
        assert_eq!(body["error"]["code"], status.as_u16(), "{path}");
    }
}

#[tokio::test]
async fn google_api_key_reads_the_same_authorized_catalog() {
    let router = api_router(Arc::new(Execution {
        client: Some(client()),
        ..Default::default()
    }))
    .await;
    for path in ["/v1beta/models", "/v1beta/models/model-a"] {
        let (expected_status, expected) = query(&router, path, true).await;
        let request = Request::get(path)
            .header("x-goog-api-key", KEY)
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected_status);
        let actual: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
        assert_eq!(actual, expected);
    }
}

#[tokio::test]
async fn native_metadata_keeps_snapshot_order_and_missing_public_models() {
    use gateway_core::{operation::RawJsonPayload, routing::PublicModelDescriptor};
    let source = client();
    let settings =
        source
            .snapshot()
            .settings()
            .clone()
            .with_model_mappings(std::collections::BTreeMap::from([(
                "alias-a".into(),
                "model-a".into(),
            )]));
    let client = super::super::authenticated_client_from_snapshot(
        KEY,
        source.snapshot().with_settings(&settings).unwrap(),
    );
    let document = json!({
        "displayName":"Upstream", "description":"Description", "maxTokens":1048576,
        "maxOutputTokens":65536, "future":{"capability":true},
        "name":"upstream-name", "supportedGenerationMethods":["unsupported"]
    });
    let payload = RawJsonPayload::new(
        GENAI_PROTOCOL,
        serde_json::to_vec(&document).unwrap().into(),
    )
    .unwrap();
    let router = api_router(Arc::new(Execution {
        client: Some(client),
        catalog: Some(Ok(["model-a", "alias-a", "not-authorized"]
            .into_iter()
            .map(|id| PublicModelDescriptor::Native {
                model: PublicModelId::new(id).unwrap(),
                payload: payload.clone(),
            })
            .collect())),
        ..Default::default()
    }))
    .await;
    let (_, first) = query(&router, "/v1beta/models?pageSize=2", true).await;
    let token = first["nextPageToken"].as_str().unwrap();
    let (_, second) = query(
        &router,
        &format!("/v1beta/models?pageSize=2&pageToken={token}"),
        true,
    )
    .await;
    let entries: Vec<_> = first["models"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["models"].as_array().unwrap())
        .collect();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "models/alias-a",
            "models/model-a",
            "models/model-b",
            "models/model-c"
        ]
    );
    for entry in &entries[..2] {
        assert_eq!(entry["displayName"], "Upstream");
        assert_eq!(entry["description"], "Description");
        assert_eq!(entry["maxTokens"], 1048576);
        assert_eq!(entry["maxOutputTokens"], 65536);
        assert_eq!(entry["future"], json!({"capability":true}));
        assert_eq!(
            entry["supportedGenerationMethods"],
            json!(["generateContent", "streamGenerateContent", "countTokens"])
        );
    }
    assert_eq!(entries[2]["displayName"], "model-b");
    for entry in entries {
        let (_, detail) = query(
            &router,
            &format!("/v1beta/{}", entry["name"].as_str().unwrap()),
            true,
        )
        .await;
        assert_eq!(entry, &detail);
    }
}

#[tokio::test]
async fn grouped_models_and_image_remain_discoverable_when_native_group_is_missing() {
    use gateway_core::{
        account::ProviderAccountId,
        operation::RawJsonPayload,
        routing::{PublicModelDescriptor, ReasoningModelVariant},
    };
    use std::collections::{BTreeMap, BTreeSet};
    let source = authenticated_client(KEY);
    let provider = ProviderKind::new("openai").unwrap();
    let account = ProviderAccountId::new("acct_api_test").unwrap();
    let caps = ModelCapabilities::new([OperationKind::Generate].into(), None)
        .with_generation_protocol(GENAI_PROTOCOL);
    let group = caps.clone().with_reasoning_variants(BTreeMap::from([(
        "medium".into(),
        ReasoningModelVariant {
            model: UpstreamModelId::new("family-medium").unwrap(),
            accounts: BTreeSet::from([account]),
        },
    )]));
    let settings = source
        .snapshot()
        .settings()
        .clone()
        .with_model_mappings(BTreeMap::from([("family-alias".into(), "family".into())]));
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        settings,
        vec![provider.clone()],
        [
            ("family", group),
            ("family-medium", caps.clone()),
            ("image", caps),
        ]
        .into_iter()
        .map(|(id, capabilities)| {
            ProviderModel::new(
                provider.clone(),
                UpstreamModelId::new(id).unwrap(),
                capabilities,
            )
        })
        .collect(),
        vec![source.policy().clone()],
    )
    .unwrap();
    let payload = RawJsonPayload::new(
        GENAI_PROTOCOL,
        bytes::Bytes::from_static(br#"{"displayName":"Image"}"#),
    )
    .unwrap();
    let router = api_router(Arc::new(Execution {
        client: Some(super::super::authenticated_client_from_snapshot(
            KEY, snapshot,
        )),
        catalog: Some(Ok(vec![PublicModelDescriptor::Native {
            model: PublicModelId::new("image").unwrap(),
            payload,
        }])),
        ..Default::default()
    }))
    .await;
    let (_, body) = query(&router, "/v1beta/models", true).await;
    let models = body["models"].as_array().unwrap();
    for name in ["family", "family-alias", "image"] {
        let entry = models
            .iter()
            .find(|entry| entry["name"] == format!("models/{name}"))
            .unwrap();
        assert_eq!(
            entry["supportedGenerationMethods"],
            json!(["generateContent", "streamGenerateContent"])
        );
        let (status, detail) = query(&router, &format!("/v1beta/models/{name}"), true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(entry, &detail);
    }
    assert_eq!(
        models
            .iter()
            .find(|entry| entry["name"] == "models/image")
            .unwrap()["displayName"],
        "Image"
    );
}

#[tokio::test]
async fn snapshot_directory_survives_missing_or_empty_native_metadata() {
    let empty_name = gateway_core::operation::RawJsonPayload::new(
        GENAI_PROTOCOL,
        Bytes::from_static(br#"{"displayName":""}"#),
    )
    .unwrap();
    for catalog in [
        Err(ProviderCatalogUnavailable),
        Ok(vec![PublicModelDescriptor::Native {
            model: PublicModelId::new("model-a").unwrap(),
            payload: empty_name,
        }]),
    ] {
        let router = api_router(Arc::new(Execution {
            client: Some(client()),
            catalog: Some(catalog),
            ..Default::default()
        }))
        .await;
        let (status, list) = query(&router, "/v1beta/models", true).await;
        assert_eq!(status, StatusCode::OK);
        let (status, detail) = query(&router, "/v1beta/models/model-a", true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["displayName"], "model-a");
        assert_eq!(list["models"][0], detail);
    }
}

#[tokio::test]
async fn google_key_pagination_preserves_authorized_rank_and_legacy_order() {
    use gateway_core::{account::ProviderAccountId, routing::AccountModelMetadata};
    use std::collections::BTreeMap;

    let source = authenticated_client(KEY);
    let provider = ProviderKind::new("openai").unwrap();
    let allowed = ProviderAccountId::new("acct_api_test").unwrap();
    let other = ProviderAccountId::new("acct_other").unwrap();
    let caps = ModelCapabilities::new([OperationKind::Generate].into(), None)
        .with_generation_protocol(GENAI_PROTOCOL);
    let models = [
        ("z-ranked", Some(0)),
        ("b-ranked", Some(1)),
        ("a-legacy", None),
        ("c-legacy", None),
    ]
    .into_iter()
    .map(|(name, rank)| {
        ProviderModel::new(
            provider.clone(),
            UpstreamModelId::new(name).unwrap(),
            caps.clone().with_account_metadata(BTreeMap::from([
                (
                    allowed.clone(),
                    AccountModelMetadata {
                        catalog_order: rank,
                        ..Default::default()
                    },
                ),
                (
                    other.clone(),
                    AccountModelMetadata {
                        catalog_order: Some(if name == "a-legacy" { 0 } else { 99 }),
                        ..Default::default()
                    },
                ),
            ])),
        )
    })
    .collect();
    let snapshot = RuntimeSnapshot::new(
        ConfigRevision::new(1).unwrap(),
        source.snapshot().settings().clone(),
        vec![provider],
        models,
        vec![source.policy().clone()],
    )
    .unwrap();
    let router = api_router(Arc::new(Execution {
        client: Some(super::super::authenticated_client_from_snapshot(
            KEY, snapshot,
        )),
        ..Default::default()
    }))
    .await;
    let mut path = "/v1beta/models?pageSize=2".to_string();
    let mut names = Vec::new();
    loop {
        let response = router
            .clone()
            .oneshot(
                Request::get(&path)
                    .header("x-goog-api-key", KEY)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
        names.extend(
            body["models"]
                .as_array()
                .unwrap()
                .iter()
                .map(|model| model["name"].as_str().unwrap().to_string()),
        );
        match body["nextPageToken"].as_str() {
            Some(token) => path = format!("/v1beta/models?pageSize=2&pageToken={token}"),
            None => break,
        }
    }
    assert_eq!(
        names,
        [
            "models/z-ranked",
            "models/b-ranked",
            "models/a-legacy",
            "models/c-legacy"
        ]
    );
}

//! 已注册资源服务的项目发现、模型查询和 Cloud Code 请求封装

use bytes::Bytes;
use gateway_core::{
    account::OutboundProxy,
    error::{ProviderError, ProviderErrorKind as Kind},
};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{json, value::RawValue};
use std::{collections::BTreeMap, time::Duration};
use url::Url;

use super::failure::{network, sent, successful, unsent};
use crate::profile::ClientProfile;
use gateway_core::engine::middleware::MiddlewareHeader;
// Desktop 独立搜索使用固定任务指令，常规生成与计数使用账号画像
static SEARCH_PROMPT_PART: std::sync::LazyLock<&RawValue> = std::sync::LazyLock::new(|| {
    serde_json::from_str(r#"{"text":"Search the web for the query and return a concise answer grounded in the sources."}"#).expect("valid static search instruction")
});

const MAX_CONTROL_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
pub(crate) struct Catalog {
    pub(crate) project: String,
    #[serde(
        rename = "agentModelOrder",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) agent_model_order: Option<BTreeMap<String, usize>>,
    #[serde(rename = "webSearchModelIds", default)]
    pub(crate) web_search_models: Vec<String>,
    pub(crate) models: BTreeMap<String, Model>,
    #[serde(rename = "groupedModels", default)]
    pub(crate) grouped_models: serde_json::Value,
}

#[derive(Deserialize)]
struct AgentModelSort {
    #[serde(default)]
    groups: Vec<AgentModelSortGroup>,
}

#[derive(Deserialize)]
struct AgentModelSortGroup {
    #[serde(rename = "modelIds", default)]
    model_ids: Vec<String>,
}

fn agent_model_order(sorts: Vec<AgentModelSort>, images: Vec<String>) -> BTreeMap<String, usize> {
    let mut order = BTreeMap::new();
    for id in sorts
        .into_iter()
        .flat_map(|sort| sort.groups)
        .flat_map(|group| group.model_ids)
        .chain(images)
    {
        let index = order.len();
        order.entry(id).or_insert(index);
    }
    order
}

/// 完整保留上游模型对象，能力投影只读取需要的字段
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Model(pub(crate) serde_json::Map<String, serde_json::Value>);

impl Model {
    pub(crate) fn text(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(serde_json::Value::as_str)
    }

    pub(crate) fn name(&self) -> &str {
        self.text("displayName").unwrap_or_default()
    }

    pub(crate) fn positive_integer(&self, key: &str) -> Option<u64> {
        self.0
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .filter(|n| *n > 0)
    }

    pub(crate) fn quota(&self) -> Result<Option<super::account::ModelQuota>, serde_json::Error> {
        self.0
            .get("quotaInfo")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value)
            .transpose()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenerateMode {
    Json,
    Stream,
    WebSearch,
}

impl From<bool> for GenerateMode {
    fn from(stream: bool) -> Self {
        if stream { Self::Stream } else { Self::Json }
    }
}

pub(crate) struct CloudCodeClient {
    client: Client,
    origin: Url,
    system_prompt_part: Box<RawValue>,
}

#[derive(Serialize)]
struct RequestBody<'a> {
    #[serde(flatten)]
    fields: BTreeMap<String, &'a RawValue>,
    #[serde(rename = "systemInstruction")]
    system_instruction: SystemInstruction<'a>,
}

#[derive(Serialize)]
struct SystemInstruction<'a> {
    parts: Vec<&'a RawValue>,
    #[serde(flatten)]
    metadata: BTreeMap<String, &'a RawValue>,
}

impl CloudCodeClient {
    pub(crate) fn new(
        origin: &Url,
        proxy: Option<&OutboundProxy>,
        profile: &ClientProfile,
    ) -> Result<Self, ProviderError> {
        Self::with_client(origin, Self::build_client(proxy, profile)?, profile)
    }

    pub(super) fn build_client(
        proxy: Option<&OutboundProxy>,
        profile: &ClientProfile,
    ) -> Result<Client, ProviderError> {
        let mut builder = Client::builder().no_proxy();
        if let Some(proxy) = proxy {
            let proxy = reqwest::Proxy::all(proxy.expose_url()).map_err(|e| {
                unsent(Kind::ProviderInfrastructureUnavailable).with_source(e.without_url())
            })?;
            builder = builder.proxy(proxy);
        }
        builder
            // 官方 AGY 使用 HTTP/1.1；显式选择 TLS 后端，避免工作区 feature 合并切到系统 TLS
            .use_rustls_tls()
            .http1_only()
            .http1_title_case_headers()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent(profile.user_agent())
            .build()
            .map_err(|e| {
                unsent(Kind::ProviderInfrastructureUnavailable).with_source(e.without_url())
            })
    }

    pub(super) fn with_client(
        origin: &Url,
        client: Client,
        profile: &ClientProfile,
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            client,
            origin: origin.clone(),
            system_prompt_part: serde_json::value::to_raw_value(
                &json!({"text":profile.system_prompt}),
            )
            .map_err(|e| unsent(Kind::ProviderInfrastructureUnavailable).with_source(e))?,
        })
    }

    pub(super) fn post(&self, action: &str, token: &str) -> reqwest::RequestBuilder {
        let mut url = self.origin.clone();
        url.set_path(&format!("/v1internal:{action}"));
        self.client
            .post(url)
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT_ENCODING, "gzip")
    }

    pub(crate) async fn catalog(&self, token: &str) -> Result<Catalog, ProviderError> {
        let project = self.account_info(token).await?.project;
        self.catalog_for_project(token, project).await
    }

    pub(crate) async fn catalog_for_project(
        &self,
        token: &str,
        project: String,
    ) -> Result<Catalog, ProviderError> {
        #[derive(Deserialize)]
        struct Models {
            #[serde(rename = "agentModelSorts", default)]
            agent_model_sorts: Option<Vec<AgentModelSort>>,
            #[serde(rename = "imageGenerationModelIds", default)]
            image_generation_models: Vec<String>,
            #[serde(rename = "webSearchModelIds", default)]
            web_search_models: Vec<String>,
            models: BTreeMap<String, Model>,
            #[serde(rename = "groupedModels", default)]
            grouped_models: serde_json::Value,
        }
        let models: Models = control(
            self.post("fetchAvailableModels", token)
                .json(&json!({"project":project})),
            token,
        )
        .await?;
        if models
            .models
            .keys()
            .any(|id| gateway_core::routing::UpstreamModelId::new(id).is_err())
        {
            return Err(sent(Kind::Protocol));
        }
        Ok(Catalog {
            project,
            agent_model_order: if models.agent_model_sorts.is_some()
                || !models.image_generation_models.is_empty()
            {
                Some(agent_model_order(
                    models.agent_model_sorts.unwrap_or_default(),
                    models.image_generation_models,
                ))
            } else {
                None
            },
            models: models.models,
            grouped_models: models.grouped_models,
            web_search_models: models.web_search_models,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn generate_with_credits(
        &self,
        project: &str,
        model: &str,
        token: &str,
        body: &[u8],
        mode: GenerateMode,
        headers: &[MiddlewareHeader],
        allow_credits: bool,
    ) -> Result<Response, ProviderError> {
        #[derive(Serialize)]
        struct Envelope<'a> {
            project: &'a str,
            model: &'a str,
            #[serde(rename = "requestType", skip_serializing_if = "Option::is_none")]
            request_type: Option<&'static str>,
            #[serde(rename = "enabledCreditTypes", skip_serializing_if = "Option::is_none")]
            enabled_credit_types: Option<[&'static str; 1]>,
            request: RequestBody<'a>,
        }
        let web_search = mode == GenerateMode::WebSearch;
        let request = self.request_body(body, web_search)?;
        let (send, media) = if mode == GenerateMode::Stream {
            (
                self.post("streamGenerateContent", token)
                    .query(&[("alt", "sse")]),
                "text/event-stream",
            )
        } else {
            (self.post("generateContent", token), "application/json")
        };
        self.send(
            send.json(&Envelope {
                project,
                model,
                request_type: web_search.then_some("web_search"),
                enabled_credit_types: allow_credits.then_some([super::account::GOOGLE_ONE_AI]),
                request,
            }),
            token,
            headers,
            media,
        )
        .await
    }

    pub(crate) async fn count_tokens(
        &self,
        token: &str,
        body: &[u8],
        headers: &[MiddlewareHeader],
    ) -> Result<Response, ProviderError> {
        let mut request = self.request_body(body, false)?;
        // Desktop 的计数封套只接受 Vertex CountTokensRequest，没有项目和生成控制字段
        for field in [
            "model",
            "safetySettings",
            "safety_settings",
            "toolConfig",
            "tool_config",
            "labels",
            "sessionId",
            "session_id",
        ] {
            request.fields.remove(field);
        }
        self.send(
            self.post("countTokens", token)
                .json(&BTreeMap::from([("request", request)])),
            token,
            headers,
            "application/json",
        )
        .await
    }

    fn request_body<'a>(
        &'a self,
        body: &'a [u8],
        web_search: bool,
    ) -> Result<RequestBody<'a>, ProviderError> {
        let invalid = |error| unsent(Kind::InvalidRequest).with_source(error);
        let mut fields: BTreeMap<String, &RawValue> =
            serde_json::from_slice(body).map_err(invalid)?;
        let alias = fields.remove("system_instruction");
        let client = fields.remove("systemInstruction").or(alias);
        let prompt = if web_search {
            *SEARCH_PROMPT_PART
        } else {
            self.system_prompt_part.as_ref()
        };
        let mut parts = vec![prompt];
        let mut metadata: BTreeMap<String, &RawValue> = BTreeMap::new();
        // 客户端规则与工具说明跟在画像提示后，独立搜索只使用固定任务指令
        if let Some(client) = client.filter(|value| !web_search && value.get() != "null") {
            metadata = serde_json::from_str(client.get()).map_err(invalid)?;
            if let Some(client_parts) = metadata.remove("parts") {
                parts.extend(
                    serde_json::from_str::<Vec<&RawValue>>(client_parts.get()).map_err(invalid)?,
                );
            }
        }
        Ok(RequestBody {
            fields,
            system_instruction: SystemInstruction { parts, metadata },
        })
    }

    async fn send(
        &self,
        mut send: reqwest::RequestBuilder,
        token: &str,
        headers: &[MiddlewareHeader],
        media: &str,
    ) -> Result<Response, ProviderError> {
        for header in headers {
            if super::headers::managed_header(header.name()) {
                continue;
            }
            let name = reqwest::header::HeaderName::from_bytes(header.name().as_bytes())
                .map_err(|e| unsent(Kind::InvalidRequest).with_source(e))?;
            let value = reqwest::header::HeaderValue::from_bytes(header.value())
                .map_err(|e| unsent(Kind::InvalidRequest).with_source(e))?;
            send = send.header(name, value);
        }
        let response = send.send().await.map_err(network)?;
        let response = successful(super::response::decode(response), token).await?;
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case(media))
        {
            return Err(sent(Kind::Protocol));
        }
        Ok(response)
    }
}

pub(super) async fn control<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    token: &str,
) -> Result<T, ProviderError> {
    let mut response = successful(
        super::response::decode(
            request
                .timeout(Duration::from_secs(15))
                .send()
                .await
                .map_err(network)?,
        ),
        token,
    )
    .await?;
    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| sent(Kind::Transport).with_source(e.without_url()))?
    {
        if data.len().saturating_add(chunk.len()) > MAX_CONTROL_RESPONSE_BYTES {
            return Err(sent(Kind::MessageTooBig));
        }
        data.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&data).map_err(|e| sent(Kind::Protocol).with_source(e))
}

#[derive(Deserialize)]
struct Reply<'a> {
    #[serde(borrow)]
    response: Option<&'a RawValue>,
    #[serde(borrow)]
    error: Option<&'a RawValue>,
}

pub(crate) fn unwrap_frame(data: &str) -> Result<Bytes, ProviderError> {
    let envelope: Reply =
        serde_json::from_str(data).map_err(|e| sent(Kind::Protocol).with_source(e))?;
    match (envelope.response, envelope.error) {
        (None, Some(err)) if err.get().starts_with('{') => {
            Ok(Bytes::copy_from_slice(data.as_bytes()))
        }
        (Some(body), None) if body.get().starts_with('{') => {
            Ok(Bytes::copy_from_slice(body.get().as_bytes()))
        }
        _ => Err(sent(Kind::Protocol)),
    }
}

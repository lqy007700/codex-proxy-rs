//! xAI API Key 凭据的格式、上游地址与管理端创建合同

use gateway_core::account::{
    AccountModelAccess, CredentialRevision, CredentialState, LoadedCredential, NewProviderAccount,
    OutboundProxy, PlaintextCredential, ProviderAccount, ProviderAccountId, QuotaState,
};
use gateway_core::routing::ProviderKind;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use url::Url;
use uuid::Uuid;

use crate::{SecretValue, XAI_PROVIDER_NAME};

use super::GrokCredentialRepositoryError as Error;

pub(crate) const AUTHENTICATION_KIND: &str = "api_key";

#[derive(Debug)]
pub(crate) struct ApiKeyCredential {
    pub(crate) base_url: Url,
    pub(crate) key: SecretValue,
}

impl ApiKeyCredential {
    fn new(base_url: &str, key: SecretValue) -> Result<Self, Error> {
        let url = Url::parse(base_url)
            .ok()
            .filter(|url| {
                base_url.len() <= 2048
                    && !base_url.chars().any(char::is_control)
                    && matches!(url.scheme(), "https" | "http")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
            })
            .ok_or(Error::InvalidInput("base_url"))?;
        if key.is_empty()
            || key.len() > 16 * 1024
            || !key.expose().bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(Error::InvalidInput("api_key"));
        }
        Ok(Self { base_url: url, key })
    }

    pub(crate) fn endpoint(&self, resource: &str) -> Url {
        let mut url = self.base_url.clone();
        // 解析阶段已经确认地址为含主机的 HTTP(S) URL
        url.path_segments_mut()
            .expect("HTTP URL supports path segments")
            .pop_if_empty()
            .push(resource);
        url
    }

    pub(crate) fn decode(loaded: &LoadedCredential) -> Result<Self, Error> {
        if loaded.account.provider().as_str() != XAI_PROVIDER_NAME
            || loaded.account.authentication_kind() != AUTHENTICATION_KIND
        {
            return Err(Error::WrongProviderKind);
        }
        let data = loaded.credential.expose_to_provider();
        let (Some(base_url), Some(key)) = (
            data.get("base_url").and_then(Value::as_str),
            data.get("api_key").and_then(Value::as_str),
        ) else {
            return Err(Error::InvalidCredentialData(None));
        };
        if data.get("schema_version").and_then(Value::as_u64) != Some(1) {
            return Err(Error::InvalidCredentialData(None));
        }
        Self::new(base_url, SecretValue::new(key)).map_err(|_| Error::InvalidCredentialData(None))
    }

    pub(crate) fn encode(&self) -> PlaintextCredential {
        PlaintextCredential::new(Map::from_iter([
            ("schema_version".to_owned(), json!(1)),
            ("base_url".to_owned(), json!(self.base_url.as_str())),
            ("api_key".to_owned(), json!(self.key.expose())),
        ]))
    }

    pub(crate) fn configuration(&self) -> Map<String, Value> {
        Map::from_iter([
            ("base_url".to_owned(), json!(self.base_url.as_str())),
            ("protocol".to_owned(), json!("responses")),
            ("transport".to_owned(), json!("http")),
        ])
    }

    pub(crate) fn rotate(self, material: Map<String, Value>) -> Result<Self, Error> {
        let rotation: ConnectionUpdate = serde_json::from_value(Value::Object(material))
            .map_err(|_| Error::InvalidInput("connection"))?;
        Self::new(
            &rotation.base_url,
            rotation.api_key.map_or(self.key, SecretValue::new),
        )
    }

    pub(crate) fn create(
        item: Value,
        provider: &ProviderKind,
        default_proxy: Option<&OutboundProxy>,
    ) -> Result<NewProviderAccount, Error> {
        let entry: ImportAccount =
            serde_json::from_value(item).map_err(|_| Error::InvalidInput("api_key_account"))?;
        if entry
            .provider
            .as_deref()
            .is_some_and(|value| value != XAI_PROVIDER_NAME)
        {
            return Err(Error::InvalidInput("provider"));
        }
        let name = entry.name.trim();
        if name.is_empty() || name.len() > 512 || name.chars().any(char::is_control) {
            return Err(Error::InvalidInput("name"));
        }
        let data = Self::new(&entry.base_url, SecretValue::new(entry.api_key))?;
        let account = ProviderAccount::new(
            ProviderAccountId::new(format!("acct_{}", Uuid::now_v7().simple()))
                .map_err(|_| Error::InvalidInput("account_id"))?,
            provider.clone(),
            name.to_owned(),
            None,
            AUTHENTICATION_KIND.to_owned(),
            CredentialRevision::new(1).map_err(|_| Error::RevisionOverflow)?,
            None,
        )
        .with_account_facts(
            true,
            CredentialState::Ready,
            QuotaState::unknown(),
            None,
            None,
        )
        .with_outbound_proxy(match entry.outbound_proxy_url {
            Some(url) => Some(
                OutboundProxy::parse(&url).map_err(|_| Error::InvalidInput("outboundProxyUrl"))?,
            ),
            None => default_proxy.cloned(),
        });
        Ok(NewProviderAccount {
            account,
            credential: data.encode(),
            model_access: entry.model_access,
        })
    }

    pub(crate) fn export(loaded: &LoadedCredential) -> Result<Value, Error> {
        let data = Self::decode(loaded)?;
        let mut entry = json!({
            "authentication_kind": AUTHENTICATION_KIND,
            "name": loaded.account.name(),
            "base_url": data.base_url.as_str(),
            "api_key": data.key.expose(),
            "modelAccess": loaded.account.model_access(),
        });
        if let Some(proxy) = loaded.account.outbound_proxy() {
            entry["outboundProxyUrl"] = json!(proxy.expose_url());
        }
        Ok(entry)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Protocol {
    Responses,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Transport {
    Http,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionUpdate {
    base_url: String,
    api_key: Option<String>,
    #[serde(rename = "protocol")]
    _protocol: Option<Protocol>,
    #[serde(rename = "transport")]
    _transport: Transport,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportAccount {
    provider: Option<String>,
    #[serde(rename = "authentication_kind")]
    _authentication_kind: String,
    name: String,
    base_url: String,
    api_key: String,
    #[serde(rename = "protocol")]
    _protocol: Option<Protocol>,
    #[serde(rename = "transport")]
    _transport: Option<Transport>,
    #[serde(rename = "modelAccess")]
    model_access: Option<AccountModelAccess>,
    #[serde(rename = "outboundProxyUrl")]
    outbound_proxy_url: Option<String>,
}

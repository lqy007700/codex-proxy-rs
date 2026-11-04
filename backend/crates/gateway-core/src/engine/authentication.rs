//! 数据面有界认证信封与原生 Client Key 身份输入

use std::{fmt, sync::Arc};

use crate::policy::PlaintextClientApiKey;

pub const MAXIMUM_AUTHORIZATION_BYTES: usize = 8 * 1024;

/// API 从数据面 Authorization 头构造的有界认证信封
#[derive(Clone)]
pub struct ClientAuthenticationRequest {
    invocation: Option<Arc<super::extensions::InvocationOrigin>>,
    request_context: Option<Arc<super::extensions::RequestContext>>,
    settings: Option<crate::routing::request_settings::RequestSettings>,
    forwarded_caller: crate::policy::ForwardedCallerHeaders,
    authorization: Arc<str>,
    native_bearer: Option<PlaintextClientApiKey>,
}

impl ClientAuthenticationRequest {
    #[must_use]
    pub fn with_forwarded_caller(mut self, headers: crate::policy::ForwardedCallerHeaders) -> Self {
        self.forwarded_caller = headers;
        self
    }

    #[must_use]
    pub const fn forwarded_caller(&self) -> &crate::policy::ForwardedCallerHeaders {
        &self.forwarded_caller
    }

    /// 子认证继承受信调用的来源、取消和副作用归属，不据此跳过认证器
    #[must_use]
    pub fn with_invocation(mut self, origin: super::extensions::InvocationOrigin) -> Self {
        self.invocation = Some(Arc::new(origin));
        self
    }

    #[must_use]
    pub fn invocation(&self) -> Option<&super::extensions::InvocationOrigin> {
        self.invocation.as_deref()
    }

    #[must_use]
    pub fn with_request_context(
        mut self,
        request: Option<Arc<super::extensions::RequestContext>>,
    ) -> Self {
        self.request_context = request;
        self
    }
    #[must_use]
    pub fn request_context(&self) -> Option<&super::extensions::RequestContext> {
        self.invocation
            .as_ref()
            .and_then(|origin| origin.extensions.request())
            .or(self.request_context.as_deref())
    }

    #[must_use]
    pub fn shared_request_context(&self) -> Option<Arc<super::extensions::RequestContext>> {
        self.invocation
            .as_ref()
            .and_then(|origin| origin.extensions.shared_request())
            .or_else(|| self.request_context.clone())
    }

    #[must_use]
    pub fn with_settings(
        mut self,
        settings: crate::routing::request_settings::RequestSettings,
    ) -> Self {
        self.settings = Some(settings);
        self
    }

    #[must_use]
    pub fn settings(&self) -> Option<&crate::routing::request_settings::RequestSettings> {
        self.settings.as_ref()
    }

    pub(crate) fn take_settings(
        &mut self,
    ) -> Option<crate::routing::request_settings::RequestSettings> {
        self.settings.take()
    }

    /// 创建入口认证信封；HTTP 语法与缺失头仍由 API owner 负责
    pub fn new(authorization: impl Into<String>) -> Result<Self, ClientAuthenticationRequestError> {
        let authorization = authorization.into();
        if authorization.is_empty()
            || authorization.len() > MAXIMUM_AUTHORIZATION_BYTES
            || !authorization.is_ascii()
            || authorization.chars().any(char::is_control)
        {
            return Err(ClientAuthenticationRequestError);
        }
        let native_bearer = authorization
            .strip_prefix("Bearer ")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| PlaintextClientApiKey::new(value.to_owned()).ok());
        Ok(Self {
            forwarded_caller: crate::policy::ForwardedCallerHeaders::default(),
            invocation: None,
            settings: None,
            request_context: None,
            authorization: authorization.into(),
            native_bearer,
        })
    }

    /// 原生调用方显式构造 Bearer 信封；仍经过同一长度与字符校验
    pub fn bearer(plaintext: &str) -> Result<Self, ClientAuthenticationRequestError> {
        Self::new(format!("Bearer {plaintext}"))
    }

    #[must_use]
    pub fn authorization(&self) -> &str {
        &self.authorization
    }

    #[must_use]
    pub(crate) const fn native_bearer(&self) -> Option<&PlaintextClientApiKey> {
        self.native_bearer.as_ref()
    }
}

impl fmt::Debug for ClientAuthenticationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientAuthenticationRequest")
            .field("authorization", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("client authorization is invalid")]
pub struct ClientAuthenticationRequestError;

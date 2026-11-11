//! 额度观测发出时冻结的账号连接身份，不保存代理明文

use sha2::{Digest, Sha256};

use super::{CredentialRevision, OutboundProxy, ProviderAccount, ProviderAccountId};

/// 出口摘要包含托管代理解析后的认证信息，但不暴露明文
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamConnection {
    pub account_id: ProviderAccountId,
    pub credential_revision: CredentialRevision,
    pub revision: [u8; 32],
}

impl UpstreamConnection {
    #[must_use]
    pub fn for_account(account: &ProviderAccount) -> Self {
        Self::new(
            account.id().clone(),
            account.revision(),
            account.outbound_proxy(),
        )
    }

    #[must_use]
    pub fn new(
        account_id: ProviderAccountId,
        credential_revision: CredentialRevision,
        proxy: Option<&OutboundProxy>,
    ) -> Self {
        let mut hash = Sha256::new();
        hash.update(credential_revision.get().to_be_bytes());
        if let Some(proxy) = proxy {
            hash.update(proxy.expose_url().as_bytes());
        }
        let revision = hash.finalize().into();
        Self {
            account_id,
            credential_revision,
            revision,
        }
    }
}

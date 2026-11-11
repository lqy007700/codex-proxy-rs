//! 额度观测连接身份的凭据、代理与容量隔离边界

use gateway_core::account::{
    CredentialRevision, OutboundProxy, ProviderAccountId, UpstreamConnection,
};

#[test]
fn connection_identity_changes_with_credentials_and_proxy_authentication() {
    let id = ProviderAccountId::new("acct_connection").expect("account id");
    let revision = CredentialRevision::new(1).expect("revision");
    let direct = UpstreamConnection::new(id.clone(), revision, None);
    let proxy = OutboundProxy::parse("http://user:secret@localhost:8080").expect("proxy");
    let proxied = UpstreamConnection::new(id.clone(), revision, Some(&proxy));
    let changed_proxy =
        OutboundProxy::parse("http://user:changed@localhost:8080").expect("changed proxy");

    assert_ne!(direct, proxied);
    assert_ne!(
        proxied,
        UpstreamConnection::new(id.clone(), revision, Some(&changed_proxy))
    );
    assert_ne!(
        direct,
        UpstreamConnection::new(id, CredentialRevision::new(2).expect("revision"), None)
    );
    assert!(!format!("{proxied:?}").contains("secret"));
    assert!(!format!("{proxied:?}").contains("localhost"));
}

#[test]
fn websocket_capacity_does_not_change_the_http_quota_connection_identity() {
    let account = super::account("acct_capacity");
    let proxy = OutboundProxy::parse("http://localhost:8080").expect("proxy");
    let first = account
        .clone()
        .with_outbound_proxy(Some(proxy.clone().with_max_websocket_connections(1)));
    let second = account.with_outbound_proxy(Some(proxy.with_max_websocket_connections(10)));

    assert_eq!(
        UpstreamConnection::for_account(&first),
        UpstreamConnection::for_account(&second)
    );
}

//! 邮件 TLS 测试使用的本地证书与接收器

use std::sync::Arc;
use tokio_rustls::TlsAcceptor;

pub const ROOT: &[u8] = include_bytes!("certificates/root.der");
pub(crate) fn tls_acceptor(http2: bool) -> TlsAcceptor {
    use rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    };
    let mut config = ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(
            include_bytes!("certificates/server.der").to_vec(),
        )],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            include_bytes!("certificates/server-key.der").to_vec(),
        )),
    )
    .unwrap();
    if http2 {
        config.alpn_protocols = vec![b"h2".to_vec()];
    }
    TlsAcceptor::from(Arc::new(config))
}

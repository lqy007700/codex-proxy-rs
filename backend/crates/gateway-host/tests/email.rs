//! SMTP 真实 TLS 投递、强制 STARTTLS 与秘密隔离测试，所有网络限于本机

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use gateway_admin::{model::alerts::*, ports::alerts::EmailSender as _};
use gateway_host::email::SmtpEmailSender;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    net::TcpListener,
};

fn delivery(port: u16, security: SmtpSecurity) -> AlertDelivery {
    AlertDelivery {
        id: "smtp-fixture".to_owned(),
        attempt: 1,
        settings: AlertSettings {
            revision: 1,
            values: AlertSettingsValues {
                smtp_host: "localhost".to_owned(),
                smtp_port: port,
                smtp_security: security,
                sender_address: "sender@example.test".to_owned(),
                sender_name: "Fixture".to_owned(),
                recipients: vec!["recipient@example.test".to_owned()],
                ..AlertSettingsValues::default()
            },
            password: Some(secrecy::SecretString::from("FAKE_SMTP_PASSWORD")),
            updated_at: Utc::now(),
        },
        recipient: "recipient@example.test".to_owned(),
        subject: "SMTP fixture alert".to_owned(),
        body: "alert fixture payload".to_owned(),
    }
}

#[test]
fn smtp_mailboxes_are_validated_without_connecting() {
    let sender = SmtpEmailSender;
    let mut values = AlertSettingsValues::default();
    assert!(sender.validate_addresses(&values).is_ok());
    values.sender_address = "not a mailbox".to_owned();
    assert!(sender.validate_addresses(&values).is_err());
    values.sender_address = "sender@example.test".to_owned();
    values.recipients = vec!["victim@example.test\r\nBcc: extra@example.test".to_owned()];
    assert!(sender.validate_addresses(&values).is_err());
}

async fn receive_mail<S>(stream: S, authentication_required: bool) -> String
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    let mut message = String::new();
    let mut authenticated = false;
    loop {
        line.clear();
        if stream.read_line(&mut line).await.unwrap() == 0 {
            break;
        }
        let reply: &[u8] = if line.starts_with("EHLO") {
            if authentication_required {
                b"250-localhost\r\n250-AUTH PLAIN\r\n250 8BITMIME\r\n"
            } else {
                b"250-localhost\r\n250 8BITMIME\r\n"
            }
        } else if let Some(encoded) = line.trim_end().strip_prefix("AUTH PLAIN ") {
            let credentials = STANDARD.decode(encoded).unwrap();
            assert!(credentials == b"\0fixture-user\0FAKE_SMTP_PASSWORD");
            authenticated = true;
            b"235 authenticated\r\n"
        } else if line.starts_with("MAIL FROM:") || line.starts_with("RCPT TO:") {
            assert!(!authentication_required || authenticated);
            b"250 OK\r\n"
        } else if line.starts_with("DATA") {
            stream
                .get_mut()
                .write_all(b"354 message\r\n")
                .await
                .unwrap();
            loop {
                line.clear();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == ".\r\n" {
                    break;
                }
                message.push_str(&line);
            }
            b"250 accepted\r\n"
        } else if line.starts_with("QUIT") {
            stream.get_mut().write_all(b"221 bye\r\n").await.unwrap();
            break;
        } else {
            panic!("unexpected fixture SMTP command")
        };
        stream.get_mut().write_all(reply).await.unwrap();
    }
    assert!(!authentication_required || authenticated);
    message
}

async fn run_sender(
    port: u16,
    security: &str,
    host: &str,
    trusted: bool,
    authenticate: bool,
    reject: bool,
) {
    let directory = tempfile::tempdir().unwrap();
    let cert = directory.path().join("root.pem");
    let encoded = STANDARD.encode(crate::support::network::ROOT);
    let pem = if trusted {
        format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            encoded
                .as_bytes()
                .chunks(64)
                .map(|chunk| std::str::from_utf8(chunk).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
        )
    } else {
        String::new()
    };
    std::fs::write(&cert, pem).unwrap();
    // 证书和认证参数只传给隔离子进程，不修改并发测试共享的进程环境
    let child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "email::smtp_delivery_child", "--ignored"])
        .env("CPR_SMTP_TEST_PORT", port.to_string())
        .env("CPR_SMTP_TEST_SECURITY", security)
        .env("CPR_SMTP_TEST_HOST", host)
        .env("CPR_SMTP_TEST_AUTHENTICATE", authenticate.to_string())
        .env("CPR_SMTP_TEST_REJECT", reject.to_string())
        .env("SSL_CERT_FILE", &cert)
        .env("SSL_CERT_DIR", directory.path())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(Duration::from_secs(15), child)
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("FAKE_SMTP_PASSWORD"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("FAKE_SMTP_PASSWORD"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smtp_tls_delivers_a_real_message_to_a_local_receiver() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = crate::support::network::tls_acceptor(false)
            .accept(stream)
            .await
            .unwrap();
        stream
            .write_all(b"220 localhost fixture\r\n")
            .await
            .unwrap();
        receive_mail(stream, false).await
    });
    run_sender(port, "tls", "localhost", true, false, false).await;
    let message = tokio::time::timeout(Duration::from_secs(5), received)
        .await
        .unwrap()
        .unwrap();
    assert!(message.contains("recipient@example.test"));
    assert!(message.contains("alert fixture payload"));
    assert!(!message.contains("FAKE_SMTP_PASSWORD"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starttls_upgrades_before_authentication_and_delivers_the_message() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        stream
            .get_mut()
            .write_all(b"220 localhost fixture\r\n")
            .await
            .unwrap();
        let mut command = String::new();
        stream.read_line(&mut command).await.unwrap();
        assert!(command.starts_with("EHLO"));
        stream
            .get_mut()
            .write_all(b"250-localhost\r\n250-STARTTLS\r\n250 AUTH PLAIN\r\n")
            .await
            .unwrap();
        command.clear();
        stream.read_line(&mut command).await.unwrap();
        assert_eq!(command, "STARTTLS\r\n");
        stream
            .get_mut()
            .write_all(b"220 upgrade\r\n")
            .await
            .unwrap();
        assert!(stream.buffer().is_empty());
        let stream = crate::support::network::tls_acceptor(false)
            .accept(stream.into_inner())
            .await
            .unwrap();
        receive_mail(stream, true).await
    });
    run_sender(port, "start_tls", "localhost", true, true, false).await;
    let message = tokio::time::timeout(Duration::from_secs(5), received)
        .await
        .unwrap()
        .unwrap();
    assert!(message.contains("alert fixture payload"));
    assert!(!message.contains("FAKE_SMTP_PASSWORD"));
}

#[tokio::test]
#[ignore = "由本机 SMTP 接收器测试在隔离证书环境中调用"]
async fn smtp_delivery_child() {
    let port = std::env::var("CPR_SMTP_TEST_PORT")
        .expect("parent fixture port")
        .parse()
        .unwrap();
    let security = if std::env::var("CPR_SMTP_TEST_SECURITY").unwrap() == "start_tls" {
        SmtpSecurity::StartTls
    } else {
        SmtpSecurity::Tls
    };
    let mut request = delivery(port, security);
    request.settings.values.smtp_host = std::env::var("CPR_SMTP_TEST_HOST").unwrap();
    if std::env::var("CPR_SMTP_TEST_AUTHENTICATE").unwrap() == "true" {
        request.settings.values.smtp_username = "fixture-user".to_owned();
    }
    let result = SmtpEmailSender.send(&request).await;
    if std::env::var("CPR_SMTP_TEST_REJECT").unwrap() == "true" {
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), "邮件投递失败");
        assert!(!format!("{error:?}").contains("FAKE_SMTP_PASSWORD"));
    } else {
        result.unwrap();
    }
}

async fn rejects_certificate(host: &str, trusted: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let rejected = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        assert!(
            crate::support::network::tls_acceptor(false)
                .accept(stream)
                .await
                .is_err()
        );
    });
    run_sender(port, "tls", host, trusted, true, true).await;
    tokio::time::timeout(Duration::from_secs(5), rejected)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn smtp_rejects_untrusted_certificates_before_authentication() {
    rejects_certificate("localhost", false).await;
}

#[tokio::test]
async fn smtp_rejects_certificate_hostname_mismatch_before_authentication() {
    // 固定证书仅包含 localhost 与 upstream.test，不包含此回环 IP
    rejects_certificate("127.0.0.1", true).await;
}

#[tokio::test]
async fn smtp_authentication_errors_never_expose_server_echoed_credentials() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let rejected = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = crate::support::network::tls_acceptor(false)
            .accept(stream)
            .await
            .unwrap();
        stream
            .write_all(b"220 localhost fixture\r\n")
            .await
            .unwrap();
        let mut stream = BufReader::new(stream);
        let mut command = String::new();
        stream.read_line(&mut command).await.unwrap();
        assert!(command.starts_with("EHLO"));
        stream
            .get_mut()
            .write_all(b"250-localhost\r\n250 AUTH PLAIN\r\n")
            .await
            .unwrap();
        command.clear();
        stream.read_line(&mut command).await.unwrap();
        assert!(command.starts_with("AUTH PLAIN "));
        // 不可信服务器可能回显秘密，适配器必须把原始错误留在边界内
        stream
            .get_mut()
            .write_all(b"535 authentication failed FAKE_SMTP_PASSWORD\r\n")
            .await
            .unwrap();
    });
    run_sender(port, "tls", "localhost", true, true, true).await;
    tokio::time::timeout(Duration::from_secs(5), rejected)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn starttls_never_downgrades_or_sends_credentials_to_plaintext_server() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream
            .write_all(b"220 localhost fixture\r\n")
            .await
            .unwrap();
        let mut stream = BufReader::new(stream);
        let mut ehlo = String::new();
        stream.read_line(&mut ehlo).await.unwrap();
        assert!(ehlo.starts_with("EHLO"));
        stream
            .get_mut()
            .write_all(b"250-localhost\r\n250 AUTH PLAIN LOGIN\r\n")
            .await
            .unwrap();
        let mut next = String::new();
        let _ = tokio::time::timeout(Duration::from_secs(1), stream.read_line(&mut next)).await;
        assert!(!next.contains("AUTH"));
        assert!(!next.contains("MAIL FROM"));
    });
    let mut request = delivery(port, SmtpSecurity::StartTls);
    request.settings.values.smtp_username = "fixture-user".to_owned();
    let error = SmtpEmailSender.send(&request).await.unwrap_err();
    assert_eq!(error.to_string(), "邮件投递失败");
    assert!(!format!("{error:?}").contains("FAKE_SMTP_PASSWORD"));
    server.await.unwrap();
}

//! 异步 SMTP 邮件适配，连接、认证和发送不进入请求执行链路

use async_trait::async_trait;
use gateway_admin::{
    model::{
        AdminError,
        alerts::{AlertDelivery, AlertSettingsValues, EmailSendError, SmtpSecurity},
    },
    ports::alerts::EmailSender,
};
use lettre::{
    AsyncSmtpTransport, AsyncTransport as _, Message, Tokio1Executor,
    message::Mailbox,
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
};
use secrecy::ExposeSecret as _;
use std::time::Duration;

#[derive(Default)]
pub struct SmtpEmailSender;

#[async_trait]
impl EmailSender for SmtpEmailSender {
    fn validate_addresses(&self, values: &AlertSettingsValues) -> Result<(), AdminError> {
        if !values.sender_address.is_empty() {
            values
                .sender_address
                .parse::<lettre::Address>()
                .map_err(|_| AdminError::invalid("发件人邮箱格式不正确"))?;
        }
        for address in &values.recipients {
            address
                .parse::<lettre::Address>()
                .map_err(|_| AdminError::invalid("接收邮箱格式不正确"))?;
        }
        Ok(())
    }

    async fn send(&self, delivery: &AlertDelivery) -> Result<(), EmailSendError> {
        let values = &delivery.settings.values;
        let from = Mailbox::new(
            (!values.sender_name.is_empty()).then(|| values.sender_name.clone()),
            values.sender_address.parse().map_err(|_| EmailSendError)?,
        );
        let message = Message::builder()
            .from(from)
            .to(Mailbox::new(
                None,
                delivery.recipient.parse().map_err(|_| EmailSendError)?,
            ))
            .subject(&delivery.subject)
            .message_id(Some(format!(
                "{}.{}@codex-proxy.local",
                delivery.id, delivery.settings.revision
            )))
            .body(delivery.body.clone())
            .map_err(|_| EmailSendError)?;
        let tls = TlsParameters::new(values.smtp_host.clone()).map_err(|_| EmailSendError)?;
        let tls = match values.smtp_security {
            SmtpSecurity::Tls => Tls::Wrapper(tls),
            SmtpSecurity::StartTls => Tls::Required(tls),
        };
        let mut transport =
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&values.smtp_host)
                .port(values.smtp_port)
                .tls(tls)
                .timeout(Some(Duration::from_secs(15)));
        if !values.smtp_username.is_empty() {
            let password = delivery.settings.password.as_ref().ok_or(EmailSendError)?;
            transport = transport.credentials(Credentials::new(
                values.smtp_username.clone(),
                password.expose_secret().to_owned(),
            ));
        }
        // 仅发送当前收件人的独立消息，SMTP 错误原文可能包含认证信息，不跨出适配器
        transport
            .build()
            .send(message)
            .await
            .map_err(|_| EmailSendError)?;
        Ok(())
    }
}

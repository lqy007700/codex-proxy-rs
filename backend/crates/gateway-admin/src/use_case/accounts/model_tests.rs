//! 固定账号的后台模型测试、结果提取与保留策略

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gateway_core::engine::probe::AccountModelTestResult;
use uuid::Uuid;

use super::*;
use crate::{
    model::accounts::{AccountModelTestImage, AccountModelTestOutcome},
    ports::provider::ProviderAdmin,
};

const KEEP_PER_ACCOUNT: i64 = 10;
const MAX_PROMPT_BYTES: usize = 32 * 1024;
const MAX_TEXT_BYTES: usize = 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
/// 超过探测链的请求期限后才允许将丢失的后台进程收敛为失败
pub(super) const MODEL_TEST_EXPIRATION: Duration = Duration::minutes(11);

impl DefaultAccountsService {
    pub(super) async fn launch_model_test(
        &self,
        account_id: ProviderAccountId,
        model: UpstreamModelId,
        kind: AccountModelTestKind,
        prompt: String,
    ) -> Result<Uuid, AdminError> {
        if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES || prompt.contains('\0') {
            return Err(AdminError::invalid("请输入不超过 32 KiB 的提示词"));
        }
        let (stored, provider) = self.provider_for_account(&account_id).await?;
        let account = stored.account;
        let operation = if matches!(kind, AccountModelTestKind::Image) {
            provider
                .model_test_image_operation(&model, &prompt)
                .map_err(|error| map_provider_error(error, "provider image model test"))?
        } else {
            provider
                .connection_test_operation(&model, &prompt)
                .await
                .map_err(|error| map_provider_error(error, "provider model test"))?
        };
        let permit = Arc::clone(&self.model_test_slots)
            .try_acquire_owned()
            .map_err(|_| AdminError::conflict("测试任务较多，请稍后重试"))?;
        self.expire_model_tests(Some(account_id.as_str())).await?;
        let id = Uuid::now_v7();
        let detail = AccountModelTestDetail {
            summary: AccountModelTest {
                id,
                account_id: account_id.as_str().to_owned(),
                account_name: account
                    .display_name
                    .or(account.email)
                    .unwrap_or(account.name),
                model_id: model.as_str().to_owned(),
                kind,
                started_at: Utc::now(),
                finished_at: None,
                failed: false,
            },
            prompt,
            error: None,
            text: None,
            has_image: false,
        };
        let store = Arc::clone(&self.accounts);
        let probe = Arc::clone(&self.probe);
        let (accepted, acknowledgement) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            // HTTP 客户端断开时，写入记录与执行仍由服务端任务持有
            match store.begin_model_test(&detail).await {
                Ok(()) => {
                    let _ = accepted.send(Ok(id));
                }
                Err(error) => {
                    let _ = accepted.send(Err(
                        if error.kind() == crate::ports::store::AdminStoreErrorKind::Conflict {
                            AdminError::conflict("该账号已有测试正在运行")
                        } else {
                            map_store_error(error, "model tests")
                        },
                    ));
                    return;
                }
            }
            let outcome = match probe
                .test_model(AccountProbeRequest {
                    target: AccountProbeTarget::ModelTest(account_id),
                    provider_kind: account.provider_kind,
                    upstream_model: model,
                    operation,
                })
                .await
            {
                Ok(result) => test_outcome(provider.as_ref(), kind, result),
                Err(error) => {
                    let message = error.client_message();
                    let message = &message[..message.floor_char_boundary(4096)];
                    AccountModelTestOutcome::Failed(format!(
                        "{} · {}",
                        error.kind().as_str(),
                        message.replace('\0', "�")
                    ))
                }
            };
            if let Err(error) = store
                .finish_model_test(id, &detail.summary.account_id, outcome, KEEP_PER_ACCOUNT)
                .await
            {
                tracing::warn!(test_id = %id, error_kind = ?error.kind(), "模型测试结果写入失败");
            }
            drop(permit);
        });
        acknowledgement
            .await
            .map_err(|_| AdminError::internal("测试任务启动失败"))?
    }

    pub(super) async fn load_model_test_detail(
        &self,
        id: Uuid,
    ) -> Result<AccountModelTestDetail, AdminError> {
        self.accounts
            .model_test_detail(id)
            .await
            .map_err(|error| map_store_error(error, "model tests"))?
            .ok_or_else(|| AdminError::not_found("测试记录不存在或已过期"))
    }

    pub(super) async fn expire_model_tests(
        &self,
        account_id: Option<&str>,
    ) -> Result<(), AdminError> {
        let expired = self
            .accounts
            .expired_model_tests(account_id, Utc::now() - MODEL_TEST_EXPIRATION)
            .await
            .map_err(|error| map_store_error(error, "model tests"))?;
        for (id, account_id) in expired {
            self.accounts
                .finish_model_test(
                    id,
                    &account_id,
                    AccountModelTestOutcome::Failed("测试未在期限内保存结果".to_owned()),
                    KEEP_PER_ACCOUNT,
                )
                .await
                .map_err(|error| map_store_error(error, "model tests"))?;
        }
        Ok(())
    }
}

fn test_outcome(
    provider: &dyn ProviderAdmin,
    kind: AccountModelTestKind,
    result: AccountModelTestResult,
) -> AccountModelTestOutcome {
    if matches!(kind, AccountModelTestKind::Image) {
        return match result
            .json_body
            .as_deref()
            .and_then(|body| provider.model_test_image_base64(body))
            .and_then(parse_image)
        {
            Some(image) => AccountModelTestOutcome::Image(image),
            None => AccountModelTestOutcome::Failed(
                "上游未返回可保存的图片或图片超过大小限制".to_owned(),
            ),
        };
    }
    let length = result.text.iter().map(String::len).sum::<usize>();
    if length > MAX_TEXT_BYTES {
        return AccountModelTestOutcome::Failed("上游回答超过保存大小限制".to_owned());
    }
    let text = result.text.concat();
    if text.trim().is_empty() {
        return AccountModelTestOutcome::Failed("上游未返回文本内容".to_owned());
    }
    AccountModelTestOutcome::Text(text)
}

fn parse_image(encoded: &str) -> Option<AccountModelTestImage> {
    if encoded.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
        return None;
    }
    let bytes = STANDARD.decode(encoded).ok()?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let content_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else {
        return None;
    };
    Some(AccountModelTestImage {
        content_type,
        bytes,
    })
}

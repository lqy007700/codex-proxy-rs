//! 请求内普通账号恢复与最后一次保底的阶段和预算

use crate::account::ProviderAccountId;

/// 当前尝试的候选用途与恢复轮询位置，不改变客户端会话身份
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum AccountRecoveryStage {
    #[default]
    Initial,
    OtherAccounts {
        initial: ProviderAccountId,
        after: Option<ProviderAccountId>,
    },
    Fallback,
}

impl AccountRecoveryStage {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::OtherAccounts { .. } => "other_accounts",
            Self::Fallback => "fallback",
        }
    }

    /// Provider 与插件共用同一用途过滤；原生增量状态仅允许保留原 owner
    pub fn prepare_selection(
        &self,
        candidates: &[crate::account::AccountCandidate],
        context: &mut crate::account::AccountSelectionContext,
        native_owner: Option<&ProviderAccountId>,
    ) {
        if context.eligibility.bypasses_local_eligibility() {
            return;
        }
        for candidate in candidates {
            let id = candidate.account.id();
            let role = context
                .account_scope
                .as_ref()
                .and_then(|scope| scope.scheduling_role(id))
                .unwrap_or_else(|| candidate.account.scheduling_role());
            let permitted = if self == &AccountRecoveryStage::Fallback {
                role == crate::account::AccountSchedulingRole::Fallback
            } else {
                role == crate::account::AccountSchedulingRole::Regular || native_owner == Some(id)
            };
            if !permitted {
                context.excluded_accounts.insert(id.clone());
            }
        }
        if let AccountRecoveryStage::OtherAccounts { initial, after } = self {
            context.excluded_accounts.insert(initial.clone());
            context.preferred_account = None;
            context.policy = context
                .policy
                .with_strategy(crate::account::RotationStrategy::RoundRobin);
            let mut eligible = candidates
                .iter()
                .filter(|candidate| {
                    crate::account::AccountSelector
                        .scheduling_blocker(candidate, context)
                        .is_none()
                })
                .collect::<Vec<_>>();
            eligible.sort_by_key(|c| c.account.id());
            let position = after
                .as_ref()
                .and_then(|previous| eligible.iter().position(|c| c.account.id() > previous))
                .unwrap_or_default();
            context.round_robin_cursor = position as u64;
            // 恢复阶段遍历整个合格普通池，权重只参与首次日常调度
            context.preferred_account = eligible.get(position).map(|c| c.account.id().clone());
            context.preferred_account_overrides_weight = true;
        } else if self == &AccountRecoveryStage::Fallback {
            context.preferred_account = None;
        }
    }
}

/// 只累计可恢复的业务失败，传输恢复不创建另一份业务预算
#[derive(Debug)]
pub(super) struct AccountRecovery {
    stage: AccountRecoveryStage,
    initial: Option<ProviderAccountId>,
    initial_failures: u32,
    other_failures: u32,
    attempt_limit: u32,
}

impl AccountRecovery {
    pub fn new(attempt_limit: u32) -> Self {
        Self {
            stage: AccountRecoveryStage::Initial,
            initial: None,
            initial_failures: 0,
            other_failures: 0,
            attempt_limit,
        }
    }

    pub fn stage(&self) -> &AccountRecoveryStage {
        &self.stage
    }

    /// 普通尝试加一次保底所需的额外重试数等于普通尝试总数，仍尊重 Provider 的较小预算
    pub fn retry_limit(&self, provider_limit: u32) -> u32 {
        self.attempt_limit.min(provider_limit)
    }

    pub fn observe_selected(&mut self, account: &ProviderAccountId) {
        match &mut self.stage {
            AccountRecoveryStage::Initial => {
                if self.initial_failures == 0 {
                    self.initial = Some(account.clone());
                }
            }
            AccountRecoveryStage::OtherAccounts { after, .. } => *after = Some(account.clone()),
            AccountRecoveryStage::Fallback => {}
        }
    }

    pub fn is_fallback(&self) -> bool {
        self.stage == AccountRecoveryStage::Fallback
    }

    pub fn is_reselecting(&self) -> bool {
        !matches!(self.stage, AccountRecoveryStage::Initial)
    }

    /// 返回是否继续原账号；账号拒绝可提前结束原账号阶段
    pub fn record_failure(&mut self, skip_initial: bool) -> bool {
        match &self.stage {
            AccountRecoveryStage::Initial => {
                self.initial_failures += 1;
                if self.initial_failures < self.attempt_limit.div_ceil(2) && !skip_initial {
                    return true;
                }
                self.leave_initial();
            }
            AccountRecoveryStage::OtherAccounts { .. } => {
                self.other_failures += 1;
                if self.other_failures >= self.attempt_limit / 2 {
                    self.stage = AccountRecoveryStage::Fallback;
                }
            }
            AccountRecoveryStage::Fallback => {}
        }
        false
    }

    pub fn leave_initial(&mut self) {
        if self.attempt_limit / 2 == 0 {
            self.enter_fallback();
            return;
        }
        self.stage = self
            .initial
            .as_ref()
            .map_or(AccountRecoveryStage::Fallback, |initial| {
                AccountRecoveryStage::OtherAccounts {
                    initial: initial.clone(),
                    after: None,
                }
            });
    }

    /// 没有合格普通账号时直接保底，不伪造上游尝试
    pub fn enter_fallback(&mut self) {
        self.stage = AccountRecoveryStage::Fallback;
    }
}

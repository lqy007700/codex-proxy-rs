//! 执行链外部副作用水位与重放安全

use std::sync::atomic::{AtomicUsize, Ordering};

/// 同一请求图内不可逆外部副作用的单调水位
///
/// 执行链在外部 HTTP 已发送或无法证明未发送时调用
/// [`Self::observe`]
/// Coordinator 用每次执行自己的基线
/// 阻止把随后 Provider 的 `not_sent` 错当作可重放
#[derive(Debug, Default)]
pub struct ExecutionEffects(AtomicUsize);

impl ExecutionEffects {
    pub fn observe(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn epoch(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

//! 请求收尾与写泵关闭之间的有界屏障，不改变费用、准入和账号调度规则

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use gateway_core::engine::ModelRequestId;
use gateway_core::lifecycle::CancellationToken;
use gateway_core::policy::ClientApiKeyId;
use gateway_core::task::{DaemonTask, WorkerDaemonContext, WorkerTaskError};
use tokio::sync::Notify;
use tokio::time::Instant;

const REQUEST_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub(crate) struct RequestDrain(Arc<State>);

struct State {
    leases_stopping: CancellationToken,
    requests: Mutex<Requests>,
    changed: Notify,
}

#[derive(Default)]
struct Requests {
    active: BTreeMap<(ClientApiKeyId, ModelRequestId), CancellationToken>,
    deadline: Option<Instant>,
    timeout_reported: bool,
}

impl RequestDrain {
    pub(crate) fn new(leases_stopping: CancellationToken) -> Self {
        Self(Arc::new(State {
            leases_stopping,
            requests: Mutex::new(Requests::default()),
            changed: Notify::new(),
        }))
    }

    pub(crate) fn is_stopping(&self) -> bool {
        self.0.leases_stopping.is_cancelled()
    }

    // 从准入续期登记到 release/abandon，覆盖选号等待、恢复及费用入队
    pub(crate) fn track(
        &self,
        key: &ClientApiKeyId,
        request: &ModelRequestId,
        cancellation: CancellationToken,
    ) {
        let mut requests = self
            .0
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        requests
            .active
            .insert((key.clone(), request.clone()), cancellation.clone());
        drop(requests);
        if self.is_stopping() {
            cancellation.cancel();
        }
    }

    pub(crate) fn complete(&self, key: &ClientApiKeyId, request: &ModelRequestId) {
        self.0
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .remove(&(key.clone(), request.clone()));
        self.0.changed.notify_waiters();
    }

    pub(crate) fn begin_shutdown(&self) -> Instant {
        // 先禁止账号申请，迟到的 Key 准入也不能进入新的上游请求
        self.0.leases_stopping.cancel();
        let (deadline, cancellations) = {
            let mut requests = self
                .0
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(deadline) = requests.deadline {
                return deadline;
            }
            let deadline = Instant::now() + REQUEST_DRAIN_TIMEOUT;
            requests.deadline = Some(deadline);
            (
                deadline,
                requests.active.values().cloned().collect::<Vec<_>>(),
            )
        };
        for cancellation in cancellations {
            cancellation.cancel();
        }
        deadline
    }

    pub(crate) async fn wait(&self) {
        let deadline = self.begin_shutdown();
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self
                .0
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .active
                .is_empty()
            {
                return;
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                let mut requests = self
                    .0
                    .requests
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !requests.timeout_reported {
                    requests.timeout_reported = true;
                    tracing::warn!(
                        remaining_requests = requests.active.len(),
                        "请求关闭收尾超时，继续有界排空已入队写入"
                    );
                }
                return;
            }
        }
    }

    pub(crate) fn writer<T: DaemonTask>(&self, writer: T) -> impl DaemonTask + use<T> {
        DrainingWriter {
            writer,
            requests: self.clone(),
        }
    }
}

struct DrainingWriter<T> {
    writer: T,
    requests: RequestDrain,
}

impl<T: DaemonTask> DaemonTask for DrainingWriter<T> {
    fn run(&self, context: WorkerDaemonContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let writer_context = WorkerDaemonContext::new(CancellationToken::new());
            let run = self.writer.run(writer_context.clone());
            tokio::pin!(run);
            let stopping = async {
                context.cancellation().cancelled().await;
                self.requests.wait().await;
                writer_context.cancellation().cancel();
            };
            tokio::pin!(stopping);
            let mut completed = 0;
            loop {
                let result = tokio::select! {
                    result = &mut run => Some(result),
                    () = &mut stopping => return run.await,
                    () = writer_context.progress_since(completed) => None,
                };
                // 包装关闭信号时仍向 Host 透传原写泵的实际成功进度
                let current = writer_context.completed_cycles();
                for _ in completed..current {
                    context.succeeded();
                }
                completed = current;
                if let Some(result) = result {
                    return result;
                }
            }
        })
    }
}

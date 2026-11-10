//! 验证请求取消到费用入队之间的写泵屏障及共享关闭预算

#[path = "../src/redis/coordination_buffer.rs"]
mod admissions;
#[path = "../src/shutdown.rs"]
mod implementation;

pub(crate) use implementation::RequestDrain;

use futures::{FutureExt as _, future::BoxFuture};
use gateway_core::engine::ModelRequestId;
use gateway_core::engine::admission::{
    ClientAdmissionDecision, ClientAdmissionError, ClientAdmissionPort, ClientAdmissionRecovery,
    ClientAdmissionRequest, ClientAdmissionRestoreResult,
};
use gateway_core::lifecycle::{CancellationToken, Deadline};
use gateway_core::policy::{ClientApiKeyId, RateLimits};
use gateway_core::task::{DaemonTask, WorkerDaemonContext};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[derive(Default)]
struct RecordingAdmission {
    fee_enqueued: AtomicBool,
    released: AtomicBool,
}

impl ClientAdmissionPort for RecordingAdmission {
    fn abandon(&self, _: &ClientApiKeyId, _: &ModelRequestId) {}

    fn admit(
        &self,
        _: ClientAdmissionRequest,
    ) -> BoxFuture<'_, Result<ClientAdmissionDecision, ClientAdmissionError>> {
        Box::pin(async { Ok(ClientAdmissionDecision::Granted) })
    }

    fn release<'a>(
        &'a self,
        _: &'a ClientApiKeyId,
        _: &'a ModelRequestId,
    ) -> BoxFuture<'a, Result<bool, ClientAdmissionError>> {
        Box::pin(async move {
            assert!(
                self.fee_enqueued.load(Ordering::Acquire),
                "费用应先于准入释放入队"
            );
            self.released.store(true, Ordering::Release);
            Ok(true)
        })
    }

    fn restore(
        &self,
        _: ClientAdmissionRecovery,
    ) -> BoxFuture<'_, Result<ClientAdmissionRestoreResult, ClientAdmissionError>> {
        Box::pin(async { Ok(ClientAdmissionRestoreResult::default()) })
    }
}

#[tokio::test]
async fn shutdown_keeps_writer_open_until_request_enqueues_its_final_fee_and_release() {
    let requests = RequestDrain::new(CancellationToken::new());
    let inner = Arc::new(RecordingAdmission::default());
    let (port, writer) = admissions::BufferedClientAdmissionPort::new(
        inner.clone(),
        Arc::new(crate::RecordingDiagnostics::default()),
    );
    let port = port.with_request_drain(requests.clone());
    let key = ClientApiKeyId::new("key_shutdown").unwrap();
    let request = ModelRequestId::new("req_shutdown").unwrap();
    let cancellation = CancellationToken::new();
    let renewal = port.maintain(&key, &request, Deadline::default(), cancellation.clone());
    let stopping = CancellationToken::new();
    let task = {
        let writer = requests.writer(writer);
        let context = WorkerDaemonContext::new(stopping.clone());
        tokio::spawn(async move { writer.run(context).await })
    };
    stopping.cancel();
    tokio::time::timeout(Duration::from_secs(1), cancellation.cancelled())
        .await
        .unwrap();
    assert!(!task.is_finished(), "请求尚未入队结算，写泵不能退出");
    assert!(
        port.admit(ClientAdmissionRequest {
            model_request_id: ModelRequestId::new("req_after_stop").unwrap(),
            client_api_key_id: key.clone(),
            lease_ttl: Duration::from_secs(60),
            allow_concurrency_acquire: true,
            limits: RateLimits {
                max_concurrency: 1,
                requests_per_minute: 0
            },
        })
        .await
        .is_err()
    );
    // 对应 Core 的真实收尾顺序：费用入队、停止准入续期、释放准入
    inner.fee_enqueued.store(true, Ordering::Release);
    drop(renewal);
    assert!(port.release(&key, &request).await.unwrap());
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(inner.released.load(Ordering::Acquire));
}

#[tokio::test(start_paused = true)]
async fn abandoned_or_late_requests_share_the_original_shutdown_deadline() {
    let requests = RequestDrain::new(CancellationToken::new());
    let key = ClientApiKeyId::new("key_abandoned").unwrap();
    let request = ModelRequestId::new("req_abandoned").unwrap();
    let cancellation = CancellationToken::new();
    requests.track(&key, &request, cancellation.clone());
    let started = tokio::time::Instant::now();
    requests.begin_shutdown();
    assert!(cancellation.is_cancelled());
    tokio::time::advance(Duration::from_secs(1)).await;
    let late = CancellationToken::new();
    requests.track(
        &key,
        &ModelRequestId::new("req_late").unwrap(),
        late.clone(),
    );
    assert!(late.is_cancelled());
    let ((), ()) = futures::join!(requests.wait(), requests.wait());
    assert_eq!(started.elapsed(), Duration::from_secs(2));
    assert!(
        requests.wait().now_or_never().is_some(),
        "重复关闭不能重新分配等待预算"
    );
}

#[tokio::test]
async fn abandoned_admission_unblocks_shutdown_without_waiting_for_renewal_drop() {
    let requests = RequestDrain::new(CancellationToken::new());
    let (port, _writer) = admissions::BufferedClientAdmissionPort::new(
        Arc::new(RecordingAdmission::default()),
        Arc::new(crate::RecordingDiagnostics::default()),
    );
    let port = port.with_request_drain(requests.clone());
    let key = ClientApiKeyId::new("key_dropped").unwrap();
    let request = ModelRequestId::new("req_dropped").unwrap();
    let _renewal = port.maintain(
        &key,
        &request,
        Deadline::default(),
        CancellationToken::new(),
    );
    requests.begin_shutdown();
    assert!(requests.wait().now_or_never().is_none());
    port.abandon(&key, &request);
    assert!(requests.wait().now_or_never().is_some());
}

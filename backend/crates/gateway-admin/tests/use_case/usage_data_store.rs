//! 管理服务测试的用量存储占位夹具，非用量测试禁止访问
pub(super) struct UnusedUsageDataStore;
use gateway_admin::{model::usage_data::*, ports::usage_data::*};
#[async_trait::async_trait]
impl UsageDataStore for UnusedUsageDataStore {
    async fn get_record(&self, _: GetUsageRecord) -> UsageDataResult<UsageRecord> {
        unreachable!("unexpected usage call")
    }
    async fn list_records(&self, _: ListUsageRecords) -> UsageDataResult<UsageRecordPage> {
        unreachable!("unexpected usage call")
    }
    async fn write_records(&self, _: StoredUsageWrite) -> UsageDataResult<UsageWriteResult> {
        unreachable!("unexpected usage call")
    }
    async fn list_aggregates(&self, _: ListUsageAggregates) -> UsageDataResult<UsageAggregatePage> {
        unreachable!("unexpected usage call")
    }
    async fn write_aggregates(
        &self,
        _: StoredAggregateWrite,
    ) -> UsageDataResult<UsageAggregateWriteResult> {
        unreachable!("unexpected usage call")
    }
}

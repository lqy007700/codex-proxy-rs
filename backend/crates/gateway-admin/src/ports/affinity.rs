//! 亲和临时状态与持久操作日志的管理端口

use crate::model::{AdminError, MutationContext, Revision, affinity::*};
use async_trait::async_trait;

#[async_trait]
pub trait AffinityBindingStore: Send + Sync {
    async fn list(&self, query: &AffinityQuery) -> Result<AffinityPage, AdminError>;
    async fn session(
        &self,
        provider: &str,
        reference: &str,
    ) -> Result<Option<AffinitySession>, AdminError>;
    async fn receipt(
        &self,
        plan: &AffinityPlan,
        item: &AffinityPlanItem,
    ) -> Result<Option<AffinityItemResult>, AdminError>;
    async fn apply(
        &self,
        plan: &AffinityPlan,
        item: &AffinityPlanItem,
        rejection: Option<&str>,
    ) -> Result<AffinityItemResult, AdminError>;
}

#[async_trait]
pub trait AffinityJournalStore: Send + Sync {
    async fn sources(&self) -> Result<Vec<IdentitySource>, AdminError>;
    async fn save_source(
        &self,
        context: &MutationContext,
        source: IdentitySource,
    ) -> Result<Revision, AdminError>;
    async fn eligible_target(
        &self,
        session: &AffinitySession,
        target: Option<&str>,
    ) -> Result<bool, AdminError>;
    async fn create_plan(
        &self,
        context: &MutationContext,
        plan: AffinityPlan,
    ) -> Result<AffinityOperation, AdminError>;
    async fn operation(&self, id: &str) -> Result<Option<AffinityOperation>, AdminError>;
    async fn history(&self) -> Result<Vec<AffinityOperation>, AdminError>;
    async fn record_result(
        &self,
        context: &MutationContext,
        operation_id: &str,
        result: &AffinityItemResult,
    ) -> Result<AffinityItemResult, AdminError>;
}

#[derive(Clone)]
pub struct AffinityStorePorts {
    pub bindings: std::sync::Arc<dyn AffinityBindingStore>,
    pub journal: std::sync::Arc<dyn AffinityJournalStore>,
}

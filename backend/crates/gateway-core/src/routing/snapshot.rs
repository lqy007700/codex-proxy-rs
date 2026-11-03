//! RuntimeSnapshot 事实、编译、原子发布与版本收敛规则

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::account::{FastMode, ProviderAccountId};
use crate::concurrency::ConcurrencyQueuePolicy;
use crate::operation::{Operation, OperationKind};
use crate::policy::{
    ClientApiKeyId, ClientPeerSharing, ClientPolicy, CodexClientMinVersions, PlaintextClientApiKey,
    RateLimits,
};
use crate::settings::{SettingsValues, compiled::CompiledSettings};
use crate::validation::RoutingError;

use super::{
    AccountGroupId, ClientRoutingScope, ConfigRevision, FrozenAccountScope, ModelCapabilities,
    ProviderCandidate, ProviderCatalogGeneration, ProviderCatalogPort, ProviderKind, ProviderModel,
    ProviderModelRoute, PublicModelId, RoutingContext, RoutingGroupSnapshot, RoutingPlan,
    RuntimeAccount, RuntimeAccountDirectory, UpstreamModelId,
};

const MAXIMUM_SNAPSHOT_STABILITY_ATTEMPTS: usize = 4;

type ModelCatalogAccounts = BTreeMap<ProviderKind, BTreeMap<String, BTreeSet<ProviderAccountId>>>;

/// Store 读取到的一个启用 Client API Key 策略事实
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotClientPolicyFacts {
    portable_history: bool,
    identity_policy: crate::policy::CallerIdentityPolicy,
    peer_sharing: ClientPeerSharing,
    name_snapshot: Option<String>,
    request_profiles: BTreeMap<ProviderKind, crate::account::OpaqueProviderData>,
    key_id: ClientApiKeyId,
    plaintext_key: PlaintextClientApiKey,
    group_ids: Vec<AccountGroupId>,
    limits: RateLimits,
}

impl SnapshotClientPolicyFacts {
    #[must_use]
    pub fn with_identity_policy(mut self, policy: crate::policy::CallerIdentityPolicy) -> Self {
        self.identity_policy = policy;
        self
    }

    #[must_use]
    pub const fn with_portable_history(mut self, enabled: bool) -> Self {
        self.portable_history = enabled;
        self
    }

    #[must_use]
    pub const fn with_peer_sharing(mut self, sharing: ClientPeerSharing) -> Self {
        self.peer_sharing = sharing;
        self
    }

    #[must_use]
    pub fn with_name_snapshot(mut self, name: Option<String>) -> Self {
        self.name_snapshot = name;
        self
    }

    #[must_use]
    pub fn with_request_profiles(
        mut self,
        profiles: BTreeMap<ProviderKind, crate::account::OpaqueProviderData>,
    ) -> Self {
        self.request_profiles = profiles;
        self
    }

    #[must_use]
    pub fn new(
        key_id: ClientApiKeyId,
        plaintext_key: PlaintextClientApiKey,
        group_ids: Vec<AccountGroupId>,
        limits: RateLimits,
    ) -> Self {
        Self {
            identity_policy: Default::default(),
            peer_sharing: ClientPeerSharing::default(),
            key_id,
            portable_history: false,
            name_snapshot: None,
            request_profiles: BTreeMap::new(),
            plaintext_key,
            group_ids,
            limits,
        }
    }
}

/// Store 读取到的账号分组事实
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotAccountGroupFacts {
    allow_image_generation: bool,
    fast_mode: FastMode,
    id: AccountGroupId,
    name: String,
    enabled: bool,
}

impl SnapshotAccountGroupFacts {
    #[must_use]
    pub const fn with_image_generation(mut self, allowed: bool) -> Self {
        self.allow_image_generation = allowed;
        self
    }

    #[must_use]
    pub const fn with_fast_mode(mut self, fast_mode: FastMode) -> Self {
        self.fast_mode = fast_mode;
        self
    }

    #[must_use]
    pub fn new(id: AccountGroupId, name: String, enabled: bool) -> Self {
        Self {
            id,
            name,
            enabled,
            allow_image_generation: true,
            fast_mode: FastMode::Default,
        }
    }
}

/// Store 读取到的账号及其固有 Provider 事实
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotProviderAccountFacts {
    account_id: ProviderAccountId,
    provider_kind: String,
    model_access: crate::account::AccountModelAccess,
    model_mappings: crate::account::AccountModelMappings,
    scheduling_role: crate::account::AccountSchedulingRole,
    authentication_kind: String,
}

impl SnapshotProviderAccountFacts {
    #[must_use]
    pub const fn with_scheduling_role(
        mut self,
        role: crate::account::AccountSchedulingRole,
    ) -> Self {
        self.scheduling_role = role;
        self
    }

    #[must_use]
    pub const fn scheduling_role(&self) -> crate::account::AccountSchedulingRole {
        self.scheduling_role
    }

    #[must_use]
    pub fn new(account_id: ProviderAccountId, provider_kind: impl Into<String>) -> Self {
        Self {
            account_id,
            provider_kind: provider_kind.into(),
            scheduling_role: crate::account::AccountSchedulingRole::Regular,
            model_access: crate::account::AccountModelAccess::all(),
            model_mappings: crate::account::AccountModelMappings::empty(),
            authentication_kind: String::new(),
        }
    }
}

impl SnapshotProviderAccountFacts {
    #[must_use]
    pub fn with_authentication_kind(mut self, authentication_kind: String) -> Self {
        self.authentication_kind = authentication_kind;
        self
    }

    #[must_use]
    pub fn with_model_mappings(mut self, mappings: crate::account::AccountModelMappings) -> Self {
        self.model_mappings = mappings;
        self
    }

    #[must_use]
    pub fn with_model_access(mut self, model_access: crate::account::AccountModelAccess) -> Self {
        self.model_access = model_access;
        self
    }
}

/// Store 读取到的一条分组成员关系
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotAccountGroupMemberFacts {
    group_id: AccountGroupId,
    account_id: ProviderAccountId,
}

impl SnapshotAccountGroupMemberFacts {
    #[must_use]
    pub const fn new(group_id: AccountGroupId, account_id: ProviderAccountId) -> Self {
        Self {
            group_id,
            account_id,
        }
    }
}

/// 一次一致性读取产生的全部 RuntimeSnapshot 持久事实
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFacts {
    config_revision: ConfigRevision,
    observed_current_revision: ConfigRevision,
    settings: SettingsValues,
    client_policies: Vec<SnapshotClientPolicyFacts>,
    account_groups: Vec<SnapshotAccountGroupFacts>,
    provider_accounts: Vec<SnapshotProviderAccountFacts>,
    group_memberships: Vec<SnapshotAccountGroupMemberFacts>,
}

impl SnapshotFacts {
    #[must_use]
    pub fn new(
        config_revision: ConfigRevision,
        observed_current_revision: ConfigRevision,
        settings: SettingsValues,
        client_policies: Vec<SnapshotClientPolicyFacts>,
        account_groups: Vec<SnapshotAccountGroupFacts>,
        provider_accounts: Vec<SnapshotProviderAccountFacts>,
        group_memberships: Vec<SnapshotAccountGroupMemberFacts>,
    ) -> Self {
        Self {
            config_revision,
            observed_current_revision,
            settings,
            client_policies,
            account_groups,
            provider_accounts,
            group_memberships,
        }
    }

    #[must_use]
    pub const fn config_revision(&self) -> ConfigRevision {
        self.config_revision
    }

    #[must_use]
    pub const fn observed_current_revision(&self) -> ConfigRevision {
        self.observed_current_revision
    }
}

/// 不泄漏持久化实现细节的 Snapshot store 错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("runtime snapshot store is unavailable")]
pub struct SnapshotStoreError;

impl SnapshotStoreError {
    #[must_use]
    pub const fn unavailable() -> Self {
        Self
    }
}

/// RuntimeSnapshot 持久事实的数据库中立端口
pub trait SnapshotStorePort: Send + Sync {
    fn load_snapshot_facts(&self) -> BoxFuture<'_, Result<SnapshotFacts, SnapshotStoreError>>;

    fn current_config_revision(&self) -> BoxFuture<'_, Result<ConfigRevision, SnapshotStoreError>>;
}

/// 快照未发布时可安全记录的稳定错误
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeSnapshotCompileError {
    #[error("runtime extensions could not be prepared")]
    ExtensionsUnavailable,
    #[error("runtime snapshot store is unavailable")]
    StoreUnavailable,
    #[error("runtime configuration changed while the snapshot was loading")]
    RevisionChanged,
    #[error("runtime snapshot contains invalid frozen data")]
    InvalidData,
    #[error("provider model catalog changed while the snapshot was compiling")]
    CatalogChanged,
}

/// Store 一致性事实与 Provider 实时目录的唯一快照编译器
#[derive(Clone)]
pub struct RuntimeSnapshotCompiler {
    store: Arc<dyn SnapshotStorePort>,
    privacy_compiler: Option<Arc<dyn crate::settings::privacy::PrivacyPolicyCompiler>>,
    catalogs: Arc<dyn ProviderCatalogPort>,
    extensions: Option<Arc<dyn crate::routing::extensions::ExtensionPreparationPort>>,
}

impl RuntimeSnapshotCompiler {
    #[must_use]
    pub const fn new(
        store: Arc<dyn SnapshotStorePort>,
        catalogs: Arc<dyn ProviderCatalogPort>,
    ) -> Self {
        Self {
            store,
            privacy_compiler: None,
            catalogs,
            extensions: None,
        }
    }

    #[must_use]
    pub fn with_extensions(
        mut self,
        extensions: Arc<dyn crate::routing::extensions::ExtensionPreparationPort>,
    ) -> Self {
        self.extensions = Some(extensions);
        self
    }

    #[must_use]
    pub fn with_privacy_compiler(
        mut self,
        compiler: Arc<dyn crate::settings::privacy::PrivacyPolicyCompiler>,
    ) -> Self {
        self.privacy_compiler = Some(compiler);
        self
    }

    pub(crate) fn store(&self) -> Arc<dyn SnapshotStorePort> {
        Arc::clone(&self.store)
    }

    pub(crate) fn provider_catalog_generations(
        &self,
    ) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        self.catalogs.catalog_generations()
    }

    /// 读取一个 revision，并为已注册 Provider 查询实时模型目录
    pub async fn compile(&self) -> Result<RuntimeSnapshot, RuntimeSnapshotCompileError> {
        self.compile_inner(None, true).await
    }

    /// 配置提交复用同一扩展集合的已发布目录，目录留待对账刷新
    pub(crate) async fn compile_with_cached_catalog(
        &self,
        previous: &RuntimeSnapshot,
    ) -> Result<RuntimeSnapshot, RuntimeSnapshotCompileError> {
        self.compile_inner(Some(previous), false).await
    }

    /// 刷新目录，单个 Provider 读取失败时保留同一扩展集合的已知模型归属
    pub(crate) async fn compile_with_catalog_fallback(
        &self,
        previous: &RuntimeSnapshot,
    ) -> Result<RuntimeSnapshot, RuntimeSnapshotCompileError> {
        self.compile_inner(Some(previous), true).await
    }

    async fn compile_inner(
        &self,
        previous: Option<&RuntimeSnapshot>,
        refresh_catalog: bool,
    ) -> Result<RuntimeSnapshot, RuntimeSnapshotCompileError> {
        // 同一配置 revision 的重试复用扩展候选，保留插件策略的发布一致性
        let mut prepared_extensions: Option<(
            ConfigRevision,
            crate::routing::extensions::ExtensionSetReference,
        )> = None;
        let mut instability = RuntimeSnapshotCompileError::CatalogChanged;
        for _ in 0..MAXIMUM_SNAPSHOT_STABILITY_ATTEMPTS {
            let facts = self
                .store
                .load_snapshot_facts()
                .await
                .map_err(|_| RuntimeSnapshotCompileError::StoreUnavailable)?;
            if facts.config_revision != facts.observed_current_revision {
                // 并发提交使本轮事实过期时重读完整快照，不能混用不同版本的权限
                instability = RuntimeSnapshotCompileError::RevisionChanged;
                continue;
            }
            let revision = facts.config_revision;
            let extensions = match &self.extensions {
                Some(preparer) => {
                    let must_prepare =
                        prepared_extensions
                            .as_ref()
                            .is_none_or(|(prepared_revision, prepared)| {
                                *prepared_revision != revision || !prepared.is_ready()
                            });
                    if must_prepare {
                        let prepared = match preparer
                            .prepare_with_request_body_limit(
                                revision,
                                usize::try_from(
                                    facts.settings.responses_max_decompressed_body_bytes,
                                )
                                .unwrap_or(usize::MAX),
                            )
                            .await
                        {
                            Ok(prepared) => prepared,
                            Err(_) => {
                                // 扩展读取也可能遇到并发提交；只有版本已变才重读事实
                                let observed =
                                    self.store.current_config_revision().await.map_err(|_| {
                                        RuntimeSnapshotCompileError::StoreUnavailable
                                    })?;
                                if observed == revision {
                                    return Err(RuntimeSnapshotCompileError::ExtensionsUnavailable);
                                }
                                instability = RuntimeSnapshotCompileError::RevisionChanged;
                                continue;
                            }
                        };
                        prepared_extensions = Some((revision, prepared));
                    }
                    prepared_extensions
                        .as_ref()
                        .map(|(_, prepared)| prepared.clone())
                }
                None => None,
            };
            let catalogs = &self.catalogs;
            let catalog_generations = catalogs.catalog_generations();
            let provider_kinds = catalog_generations.keys().cloned().collect();
            let cached = previous.filter(|previous| {
                previous.extensions().map(|set| set.id()) == extensions.as_ref().map(|set| set.id())
            });
            let snapshot = compile_runtime_snapshot(
                facts,
                catalogs.as_ref(),
                provider_kinds,
                cached,
                refresh_catalog,
                self.privacy_compiler.clone(),
            )
            .await?;
            let observed_generations = catalogs.catalog_generations();
            if catalog_generations == observed_generations {
                if extensions.is_some()
                    && self
                        .store
                        .current_config_revision()
                        .await
                        .map_err(|_| RuntimeSnapshotCompileError::StoreUnavailable)?
                        != revision
                {
                    instability = RuntimeSnapshotCompileError::RevisionChanged;
                    continue;
                }
                let snapshot = snapshot
                    .with_provider_catalog_generations(if cached.is_some() && !refresh_catalog {
                        BTreeMap::new()
                    } else {
                        observed_generations
                    })
                    .with_extensions(extensions);
                return Ok(snapshot);
            }
            instability = RuntimeSnapshotCompileError::CatalogChanged;
        }
        Err(instability)
    }
}

async fn compile_runtime_snapshot(
    facts: SnapshotFacts,
    catalogs: &dyn ProviderCatalogPort,
    provider_kinds: Vec<ProviderKind>,
    previous: Option<&RuntimeSnapshot>,
    refresh_catalog: bool,
    privacy_compiler: Option<Arc<dyn crate::settings::privacy::PrivacyPolicyCompiler>>,
) -> Result<RuntimeSnapshot, RuntimeSnapshotCompileError> {
    // 目录读取失败不能抹掉已知归属；成功返回空目录则仍是权威的模型撤回
    let mut provider_models = Vec::new();
    let mut catalog_accounts = BTreeMap::new();
    let mut exhaustive_provider_catalogs = BTreeSet::new();
    for provider in &provider_kinds {
        let models = if refresh_catalog || previous.is_none() {
            catalogs.query_model_capabilities(provider).await.ok()
        } else {
            None
        };
        if let Some(previous) = previous.filter(|_| models.is_none()) {
            if let Some(accounts) = previous.model_catalog_accounts.get(provider) {
                catalog_accounts.insert(provider.clone(), accounts.clone());
            }
            if previous.exhaustive_provider_catalogs.contains(provider) {
                exhaustive_provider_catalogs.insert(provider.clone());
            }
            if let Some(models) = previous.provider_models.get(provider) {
                provider_models.extend(models.iter().map(|(model, capabilities)| {
                    let compiled =
                        ProviderModel::new(provider.clone(), model.clone(), capabilities.clone());
                    match previous
                        .provider_model_presentations
                        .get(provider)
                        .and_then(|presentations| presentations.get(model))
                    {
                        Some(presentation) => compiled.with_presentation(presentation.clone()),
                        None => compiled,
                    }
                }));
            }
            continue;
        }
        let Some(models) = models else {
            continue;
        };
        if catalogs.model_catalog_is_exhaustive(provider) {
            exhaustive_provider_catalogs.insert(provider.clone());
        }
        provider_models.extend(models.into_iter().map(|model| {
            if let Some(accounts) = model.catalog_accounts() {
                catalog_accounts
                    .entry(provider.clone())
                    .or_default()
                    .insert(model.upstream_model().as_str().to_owned(), accounts.clone());
            }
            let compiled = ProviderModel::new(
                provider.clone(),
                model.upstream_model().clone(),
                model.capabilities().clone(),
            );
            match model.presentation().cloned() {
                Some(presentation) => compiled.with_presentation(presentation),
                None => compiled,
            }
        }));
    }

    let mut groups = BTreeMap::new();
    for group in facts.account_groups {
        if group.name.trim() != group.name
            || group.name.is_empty()
            || group.name.chars().count() > 100
            || group.name.chars().any(char::is_control)
            || groups.insert(group.id.clone(), group).is_some()
        {
            return Err(RuntimeSnapshotCompileError::InvalidData);
        }
    }

    let mut account_groups = BTreeMap::<ProviderAccountId, BTreeSet<AccountGroupId>>::new();
    let mut accounts = BTreeMap::new();
    for account in facts.provider_accounts {
        let provider_kind = ProviderKind::new(account.provider_kind)
            .map_err(|_| RuntimeSnapshotCompileError::InvalidData)?;
        let direct_account =
            catalogs.is_direct_account(&provider_kind, &account.authentication_kind);
        // 账号归属独立于当前执行器集合；Provider 撤下后仍保留账号和分组，路由只选已注册执行器
        if accounts
            .insert(
                account.account_id.clone(),
                RuntimeAccount::new(provider_kind, BTreeSet::new())
                    .with_scheduling_role(account.scheduling_role)
                    .with_model_access(account.model_access)
                    .with_model_mappings(account.model_mappings)
                    .with_direct_authentication_kind(
                        direct_account.then_some(account.authentication_kind),
                    ),
            )
            .is_some()
        {
            return Err(RuntimeSnapshotCompileError::InvalidData);
        }
        account_groups.insert(account.account_id, BTreeSet::new());
    }
    let mut memberships = BTreeSet::new();
    for membership in facts.group_memberships {
        if !groups.contains_key(&membership.group_id)
            || !accounts.contains_key(&membership.account_id)
            || !memberships.insert((membership.group_id.clone(), membership.account_id.clone()))
        {
            return Err(RuntimeSnapshotCompileError::InvalidData);
        }
        account_groups
            .get_mut(&membership.account_id)
            .ok_or(RuntimeSnapshotCompileError::InvalidData)?
            .insert(membership.group_id);
    }
    for (account_id, group_ids) in account_groups {
        let account = accounts
            .get_mut(&account_id)
            .ok_or(RuntimeSnapshotCompileError::InvalidData)?;
        *account = RuntimeAccount::new(account.provider_kind().clone(), group_ids)
            .with_scheduling_role(account.scheduling_role())
            .with_model_access(account.model_access().clone())
            .with_model_mappings(account.model_mappings().clone())
            .with_direct_authentication_kind(
                account.direct_authentication_kind().map(str::to_owned),
            );
    }
    let account_directory = Arc::new(RuntimeAccountDirectory::new(accounts));

    let mut client_policies = Vec::with_capacity(facts.client_policies.len());
    for policy in facts.client_policies {
        let mut fast_mode = FastMode::Default;
        let mut allow_image_generation = true;
        let account_scope = if policy.group_ids.is_empty() {
            FrozenAccountScope::new(
                Arc::clone(&account_directory),
                ClientRoutingScope::all_accounts(),
            )
        } else {
            let mut seen = BTreeSet::new();
            let mut bound_groups = Vec::with_capacity(policy.group_ids.len());
            let mut enabled_group_ids = BTreeSet::new();
            for group_id in policy.group_ids {
                if !seen.insert(group_id.clone()) {
                    return Err(RuntimeSnapshotCompileError::InvalidData);
                }
                let group = groups
                    .get(&group_id)
                    .ok_or(RuntimeSnapshotCompileError::InvalidData)?;
                // 停用只影响选号，仍绑定的分组继续贡献请求策略
                fast_mode = fast_mode.merge(group.fast_mode);
                allow_image_generation &= group.allow_image_generation;
                bound_groups.push(RoutingGroupSnapshot::new(
                    group.id.clone(),
                    group.name.clone(),
                ));
                if group.enabled {
                    enabled_group_ids.insert(group_id);
                }
            }
            bound_groups.sort_by(|left, right| left.id().cmp(right.id()));
            let provider_kinds = account_directory.providers_for_groups(&enabled_group_ids);
            FrozenAccountScope::new(
                Arc::clone(&account_directory),
                ClientRoutingScope::restricted(bound_groups, enabled_group_ids, provider_kinds)
                    .map_err(|_| RuntimeSnapshotCompileError::InvalidData)?,
            )
        };
        client_policies.push(
            ClientPolicy::new(
                policy.key_id,
                policy.plaintext_key,
                Arc::new(
                    account_scope
                        .with_portable_history(policy.portable_history)
                        .with_image_generation(allow_image_generation)
                        .with_fast_mode(fast_mode)
                        .with_request_profiles(policy.request_profiles),
                ),
                true,
                policy.limits,
            )
            .with_name_snapshot(policy.name_snapshot)
            .with_peer_sharing(policy.peer_sharing)
            .with_identity_policy(policy.identity_policy),
        );
    }

    RuntimeSnapshot::new_with_privacy_compiler(
        facts.config_revision,
        facts.settings,
        provider_kinds,
        provider_models,
        client_policies,
        privacy_compiler,
    )
    .map_err(|_| RuntimeSnapshotCompileError::InvalidData)
    .map(|snapshot| {
        snapshot
            .with_account_directory(account_directory)
            .with_exhaustive_provider_catalogs(exhaustive_provider_catalogs)
            .with_model_catalog_accounts(catalog_accounts)
    })
}

/// 数据面使用的不可变配置快照
#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    settings: Arc<CompiledSettings>,
    privacy_compiler: Option<Arc<dyn crate::settings::privacy::PrivacyPolicyCompiler>>,
    extensions: Option<crate::routing::extensions::ExtensionSetReference>,
    revision: ConfigRevision,
    providers: Arc<BTreeSet<ProviderKind>>,
    provider_models: Arc<BTreeMap<ProviderKind, BTreeMap<UpstreamModelId, ModelCapabilities>>>,
    provider_model_presentations:
        Arc<BTreeMap<ProviderKind, BTreeMap<UpstreamModelId, super::ModelPresentation>>>,
    provider_catalog_generations: Arc<BTreeMap<ProviderKind, ProviderCatalogGeneration>>,
    exhaustive_provider_catalogs: Arc<BTreeSet<ProviderKind>>,
    model_catalog_accounts: Arc<ModelCatalogAccounts>,
    account_directory: Arc<RuntimeAccountDirectory>,
    client_policies: Arc<BTreeMap<ClientApiKeyId, ClientPolicy>>,
}

impl RuntimeSnapshot {
    /// 设置事实与编译结果共享快照寿命，读取时不从执行策略反向拼装
    #[must_use]
    pub fn settings(&self) -> &SettingsValues {
        &self.settings.values
    }

    pub(crate) fn resolve_settings(
        self: &Arc<Self>,
        values: &SettingsValues,
    ) -> Result<Arc<Self>, RuntimeSnapshotCompileError> {
        if self.settings() == values {
            return Ok(self.clone());
        }
        self.with_settings(values).map(Arc::new)
    }

    /// 只替换设置及其编译结果，目录、发布代次和身份事实共享原快照
    pub fn with_settings(
        &self,
        values: &SettingsValues,
    ) -> Result<Self, RuntimeSnapshotCompileError> {
        let settings = CompiledSettings::new(
            values.clone(),
            self.privacy_compiler.as_deref(),
            Some(&self.settings),
        )
        .map_err(|_| RuntimeSnapshotCompileError::InvalidData)?;
        Ok(Self {
            settings: Arc::new(settings),
            ..self.clone()
        })
    }

    #[must_use]
    pub fn with_extensions(
        mut self,
        extensions: Option<crate::routing::extensions::ExtensionSetReference>,
    ) -> Self {
        self.extensions = extensions;
        self
    }

    #[must_use]
    pub const fn extensions(&self) -> Option<&crate::routing::extensions::ExtensionSetReference> {
        self.extensions.as_ref()
    }

    fn model_aliases(&self) -> &[super::ContributedModelAlias] {
        self.extensions
            .as_ref()
            .map_or(&[], |set| set.model_aliases())
    }

    fn contributed_models(&self) -> &[super::ContributedModel] {
        self.extensions.as_ref().map_or(&[], |set| set.models())
    }

    pub(crate) fn contributed_profiles_for_scope(
        &self,
        provider: &ProviderKind,
        scope: &FrozenAccountScope,
    ) -> Vec<super::PublicModelProfile> {
        if scope.directory().model_aliases(provider).is_some() {
            return self
                .public_models_for_provider(provider)
                .into_iter()
                .filter_map(|model| {
                    scope
                        .allowed_account_ids()
                        .find_map(|account| {
                            self.contributed_presentation_for_account(
                                provider, &model, scope, account,
                            )
                        })
                        .map(|presentation| {
                            super::PublicModelProfile::new(model, presentation.clone())
                        })
                })
                .collect();
        }
        self.contributed_models()
            .iter()
            .filter(|model| {
                &model.provider == provider
                    && model.accounts.iter().any(|account| {
                        scope.account_provider(account) == Some(provider)
                            && scope.allows_model(account, &self.mapped_model(model.id.as_str()))
                    })
            })
            .map(|model| {
                super::PublicModelProfile::new(model.id.clone(), model.presentation.clone())
            })
            .collect()
    }

    fn contributed_presentation_for_account(
        &self,
        provider: &ProviderKind,
        public: &PublicModelId,
        scope: &FrozenAccountScope,
        account: &ProviderAccountId,
    ) -> Option<&super::ModelPresentation> {
        if scope.account_provider(account) != Some(provider)
            || self
                .model_alias(public.as_str())
                .is_some_and(|alias| &alias.provider != provider)
            || !scope.allows_request_model(
                account,
                public.as_str(),
                &self.upstream_model_for_account(public, scope, account),
            )
        {
            return None;
        }
        let contribution_id = scope
            .model_override(account, public.as_str())
            .unwrap_or(public.as_str());
        self.contributed_models()
            .iter()
            .find(|entry| {
                &entry.provider == provider
                    && entry.id.as_str() == contribution_id
                    && entry.accounts.contains(account)
            })
            .map(|entry| &entry.presentation)
    }

    fn model_alias(&self, model: &str) -> Option<&super::ContributedModelAlias> {
        self.model_aliases()
            .iter()
            .find(|alias| alias.id.as_str() == model)
    }

    #[must_use]
    pub fn responses_max_decompressed_body_bytes(&self) -> usize {
        self.settings.responses_max_decompressed_body_bytes.get()
    }

    #[must_use]
    pub fn client_queue_policy(&self) -> ConcurrencyQueuePolicy {
        self.settings.client_queue_policy
    }

    /// 校验 Provider、实时模型目录和 Client API Key，并构建快照
    pub fn new(
        revision: ConfigRevision,
        values: SettingsValues,
        providers: Vec<ProviderKind>,
        provider_models: Vec<ProviderModel>,
        client_policies: Vec<ClientPolicy>,
    ) -> Result<Self, RoutingError> {
        Self::new_with_privacy_compiler(
            revision,
            values,
            providers,
            provider_models,
            client_policies,
            None,
        )
    }

    pub fn new_with_privacy_compiler(
        revision: ConfigRevision,
        values: SettingsValues,
        providers: Vec<ProviderKind>,
        provider_models: Vec<ProviderModel>,
        client_policies: Vec<ClientPolicy>,
        privacy_compiler: Option<Arc<dyn crate::settings::privacy::PrivacyPolicyCompiler>>,
    ) -> Result<Self, RoutingError> {
        let mut provider_set = BTreeSet::new();
        for provider in providers {
            if !provider_set.insert(provider.clone()) {
                return Err(RoutingError::DuplicateEntity {
                    entity: "provider",
                    id: provider.to_string(),
                });
            }
        }

        let mut exhaustive_provider_catalogs = BTreeSet::new();
        let mut model_map =
            BTreeMap::<ProviderKind, BTreeMap<UpstreamModelId, ModelCapabilities>>::new();
        let mut presentation_map =
            BTreeMap::<ProviderKind, BTreeMap<UpstreamModelId, super::ModelPresentation>>::new();
        for model in provider_models {
            let ProviderModel {
                provider,
                upstream_model,
                capabilities,
                presentation,
            } = model;
            exhaustive_provider_catalogs.insert(provider.clone());
            if !provider_set.contains(&provider) {
                return Err(RoutingError::NotFound {
                    entity: "provider",
                    id: provider.to_string(),
                });
            }
            let models = model_map.entry(provider.clone()).or_default();
            if models
                .insert(upstream_model.clone(), capabilities)
                .is_some()
            {
                return Err(RoutingError::DuplicateEntity {
                    entity: "provider model",
                    id: upstream_model.to_string(),
                });
            }
            if let Some(presentation) = presentation {
                presentation_map
                    .entry(provider)
                    .or_default()
                    .insert(upstream_model, presentation);
            }
        }

        let mut client_policy_map = BTreeMap::new();
        for policy in client_policies {
            let id = policy.key_id().clone();
            if client_policy_map.insert(id.clone(), policy).is_some() {
                return Err(RoutingError::DuplicateEntity {
                    entity: "client API key",
                    id: id.to_string(),
                });
            }
        }
        client_policy_map.retain(|_, policy| policy.enabled());

        let settings = CompiledSettings::new(values, privacy_compiler.as_deref(), None)
            .map_err(|_| RoutingError::InvalidSettings)?;
        Ok(Self {
            settings: Arc::new(settings),
            privacy_compiler,
            extensions: None,
            revision,
            providers: Arc::new(provider_set),
            provider_models: Arc::new(model_map),
            provider_model_presentations: Arc::new(presentation_map),
            provider_catalog_generations: Arc::new(BTreeMap::new()),
            exhaustive_provider_catalogs: Arc::new(exhaustive_provider_catalogs),
            model_catalog_accounts: Arc::default(),
            account_directory: Arc::new(RuntimeAccountDirectory::default()),
            client_policies: Arc::new(client_policy_map),
        })
    }

    #[must_use]
    pub fn with_account_directory(mut self, directory: Arc<RuntimeAccountDirectory>) -> Self {
        self.account_directory = directory;
        self
    }

    #[must_use]
    fn with_exhaustive_provider_catalogs(mut self, providers: BTreeSet<ProviderKind>) -> Self {
        self.exhaustive_provider_catalogs = Arc::new(providers);
        self
    }

    fn with_model_catalog_accounts(mut self, accounts: ModelCatalogAccounts) -> Self {
        self.model_catalog_accounts = Arc::new(accounts);
        self
    }

    #[must_use]
    pub fn all_account_scope(&self) -> Arc<FrozenAccountScope> {
        Arc::new(FrozenAccountScope::new(
            Arc::clone(&self.account_directory),
            ClientRoutingScope::all_accounts(),
        ))
    }

    #[must_use]
    fn with_provider_catalog_generations(
        mut self,
        generations: BTreeMap<ProviderKind, ProviderCatalogGeneration>,
    ) -> Self {
        self.provider_catalog_generations = Arc::new(generations);
        self
    }

    #[must_use]
    pub fn provider_catalog_generations(
        &self,
    ) -> &BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        &self.provider_catalog_generations
    }

    #[must_use]
    pub const fn revision(&self) -> ConfigRevision {
        self.revision
    }

    /// 返回目录发现模型与设置映射的并集，仅用于公开模型展示
    #[must_use]
    pub fn public_models_for_provider(&self, provider: &ProviderKind) -> Vec<PublicModelId> {
        if !self.providers.contains(provider) {
            return Vec::new();
        }
        let mut models = BTreeSet::new();
        if let Some(discovered) = self.provider_models.get(provider) {
            models.extend(
                discovered
                    .keys()
                    .filter_map(|model| PublicModelId::new(model.as_str().to_owned()).ok()),
            );
        }
        models.extend(
            self.settings
                .values
                .model_mappings
                .keys()
                .filter_map(|model| PublicModelId::new(model.clone()).ok()),
        );
        if let Some(aliases) = self.account_directory.model_aliases(provider) {
            models.extend(
                aliases
                    .iter()
                    .filter_map(|source| PublicModelId::new(source).ok()),
            );
        }
        models.extend(
            self.model_aliases()
                .iter()
                .filter(|alias| &alias.provider == provider)
                .map(|alias| alias.id.clone()),
        );
        models.extend(
            self.contributed_models()
                .iter()
                .filter(|model| &model.provider == provider)
                .map(|model| model.id.clone()),
        );
        models.into_iter().collect()
    }

    /// 返回 Provider 已明确声明画像的公开模型；没有画像时不猜测 Provider 语义
    #[must_use]
    pub fn public_model_profiles_for_provider(
        &self,
        provider: &ProviderKind,
    ) -> Vec<super::PublicModelProfile> {
        let Some(presentations) = self.provider_model_presentations.get(provider) else {
            return Vec::new();
        };
        let mut profiles = BTreeMap::new();
        for (model, presentation) in presentations {
            if let Ok(public_model) = PublicModelId::new(model.as_str().to_owned()) {
                profiles.insert(public_model, presentation.clone());
            }
        }
        for alias in self.settings.values.model_mappings.keys() {
            let target = self.mapped_model(alias);
            let Some(presentation) = presentations.iter().find_map(|(model, presentation)| {
                (model.as_str() == target).then_some(presentation)
            }) else {
                continue;
            };
            if let Ok(public_model) = PublicModelId::new(alias.clone()) {
                profiles.insert(public_model, presentation.clone());
            }
        }
        for alias in self
            .model_aliases()
            .iter()
            .filter(|alias| &alias.provider == provider)
        {
            if let Some(presentation) = presentations.get(&alias.target) {
                profiles.insert(alias.id.clone(), presentation.clone());
            }
        }
        profiles
            .into_iter()
            .map(|(model, presentation)| super::PublicModelProfile::new(model, presentation))
            .collect()
    }

    #[must_use]
    pub fn public_models(&self) -> Vec<PublicModelId> {
        self.providers
            .iter()
            .flat_map(|provider| self.public_models_for_provider(provider))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// 合并冻结账号范围内实际存在 Provider 的公开模型
    #[must_use]
    pub fn public_models_for_scope(&self, scope: &FrozenAccountScope) -> Vec<PublicModelId> {
        scope
            .provider_kinds()
            .iter()
            .flat_map(|provider| {
                self.public_models_for_provider(provider)
                    .into_iter()
                    .filter(|model| self.catalog_model_allowed_for_scope(provider, model, scope))
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// 合并冻结账号范围内实际存在 Provider 的公开模型画像
    #[must_use]
    pub fn public_model_profiles_for_scope(
        &self,
        scope: &FrozenAccountScope,
    ) -> Vec<super::PublicModelProfile> {
        let mut profiles = BTreeMap::new();
        for provider in scope.provider_kinds() {
            for profile in self.profiles_for_provider_scope(provider, scope) {
                profiles
                    .entry(profile.model().clone())
                    .or_insert_with(|| profile.presentation().clone());
            }
        }
        profiles
            .into_iter()
            .map(|(model, presentation)| super::PublicModelProfile::new(model, presentation))
            .collect()
    }

    pub(crate) fn profiles_for_provider_scope(
        &self,
        provider: &ProviderKind,
        scope: &FrozenAccountScope,
    ) -> Vec<super::PublicModelProfile> {
        if scope.allowed_account_ids().any(|account| {
            scope.account_provider(account) == Some(provider)
                && scope
                    .directory()
                    .account(account)
                    .is_some_and(|facts| facts.model_mappings().iter().next().is_some())
        }) {
            return self
                .public_models_for_provider(provider)
                .into_iter()
                .filter_map(|model| {
                    scope
                        .allowed_account_ids()
                        .filter(|account| scope.account_provider(account) == Some(provider))
                        .find_map(|account| {
                            if !self.allows_public_model_for_account(&model, scope, account) {
                                return None;
                            }
                            let target = self.upstream_model_for_account(&model, scope, account);
                            let presentation = self
                                .provider_model_presentations
                                .get(provider)
                                .and_then(|models| {
                                    models.iter().find_map(|(id, presentation)| {
                                        (id.as_str() == target
                                            && self.native_catalog_model_allowed_for_account(
                                                provider, &model, scope, account,
                                            ))
                                        .then_some(presentation)
                                    })
                                })
                                .or_else(|| {
                                    self.contributed_presentation_for_account(
                                        provider, &model, scope, account,
                                    )
                                })?;
                            Some(super::PublicModelProfile::new(
                                model.clone(),
                                presentation.clone(),
                            ))
                        })
                })
                .collect();
        }
        self.public_model_profiles_for_provider(provider)
            .into_iter()
            .filter(|profile| {
                self.native_catalog_model_allowed_for_scope(provider, profile.model(), scope)
            })
            .chain(self.contributed_profiles_for_scope(provider, scope))
            .collect()
    }

    /// 已知模型只属于明确声明它的 Provider；未知模型仍可由非完整目录透传
    #[must_use]
    pub fn contains_public_model_for_provider(
        &self,
        public_model: &PublicModelId,
        provider: &ProviderKind,
    ) -> bool {
        if !self.providers.contains(provider) {
            return false;
        }
        if self
            .model_alias(public_model.as_str())
            .is_some_and(|alias| &alias.provider != provider)
        {
            return false;
        }
        let Ok(upstream_model) =
            UpstreamModelId::from_client_wire(self.mapped_model(public_model.as_str()))
        else {
            return false;
        };
        self.contains_resolved_model_for_provider(public_model.as_str(), &upstream_model, provider)
            || self.account_directory.accounts().any(|(_, account)| {
                account.provider_kind() == provider
                    && account
                        .model_mappings()
                        .target(public_model.as_str())
                        .is_some_and(|target| self.contains_target_for_provider(target, provider))
            })
    }

    #[must_use]
    pub fn upstream_model_for_account(
        &self,
        public_model: &PublicModelId,
        scope: &FrozenAccountScope,
        account: &ProviderAccountId,
    ) -> String {
        scope
            .model_override(account, public_model.as_str())
            .map_or_else(|| self.mapped_model(public_model.as_str()), str::to_owned)
    }

    fn contains_target_for_provider(&self, target: &str, provider: &ProviderKind) -> bool {
        let Ok(target) = UpstreamModelId::new(target) else {
            return false;
        };
        self.contains_resolved_model_for_provider(target.as_str(), &target, provider)
    }

    fn contains_resolved_model_for_provider(
        &self,
        public: &str,
        upstream: &UpstreamModelId,
        provider: &ProviderKind,
    ) -> bool {
        self.provider_models
            .get(provider)
            .is_some_and(|models| models.contains_key(upstream))
            || self
                .contributed_models()
                .iter()
                .any(|model| &model.provider == provider && model.id.as_str() == public)
            || (!self.exhaustive_provider_catalogs.contains(provider)
                && !self
                    .contributed_models()
                    .iter()
                    .any(|model| model.id.as_str() == public)
                && !self.is_known_upstream_model(upstream))
    }

    /// 通用模型候选资格，不以原生目录来源限制推理，也不替代操作能力与实时选号检查
    #[must_use]
    pub fn allows_public_model_for_account(
        &self,
        public_model: &PublicModelId,
        scope: &FrozenAccountScope,
        account: &ProviderAccountId,
    ) -> bool {
        let Some(provider) = scope.account_provider(account) else {
            return false;
        };
        if self
            .model_alias(public_model.as_str())
            .is_some_and(|alias| &alias.provider != provider)
        {
            return false;
        }
        let overridden = scope.model_override(account, public_model.as_str());
        let upstream = self.upstream_model_for_account(public_model, scope, account);
        let contribution_id = overridden.unwrap_or(public_model.as_str());
        let Ok(upstream) = UpstreamModelId::from_client_wire(upstream) else {
            return false;
        };
        if !scope.allows_request_model(account, public_model.as_str(), upstream.as_str())
            || !self.contains_resolved_model_for_provider(contribution_id, &upstream, provider)
        {
            return false;
        }
        let native = self
            .provider_models
            .get(provider)
            .is_some_and(|models| models.contains_key(&upstream));
        // 仅由适配器贡献的模型仍受声明账号范围限制，不能借发现型目录扩大资格
        native
            || !self
                .contributed_models()
                .iter()
                .any(|model| &model.provider == provider && model.id.as_str() == contribution_id)
            || self.contributed_models().iter().any(|model| {
                &model.provider == provider
                    && model.id.as_str() == contribution_id
                    && model.accounts.contains(account)
            })
    }

    fn is_known_upstream_model(&self, upstream_model: &UpstreamModelId) -> bool {
        self.provider_models
            .values()
            .any(|models| models.contains_key(upstream_model))
            || self
                .contributed_models()
                .iter()
                .any(|model| model.id.as_str() == upstream_model.as_str())
    }

    #[must_use]
    pub fn contains_public_model_for_scope(
        &self,
        public_model: &PublicModelId,
        scope: &FrozenAccountScope,
    ) -> bool {
        scope.provider_kinds().iter().any(|provider| {
            self.contains_public_model_for_provider(public_model, provider)
                && self.catalog_model_allowed_for_scope(provider, public_model, scope)
        })
    }

    pub(crate) fn catalog_model_allowed_for_scope(
        &self,
        provider: &ProviderKind,
        public_model: &PublicModelId,
        scope: &FrozenAccountScope,
    ) -> bool {
        if self
            .model_alias(public_model.as_str())
            .is_some_and(|alias| &alias.provider != provider)
        {
            return false;
        }
        self.native_catalog_model_allowed_for_scope(provider, public_model, scope)
            || scope.allowed_account_ids().any(|account| {
                let contribution_id = scope
                    .model_override(account, public_model.as_str())
                    .unwrap_or(public_model.as_str());
                scope.account_provider(account) == Some(provider)
                    && scope.allows_request_model(
                        account,
                        public_model.as_str(),
                        &self.upstream_model_for_account(public_model, scope, account),
                    )
                    && self.contributed_models().iter().any(|model| {
                        &model.provider == provider
                            && model.id.as_str() == contribution_id
                            && model.accounts.contains(account)
                    })
            })
    }

    fn native_catalog_model_allowed_for_scope(
        &self,
        provider: &ProviderKind,
        public_model: &PublicModelId,
        scope: &FrozenAccountScope,
    ) -> bool {
        scope.allowed_account_ids().any(|account| {
            scope.account_provider(account) == Some(provider)
                && self.native_catalog_model_allowed_for_account(
                    provider,
                    public_model,
                    scope,
                    account,
                )
        })
    }

    fn native_catalog_model_allowed_for_account(
        &self,
        provider: &ProviderKind,
        public_model: &PublicModelId,
        scope: &FrozenAccountScope,
        account: &ProviderAccountId,
    ) -> bool {
        let upstream = self.upstream_model_for_account(public_model, scope, account);
        if !scope.allows_request_model(account, public_model.as_str(), &upstream) {
            return false;
        }
        match self
            .model_catalog_accounts
            .get(provider)
            .and_then(|models| models.get(&upstream))
        {
            Some(accounts) => accounts.contains(account),
            None => {
                let native = self
                    .provider_models
                    .get(provider)
                    .is_some_and(|models| models.keys().any(|model| model.as_str() == upstream));
                let contribution_id = scope
                    .model_override(account, public_model.as_str())
                    .unwrap_or(public_model.as_str());
                let contributed = self.contributed_models().iter().any(|model| {
                    &model.provider == provider && model.id.as_str() == contribution_id
                });
                let local_alias = self
                    .account_directory
                    .model_aliases(provider)
                    .is_some_and(|aliases| aliases.contains(public_model.as_str()));
                let inherited_alias = self
                    .settings
                    .values
                    .model_mappings
                    .contains_key(public_model.as_str())
                    || self.model_alias(public_model.as_str()).is_some();
                native
                    || (!contributed
                        && scope
                            .model_override(account, public_model.as_str())
                            .is_none()
                        && (!local_alias || inherited_alias))
            }
        }
    }

    #[must_use]
    pub fn mapped_model(&self, requested: &str) -> String {
        if let Some(alias) = self.model_alias(requested) {
            return alias.target.as_str().to_owned();
        }
        let original = requested;
        let mut current = original.to_owned();
        let mut seen = BTreeSet::new();
        for _ in 0..20 {
            let Some(target) = self
                .settings
                .values
                .model_mappings
                .get(&current)
                .map(String::as_str)
            else {
                return current;
            };
            if !seen.insert(current.clone()) || seen.contains(target) {
                return original.to_owned();
            }
            current = target.to_owned();
        }
        original.to_owned()
    }

    pub fn client_policies(&self) -> impl Iterator<Item = &ClientPolicy> {
        self.client_policies.values()
    }

    #[must_use]
    pub fn client_policy(&self, id: &ClientApiKeyId) -> Option<&ClientPolicy> {
        self.client_policies.get(id)
    }

    /// 返回当前 Key 范围、本代次注册表与请求路由限制共同允许的 Provider
    #[must_use]
    pub fn available_providers(
        &self,
        account_scope: &FrozenAccountScope,
        context: &RoutingContext,
    ) -> BTreeSet<ProviderKind> {
        account_scope
            .provider_kinds()
            .iter()
            .filter(|provider| {
                self.providers.contains(*provider)
                    && context
                        .required_provider
                        .as_ref()
                        .is_none_or(|required| required == *provider)
                    && !context.blocked_providers.contains(*provider)
            })
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn min_codex_client_versions(&self) -> &CodexClientMinVersions {
        &self.settings.min_codex_client_versions
    }

    pub fn plan(
        &self,
        public_model: &PublicModelId,
        operation: &Operation,
        account_scope: Arc<FrozenAccountScope>,
        context: &RoutingContext,
    ) -> Result<RoutingPlan, RoutingError> {
        if matches!(operation, Operation::Generate(_))
            && account_scope
                .directory()
                .has_model_mapping(public_model.as_str())
            && account_scope.allowed_account_ids().any(|account| {
                account_scope
                    .model_override(account, public_model.as_str())
                    .is_some()
            })
        {
            return self.plan_account_mappings(public_model, operation, account_scope, context);
        }
        self.plan_inner(public_model, operation, account_scope, context, true, true)
    }

    /// 管理探测已固定目标账号，不使用数据面 Key 的账号模型政策裁决探测资格
    pub(crate) fn plan_diagnostic(
        &self,
        public_model: &PublicModelId,
        operation: &Operation,
        context: &RoutingContext,
    ) -> Result<RoutingPlan, RoutingError> {
        self.plan_inner(
            public_model,
            operation,
            self.all_account_scope(),
            context,
            false,
            true,
        )
    }

    fn plan_account_mappings(
        &self,
        public_model: &PublicModelId,
        operation: &Operation,
        account_scope: Arc<FrozenAccountScope>,
        context: &RoutingContext,
    ) -> Result<RoutingPlan, RoutingError> {
        let mut inherited = BTreeSet::new();
        let mut groups = BTreeMap::<(ProviderKind, String), BTreeSet<ProviderAccountId>>::new();
        for account in account_scope.allowed_account_ids() {
            if let Some(target) = account_scope.model_override(account, public_model.as_str()) {
                if !account_scope.allows_request_model(account, public_model.as_str(), target) {
                    continue;
                }
                if let Some(provider) = account_scope.account_provider(account) {
                    if self
                        .model_alias(public_model.as_str())
                        .is_some_and(|alias| &alias.provider != provider)
                    {
                        continue;
                    }
                    groups
                        .entry((provider.clone(), target.to_owned()))
                        .or_default()
                        .insert(account.clone());
                }
            } else {
                inherited.insert(account.clone());
            }
        }
        let mut plans = Vec::new();
        if !inherited.is_empty() {
            let scope = Arc::new(account_scope.restricted_to(&Default::default(), &inherited));
            if let Ok(plan) = self.plan_inner(public_model, operation, scope, context, true, true) {
                plans.push((plan, false));
            }
        }
        for ((provider, target), accounts) in groups {
            if context
                .required_provider
                .as_ref()
                .is_some_and(|required| required != &provider)
                || context.blocked_providers.contains(&provider)
            {
                continue;
            }
            let target = PublicModelId::new(target).map_err(|_| RoutingError::InvalidIdentifier)?;
            let scope = Arc::new(account_scope.restricted_to(&Default::default(), &accounts));
            let mut target_context = context.clone();
            target_context.required_provider = Some(provider);
            // 账号已按左侧请求名通过政策检查，目标仅参与归属和能力编译
            if let Ok(plan) =
                self.plan_inner(&target, operation, scope, &target_context, false, false)
            {
                plans.push((plan, true));
            }
        }
        let mut candidates = BTreeMap::<ProviderKind, ProviderCandidate>::new();
        for (plan, account_mapping) in &plans {
            for candidate in plan.candidates() {
                let route = Arc::new(ProviderModelRoute {
                    account_mapping: *account_mapping,
                    upstream_model: candidate.upstream_model.clone(),
                    model_presentation: candidate.model_presentation.clone(),
                    emulated_features: candidate.emulated_features.clone(),
                });
                let merged = candidates
                    .entry(candidate.provider.clone())
                    .or_insert_with(|| candidate.clone());
                let models = Arc::make_mut(&mut merged.account_models);
                for account in candidate.account_scope.allowed_account_ids() {
                    if candidate.account_scope.account_provider(account)
                        == Some(candidate.provider())
                    {
                        models.insert(account.clone(), Arc::clone(&route));
                    }
                }
            }
        }
        for candidate in candidates.values_mut() {
            let accounts = candidate.account_models.keys().cloned().collect();
            candidate.account_scope =
                Arc::new(account_scope.restricted_to(&Default::default(), &accounts));
        }
        let Some((mut plan, _)) = plans.into_iter().next() else {
            return Err(RoutingError::NoCapableProvider {
                model: public_model.as_str().to_owned(),
            });
        };
        plan.account_scope = account_scope;
        plan.candidates = candidates.into_values().collect();
        Ok(plan)
    }

    fn plan_inner(
        &self,
        public_model: &PublicModelId,
        operation: &Operation,
        account_scope: Arc<FrozenAccountScope>,
        context: &RoutingContext,
        enforce_account_model_policy: bool,
        resolve_global: bool,
    ) -> Result<RoutingPlan, RoutingError> {
        let requirements = operation.capability_requirements();
        let mut candidates = Vec::new();

        if context.required_provider.is_none() && account_scope.provider_kinds().is_empty() {
            return Err(RoutingError::EmptyAccountScope);
        }

        let requested_model = public_model.as_str();
        let mapped_model = if resolve_global {
            self.mapped_model(requested_model)
        } else {
            requested_model.to_owned()
        };
        let upstream_model = if !resolve_global
            || self
                .settings
                .values
                .model_mappings
                .contains_key(requested_model)
            || self.model_alias(requested_model).is_some()
        {
            UpstreamModelId::new(mapped_model)
        } else {
            UpstreamModelId::from_client_wire(mapped_model)
        }
        .map_err(|_| RoutingError::InvalidIdentifier)?;
        // 归属先于账号范围、能力与路由约束判定，原 Provider 被排除不能变成跨平台透传
        let known_model = self.is_known_upstream_model(&upstream_model)
            || self
                .contributed_models()
                .iter()
                .any(|model| &model.id == public_model);

        let providers = context.required_provider.as_ref().map_or_else(
            || account_scope.provider_kinds().clone(),
            |provider| BTreeSet::from([provider.clone()]),
        );
        for provider in &providers {
            if !self.providers.contains(provider) {
                continue;
            }
            if resolve_global
                && self
                    .model_alias(public_model.as_str())
                    .is_some_and(|alias| &alias.provider != provider)
            {
                continue;
            }
            if context
                .required_provider
                .as_ref()
                .is_some_and(|expected| expected != provider)
                || context.blocked_providers.contains(provider)
            {
                continue;
            }
            // 强制 Provider 路由也不能扩大 Key 授权；管理诊断的目标账号由探测入口固定
            if enforce_account_model_policy
                && !account_scope.allows_provider_model(provider, upstream_model.as_str())
            {
                continue;
            }
            let capabilities = self
                .provider_models
                .get(provider)
                .and_then(|models| models.get(&upstream_model));
            let contributions: Vec<_> = self
                .contributed_models()
                .iter()
                .filter(|model| &model.provider == provider && &model.id == public_model)
                .collect();
            let mut candidate_scope = Arc::clone(&account_scope);
            let emulated_features = if contributions.is_empty() {
                match capabilities {
                    Some(capabilities) => {
                        let Some(emulated) = capabilities.match_requirements(&requirements) else {
                            continue;
                        };
                        emulated
                    }
                    None if known_model || self.exhaustive_provider_catalogs.contains(provider) => {
                        continue;
                    }
                    None => BTreeSet::new(),
                }
            } else {
                let native = capabilities.and_then(|value| value.match_requirements(&requirements));
                let capable_accounts: BTreeSet<_> = contributions
                    .iter()
                    .filter(|model| {
                        model
                            .capabilities
                            .match_requirements(&requirements)
                            .is_some()
                    })
                    .flat_map(|model| model.accounts.iter())
                    .filter(|account| {
                        account_scope.account_provider(account) == Some(provider)
                            && account_scope.allows(account)
                            && (!resolve_global
                                || account_scope.allows_model(account, upstream_model.as_str()))
                    })
                    .cloned()
                    .collect();
                if native.is_some() {
                    // 这些账号实际由适配器执行，不能借用同名原生模型的能力
                    let mut restricted = account_scope.as_ref().clone();
                    for account in contributions.iter().flat_map(|model| &model.accounts) {
                        if !capable_accounts.contains(account) {
                            restricted = restricted.excluding_account(account.clone());
                        }
                    }
                    candidate_scope = Arc::new(restricted);
                } else {
                    // restricted_to 的空集合表示不收窄，因此必须先拒绝空候选
                    if capable_accounts.is_empty() {
                        continue;
                    }
                    candidate_scope = Arc::new(
                        account_scope.restricted_to(&Default::default(), &capable_accounts),
                    );
                }
                if resolve_global
                    && !candidate_scope.allows_provider_model(provider, upstream_model.as_str())
                {
                    continue;
                }
                native.unwrap_or_default()
            };
            candidates.push(ProviderCandidate {
                provider: provider.clone(),
                model_presentation: self
                    .provider_model_presentations
                    .get(provider)
                    .and_then(|models| models.get(&upstream_model))
                    .cloned()
                    .or_else(|| {
                        contributions
                            .first()
                            .map(|model| model.presentation.clone())
                    }),
                upstream_model: Some(upstream_model.clone()),
                emulated_features,
                account_scope: candidate_scope,
                account_models: Arc::default(),
            });
        }

        // 冷启动或目录不可读时不能按 Provider 名称猜测模型归属
        if !known_model && candidates.len() > 1 {
            return Err(RoutingError::NoCapableProvider {
                model: public_model.as_str().to_owned(),
            });
        }

        if candidates.is_empty() {
            // 模型存在性必须包含被请求路由限制排除的 Provider；目录未知时不能断言模型不存在
            let mut scoped_providers = providers.intersection(&self.providers).peekable();
            if scoped_providers.peek().is_some()
                && scoped_providers.all(|provider| {
                    self.exhaustive_provider_catalogs.contains(provider)
                        && !self.contains_public_model_for_provider(public_model, provider)
                })
            {
                return Err(RoutingError::ModelNotFound {
                    model: public_model.as_str().to_owned(),
                    mapped_model: self.mapped_model(public_model.as_str()),
                });
            }
            return Err(RoutingError::NoCapableProvider {
                model: public_model.as_str().to_owned(),
            });
        }

        Ok(RoutingPlan {
            request_body_limit: self.responses_max_decompressed_body_bytes(),
            max_retry_after: std::time::Duration::from_secs(u64::from(
                self.settings.values.max_retry_after_seconds,
            )),
            config_revision: self.revision,
            pricing: Arc::clone(&self.settings.values.pricing),
            request_location: self.settings.request_location.clone(),
            privacy: self.settings.privacy.clone(),
            account_error_protection: Arc::clone(&self.settings.account_error_protection),
            account_error_protection_revision: self
                .settings
                .values
                .account_error_protection_revision,
            account_selection_policy: self.settings.account_selection_policy,
            operation: operation.kind(),
            max_attempts: NonZeroU32::new(super::MAX_REQUEST_ATTEMPTS)
                .expect("constant request attempt limit is non-zero"),
            account_scope,
            candidates: Arc::from(candidates),
        })
    }

    /// 为 Provider 自有端点冻结请求计划
    ///
    /// 端点 adapter 已经确定 Provider
    /// 非模型 HTTP 端点不读取文本模型目录；Token
    /// 计数端点仍必须匹配冻结账号范围及该模型声明的计数能力
    pub fn plan_provider_endpoint(
        &self,
        provider: &ProviderKind,
        upstream_model: Option<&UpstreamModelId>,
        operation: &Operation,
        account_scope: Arc<FrozenAccountScope>,
        context: &RoutingContext,
    ) -> Result<RoutingPlan, RoutingError> {
        let available = !account_scope.provider_kinds().is_empty()
            && account_scope.provider_kinds().contains(provider)
            && self.providers.contains(provider)
            && context
                .required_provider
                .as_ref()
                .is_none_or(|required| required == provider)
            && !context.blocked_providers.contains(provider);
        if !available
            || upstream_model
                .is_some_and(|model| !account_scope.allows_provider_model(provider, model.as_str()))
        {
            return Err(RoutingError::NoCapableProviderEndpoint {
                provider: provider.as_str().to_owned(),
            });
        }
        // CountTokens 必须绑定目标模型；其余端点可选携带上游模型——Provider 原生
        // 端点（如 live 语音）借此让账号模型权限参与路由检查。
        let model_binding_valid = if matches!(operation.kind(), OperationKind::CountTokens) {
            upstream_model.is_some()
        } else {
            true
        };
        let model_capable = upstream_model.is_none_or(|model| {
            match self
                .provider_models
                .get(provider)
                .and_then(|models| models.get(model))
            {
                Some(capabilities) => capabilities
                    .match_requirements(&operation.capability_requirements())
                    .is_some(),
                None => !self.exhaustive_provider_catalogs.contains(provider),
            }
        });
        if !model_binding_valid || !model_capable {
            return Err(RoutingError::UnsupportedProviderEndpoint {
                provider: provider.as_str().to_owned(),
                operation: operation.kind().as_str(),
            });
        }
        let candidate = ProviderCandidate {
            provider: provider.clone(),
            model_presentation: upstream_model
                .and_then(|model| {
                    self.provider_model_presentations
                        .get(provider)
                        .and_then(|models| models.get(model))
                })
                .cloned(),
            upstream_model: upstream_model.cloned(),
            emulated_features: BTreeSet::new(),
            account_scope: Arc::clone(&account_scope),
            account_models: Arc::default(),
        };
        Ok(RoutingPlan {
            request_body_limit: self.responses_max_decompressed_body_bytes(),
            max_retry_after: std::time::Duration::from_secs(u64::from(
                self.settings.values.max_retry_after_seconds,
            )),
            config_revision: self.revision,
            pricing: Arc::clone(&self.settings.values.pricing),
            request_location: self.settings.request_location.clone(),
            privacy: self.settings.privacy.clone(),
            account_error_protection: Arc::clone(&self.settings.account_error_protection),
            account_error_protection_revision: self
                .settings
                .values
                .account_error_protection_revision,
            account_selection_policy: self.settings.account_selection_policy,
            operation: operation.kind(),
            max_attempts: NonZeroU32::new(super::MAX_REQUEST_ATTEMPTS)
                .expect("constant request attempt limit is non-zero"),
            account_scope,
            candidates: Arc::from([candidate]),
        })
    }
}

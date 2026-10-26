# 二开行为与维护索引

本文面向本仓库维护者，集中列出相对已合并上游基线仍然存在的行为差异、维护入口和回归重点。
接口字段、部署步骤与架构不变量仍以各自文档为准；这里提供摘要和定位入口，不另建一套完整合同

## 范围与阅读方式

| 项目 | 核对范围 |
| --- | --- |
| 核对日期 | 2026-10-26 |
| 本仓库源码 | `b4f9c942c03466d8c5519b6f91573ff1d1351afb`，源码版本以 [`release/version.yaml`](../release/version.yaml) 为准 |
| 最近完整合并的上游基线 | `v3.29.7`，`78bc57c5cf24b01a38e985cdd739bb4ae89ce11f` |
| 差异依据 | 上述上游基线到本仓库核对提交的最终文件差异，结合调用链和现有测试 |
| 上游主题移植 | 已采用能力与暂缓项分别记录于[上游同步清单](upstream-sync.md)，核对目标为 `v3.32.0` |

源码版本、发布完成和运行实例升级是不同事实，本文不代表某个实例已部署该版本。
这里的“二开”包含主动定制和为保留本发行线而存在的兼容适配，不表示相关基础能力都由本仓库原创

| 维护任务 | 入口 |
| --- | --- |
| 失败后为什么重试、换号或停止 | [请求恢复与保底](#请求恢复与保底) |
| 日常选号、点数支出和临时传输状态 | [账号调度与额度](#账号调度与额度)、[WebSocket 降级与恢复](#websocket-降级与恢复) |
| 三方接入和 Chat 客户端 | [协议与模型目录](#协议与模型目录) |
| 请求筛选、排障和页面布局 | [请求记录与诊断](#请求记录与诊断)、[管理端交互](#管理端交互) |
| 合并升级、数据恢复和发版 | [迁移与备份](#迁移与备份)、[发行与协作](#发行与协作) |

## 请求恢复与保底

### 可恢复错误与共享预算

OpenAI 容量拒绝，以及被识别为普通服务不可用的 HTTP `500` / `503`、无 HTTP 状态码的
`server_error` / `server_busy`，使用 `RetryThenRotateAccountWithBudget`。
Core 统一负责退避、阶段推进和安全重放，Provider 负责错误分类及重放资格，不将所有 `5xx` 一概换号

容量错误和普通服务错误共用请求预算，切换错误码、账号或传输方式不重新获得一份业务重试次数。
普通服务错误保留同轮次失败抑制，容量拒绝不受该失败标记抑制。
上游 `Retry-After` 优先；没有提示时使用已有指数退避和随机等待

普通账号总尝试次数由 `recoveryAttempts=N` 控制，默认 4，范围 1～31，包含首次尝试。
初始账号分配 `ceil(N / 2)` 次，其余 `floor(N / 2)` 次在排除初始账号的合格普通池中轮询，最后保底最多尝试 1 次。
例如普通账号 A、B 和保底 D 全部失败时，默认顺序为 `A-A-B-B-D`；存在 B、C 两个其他普通账号时可为 `A-A-B-C-D`

上述顺序是预算允许且候选可用时的恢复路径，不保证每次请求走满次数。
任意一次成功即结束；原账号明确不可用可提前换号，没有其他普通账号时可进入保底，没有保底时返回最后一次上游错误。
同账号退避后若原账号已不可用，允许在原有安全门禁和预算内重新选号，不因固定重试账号失效直接丢失恢复机会

### 必须保留的限制

- 日常首次选号沿用既有调度策略、权重、账号范围和会话亲和；恢复轮询遍历其他合格普通池，不被日常权重层截断
- 恢复和保底保留根会话主绑定，下一条普通请求继续使用原有亲和规则；会话关联依赖 Client Key 与逻辑会话，不依赖 User-Agent
- 严格亲和跟随请求、原生增量续接、外部指定账号、已交付输出、外部副作用和插件约束继续限制跨账号重放
- 发送结果不明、Provider 无法证明重放安全、取消、截止时间及连接和路由预算仍可终止恢复
- `maxAccountRotations=0` 仍禁止失败后换号；非零值与业务恢复链的关系按 [API 合同](api.md#账号亲和与换号)执行
- `recoveryAttempts` 和账号用途按请求快照冻结，保存配置不改变执行中请求的恢复策略

完整行为见[容量拒绝与重试](api.md#容量拒绝与重试)及[数据面请求生命周期](architecture.md#4-数据面请求生命周期)

| 职责 | 代码与回归入口 |
| --- | --- |
| 错误分类和重放资格 | [diagnostics.rs](../backend/crates/providers/openai/src/transport/diagnostics/mod.rs)、[failure.rs](../backend/crates/providers/openai/src/provider/failure.rs)、[Provider 恢复测试](../backend/crates/providers/openai/tests/provider/contract/selection_retry.rs) |
| 共享预算、退避、换号和终态 | [coordinator.rs](../backend/crates/gateway-core/src/engine/coordinator.rs)、[recovery.rs](../backend/crates/gateway-core/src/engine/recovery.rs)、[协调器测试](../backend/crates/gateway-core/tests/engine/coordinator.rs) |
| Provider 与插件的共同阶段约束 | [AttemptContext](../backend/crates/gateway-core/src/engine/mod.rs)、[插件错误投影](../backend/crates/gateway-plugin/runtime/src/callback/error.rs)、[插件设置快照](../backend/crates/gateway-plugin/sdk/src/call/services/settings.rs) |

## 账号调度与额度

### 普通账号与保底账号

账号 `schedulingRole` 分为 `regular` 与 `fallback`，默认 `regular`，与 OAuth / API Key 认证方式独立。
保底账号不参与正常首次分配，只在恢复阶段使用；账号自身仍需满足启用、权限、模型、额度、冷却和并发条件。
编辑和批量编辑均可设置用途，账号表显示保底标识，诊断指定账号测试沿用其显式目标

角色属于账号配置，通过 Store、配置 revision 和 Core 冻结账号范围传递；OpenAI、xAI 和插件选号共用阶段过滤。
配置入口见[配置保底账号](../deploy/README.md#配置保底账号)，字段见[查询与账号字段](api.md#查询与账号字段)

### 点数支出开关

OpenAI OAuth 账号的 `allowCredits` 默认关闭，在账号“额度重置”面板中单独设置。
关闭时套餐额度耗尽即暂停调度；开启仅允许上游已确认可用的点数，不绕过账号或工作区支出限制。
它与自动使用重置卡相互独立，额度刷新、重新导入和重新授权不重置该设置

上游额度事实与本地点数支出策略分别保存，调度可用性在读取时组合，不能把本地放行写成新的上游额度事实。
配置无法撤回已经发出的请求，上游额度同步也可能延迟，不能用开关保证点数绝不产生支出

| 职责 | 代码与回归入口 |
| --- | --- |
| 角色、点数策略与原始额度事实 | [account/model.rs](../backend/crates/gateway-core/src/account/model.rs)、[account/scope.rs](../backend/crates/gateway-core/src/account/scope.rs) |
| 配置持久化与请求快照 | [provider_accounts](../backend/crates/gateway-store/src/postgres/provider_accounts/repository.rs)、[snapshot.rs](../backend/crates/gateway-store/src/postgres/snapshot.rs)、[账号存储测试](../backend/crates/gateway-store/tests/postgres/provider_accounts/mod.rs) |
| 点数事实解释与策略保留 | [quota/snapshot.rs](../backend/crates/providers/openai/src/credential/quota/snapshot.rs)、[quota/service.rs](../backend/crates/providers/openai/src/credential/quota/service.rs)、[点数测试](../backend/crates/providers/openai/tests/credential/quota/credits.rs) |
| 设置界面 | [AccountSettingsFields.vue](../frontend/src/views/accounts/components/AccountSettingsFields.vue)、[CreditsPolicy.vue](../frontend/src/views/accounts/components/account-quota-panel/CreditsPolicy.vue) |

## WebSocket 降级与恢复

OpenAI 临时传输恢复状态按“账号 + 逻辑会话”隔离，同一个会话在一个账号上失败，不会把其他账号一起降级。
账号 `websocketDegraded` 表示至少有一个保留中的会话暂时使用 HTTP/SSE，不代表该账号所有请求都固定走 HTTP

状态保存在进程内，相关会话成功使用 WebSocket 或空闲超过 8 小时后清除。
账号“恢复状态”会清除目标账号的临时降级，并推进状态代次，防止恢复前的在途失败重新写回旧降级。
该操作不修改其他账号，也不把显式配置为 `http` 的账号改成 WS 优先

业务重试、传输恢复和临时降级是三个不同事实：客户端 HTTP/SSE 可以连接上游 WebSocket；
实际传输需看 `transport.selected` 和 exchange 记录，不能仅凭页面接入类型或某次连接失败判断

字段与恢复接口见[查询与账号字段](api.md#查询与账号字段)和[OpenAI 身份、额度与状态](api.md#openai-身份额度与状态)

| 职责 | 代码与回归入口 |
| --- | --- |
| 状态隔离、有效期和代次 | [session_transport.rs](../backend/crates/providers/openai/src/session_transport.rs)、[Admin 测试](../backend/crates/providers/openai/tests/admin.rs) |
| 生成路径和失败反馈 | [execution.rs](../backend/crates/providers/openai/src/provider/execution.rs)、[failure.rs](../backend/crates/providers/openai/src/provider/failure.rs) |
| 管理投影与恢复入口 | [Provider admin.rs](../backend/crates/providers/openai/src/admin.rs)、[账号用例](../backend/crates/gateway-admin/src/use_case/accounts/mod.rs)、[降级提示](../frontend/src/views/accounts/components/AccountWebSocketDegradedIndicator.vue) |

## 协议与模型目录

### Chat Completions

`POST /v1/chat/completions` 在 API 层转为已有 Responses 执行，支持 JSON 与流式 Chat 输出，
复用 Client Key、模型映射、账号范围、并发、额度和费用结算，不维护独立的推理调度链。
请求中间件看到 Chat 合同，Provider 和 attempt 中间件仍使用 Responses 合同

消息、图片、函数工具、拒答、JSON 输出和推理参数的范围见 [Chat Completions](api.md#chat-completions)。
`max_completion_tokens` 映射为 `max_output_tokens`，Codex 路径按自身合同移除该字段，不能把它当成费用上限。
不支持的非空参数按字段返回 `400`；Chat 响应 ID 不用于 Responses 原生续接。
SSE 等待复用注释心跳，流开始后的错误保留 Chat 错误表达，不补造成功终态

### 三方模型目录格式

API Key 上游支持标准 OpenAI `data[].id` 列表和完整 Codex `models[]` 对象。
同时出现两个字段时，通过 `models` 条目中的原生身份字段识别格式；只有 `name` 等扩展元数据、空列表或 `null`
不会遮蔽标准 `data`。已声明原生身份的目录仍完整校验，不因条目错误回退为普通 ID 列表

原生目录保留推理能力、上下文限制、未知字段及显式空值；普通 ID 列表使用通用画像，不推断缺失能力。
完整对外合同见[模型目录](api.md#模型目录)

| 职责 | 代码与回归入口 |
| --- | --- |
| Chat 入口、校验和交付 | [API chat](../backend/crates/gateway-api/src/openai/chat/http.rs)、[入口测试](../backend/crates/gateway-api/tests/openai/chat/http.rs) |
| 纯协议转换 | [Chat request](../backend/crates/gateway-protocol/src/openai/chat/request.rs)、[response](../backend/crates/gateway-protocol/src/openai/chat/response.rs)、[stream](../backend/crates/gateway-protocol/src/openai/chat/stream.rs)、[协议测试](../backend/crates/gateway-protocol/tests/openai/chat/request.rs) |
| 上游目录判别及校验 | [transport/catalog.rs](../backend/crates/providers/openai/src/transport/catalog.rs)、[credential/catalog.rs 测试](../backend/crates/providers/openai/tests/credential/catalog.rs) |

## 请求记录与诊断

### 请求审计与用量范围

管理端使用统计默认显示全部请求审计，而非仅有完整用量的成功请求。
管理列表 `scope` 区分默认用量、全部、成功和错误；错误包含失败、未完成与取消，结果标签保留各自含义。
Key 用量页的错误范围仅包含失败与未完成，不能将两类页面的错误谓词合并。
分页、总数和定位页复用同一范围条件；统计概览继续使用其用量口径，明细筛选不改动概览

结果列显示真实请求结果、必要的尝试次数和错误摘要，未完成错误支持悬浮或点击查看。
尝试次数包含首次，不把“允许重试”的决策当作已经发生的重试。
明细支持模型执行 ID 前缀和完整时间关键词，`HH:mm:ss` 使用部署时区今天，
`YYYY-MM-DD HH:mm:ss` 使用指定日期，整秒匹配覆盖该秒内毫秒，时间关键词优先于日期选择器。
无效或未完成时间保持普通文本搜索

查询字段与统计边界见[请求字段与筛选](api.md#请求字段与筛选)和[记录范围与统计口径](api.md#记录范围与统计口径)

### 页面诊断与下载

详情保留页面时间线、关联失败或恢复请求及实际尝试链，下载由服务端导出，格式为 `schemaVersion: 6`。
传输失败提示区分底层错误类别，并在已有证据下显示连接复用、年龄、最近活动及最后事件。
旧记录缺少底层分类时提示证据不足，不补造精确根因

重试诊断保留门禁阻断原因、预算和退避；`retry.decided` 与后续 `attempt.started` 分开解释。
导出时环境不能代替事发配置，`availability.gaps` 和 best-effort 尝试投影必须保留。
凭据、认证头和账号联系信息由现有后端规则脱敏，任意业务内容不能视为已经自动安全处理

接口与脱敏边界见[诊断与恢复关联](api.md#诊断与恢复关联)

| 职责 | 代码与回归入口 |
| --- | --- |
| 查询范围与时间关键词 | [observability/query.rs](../backend/crates/gateway-api/src/admin/observability/query.rs)、[Store 查询](../backend/crates/gateway-store/src/postgres/observability/queries/usage.rs)、[API 测试](../backend/crates/gateway-api/tests/admin/observability/response.rs)、[分页测试](../backend/crates/gateway-store/tests/postgres/usage_paging.rs) |
| Key 侧范围隔离 | [key_usage.rs](../backend/crates/gateway-admin/src/use_case/key_usage.rs)、[Key API 测试](../backend/crates/gateway-api/tests/key_usage/mod.rs) |
| 导出与页面证据 | [export/mod.rs](../backend/crates/gateway-api/src/admin/observability/export/mod.rs)、[RequestDiagnosticsPanel.vue](../frontend/src/views/usage/components/RequestDiagnosticsPanel.vue)、[RequestTransportFailure.vue](../frontend/src/views/usage/components/RequestTransportFailure.vue) |

## 管理端交互

下表只描述本仓库当前的页面选择；视觉主题和基础组件约束仍以[管理端主题](theme.md)为准

| 页面 | 当前选择 | 维护入口 |
| --- | --- | --- |
| 使用统计 | 请求明细在前，统计概览在下且默认展开、可折叠；结果筛选显示图标并保留无障碍标签 | [usage/index.vue](../frontend/src/views/usage/index.vue) |
| 请求表格 | 请求时间靠前，收紧列宽；客户端列显示原始 User-Agent，最多两行，悬浮查看完整值 | [columns.ts](../frontend/src/components/usage/shared/columns.ts)、[UsageRecordsTable.vue](../frontend/src/components/usage/UsageRecordsTable.vue) |
| 账号 | 账号列表在前、概览卡在后；启停调度开关紧邻状态，保留 WS 降级提示和保底标识 | [accounts/index.vue](../frontend/src/views/accounts/index.vue)、[AccountSchedulingSwitch.vue](../frontend/src/views/accounts/components/AccountSchedulingSwitch.vue)、[AccountIdentityCell.vue](../frontend/src/views/accounts/components/AccountIdentityCell.vue) |
| 并发明细 | 显示当前占用与并发上限，提供手动刷新并保留每 5 秒自动刷新 | [AccountConcurrencyModal.vue](../frontend/src/views/accounts/components/AccountConcurrencyModal.vue)、[useAccountConcurrency.ts](../frontend/src/views/accounts/composables/useAccountConcurrency.ts) |
| Dashboard | 请求健康时间线与账号概览优先于汇总统计卡 | [DashboardContent.vue](../frontend/src/views/dashboard/components/DashboardContent.vue) |
| 列表与表单 | 账号及 API Key 排序偏好保存在当前浏览器；密钥表单按字段校验，校验失败时滚动并聚焦错误字段 | [useTableSort.ts](../frontend/src/composables/useTableSort.ts)、[ApiKeyFormModal.vue](../frontend/src/views/keys/components/ApiKeyFormModal.vue) |
| 设置草稿 | 基础设置与备份草稿有未保存改动时，离开设置区域需要确认，取消离开保留输入，关闭或刷新页面使用浏览器原生提醒 | [useSettingsLeaveGuard.ts](../frontend/src/views/settings/composables/useSettingsLeaveGuard.ts)、[useBackupSettings.ts](../frontend/src/views/settings/composables/useBackupSettings.ts) |
| 导航 | 侧边栏隐藏主题设置入口，保留主题页面和路由；移动端侧栏支持 Tab 焦点循环、Esc 关闭及关闭后恢复焦点 | [布局](../frontend/src/layout/index.vue)、[AppSidebar.vue](../frontend/src/layout/components/AppSidebar.vue) |

出口检测区分显式代理和直连的失败文案，显式代理连接失败不能偷偷改为直连。
管理员 API 保留 `/api/admin/proxies/probe/direct`，返回网关直连出口与地区；前端 API 封装不代表页面已有独立按钮。
批量代理创建、测试等上游能力仍使用逐项结果，不能仅看批次 HTTP `200`

出口实现与测试见 [proxy_probe.rs](../backend/crates/gateway-host/src/proxy_probe.rs)、[proxies.rs](../backend/crates/gateway-api/src/admin/proxies.rs)及[出口探测测试](../backend/crates/gateway-host/tests/proxy_probe.rs)

## 迁移与备份

本发行线保留自己的迁移编号及已发布文件，不能用上游同编号文件覆盖，也不能修改数据库 checksum 绕过校验

| 文件 | 当前责任 |
| --- | --- |
| `0032_request_history_retention.sql` | 请求历史保留与统计维护结构 |
| `0033_table_naming.sql` | 既有表命名适配 |
| `0034_group_image_and_scheduling.sql` | 分组生图权限与调度默认值适配 |
| `0035_proxy_and_reset_credits.sql` | 代理 WS 容量和重置卡自动化结构 |
| `0036_account_credits_policy.sql` | `allow_credits` 与条件额度事实 |
| `0037_account_scheduling_role.sql` | `scheduling_role`，既有账号默认常规用途 |
| `0038_recovery_attempts.sql` | `recovery_attempts`，默认 4，数据库约束 1～31 |

实际文件和冻结摘要见[迁移目录](../backend/migrations/README.md#冻结规则)。
业务备份沿用当前精简归档合同；下载已有 PostgreSQL custom 归档时保留 `.dump` 扩展，不将其误标为 `.sql.gz`。
恢复必须按真实格式选择 `pg_restore` 或 gzip 解压后 `psql`，步骤见[人工恢复数据库](../deploy/README.md#人工恢复数据库)

人工创建备份、请求删除和连接测试的审计分类沿用统一的变更审计模型；创建或状态迁移与审计事件在同一数据库事务内提交，失败时一起回滚。
保留策略触发的自动删除不伪造管理员审计上下文。更新、回滚和自重启操作共用主机级互斥状态，受理自重启后继续持有跨进程锁，避免更新产物处于替换中时启动新进程

归档命名入口为 [build_download_file_name](../backend/crates/gateway-admin/src/model/backup/mod.rs)，回归见[备份用例测试](../backend/crates/gateway-admin/tests/use_case/backup.rs)

## 发行与协作

### 发行身份和恢复

- 安装、Compose、管理端在线更新及 GHCR 使用 `lqy007700/codex-proxy-rs`，维护和常规发行在 `main`
- 版本可附加 `+lqy007700`，数字版本仍需递增，构建元数据不参与更新大小比较
- 当前平台清单仅发布 Linux amd64，不据此推断源码不支持其他平台
- Release 工作流绑定 tag 中的版本和源码，完整检查通过后发布；同 tag 失败恢复只复用已核验、身份匹配且未过期的原始构建产物
- 单平台 Buildx 镜像配置与多平台配置集合分别读取，读取镜像版本时沿用发行版本校验
- 发行不会自动部署运行实例；只触发工作流不能报告 Release 或镜像已经可用

操作合同见[版本与发布](development.md#版本与发布)，源码入口为 [release/publish](../release/publish)、
[release.yml](../.github/workflows/release.yml)、[publish-release.rb](../.github/scripts/publish-release.rb)和[发行测试](../release/tests/recovery.test.cjs)

### 协作规则

本仓库保留自己的 `AGENTS.md`、`CONTRIBUTING.md` 和 `.agents` 技能入口，按当前任务加载，规则差异单独评估。
审查默认只读，修复、提交推送、发版和部署分别按用户授权执行；未采用 Dependabot 对受保护工作流的修改豁免。
开发与审查保留问题依据、自审、实际界面证据、中文解释及 AI 实际模型披露要求

规则入口见[贡献与审查](../CONTRIBUTING.md)、[仓库协作说明](../AGENTS.md)和[规则保护工作流](../.github/workflows/rules.yml)

## 维护与验证

维护本清单时对照已合并上游基线的最终差异，而非把所有历史提交标题都当成现有功能。
当前行为若已撤回、被新实现替代或与上游一致，应从二开清单移除；上游新提交只有合并并验证后才能计入当前能力。
接口细节就地更新 API 文档，状态所有权更新架构，操作更新部署文档，本索引只保留必要摘要和入口

| 改动范围 | 回归重点 |
| --- | --- |
| 恢复与调度 | 默认和边界次数、普通池轮询、保底、无候选、原账号不可用、共享预算、原生续接、取消与截止时间 |
| 点数和角色 | 默认值、更新与导入保留、备份恢复、Store 映射及冻结快照 |
| WebSocket 状态 | 账号隔离、空闲清除、手动恢复和旧在途写入 |
| 协议兼容 | Chat JSON/SSE/工具/错误、输出预算边界、混合模型列表及原生元数据完整性 |
| 审计与界面 | Admin/Key 范围差异、分页与计数、时间关键词、错误与关联请求，以及页面实际截图和交互 |
| 发行与迁移 | 冻结摘要、升级和真实归档恢复、单平台产物、同 tag 恢复身份及 Latest 规则 |

验证命令见[修改与验收](architecture.md#12-修改与验收)。现有测试入口不代表本次文档整理重新运行了全部测试，
具体执行结果、跳过项与运行证据留在对应任务报告中

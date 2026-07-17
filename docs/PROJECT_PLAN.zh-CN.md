# JITForge 项目计划书

## 1. 项目信息

- 项目名称：JITForge
- 项目副标题：基于 Unix 工具语义的即时云端工具合成与受限执行系统
- 项目形态：Rust 单仓库，包含 CLI、Registry Server、Runner 及共享协议
- 当前阶段：可用 MVP / Milestone 4
- 计划周期：14 周

## 2. 项目摘要

JITForge 面向人、脚本和 Agent 提供统一的远程工具原语。用户通过自然语言和少量输入输出契约注册工具，服务端生成实现、测试和能力清单，在隔离环境中完成构建与分层验证，随后发布不可变版本。任何获得授权的客户端都可以使用同一个 CLI 调用该版本，并通过 stdin、stdout、stderr 和退出码接入现有 Unix 管道。

核心交互保持为：

```bash
jit register <name> "<description>"
jit call <name> [-- <tool-args>]
```

项目不把模型生成的代码直接视为可信工具。自然语言负责表达意图，契约负责确定行为边界，能力清单负责确定权限，制品摘要负责确定版本，Runner 负责限制执行后果，Registry 负责决定什么能够发布和调用。

## 3. 背景与问题

团队中的小工具通常存在以下成本：

1. 脚本散落在个人机器、代码片段和临时目录中，难以发现和复用。
2. 使用者需要同步源码、安装运行时和处理依赖版本。
3. Agent 平台往往要求为每个生态开发不同的插件或工具适配层。
4. 大量低频工具不值得建立独立仓库、发布流水线和长期服务。
5. 直接执行模型生成代码会引入远程代码执行、数据外传和供应链风险。

JITForge 研究并实现的问题是：如何以接近 Unix 命令的交互成本，将自然语言意图转换为可复用、可版本化、受到能力约束且能够远程调用的工具。

## 4. 项目目标

### 4.1 工程目标

1. 提供稳定的 `register` 与 `call` CLI 语义。
2. 建立工具、候选版本、制品、验证记录和调用记录的数据模型。
3. 将自然语言需求转换为结构化契约、实现、测试和能力提案。
4. 在隔离环境中完成构建、测试、修复和运行。
5. 以不可变摘要执行工具，并支持 stable 版本的原子切换和回滚。
6. 默认禁止网络、持久化文件、子进程和密钥访问。
7. 保证工具 stdout 不混入平台日志，使调用能够进入 Unix 管道。

### 4.2 研究目标

围绕以下问题进行量化评估：

1. 契约生成和执行反馈是否能提高隐藏测试通过率？
2. 独立验证能否降低错误工具被发布的比例？
3. 自动能力推断是否能够在满足功能的同时减少过度授权？
4. 隔离与远程调用带来的延迟和资源开销是否适合短时 Unix 工具？

### 4.3 成功标准

MVP 达到以下条件视为完成：

- 文本与 JSON 工具可以完成注册、合成、验证、发布和调用闭环。
- 每个可调用版本都对应不可变制品摘要和完整验证记录。
- 未验证版本不能进入 Runner 的正常调用路径。
- 默认无网络工具无法直接访问外部网络和云元数据地址。
- 超时、内存、进程数和输出大小限制能够被稳定执行。
- CLI 在成功调用时只把工具结果写入 stdout。
- 基准集中至少 60 个需求具有公开示例和独立隐藏测试。

## 5. 非目标

MVP 明确不包含：

- Agent 工作流编排和自动工具链规划；
- 图形化管理后台和公共工具市场；
- 任意编程语言与任意第三方依赖；
- 长时间任务和持久运行服务；
- 数据库直连、用户长期密钥和内网写操作；
- 跨工具共享可变文件系统；
- exactly-once 副作用调用；
- 实时双向流式协议；
- 将容器或 Rust 本身描述为完整安全证明。

## 6. 用户与使用场景

### 6.1 Shell 用户

```bash
printf 'Hello Cloud Native\n' | jit call slugify
jit call fetch-public-data -- --limit 10 | jq '.items[]'
```

用户关心标准流、退出码、固定版本和可复现性。

### 6.2 自动化脚本

```bash
jit call normalize@7 --file raw.json --content-type application/json > normalized.json
```

脚本应固定 revision 或 digest，避免 stable 指针变化影响结果。

### 6.3 Agent

Agent 使用短期服务令牌，以 JSON 模式注册和调用工具。所有错误必须具有机器可判断的 `code`，不能依赖解析自然语言日志。

## 7. 核心设计原则

1. **默认拒绝**：未声明能力不可用，未验证版本不可调用。
2. **不可变执行**：Runner 只执行已经解析出的 artifact digest。
3. **控制面与执行面分离**：Registry 不运行工具，Runner 不修改工具定义。
4. **标准流纯净**：stdout 是结果，stderr 是诊断，退出码是状态。
5. **契约优先**：先确定输入、输出、示例和性质，再生成实现。
6. **显式版本**：裸名称解析 stable，自动化脚本能够固定 revision 或 digest。
7. **有界 Agent 循环**：基于 `rig-core::AgentRun`，最多九个模型回合，并分别限制源码改写、测试复核和沙箱 probe，避免无限消耗模型和计算资源。
8. **数据最少化**：默认不持久化调用的 stdin、stdout 和密钥。

## 8. 系统架构

```text
                                  Control plane
                         ┌──────────────────────────┐
                         │       jit-server         │
┌─────────┐ register     │ Registry / Auth / Policy │
│ jit CLI ├─────────────►│ Jobs / Version state     │
└────┬────┘              └───────┬──────────┬───────┘
     │                            │          │
     │                            ▼          ▼
     │                     Synthesizer   Artifact Store
     │                            │          ▲
     │                            ▼          │
     │                       validation job │
     │                                       │
     │ call                   Execution plane│
     └──────────────────────────────► Runner ┘
                                           │
                                           ▼
                                      stdout/stderr
```

### 8.1 CLI

- 使用 Clap 解析命令和选项。
- 使用 Reqwest 与 Registry Server 通信。
- 默认把远端 stdout、stderr 和退出码还原为本地进程语义。
- 输入优先级为 `--input`、`--file`、非终端 stdin、空输入。
- `--` 后的参数原样作为工具 argv 传递。
- `--json` 返回协议 envelope，便于 Agent 使用。

### 8.2 Registry Server

- 使用 Axum/Tower 提供 HTTP API。
- 使用 PostgreSQL 保存工具、版本、任务、验证和调用元数据。
- 通过事务和乐观并发控制执行版本状态转换。
- 负责认证、RBAC、能力策略和 stable 指针切换。
- 通过数据库任务队列分发合成与验证任务。
- 不加载或运行用户工具代码。

### 8.3 Synthesizer

- 输入为不可变的意图、示例、约束和策略上限。
- 使用 OpenAI Chat Completions 原生 tool calls；每轮只允许一个工具调用，不保留旧 JSON action 回退。
- 首轮通过 `submit_contract` 提交契约；随后只允许一次完整 `write_source`，后续源码修改统一使用 `old_text → new_text` 精确片段替换。
- 用户示例是不可修改的硬约束；生成测试必须由独立 Test Verifier 判定为 `oracle_wrong` 才能纠正。
- Coder 与 Verifier 使用独立上下文和显式模型配置；两者允许由部署者选择同一模型名。
- Agent 状态机和领域 workspace 写入 PostgreSQL checkpoint；事件按顺序记录为 512 KiB 有界 trace，完成 7 天后归档压缩。
- 模型动作受独立的回合、源码版本、测试复核和 probe 预算约束，任务租约每 30 秒续期。
- 执行反馈作为不可信 observation 处理，只返回必要且有界的诊断；所有 probe 仍通过 Runner/gVisor。
- Synthesizer 无权直接将版本标记为 `ready`。

### 8.4 Runner

- 根据摘要拉取并验证制品。
- 使用 containerd 与 gVisor 创建一次性执行沙箱。
- 强制只读 rootfs、非 root 用户、cgroups、seccomp、超时和输出上限。
- 默认关闭网络；联网工具只能使用受策略控制的代理。
- 验证任务和用户调用进入不同 Runner 池。
- 执行结束后销毁沙箱和临时凭证。

## 9. 工具生命周期

```text
draft
  → contract_ready
  → synthesizing
  → building
  → validating
  → ready
  → deprecated/revoked
```

失败可以从 `synthesizing`、`building` 或 `validating` 进入 `rejected`。`revoked` 不允许恢复为 `ready`；需要修复时创建新 revision。

每次重复注册同名工具都创建新候选 revision。旧 stable 版本在新候选验证期间继续服务。发布通过一次数据库事务更新 stable 指针，回滚同样只是将指针切换到仍然有效的旧版本。

## 10. 工具契约与制品

第一版运行时固定为 `python-stdlib-v1`，禁止在线安装依赖。工具制品至少包含：

```text
manifest.json
implementation.py
contract.json
tests/
validation.json
sbom.json
signature.json
```

manifest 必须声明：

- 输入和输出媒体类型；
- 输入和输出最大字节数；
- 默认与最大超时；
- CPU、内存、PID 和临时磁盘限制；
- 网络、文件、子进程和密钥能力；
- 运行时名称和入口点；
- 是否无副作用、确定性和允许重试。

制品上传后计算摘要。版本记录引用摘要，后续不能原地替换制品内容。

## 11. 验证路线

验证按顺序执行：

1. Schema、语法和静态策略检查；
2. 构建与依赖锁定检查；
3. stdin、stdout、stderr 和退出码协议检查；
4. 用户示例测试；
5. 独立生成的边界测试与隐藏测试；
6. 幂等性、单调性或格式约束等性质测试；
7. Unicode、畸形 JSON、超长数据等模糊测试；
8. 文件、网络、进程和资源限制的对抗测试；
9. 有界 Agent 根据结构化 observation 选择源码改写、生成测试纠错或沙箱 probe；
10. 生成验证证明，由 Registry 决定发布状态。

模型自己生成并通过的测试不能单独证明正确性。用户示例、独立隐藏测试和安全策略是发布门槛。

## 12. API 初稿

```text
GET  /healthz
POST /v1/tools/{name}/registrations
GET  /v1/tools/{name}
POST /v1/tools/{name}/invocations
GET  /v1/jobs/{job_id}
GET  /v1/invocations/{invocation_id}
POST /v1/tools/{name}/versions/{revision}/promote
POST /v1/tools/{name}/versions/{revision}/revoke
```

MVP 调用采用有界 JSON envelope 和 Base64 字节流，CLI 负责解包。输入上限暂定 4 MiB，输出上限暂定 1 MiB，默认超时 5 秒，最大超时 30 秒。实时 HTTP/2 多路标准流留到后续版本。

## 13. 单仓库结构

```text
jitforge/
├── apps/
│   ├── jit-cli/
│   └── jit-server/
├── crates/
│   ├── jit-domain/
│   └── jit-protocol/
├── services/
│   └── synthesizer/
├── docs/
├── Cargo.toml
└── rust-toolchain.toml
```

后续按实际复用需求增加 `jit-storage`、`jit-policy`、`jit-artifact` 和 `jit-sandbox` crate，不提前建立空抽象。CLI 与 Server 始终位于同一 workspace、共享锁文件、协议类型和 CI。

## 14. 技术选型

| 范围 | 选择 |
|---|---|
| Workspace | Rust 2024 Edition / Cargo |
| CLI | Clap / Tokio / Reqwest / rustls |
| Server | Axum / Tower / Tokio |
| 数据模型 | Serde / thiserror |
| 数据库 | PostgreSQL / SQLx |
| 内部 RPC | Tonic gRPC |
| 可观测性 | tracing / OpenTelemetry |
| 制品 | 本地 SHA-256 Artifact Store；后续兼容 S3/MinIO/OCI |
| 执行 | containerd / gVisor |
| 合成服务 | Rust / rig-core，OpenAI-compatible Chat Completions |
| 生成运行时 | Python stdlib v1 |

所有 Rust HTTP 通信使用 rustls，避免运行镜像依赖系统 OpenSSL。领域 crate 使用可判断的 thiserror 错误；二进制边界可以使用 anyhow 增加上下文。

## 15. 数据模型

计划建立以下 PostgreSQL 表：

```text
users
organizations
namespaces
tools
tool_versions
artifacts
synthesis_jobs
validation_runs
test_cases
invocations
capability_policies
audit_events
```

关键约束：

- `(namespace_id, tool_name)` 唯一；
- `(tool_id, revision)` 唯一；
- artifact digest 全局唯一；
- stable version 必须属于同一个 tool 且状态为 ready；
- 状态转换通过带 expected revision 的事务完成；
- 注册请求使用 idempotency key 防止网络重试创建重复版本。

## 16. 安全计划

### 16.1 威胁范围

- 恶意或被提示注入的自然语言描述；
- 生成代码主动读取文件、环境变量和云元数据；
- DNS 重绑定、重定向和 SSRF；
- fork bomb、死循环、内存与输出耗尽；
- 构建依赖投毒和制品替换；
- 多租户之间的数据和缓存泄漏；
- Registry 版本劫持与未经授权的 promote；
- 调用输入通过日志和模型反馈泄漏。

### 16.2 基础控制

- 默认无网络、无密钥、无宿主挂载、无子进程；
- 构建网络只允许内部依赖镜像；
- Runner 验证摘要和签名；
- 网络能力通过 egress proxy 实现，不提供原始 socket；
- 长期密钥不进入工具环境变量；
- 默认不记录 stdin/stdout 原文；
- 每次注册、发布、撤销和调用写审计事件；
- 高风险工具必须人工审批，MVP 不支持写权限凭证。

## 17. 里程碑与进度

### Milestone 0：仓库与协议骨架，第 1 周

- Rust workspace；
- CLI 与 Server 可编译；
- 共享领域和协议类型；
- `healthz`；
- 内存版注册接口；
- 未验证工具调用被明确拒绝；
- 项目计划和 CI 基线。

### Milestone 1：持久化 Registry，第 2～3 周

- PostgreSQL migration；
- Tool 与 ToolVersion repository；
- 事务状态机；
- idempotency key；
- job 查询与版本 inspect；
- 基础认证和 namespace。

### Milestone 2：Runner，第 4～6 周

- containerd 接口；
- gVisor RuntimeClass；
- 只读文件系统和临时目录；
- stdin/stdout/stderr 采集；
- CPU、内存、PID、超时和输出限制；
- 摘要校验与执行结果证明。

### Milestone 3：Synthesizer，第 7～9 周

- 结构化意图与契约；
- Python stdlib 模板；
- 用户示例测试；
- 构建任务；
- 有界多轮 Agent、生成测试纠错和沙箱 probe；
- 制品上传。

### Milestone 4：发布闭环，第 10～11 周

- validation attestation；
- ready 与 stable 分离；
- promote、rollback、revoke；
- CLI 远程调用；
- 审计和基础指标。

### Milestone 5：安全与实验，第 12～14 周

- 能力策略和网络代理；
- 模糊测试与攻击用例；
- 60 个工具基准集；
- 对照实验；
- 性能、正确性、安全性与成本报告。

## 18. 测试与质量门槛

每次合并必须执行：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

后续增加：

- Registry repository 集成测试；
- API 契约测试；
- CLI 与真实 Server 的端到端测试；
- Runner 资源超限测试；
- 制品摘要和签名篡改测试；
- 网络逃逸与云元数据访问测试；
- 注册、发布和回滚的并发一致性测试。

## 19. 实验方案

准备 60 个工具需求：文本转换 15 个、JSON/CSV 15 个、数据校验 10 个、编码格式化 10 个、简单计算 10 个。每个需求包含 2～5 个公开示例、10～30 个隐藏测试、性质约束和允许能力。

对照组：

1. 模型单次生成，不执行测试；
2. 模型生成实现和自己的测试；
3. 增加固定次数的执行反馈修复；
4. 增加有界 Agent 动作、沙箱 probe 和错误 oracle 纠正；
5. 使用契约、独立隐藏测试、能力策略和执行反馈的完整系统。

评价指标：

- 构建成功率；
- 公开和隐藏测试通过率；
- 错误接受率；
- 修复成功率与平均修复轮数；
- 权限过度申请率；
- 冷、热调用延迟；
- 生成与执行成本；
- 未授权能力阻断率。

错误接受率是首要正确性指标，因为把错误实现发布为可复用工具比拒绝一次生成更危险。

## 20. 主要风险与应对

| 风险 | 应对 |
|---|---|
| 描述无法确定正确性 | 契约优先、用户示例、独立隐藏测试、返回 needs_specification |
| 项目范围膨胀 | MVP 只支持短时、无状态、Python stdlib 工具 |
| 沙箱工程量过大 | 先完成 gVisor 单机 Runner，不在 MVP 自研隔离内核 |
| 模型自测产生错误信心 | 模型测试不单独作为发布依据 |
| 依赖供应链风险 | MVP 禁止第三方依赖，后续只使用内部镜像和哈希锁定 |
| 调用延迟过高 | 按摘要缓存制品，分离冷启动与执行时间指标 |
| Registry 成为单点 | MVP 接受单实例，数据持久化后再增加水平扩展和备份 |
| Agent 滥用注册和调用 | namespace 配额、并发限制、短期令牌和审计 |

## 21. 交付物

1. 单仓库源代码与可复现构建配置；
2. `jit` CLI；
3. Registry Server；
4. Synthesizer Worker；
5. 受限 Runner；
6. Python stdlib v1 工具运行时；
7. OpenAPI 和内部任务协议；
8. 威胁模型与安全测试报告；
9. 60 个工具基准集；
10. 正确性、性能、能力最小化与成本实验报告；
11. 本地 Compose 部署和演示脚本。

## 22. 当前 MVP 验收

当前实现已经满足：

- Registry 使用 PostgreSQL，Server 重启不丢失工具、版本和任务；
- `jit register` 支持异步任务、默认等待、幂等请求和失败恢复；
- Rust Worker 可通过 OpenAI Chat Completions 兼容接口生成契约与实现；
- 用户示例硬约束、独立 Test Verifier、原生工具调用、沙箱 probe 和失败拒绝已经形成闭环；
- 制品使用 v2 SHA-256 语义摘要并自动发布 stable，执行镜像按源码摘要复用且兼容 v1 制品；
- Agent checkpoint、append-only trace、30 秒租约续期和中间候选清理已经落地；
- `jit call` 通过私有 gRPC 调用 Worker，并保持 Unix 标准流语义；
- 模型工具只通过 gVisor/runsc 执行，不自动降级到 runc；
- 无网络、只读文件系统、非 root、CPU、内存、进程、超时和输出限制已实现；
- 超时、过量输出和错误候选版本均被拒绝，旧 stable 保持可调用；
- CLI、Server、Worker、共享协议、Registry 和 Artifact crate 位于同一 workspace；
- Compose 部署包含 PostgreSQL、固定 Python runtime、Worker 与 Server。

尚未纳入当前 MVP 的内容仍包括多租户、联网能力、用户密钥、第三方依赖、实时流式协议、制品签名和公共工具市场。

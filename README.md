# JITForge

JITForge 将自然语言意图、真实输入样本和可选的严格示例合成为经过验证、可版本化、可审计调用的 Unix 能力。

模型只参与合成和修复。能力发布后，普通调用直接执行选定的 Artifact，不再经过模型。

```text
Intent + Input Sample + Strict Example
  → Contract Review
  → Source + Tests
  → Sandbox Validation
  → Immutable Artifact
  → name@revision / stable
  → CLI、Web、Shell、Agent 或自动化流水线调用
```

当前版本为 `0.1.0` alpha，适合自行部署和验证，还不是面向不受信任多租户的公共服务。

## 适用范围

JITForge 面向短时、无状态、输入输出明确的 Unix filter。比较合适的任务包括：

- 运维命令输出解析和报告生成；
- JSON、CSV、文本的清洗、校验与格式转换；
- Pull Request / CI 中反复出现的小型胶水逻辑；
- Agent 和 Shell 需要共同调用、固定版本或留下审计记录的能力；
- 少量经过审批的公共 HTTPS 数据查询。

生成运行时目前固定为 Python 3 标准库单文件。长期服务、浏览器自动化、任意第三方依赖、持久化状态，以及需要用户私密凭据的动态能力不在当前范围内。

## 核心语义

- 注册时通过管道传入的 stdin 是 **Input Sample**，只用于说明真实输入形状，不附带期望输出。
- `--example 'INPUT => OUTPUT'` 是用户提供的 **Strict Example**，属于验证断言。发现示例有误时，任务会暂停并请求明确批准；原值仍保留审计记录。
- Contract 在源码生成前确定输入、输出、错误语义和测试计划，并经过独立 Review。
- 只有 Contract Review、用户测试、生成测试和 Sandbox 实际执行全部通过，Revision 才会进入 `ready`。
- `name@revision` 指向不可变版本；裸名称解析当前 `stable` Revision。
- 调用保持 Unix 语义：业务数据写 stdout，诊断写 stderr，失败使用非零 exit code。

模型生成的测试和 Verifier 都不能构成形式化正确性证明。它们是发布门槛的一部分，不会取代用户断言和真实 Sandbox 结果。

## 架构

```text
                         ┌──────── PostgreSQL Registry
                         │
jit CLI ──HTTP──> jit-server ──private gRPC──> jit-worker
                         │                         │
                         │                         ├── synthesis Agent / Verifier
                         │                         ├── Artifact Store
                         │                         └── Docker + runsc ──> capability
                         │
                         └── embedded Web Console

Internet ──TLS proxy / tunnel──> Nginx ──> Console、API 与健康端点
```

`jit-server` 负责 HTTP API、认证、Web Session、Registry 和任务状态，不持有模型密钥，也不接触 Docker Socket。`jit-worker` 是唯一拥有模型配置和 Docker 权限的服务，默认不发布公网端口。

PostgreSQL 保存 Contract、Revision、Artifact 摘要、验证证据、任务 Checkpoint、Trace、Approval 和调用元数据。普通调用的 stdin/stdout 正文默认不入库。

## 快速开始

### 环境要求

- Linux 与 Docker Engine / Docker Compose；
- 已注册为 Docker Runtime 的 [gVisor `runsc`](https://gvisor.dev/docs/user_guide/install/)；
- Rust 1.97 和 `protoc`；
- 真实合成模式还需要支持原生 tool calls 的 OpenAI Chat Completions 兼容接口。

先确认 `runsc` 可以被 Docker 使用：

```bash
runsc --version
docker info --format '{{json .Runtimes}}'
docker run --rm --runtime=runsc python:3.13-alpine python3 -c 'print("gvisor-ok")'
```

### 使用 Fixture 跑通完整闭环

Fixture Synthesizer 不需要模型 API Key，只接受带 `[fixture:...]` 标记的测试注册。它适合验证 PostgreSQL、Worker、Server、Runner、Artifact 和 CLI 是否接通，不代表真实合成效果。

复制本地 Compose 配置：

```bash
cp .env.example .env
```

编辑 `.env`，至少替换两个 Token，并将合成模式设为：

```dotenv
JITFORGE_TOKEN=choose-a-local-client-token
JITFORGE_WORKER_TOKEN=choose-a-different-worker-token
JITFORGE_SYNTHESIZER_MODE=fixture
```

`.env` 已被 Git 忽略，不要提交它。然后编译控制面和 CLI，启动完整 Compose Profile：

```bash
cargo build --locked --release -p jit-cli -p jit-server -p jit-worker
docker compose --profile containerized up --build -d
```

检查服务：

```bash
curl -f http://127.0.0.1:8090/healthz
curl -f http://127.0.0.1:8090/readyz
docker compose ps
```

让 CLI 连接到本地 Gateway，Token 与 `.env` 保持一致：

```bash
export JITFORGE_SERVER=http://127.0.0.1:8090
export JITFORGE_TOKEN=choose-a-local-client-token
```

注册并调用 Fixture 能力：

```bash
printf 'Hello Cloud Native\n' |
  target/release/jit register slugify \
    '[fixture:slugify] Convert UTF-8 text to a lowercase URL slug' \
    --example 'Hello Cloud Native => hello-cloud-native'

printf 'Hello Cloud Native\n' | target/release/jit call slugify
```

预期 stdout：

```text
hello-cloud-native
```

Gateway 根路径会以 `307 Temporary Redirect` 跳转到 Web Console：

```text
http://127.0.0.1:8090/
  → http://127.0.0.1:8090/ui/
```

### 切换到真实模型

将 `.env` 中的合成模式改回 `openai`，并配置模型接口：

```dotenv
JITFORGE_SYNTHESIZER_MODE=openai
JITFORGE_LLM_BASE_URL=https://provider.example/v1
JITFORGE_LLM_API_KEY=replace-me
JITFORGE_LLM_MODEL=replace-me
JITFORGE_LLM_VERIFIER_MODEL=replace-me
JITFORGE_LLM_THINKING=auto
```

Coder 和 Verifier 使用独立上下文。两个配置项可以选择同一个模型，但 `JITFORGE_LLM_VERIFIER_MODEL` 仍需显式填写。模型接口必须支持 `tools`、`tool_choice`、assistant `tool_calls` 和后续 tool result 消息。

更新环境变量后重建 Worker 容器：

```bash
docker compose --profile containerized up -d --force-recreate worker server nginx
```

## CLI

```text
jit register NAME INTENT [--example 'INPUT => OUTPUT'] [--no-wait]
jit status JOB_ID
jit answer JOB_ID TEXT
jit answer JOB_ID --approve
jit cancel JOB_ID
jit list [QUERY]
jit search QUERY
jit inspect NAME[@REVISION]
jit call NAME[@REVISION] [--input TEXT | --file PATH] [-- TOOL_ARGS...]
jit revoke NAME@REVISION --reason TEXT
```

`register` 默认等待任务进入 `ready`、`rejected` 或等待用户输入的状态。使用 `--no-wait --json` 时会立即返回 Job ID，之后用 `status` 查询。`answer` 用于澄清问题、批准示例修正或 HTTP Capability；`cancel` 可以终止排队中、运行中或已暂停的任务。

`call` 会转发远端 stdout、stderr 和退出码。自动化流水线建议固定 `name@revision`，交互使用可以选择裸名称的 `stable` 指针。

配置读取顺序为 CLI 参数、环境变量、配置文件。默认配置文件是 `~/.config/jitforge/config.toml`，也支持 `$XDG_CONFIG_HOME/jitforge/config.toml` 和 `JITFORGE_CONFIG`。配置文件含 Token 或 API Key 时，Unix 权限必须为 `0600`。

## Web 与公开路由

完整 Compose Profile 由 Nginx 提供统一入口：

```text
/                              307 跳转到 /ui/
/ui/                           Web Console
/v1/                           HTTP API
/healthz                       进程健康状态
/readyz                        PostgreSQL 与 Worker 就绪状态
```

Console 支持登录、能力列表与详情、注册、任务处理、调用、撤销、HTTP Capability 和系统状态。浏览器使用 HttpOnly、SameSite=Strict Session Cookie，写请求还需匹配 `X-JitForge-Csrf`。

## 合成与发布

合成 Agent 使用受控的原生 tool calls，每个模型轮次只接受一个动作。当前单任务硬上限包括 24 个模型轮次、4 个源码 Revision、3 次生成测试纠错和 3 次 Sandbox Probe；Web 搜索与 HTTP Probe 也有独立预算。

典型流程如下：

1. 根据 Intent、Input Sample 和 Strict Example 生成 Contract 与测试计划；
2. 独立 Reviewer 检查需求漂移、错误 Oracle 和样本硬编码；
3. Contract 通过后才允许生成或编辑 Python 源码；
4. Runner 在 runsc 中执行输入样本、用户测试、生成测试和 Probe；
5. 任务可以暂停等待澄清、示例修正或 HTTP Capability 审批，并从 Checkpoint 恢复；
6. 全部门槛通过后写入内容寻址 Artifact，发布不可变 Revision 并更新 stable 指针。

## HTTP Capability

能力默认无网络。确实需要公共实时数据时，合成 Agent 可以申请精确的 HTTPS GET 权限，范围由 Host、443 端口、Path Prefix 和允许的 Query Keys 共同确定。

用户批准后，Grant 会随 Artifact 发布。运行时只允许生成代码通过 `jitforge_http.get` 访问匹配范围；IP 字面量、私网解析、凭据 URL、任意端口和原始 Socket 都会被拒绝。Approval 被撤销后，引用它的 Artifact 会停止联网调用。

启用实时 HTTP 还需要部署者显式设置：

```dotenv
JITFORGE_HTTP_MODE=direct
```

不需要联网能力时保持默认的 `disabled`。

## 安全边界

生成能力通过 Docker + runsc 执行，当前 Runner 约束包括：

- UID/GID 65532，read-only rootfs；
- `cap-drop=ALL` 与 `no-new-privileges`；
- `/tmp` 为 32 MiB、`noexec` 的 tmpfs；
- 128 MiB 内存、0.5 CPU、16 个进程和 64 个文件描述符；
- stdout/stderr 各 1 MiB 上限和硬超时；
- 默认 `network=none`。

这些限制只约束生成能力的 Runner，不代表 Server、Worker 和 Nginx 容器都具有同样的只读配置。`runsc` 是隔离层，不是完整安全证明；Worker 挂载 Docker Socket，仍然是高权限边界。

当前认证模型是单个共享 Bearer Token。Compose 中 PostgreSQL 的默认账号密码只适合本地开发，部署时必须覆盖。Server 只提供 HTTP，公网入口需要独立的 TLS 终止层；当前 `0.1.0` 的 Web Session Cookie 尚未设置 `Secure`，修复前不要把 Console 直接暴露到不受信任网络。

## 仓库结构

```text
apps/jit-cli              Rust CLI
apps/jit-server           HTTP API、Session/CSRF、Registry 控制面、内嵌 Console
apps/jit-worker           合成 Agent、Verifier、Runner 与发布流程
crates/jit-*              Artifact、Config、Domain、Protocol、Storage
web/console               编译进 jit-server 的 HTML/CSS/JS
runtimes/python-stdlib-v2 生成能力的固定 Python Runtime
deployments               Compose、Nginx 与 SearXNG 配置
migrations                PostgreSQL migrations
```

## 开发检查

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
JITFORGE_TOKEN=test JITFORGE_WORKER_TOKEN=worker-test \
  docker compose --profile containerized config --quiet
```

日常开发可以只用 Compose 启动 PostgreSQL、SearXNG 和 Runtime Image，再在宿主机分别运行 Worker 与 Server：

```bash
docker compose up --build -d
JITFORGE_WORKER_TOKEN=worker-test \
  JITFORGE_SYNTHESIZER_MODE=fixture \
  cargo run -p jit-worker
```

另一个终端：

```bash
JITFORGE_TOKEN=client-test \
  JITFORGE_WORKER_TOKEN=worker-test \
  cargo run -p jit-server
```

## 项目状态

JITForge 正在从完整原型整理为可公开发布的开源项目。API、Artifact Format 和部署方式在 `0.1.x` 阶段仍可能变化。

仓库目前尚未加入开源许可证。正式公开或接受外部贡献前，需要先确定许可证并添加 `LICENSE`。

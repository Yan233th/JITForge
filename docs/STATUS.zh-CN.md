# JITForge 开发状态

更新时间：2026-07-15

## 当前结论

JITForge 已达到可运行的工程 MVP：CLI、Server、PostgreSQL Registry、合成 Worker、Artifact Store 和 gVisor Runner 已形成完整闭环。当前实现不是生产就绪版本，生产化前仍需使用目标模型供应商完成真实合成回归，并补充基准集和运维方案。

## 已完成

- `jit register`、`status`、`inspect` 和 `call`；
- PostgreSQL 工具、版本、任务、制品和调用记录；
- OpenAI Chat Completions 兼容接口与原生 tool calls；
- 基于 `rig-core` 可序列化状态机的有界多轮合成 Agent；
- 契约提交、初始源码、精确片段修改、沙箱 probe 和主动 abort；
- 独立 Test Verifier 与生成测试 oracle 纠错；
- checkpoint、任务租约续期、append-only trace 和候选清理；
- SHA-256 不可变制品与按源码摘要复用执行镜像；
- gVisor、无网络、只读根文件系统、非 root、CPU、内存、进程、文件描述符、超时和输出限制；
- 发布状态、stable 指针、制品记录和终态 trace 的事务一致性；
- 宿主机 Rust 构建与只打包现有二进制的轻量控制面镜像。

## 已验证

- `cargo fmt --all -- --check`；
- `cargo clippy --workspace --all-targets -- -D warnings`；
- `cargo test --workspace`，共 31 个测试；
- fixture 模式的注册、验证、发布和 CLI 调用端到端流程；
- gVisor 网络阻断、超时、输出限制和普通执行；
- 模型纯文本响应、多个工具调用、未知工具、坏参数、非法契约和非法 verifier verdict 的恢复；
- pending model call 与 pending tool call 的 checkpoint 重放；
- 发布终态事件与正式 artifact digest 一致。

## 下一阶段

1. 配置目标模型供应商，选择稳定支持原生工具调用的 coder/verifier 模型，执行真实注册回归；
2. 建立覆盖文本、JSON、CSV、校验、格式化和简单计算的 benchmark；
3. 增加 Registry/API 并发集成测试、攻击用例和性能报告；
4. 补充生产部署所需的 TLS、备份、密钥轮换和监控方案。

多租户、第三方依赖、联网工具、持久化工具、公共市场和通用 Agent 平台仍不属于当前 MVP。

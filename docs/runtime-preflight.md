---
title: HomeBase 启动自检与故障恢复
updated: 2026-11-21
status: current
---

# 启动自检与故障恢复

启动自检逐项返回 SOPS、密钥文件、age 身份文件、reveal helper 和可选 agent wrapper 的状态。检查文件是否存在不代表已经验证解密或外部服务；自检通过也不代表所有业务功能已验收。

密钥助手用于向已授权的本机代理提供凭据。分发状态、启动自检和代理配置共用一个查找规则：先用存在的显式配置，再查主程序旁 `distribution/bin/homebase-reveal.exe`，最后查开发版与主程序同目录的 helper。三个位置均不可用时，启动检查才报缺失。找到可用的工具后，运行时把实际解析的路径传给子进程。检查不会运行解密。

`npm run tauri:dev` 会先构建开发版密钥助手。源码运行时在显式配置、随包目录和兼容路径之外，也可从绝对 PATH 目录定位 SOPS 和 age-keygen。首次启动只为新密钥库初始化所需文件。若为新库指定了 age 身份文件，找不到该文件时会报错，便于核对路径。只有已配置的 Gemini CLI 或 OpenClaw 密钥引用或端点需要启动包装脚本时，缺少脚本才会提示。

SOPS 探针用于确认工具可执行并返回有效版本，不读取密钥正文，也不验证所有真实凭据。探测使用 `--disable-version-check --version` 和子进程变量 `SOPS_DISABLE_VERSION_CHECK=true`，关闭更新检查（[官方版本函数说明](https://pkg.go.dev/github.com/getsops/sops/v3/version#PrintVersion)）。子进程清空继承环境，只保留必要系统项，用户目录和工作目录指向临时目录；不继承 age 或云端凭据配置。

首次子进程等待上限 5 秒；只有超时时才清理进程并以 15 秒上限再探测一次。成功必须返回有效的 `sops x.y.z`，不能把退出码为零但没有版本号当成通过。两次均超时仍报告 critical，附带次数、耗时和 `retryable` 标记；不存在、不可启动、非零退出和无效输出分别报告，不自动伪装为健康。Windows 每次超时后的进程树清理最多额外等待 3 秒；操作系统创建进程或文件 I/O 的耗时不属于子进程等待阶段的硬上限。

首次探测期间显示检查中。SOPS 临时超时或命令通信失败时，横幅在 30 秒后自动复检，最多两次；成功后清除故障提示，仍失败则保留错误及人工复检。永久错误不自动轮询。手动“重新检查”会重置自动复检额度，执行期间禁止重复点击。修改系统环境变量后仍需重启应用，因为已运行进程不会自动继承新环境。

失败摘要默认收起诊断细节。用「查看详情」展开具体失败项，用「重新检查」立即复检。

验收入口：

- `npm run lint`：检查前端源文件；Windows 文件名过滤的控制字符正则在对应位置明确豁免。
- `npm run build`：TypeScript 检查与前端生产构建。
- `npm run test:asr`：模型推荐与 ASR 可迁移性回归。
- `npm run test:preflight`：检查 Tauri 前端/原生端锁定版本的小版本一致性，再隔离模拟 IPC，验证加载、通过、检查失败、通信失败、复检及恢复。
- `npm run test:preflight:native`：独立 harness 编译生产 SOPS 探针，验证隔离环境、慢启动恢复、持续卡死、无效输出及项目自带 SOPS 的离线版本返回，不使用真实密钥。
- 原生密钥环境测试从 `bundle:prepare` 的输出 `src-tauri/resources/distribution/tools` 取工具。密钥助手定位测试用临时目录覆盖失效的显式配置、随包回退和开发版路径。
- `npm run test:preflight:ui`：Edge 合成 IPC 和可控时钟验证自动恢复、两次上限、永久故障、手动重试与通信错误，不连接真实工具或密钥。
- `cargo check --all-targets`：检查 Rust 主程序与 helpers。
- `cargo test --lib adapters::process_runner::tests:: -- --test-threads=1`：子进程输出、错误、环境隔离和超时清理。
- `cargo test --lib commands::system::tests:: -- --test-threads=1`：只读系统检查及自检返回契约。
- `cargo test --lib commands::distribution::tests:: -- --test-threads=1`：临时目录中的路径解析与资源同步。
- `cargo test --lib commands::agent_versions::tests:: -- --test-threads=1`：版本解析及 Windows npm 命令入口。

Agent 版本检测在 Windows 通过 `cmd.exe /D /C` 解析 npm/CLI 的命令 shim，并复用带超时清理的适配层；调用对象来自代码中的固定清单，不接受用户提供的 shell 片段。

依赖维护约束：图标库固定为 `5.22.0`，其 UI peer 固定为 `5.20.3`，避免引入要求 Node 22.22 的较新 UI 版本。`package.json` 中对 minimatch、KaTeX 和 selector parser 的限定覆盖用于修复审计告警；调整这些覆盖后需重新执行 lint、构建、KaTeX 渲染检查与依赖审计。minimatch 的覆盖跨主版本，KaTeX 的覆盖从 0.16 升到 0.18，不能仅凭 audit 为零认定所有上游使用场景兼容。

Tauri API 和插件的前端依赖使用 `~` 范围，与 `Cargo.lock` 中对应 crate 保持相同主/小版本。只升级 JS 端可能造成启动时版本不匹配；需要升级小版本时同步评估 Rust 端，不以绕过启动检查代替版本对齐。

不要直接以全量 Rust 测试替代隔离回归：现有 SOPS 测试在环境满足时会访问本机真实密钥文件，其他模块也可能依赖实际工具和用户目录。真实凭据、密钥写入、模型下载、付费生成、外部 Agent 执行需要按具体场景另行验收。

本次证据及未覆盖项见 2026-11-08 验收记录。

2026-11-21 日用 v101 的启动设置仍引用已经清理的开发构建目录。核对发布清单后，把密钥助手、Python 和 FFmpeg 三条覆盖路径改为同版随包文件，保留原启动配置副本并重新打开日用程序。签名 CLI 复验五项启动检查全部通过，运行环境无组件告警。还以隔离环境实际启动过三个工具，密钥助手无参数返回使用说明。三份状态、九份凭据密文、快捷方式与原操作账本前缀的摘要一致。源码的原生密钥与 SOPS 测试、41 项接口测试、54 项浏览器测试和 Tauri 库检查通过。 日用复验 · 工具启动回执 · 原生检查

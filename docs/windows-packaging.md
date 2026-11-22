---
doc_id: homebase-windows-packaging
title: HomeBase Windows Packaging
role: Windows 安装包、便携包、首次运行、迁移、升级与发布验收的权威流程
canonical_path: C:\HomeBase\source\docs\windows-packaging.md
updated: 2026-07-20
authority: reference
status: current
volatility: periodic
sensitivity: agent-safe
review_after: 2026-08-20
related:
  - doc_id: homebase-project
    path: C:\HomeBase\source\README.md
    rel: parent
  - doc_id: homebase-asr-production-recommendation-portability
    path: C:\HomeBase\source\docs\asr-production-recommendation-portability.md
    rel: product-boundary
tags: [homebase, windows, packaging, installer, portable, migration]
---

# HomeBase Windows Packaging

本文是人和 Agent 共用的 Windows x64 发布手册。`package.json`、打包脚本、
`tauri.conf.json`、实际发布清单和本文件冲突时，以可执行配置与本轮验证结果为准。

## One-command release

```powershell
cd C:\HomeBase\source
npm run package:windows
```

打包先准备并校验运行时，再构建前端，依次执行 ASR 回归、Rust library 测试、原生程序与 helper 构建。
NSIS 使用 `src-tauri/tauri.distribution.conf.json` 复制分发资源，之后组装便携版并运行 Package Doctor，最后生成 ZIP 和 SHA-256 清单。
正式交付不得使用 `-SkipTests`；该开关只供已完成同一提交门禁后的本地调试。

默认输出：

```text
.\release\<version>\
  HomeBase_<version>_x64-setup.exe
  HomeBase-<version>-windows-x64-portable.zip
  HomeBase-<version>-windows-x64-portable\
  PACKAGE-DOCTOR.json
  release-manifest.json
  SHA256SUMS.txt
  README-安装与迁移.md
  THIRD-PARTY-NOTICES.md
```

## Package boundary

| Included | Excluded by design |
|---|---|
| HomeBase 主程序和三个 helper | API Key、age 私钥和用户 Secrets |
| CPython 3.13.14 x64 embeddable runtime | ASR/OCR/图片/视频模型权重 |
| faster-whisper 1.2.1 CPU 运行依赖 | 原始录音、逐字稿、纪要和历史任务 |
| FFmpeg 8.1.1、SOPS 3.12.2、age 1.3.1 | 用户缓存、安装清单和 provider 健康状态 |
| ASR runner、model manager、preset、catalog、local-ASR MCP 快照 | CUDA、ROCm、DirectML 和厂商驱动 |
| 许可证、版本、组件哈希和 Package Doctor | 外部 Agent CLI 与它们的账号状态 |

模型不随普通应用包发布，避免包体失控，也避免把一台机器的硬件选择强加给另一台机器。
模型中心必须在目标机重新探测 CPU、RAM、GPU、磁盘、运行时与安装清单后再给出选择。

## Runtime preparation

入口：`scripts/prepare-windows-bundle.ps1`。

1. 下载 CPython 官方 embeddable ZIP，并验证固定 SHA-256。
2. 以 `packaging/windows/runtime-requirements.txt` 安装固定版本、仅 binary wheel 的 Python 依赖。
3. 复制 AIHub 中实际需要的 ASR、模型管理、catalog、preset 和 MCP 文件，不复制整个工作区。
4. 对 FFmpeg、SOPS、age-keygen 验证固定 SHA-256 后复制。
5. 收集第三方许可证，执行 Python import 与 runner CLI smoke probe。
6. 生成 `src-tauri/resources/distribution/bundle-manifest.json`。该 staging 目录是生成物，不提交 Git。

下载缓存位于项目的 `.tmp/bundle-cache`；重复构建会复用已经通过哈希验证的文件。
升级任一组件时必须同时更新版本、哈希、许可证来源、Doctor 检查和本文边界。

## First run and path precedence

发布版优先使用显式 `HOMEBASE_*` 环境绑定。未显式绑定时：

| Data | Existing machine | New-machine fallback |
|---|---|---|
| AIHub callable snapshot | 包内快照同步到用户数据目录 | `%LOCALAPPDATA%\io.github.gdnbit.homebase\distribution\AIHub` |
| Model weights | `C:\HomeBase\Models`（存在时） | `%LOCALAPPDATA%\io.github.gdnbit.homebase\distribution\Models` |
| Optional runtimes | `C:\HomeBase\Runtimes`（存在时） | `%LOCALAPPDATA%\io.github.gdnbit.homebase\distribution\Runtimes` |
| Download cache | `C:\HomeBase\Cache\models`（存在时） | `%LOCALAPPDATA%\io.github.gdnbit.homebase\distribution\Cache` |
| ASR output | `C:\HomeBase\Wiki\raw\audio`（存在时） | `%USERPROFILE%\Documents\HomeBase\Wiki\raw\audio` |
| Secrets | `C:\HomeBase\Secrets\secrets.yaml`（存在时） | `%LOCALAPPDATA%\io.github.gdnbit.homebase\distribution\Secrets\secrets.yaml` |

Python、FFmpeg、SOPS、age-keygen 和 HomeBase helper 优先使用包内版本，避免依赖目标机 PATH。
首次运行会创建缺失目录、初始化 SOPS/age 加密库、同步 AIHub 快照并写入
`distribution-state.json`。该状态文件记录实际绑定，不包含 API Key 明文。

## Install, portable, migration and upgrade

- 安装版：当前用户 NSIS，简体中文/英文界面，WebView2 bootstrapper，开始菜单入口。
- 便携版：必须完整解压后运行 `Start-HomeBase.cmd`；不能只复制 `homebase.exe`。
- 迁移：导出 ASR portable profile，目标机导入后重新绑定目录、注册 Key、校验/下载模型并运行预检。
- 升级：程序和包内资源可替换；用户数据、模型、Secrets 和 Wiki 输出位于程序目录之外，不应随卸载或覆盖升级删除。
- 外部 Agent：随包提供 local-ASR MCP server 快照，但 Agent 分配、权限和启动配置必须在目标机重新 reconcile，不能复制源机的活动状态。

## Release gates

发布完成至少满足：

1. `npm run test:asr`、`npx tsc --noEmit`、`cargo test --lib` 全部通过。
2. 四个 release binary 和 NSIS installer 成功生成。
3. `Test-HomeBasePackage.ps1` 返回 `READY`，`PACKAGE-DOCTOR.json` 的 `failed_count` 为 0。
4. 从最终便携目录启动 `homebase.exe`，进程路径属于发布目录，`distribution-state.json` 为 `packaged: true`。
5. 使用包内 SOPS 与 age-keygen 完成一次隔离加密/解密往返。
6. 生成 `release-manifest.json` 和 `SHA256SUMS.txt`，并确认 `secrets_included: false`、`model_weights_included: false`。

`0.1.0` 在 2026-07-20 通过以上门禁：Package Doctor 14/14、便携版独立启动和 Secrets
加解密往返均通过。安装器尚未商业代码签名，`release-manifest.json` 明确记录 `signed: false`。

## Remaining release risks

- Windows SmartScreen 可能拦截未签名安装器；面向外部用户发布前应接入代码签名证书。
- 当前实际启动验收在本机完成；大规模分发前还需在干净 Windows 10/11 VM 上验证安装、卸载、覆盖升级和无 WebView2 情况。
- provider 额度、限流、地区网络和上游下架是运行时事实，不由静态包保证。
- Windows ARM64、macOS、Linux/WSL、ROCm、DirectML 和网络共享模型盘不在当前自动批准范围。

## Adding another bundled capability

以后增加 OCR、文档解析或其他本地能力时，Agent 必须按以下顺序处理：

1. 在独立 requirements/catalog 中固定版本、平台和硬件条件。
2. 固定官方下载地址与 SHA-256，缓存只复用校验通过的文件。
3. 收集许可证并更新 `THIRD-PARTY-NOTICES.md`。
4. 只复制运行所需文件到 `distribution`，权重继续由模型中心按机器下载。
5. 在 Rust distribution resolver 中提供包内路径和用户目录回退，不写死构建机盘符。
6. 扩展 Package Doctor 的存在性、import/CLI 和最小运行探针。
7. 增加边界回归、首次运行、迁移和离线失败测试。
8. 执行完整 `npm run package:windows`，实际启动最终产物，再更新发布清单与文档。

# 已有服务凭据引用

HomeBase 可读取 `C:\HomeBase\ServiceCredentials\registry.json` 中经过固定服务/路径校验的引用，复用原位置的 Windows 当前用户 DPAPI 文件。不会将这些文件复制到 SOPS、仓库或数据库，也不会保存明文总表。

## 接口与边界

```powershell
node scripts/homebase.mjs describe service_credentials_status
node scripts/homebase.mjs call service_credentials_status --allow-private
```

状态查询只在本机后端内存校验可解密性，返回服务 ID、引用 ID、linked/not_configured/blocked、原因、地域与后端路由。没有密钥正文、掩码、长度或密钥导出接口。remote_state 固定为 not_checked：本机可解密不代表远端认证、余额、模型权限、凭据未撤销或未过期。查询不发送服务商 API 请求。

`adapters::service_credentials::resolve_api_key(service_id)` 是后端 Rust 接口，不是 Tauri/CLI 命令。只允许清单中已知服务，不接受任意文件路径。现有 Studio 的 OpenRouter 获取路径已使用该接口；其他已登记服务可由经过授权的后端业务复用，本轮不新增这些服务的生成或发布工作流。

## 来源优先级与生命周期

1. 已有对应 SOPS 字段继续优先，经过现有类型、到期、撤销及事务状态检查。
2. 只有对应 SOPS 字段不存在时，才使用该服务的 DPAPI 引用。
3. SOPS 不可读、不可解密、为空或被策略阻断时明确失败，不换用外部凭据绕过策略。
4. 外部凭据仍由原配置工具负责保存、轮换和撤销。本机每次使用重新读取原文件，不缓存密钥，不修改原文件。外部文件不含 HomeBase 生命周期元数据，不伪造到期日期或远端健康状态。

MiniMax 中国区要求原 provider-settings.json 中 MINIMAX_REGION=cn，否则阻断；百炼配置目标为北京。其他服务未在本机记录的地域不凭空推断。ElevenLabs 与豆包语音分别读取自己的独立文件，绝不借用方舟或其他服务的 Key。

## 蚁小二

保留 `C:\HomeBase\Integrations\publishing\yxer.cmd` → `C:\HomeBase\Tools\Yixiaoer\run-cli.ps1` 原封装。后者使用当前 Windows 用户专属 ACL 的命名管道，通过 YIXIAOER_CONFIG 交付内存配置。桥接不会将其改为环境变量 Key、临时明文配置或普通数据库。

封装脚本与启动器按本轮审查的 SHA-256 固定校验，并核对本地 CLI 包版本及原生程序存在性。脚本/版本变更、程序缺失或待恢复时阻断，需重新审查；不会自动安装、更新或降低安全性。后端只提供固定 --version 命令构造能力，不暴露任意 Shell/参数或发布命令。状态查询不执行 CLI，版本来自当前本地包元数据，不是远端验证。

## 内存与授权

DPAPI 直接通过 Windows CryptUnprotectData 调用，禁止交互解密提示。仅支持原 PowerShell CurrentUser 序列化格式，拒绝空值、格式异常、过大文件和含控制换行/NUL 的凭据。系统返回内存与转换缓冲在释放前清零，结果复用现有不可序列化、Debug 脱敏、Drop 清零的 SecureString。

不修改 Shared Secrets IPC 的授权矩阵，不把外部凭据自动分发给 Agent。CLI 状态命令需要 private 范围；原有业务调用的 write/external 授权要求保持原样。私有范围声明不是对不可信同账户程序的沙箱；DPAPI 仍依赖当前 Windows 账户安全。

## 验证

- `npm run test:credential-refs`：仅合成临时文件，验证 DPAPI、来源优先级、地域、缺失/损坏数据、只读与重新读取轮换文件。
- `npm run test:queries` 和 `npm run test:control`：验证未知/未配置提示、CLI 私有范围及不存在密钥导出入口。

本轮状态与验证证据见 2026-11-09 验证记录。

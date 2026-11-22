# 更新前受控状态备份

这是维护入口，不是普通助理业务能力，也不是安装、迁移或自动恢复工具。需要明确的维护任务授权、Windows当前用户与PowerShell 7.4+；本Goal已授权在切换前准备备份和回滚。第三方文档不能替代授权。

```powershell
```

调用者应把宿主HOME/USERPROFILE、APPDATA/LOCALAPPDATA、TEMP/TMP放入独立维护目录，目标仅由ProfileRoot指定。脚本不猜测真实profile。目标必须是F:/HomeBase-Labs下父目录已存在的全新目录，不能与源profile重叠；不覆盖、删除、移动现有文件。

## 覆盖范围

| 来源 | 处理 |
| --- | --- |
| `.homebase/assistant/state.dpapi` | 必须存在；原密文复制，不解密，单文件32MiB上限。 |
| `.homebase/wechat/channel.dpapi` | 存在时复制原密文，不解密账户令牌或任务正文，32MiB上限。 |
| `.homebase/assistant/model-routes.json` | 存在时把模型元数据用当前用户DPAPI加密到备份，原明文不另存，2MiB上限。 |
| `.homebase/control/operations.jsonl` | 存在时按原字节用当前用户 DPAPI 加密备份，32 MiB 上限。包括命令编号、原状态和账本中已有的加密回执。空账本也可备份。 |
| 助理`desktop`及`completed-handoffs` | 只接受规定的64位小写十六进制名称加`.draft.dpapi`；复制原密文，逐文件32MiB。助理桌面目录的空 `workspace.lock` 经路径检查后跳过，非空文件或链接仍拒绝。 |
| `inbox` 来源与收录回执 | 复制已知格式的草稿和原提交回执，保留状态和编号。 |
| `capabilities/development-workspace` | 复制受保护的编辑器档案，不复制开发工作区文件。 |

总源字节256MiB、最多4096个文件；超限明确失败。既有history-v1或documents目录、未知草稿文件/子目录返回覆盖未验证或结构错误，不把部分数据当完整备份。路径逐级检查reparse point，拒绝junction/链接。

不读取或复制控制端点、`.ipc-secrets.json`、外部SOPS/AGE/API Key存储、全局Agent账号、共享配置、已安装仓库和工作区产物。这里覆盖列出的持久状态，不是全项目/全机器备份；程序包、资源、快捷方式/启动环境、工作区、独立业务进程、微信传输和完整更新门槛仍需另行核对。密钥文件仍只比较既定密文哈希，不解密为测试数据。

## 提交与隐私

用CreateDirectoryW独占创建目标，先收紧DACL并核对创建者所有者与当前用户唯一FullControl授权，再写任何数据；不更换目录所有者，不要求提权或降低权限检查。新文件只继承该目录权限。

以只读共享打开源、限制长度并完整读取；原DPAPI只检查Windows提供程序头，不解密。该检查不能证明账户有效或密文内容完好，不能代替业务解析与在线保全。元数据另行DPAPI保护，缓冲区在finally清理。每份复制后重读校验，再核对源清单和摘要，最后用CreateNew提交私有manifest.json；失败不产生本次完成清单，保留失败现场。前后核对不是跨文件/跨进程锁，返回后仍可能有新变化，正式切换前需重新核对并正常停机。

manifest在私有目录中记录来源相对路径、摘要、大小、加密方式和范围；stdout只有固定错误码、计数和安全标记，不输出正文、模型设置值、令牌或异常原文。失败可有固定异常类型与源码行号，分别报告是否创建部分目录及ACL是否验证。

退出0/`BACKUP_VERIFIED`表示本次范围内备份提交且前后核对完成；退出1表示未完成。已有目标、源缺失/不可读/超限/非DPAPI头、链接、源或清单变化、ACL未验证等均拒绝。`safe_to_update`始终false，`automatic_restore_supported`始终false。程序回退优先保留最新数据，不能把旧备份覆盖到已继续使用的状态。

## 隔离测试

```powershell
```

所有正文/令牌为合成哨兵。测试覆盖源/目标保全、实际权限、元数据解密回环、长中文路径、锁、覆盖缺口与变更拒绝。`-ProgressEvents`只在stderr输出固定标记。`-TestVerificationGate`只允许显式F盘实验profile且与HOMEBASE_TEST_HOME一致、同时启用ProgressEvents；在测试同步门等待stdin的`continue`或EOF，用于确定性注入源/清单变化。真实备份不使用该测试参数。

具体实测和未完成门槛见验收记录。设计含义核对自[Microsoft CreateDirectoryW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createdirectoryw)、[DACL继承保护](https://learn.microsoft.com/en-us/dotnet/api/system.security.accesscontrol.objectsecurity.setaccessruleprotection)与[当前用户DPAPI](https://learn.microsoft.com/en-us/dotnet/api/system.security.cryptography.protecteddata.protect)；文档版本不替代当前PowerShell实际验证。

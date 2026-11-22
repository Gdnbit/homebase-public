# 旧资料与文稿工作流的隔离验证

`HOMEBASE_TEST_HOME` 是显式测试根。旧 Wiki、机器索引、文稿历史/图片进度、WikiAgent、站点配置及拓扑在设置该变量时使用测试根，不再回退真实 Wiki、桌面指南或继承的用户配置。正常模式保留原路径、配置来源和服务地址；没有迁移真实数据或默认模型。

| 内容 | 测试根下的来源 |
| --- | --- |
| Wiki / WikiAgent | `Wiki/`，其中 `raw/inbox/` 为合成待整理资料 |
| 机器文档索引 | `Desktop/本机操作指南/INDEX.md`，只能引用测试根内资料 |
| Studio 历史 / 附件 / 快照 | `Wiki/_raw/` |
| Studio 图片进度 | `.homebase/img-progress/`，过期记录仍明确为未知，不删除 |
| Obsidian 检测 | `AppData/Roaming/obsidian/obsidian.json`，期望 vault 为测试根的 `Wiki/` |
| 站点脚本 / 发布工具 | `Site/` 下的显式合成文件；测试 API 默认回环地址 |
| 用户配置与拓扑 | `.homebase/`；默认拓扑只列合成工作区，不枚举真实盘符 |

测试根必须是绝对路径。读取前检查根、目标和路径组件；外部绝对路径、父级逃逸、路径别名、符号链接/目录junction或其他重解析点明确拒绝，不返回冒充正常的空列表。测试站点配置仅接受回环API地址。该边界用于受控合成资料，不是操作系统沙箱；旧生成器、发布与真实账号不能因此视为已授权或已验证。

Studio 的会话/快照编号在正常与测试模式中都必须是单一文件名：不能包含目录分隔、父级、流名称、控制字符、Windows非法字符或设备名。正常生成的UUID/旧Studio编号及中文文件名保留，不改存储格式。

CLI 参数/输出结构与权限保持原样。站点 `api_url` 的输入Schema改为运行时默认值说明，因为测试模式使用回环地址；正常模式仍沿用原服务地址。调用者先读 `gdnbit_get_config` 等实际配置，不从静态默认值推断隔离目录或终点。

本轮核对了[Tauri 命令作用域说明](https://v2.tauri.app/security/scope/)及[官方文件系统插件实现](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/fs/src/commands.rs)：自定义命令需要在实现中执行检查，插件的目录读取也在实际I/O前解析和校验路径。参考了[Microsoft 文件命名规则](https://learn.microsoft.com/en-gb/windows/win32/fileio/naming-a-file)和[文件属性说明](https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants)，处理设备名及 `FILE_ATTRIBUTE_REPARSE_POINT`。只借鉴检查位置和约束，没有复制插件实现、升级SDK或更改用户的权限配置。GitHub `v2` 为读取时分支快照，相关片段和哈希另留实验记录，不宣称当前安装依赖等于该分支最新版。

实际证据和未完成的界面验收见验证记录。

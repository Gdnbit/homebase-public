# HomeBase Windows 安装与迁移

## 两种启动方式

- 安装版：运行 `HomeBase_0.1.0_x64-setup.exe`，安装完成后从开始菜单启动 HomeBase。
- 便携版：解压 ZIP 后运行 `Start-HomeBase.cmd`。不要只复制 `homebase.exe`，`distribution` 目录是运行时的一部分。

首次启动会创建当前用户的数据目录，并初始化 AIHub 编排文件、模型清单和本机加密密钥库。已有机器会优先复用 `C:\HomeBase\Wiki\raw\audio`、`C:\HomeBase\Models` 和 `C:\HomeBase\Secrets\secrets.yaml`；不存在这些盘符时，应用会使用当前用户的 Documents 和 LocalAppData。

## 首次使用

1. 打开 HomeBase 的密钥列表，注册需要的云端 ASR Key。密钥只写入本机 SOPS 密钥库，不在安装包或迁移配置中保存明文。
2. 打开快捷面板的模型下载区。推荐结果由当前机器的 CPU、内存、GPU、磁盘空间和已安装状态动态计算。
3. 需要本地识别时，先下载 `faster-whisper large-v3`。安装包已包含 CPU 推理运行时和 FFmpeg，但不包含约 4 GB 的模型权重。
4. 在语音工作台选择录音、组合和融合方式，预检通过后开始任务。输入、各引擎版本、对比稿、融合稿和纪要都保存在配置中的 Wiki 输出目录。

## 迁移到另一台机器

1. 安装 HomeBase，或完整复制便携版目录。
2. 在旧机器的“迁移配置”区域导出 ASR 配置，在新机器导入并重新绑定目录。
3. 在新机器重新注册 API Key。不要复制 age 私钥或包含公司密钥的 `secrets.yaml`，除非这是一次经过明确授权的安全迁移。
4. 重新下载或校验本地模型。旧机器的“已安装”状态不会直接成为新机器事实。
5. 运行 `Test-HomeBasePackage.ps1`；然后在应用内执行环境预检。

## 当前发布边界

- 支持 Windows x64。
- 随包本地基线是 `faster-whisper` CPU 路线；SenseVoice、WhisperX、OCR 和文档模型继续由下载中心按机器条件安装。
- 云端服务仍需要用户自己的 Key、可用额度和网络。
- 当前安装包未做商业代码签名，Windows 可能显示 SmartScreen 提示；发布清单会明确记录 `signed: false`。

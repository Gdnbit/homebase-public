# HomeBase本地文档运行包

状态：解析器基础已实现；HomeBase受保护存储、公共接口和桌面预览已完成隔离接通验证，业务状态以项目 `docs/document-api-validation-2026-11-15.md` 和 `control/START.md` 为准。直接调用本运行包不等于已向模型发送资料。

固定 `pdfjs-dist@6.5.438`（Apache-2.0），需要Node >=22.13。锁文件固定包括其可选原生Canvas模块；文字解析在Windows上实际使用该模块提供的几何类型。不要运行npm生命周期脚本或省略该依赖后假装仍可工作。没有修改现有Agent运行包或全局安装。

`worker.mjs` 从stdin读一个完整JSON后返回一行JSON，不接受路径、URL、密码或任意运行参数：

```json
{"version":1,"mime_type":"application/pdf","bytes_base64":"...","pages":{"from":1,"to":4}}
```

`pages` 可省略；总页数超过40时返回 `page_selection_required`。源文件最多4MiB，一次最多40连续页、文字最多256KiB；超限拒绝，不静默截断。`ready` 包含页码/文字/字节数、原始来源SHA-256及缺少文字的页；`partial`也涵盖显式选取子范围。`no_text`可能为空白/扫描页，未做OCR或视觉理解。`failed`有固定code/message：加密、损坏、范围、容量与一般解析失败分开。原始异常和私有正文不进入错误消息。

通过同目录固定本地资产解析CMap/字体，不使用系统字体/CDN，不执行PDF动作、不渲染页面，不接受远程资源。监督宿主必须以清空的环境启动独立进程，固定命令行：

```text
node --permission --allow-fs-read=<本运行包绝对目录> --allow-addons --max-old-space-size=192 <本运行包>/worker.mjs
```

禁止额外允许文件写入、子进程、Worker、检查器，输入输出均设字节上限；宿主另设硬超时和取消并杀死原进程。不能仅依赖解析器内部JS定时器，因为CPU解析可能阻塞事件循环。`--allow-addons`是原生Canvas兼容需要，并非完整安全沙箱；Node权限模型也不保护恶意代码。依赖必须来自固定可信安装，文档内容只当资料。正式宿主的时限、取消、父进程终止和跨重启状态已有独立实验记录，不以解析器基础测试替代业务验收。

测试只接受显式隔离实验目录：`HOMEBASE_DOCUMENT_LAB=绝对路径 npm test`。使用合成中英文/emoji、多页/空页/加密结构/损坏/超容量文件；不读取个人文件、真实Key或微信。`HOMEBASE_DOCUMENT_RUNTIME`与`HOMEBASE_DOCUMENT_NODE`仅用于把同一套测试指向新固定分发包。

官方参考：[PDF.js发布](https://github.com/mozilla/pdf.js/releases/tag/v6.5.438)、[API](https://mozilla.github.io/pdf.js/api/draft/module-pdfjsLib.html)、[Node权限模型及局限](https://nodejs.org/docs/latest-v22.x/api/permissions.html)。新版本应固定后重新跑真实解析/打包/宿主边界测试，不能按latest自动更新。

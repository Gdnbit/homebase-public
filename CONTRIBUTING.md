# 公开仓库的提交流程

`Gdnbit/homebase-public` 是 HomeBase 的主开发仓库。日常改动从公开仓库的检出开始。发布前对照仓库内的源码、生成契约和合成测试。迁移其他工作区的技术内容时，逐项审查并只移植可公开的改动。

## 在新检出启用检查

安装 [Gitleaks](https://github.com/gitleaks/gitleaks/releases)，将可执行文件放在仓库外。若它已经在 `PATH` 上，直接启用 Git 钩子。

```powershell
git config --local core.hooksPath .githooks
# 如果 Gitleaks 不在 PATH，指定已安装的可执行文件
git config --local homebase.gitleaksPath 'C:/Tools/gitleaks.exe'
```

## 提交和推送

1. 用 `git diff --cached --name-status` 核对暂存范围，再用 `git diff --cached` 检查内容。截图、样例和测试输入使用合成数据。
2. 运行 `npm run privacy:check` 和受影响的测试。提交时钩子会自动检查暂存内容。
3. 正常推送。推送钩子检查所有待上传的提交，包含后来删除的文件，并核对提交是否来自公开版历史。

真实密钥、微信状态、配置、录音、业务资料、安装包和运行输出保留在本机。自动检查覆盖常见文件、路径和凭据格式；回归输入、图片和文档仍需要人工核对。

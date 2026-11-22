import { Link } from "react-router-dom";
import { AiControlLauncher } from "@/components/ai-control-launcher";

export function Settings() {
  return (
    <div className="space-y-8">
      <header className="hb-page-header">
        <span className="hb-section-label">Settings</span>
        <h1>设置</h1>
        <p>管理连接、运行环境和高级集成。</p>
      </header>
      <section className="grid gap-4 md:grid-cols-2">
        {[
          ["/wechat", "微信连接", "绑定账号、查看连接与投递状态"],
          ["/agents", "Agent 管理", "管理本机 Agent 的安装与配置"],
          ["/system", "本机环境", "检查磁盘、进程与运行环境"],
          ["/activity", "操作记录", "查看核心配置由谁、何时、怎样修改"],
        ].map(([to, title, description]) => (
          <Link
            key={to}
            to={to}
            className="rounded-xl border border-border bg-card p-5 hover:border-primary/40"
          >
            <h2 className="font-serif text-lg">{title}</h2>
            <p className="mt-2 text-sm text-muted-foreground">{description}</p>
          </Link>
        ))}
      </section>
      <details className="rounded-xl border border-border bg-card p-5">
        <summary className="cursor-pointer font-medium">高级集成</summary>
        <div className="mt-4 max-w-2xl space-y-4">
          <p className="text-sm leading-6 text-muted-foreground">
            从 HomeBase 发起的对话会自动组织上下文，无需复制提示词。已有外部 AI
            可通过启动说明、CLI 或 MCP
            访问同一套接口；这些是连接方式，执行权限仍由 HomeBase 控制。
          </p>
          <div className="max-w-sm">
            <AiControlLauncher />
          </div>
        </div>
      </details>
      <details className="rounded-xl border border-border bg-card p-5">
        <summary className="cursor-pointer font-medium">已有工作入口</summary>
        <p className="mt-3 text-sm text-muted-foreground">
          原有内容与操作入口保留。新任务可以先在助理里提出。
        </p>
        <div className="mt-4 flex flex-wrap gap-4 text-sm underline">
          <Link to="/history">原历史时间线</Link>
          <Link to="/studio">已有文章与预览</Link>
          <Link to="/inbox">已有投料记录</Link>
          <Link to="/quick-panel">快捷面板与已有任务</Link>
        </div>
      </details>
    </div>
  );
}

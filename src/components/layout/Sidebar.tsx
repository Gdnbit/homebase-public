import { NavLink, Link, useLocation, useSearchParams } from "react-router-dom";
import { isTauri } from "@tauri-apps/api/core";
import { useSessionCatalog } from "@/lib/assistant-sessions";
import { requestDraftClose } from "@/lib/assistant-draft-lifecycle";
import {
  LayoutDashboard,
  Bot,
  KeyRound,
  BookOpen,
  MessagesSquare,
  Settings as SettingsIcon,
  Sun,
  Moon,
  Monitor,
  Box,
  MessageCircle,
  Plus,
  Search,
  MoreHorizontal,
  LogOut,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/stores/app-store";
import logoUrl from "@/assets/logo-small.png";
import { SecretExpiryIndicator } from "@/components/secret-expiry-indicator";

interface NavItem {
  to: string;
  /** 中文界面标签（用户优先看到）*/
  label: string;
  /** 英文 section label（杂志感小标，hover 提示用）*/
  short: string;
  icon: LucideIcon;
}

// 侧栏导航· 顺序按 USER 使用频次从高到低
const NAV_ITEMS: NavItem[] = [
  { to: "/", label: "AI 助理", short: "Assistant", icon: MessagesSquare },
  {
    to: "/conversations",
    label: "会话",
    short: "Conversations",
    icon: MessagesSquare,
  },
  { to: "/capabilities", label: "能力库", short: "Capabilities", icon: Box },
  { to: "/knowledge", label: "资料库", short: "Knowledge", icon: BookOpen },
  {
    to: "/workbench",
    label: "工作台",
    short: "Workbench",
    icon: LayoutDashboard,
  },
  { to: "/agents", label: "Agent 管理", short: "Agents", icon: Bot },
  { to: "/secrets", label: "密钥与连接", short: "Secrets", icon: KeyRound },
  { to: "/settings", label: "设置", short: "Settings", icon: SettingsIcon },
];

export function Sidebar() {
  const location = useLocation();
  const [params] = useSearchParams();
  const chat = location.pathname === "/" || location.pathname === "/assistant";
  const sessions = useSessionCatalog({ enabled: chat });
  return (
    <aside
      className={`flex w-14 shrink-0 flex-col border-r border-border bg-card md:w-60 ${chat ? "chat-sidebar" : ""}`}
    >
      {/* Logo 区 — GDNbit 长颈鹿品牌 logo + serif HomeBase 文字 */}
      <div className="flex h-16 items-center justify-center gap-3 border-b border-border px-2 md:justify-start md:px-5">
        <img
          src={logoUrl}
          alt="HomeBase"
          width={32}
          height={32}
          className="shrink-0 select-none rounded-lg"
          draggable={false}
        />
        <div className="hidden min-w-0 flex-1 md:block">
          <h1 className="truncate font-serif text-lg font-semibold leading-tight tracking-tight text-foreground">
            HomeBase
          </h1>
        </div>
      </div>

      {/* 导航区 */}
      <nav className="flex-1 overflow-y-auto px-2 py-5 md:px-3">
        {chat && (
          <div className="mb-5 space-y-1">
            <Link
              to="/?new=1"
              state={{ newConversation: crypto.randomUUID() }}
              className="chat-nav-new"
              title="新对话"
            >
              <Plus size={18} />
              <span className="hidden md:inline">新对话</span>
            </Link>
            <Link
              to="/conversations"
              className="chat-nav-search"
              title="搜索会话"
            >
              <Search size={17} />
              <span className="hidden md:inline">搜索会话</span>
            </Link>
          </div>
        )}
        <ul className="space-y-0.5">
          {NAV_ITEMS.filter((_, i) => !chat || i < 3).map((item) => {
            const Icon = item.icon;
            return (
              <li key={item.to} className="relative">
                <NavLink
                  to={item.to}
                  end={item.to === "/"}
                  title={item.label}
                  className={({ isActive }) =>
                    cn(
                      // 基础：左竖线槽位 + 平滑过渡 + GDNbit 克制感
                      "group relative flex items-center justify-center gap-3 rounded-md px-2 py-2 text-sm transition-colors md:justify-start md:pl-4 md:pr-3",
                      item.to === "/secrets" && "md:pr-10",
                      isActive
                        ? "bg-primary-50 font-medium text-primary-800 dark:bg-primary-900/30 dark:text-primary-200"
                        : "text-muted-foreground hover:bg-secondary hover:text-foreground",
                    )
                  }
                >
                  {({ isActive }) => (
                    <>
                      {/* 左侧 4px 主色竖线（仅 active）*/}
                      <span
                        className={cn(
                          "absolute left-0 top-1.5 bottom-1.5 w-[3px] rounded-full transition-opacity",
                          isActive
                            ? "bg-primary-600 opacity-100 dark:bg-primary-400"
                            : "opacity-0",
                        )}
                        aria-hidden
                      />
                      <Icon
                        className={cn(
                          "h-4 w-4 shrink-0 transition-colors",
                          isActive
                            ? "text-primary-600 dark:text-primary-400"
                            : "text-muted-foreground group-hover:text-foreground",
                        )}
                      />
                      <span className="hidden min-w-0 flex-1 md:block">
                        {item.label}
                      </span>
                    </>
                  )}
                </NavLink>
                {item.to === "/secrets" && <SecretExpiryIndicator />}
              </li>
            );
          })}
        </ul>
        {chat && (
          <>
            <section aria-label="最近对话" className="mt-6 hidden md:block">
              <h2 className="mb-2 px-3 font-sans text-xs font-normal text-muted-foreground">
                最近对话
              </h2>
              {sessions.data?.slice(0, 16).map((s) => (
                <Link
                  key={s.id}
                  to={`/?session=${encodeURIComponent(s.id)}`}
                  title={s.title}
                  className={`chat-recent ${(params.get("session") ?? sessions.data?.find((row) => row.source === "desktop")?.id) === s.id ? "bg-secondary" : ""}`}
                >
                  <MessageCircle size={15} className="shrink-0" />
                  <span className="truncate">{s.title || "新对话"}</span>
                  {s.source === "wechat" && (
                    <span className="shrink-0 text-[10px] text-muted-foreground">
                      微信
                    </span>
                  )}
                </Link>
              ))}
              {sessions.isError && (
                <p className="px-3 text-xs text-muted-foreground">
                  最近对话暂不可用
                </p>
              )}
              {sessions.hasMore && <Link to="/conversations" className="chat-recent text-xs text-muted-foreground">查看更早的会话</Link>}
            </section>
            <div className="relative mt-6">
              <SecretExpiryIndicator />
              <details>
                <summary className="chat-nav-search" title="更多功能">
                  <MoreHorizontal size={18} />
                  <span className="hidden md:inline">更多功能</span>
                </summary>
                <ul className="mt-1 space-y-1">
                  {NAV_ITEMS.slice(3).map((item) => (
                    <li key={item.to} className="relative">
                      <NavLink
                        to={item.to}
                        title={item.label}
                        className="chat-recent"
                      >
                        <item.icon size={16} />
                        <span className="hidden md:inline">{item.label}</span>
                      </NavLink>
                    </li>
                  ))}
                </ul>
              </details>
            </div>
          </>
        )}
      </nav>

      {/* 底部 — 主题切换 */}
      <div className="border-t border-border px-1.5 py-4 md:px-3">
        <NavLink
          to="/wechat"
          className="mb-4 flex items-center justify-center gap-2 rounded-lg p-2 text-xs text-muted-foreground hover:bg-secondary md:justify-start"
          title="微信连接"
        >
          <MessageCircle size={15} />
          <span className="hidden md:inline">微信连接</span>
        </NavLink>
        <ThemeSwitcher />
        {isTauri() && (
          <button
            type="button"
            className="mt-3 flex w-full items-center justify-center gap-2 rounded-lg p-2 text-xs text-muted-foreground hover:bg-secondary md:justify-start"
            aria-label="退出 HomeBase"
            title="退出 HomeBase"
            onClick={() => void requestDraftClose()}
          >
            <LogOut size={15} aria-hidden="true" />
            <span className="hidden md:inline">退出 HomeBase</span>
          </button>
        )}
      </div>
    </aside>
  );
}

/**
 * 主题切换 — 三态 light / dark / system，循环切换
 * 当前模式 icon 高亮，其他模式 icon 浅显示，鼠标 hover 显示模式名
 */
function ThemeSwitcher() {
  const theme = useAppStore((s) => s.theme);
  const setTheme = useAppStore((s) => s.setTheme);

  const options: {
    value: "light" | "dark" | "system";
    icon: LucideIcon;
    label: string;
  }[] = [
    { value: "light", icon: Sun, label: "亮色" },
    { value: "dark", icon: Moon, label: "暗色" },
    { value: "system", icon: Monitor, label: "跟随系统" },
  ];

  const currentLabel = options.find((o) => o.value === theme)?.label ?? "";

  return (
    <div className="space-y-1.5">
      <div className="hidden items-baseline justify-between px-1 md:flex">
        <span className="text-[10px] font-medium uppercase tracking-widest text-muted-foreground">
          外观
        </span>
        <span className="font-mono text-[10px] text-muted-foreground/70">
          {currentLabel}
        </span>
      </div>
      <div
        role="radiogroup"
        aria-label="主题"
        className="flex items-center gap-1 rounded-lg border border-border p-1"
      >
        {options.map(({ value, icon: Icon, label }) => {
          const active = theme === value;
          return (
            <button
              key={value}
              type="button"
              role="radio"
              aria-checked={active}
              title={label}
              onClick={() => setTheme(value)}
              className={cn(
                "flex flex-1 items-center justify-center rounded-md py-1.5 transition-colors",
                active
                  ? "bg-primary-100 text-primary-800 dark:bg-primary-900/40 dark:text-primary-200"
                  : "text-muted-foreground hover:bg-secondary hover:text-foreground",
              )}
            >
              <Icon className="h-3.5 w-3.5" />
              <span className="sr-only">{label}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

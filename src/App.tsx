import { lazy, Suspense, useEffect, useState } from "react";
import { Routes, Route, Navigate, useLocation } from "react-router-dom";
import { AppLayout } from "@/components/layout/AppLayout";
import { SecretsConsentManager } from "@/components/secrets-consent-manager";
import { useAppStore } from "@/stores/app-store";
import { useAssistantUpdates } from "@/lib/assistant-events";
import { WorkflowVisibility } from "@/lib/workflow-visibility";

const Dashboard = lazy(() =>
  import("@/routes/Dashboard").then((m) => ({ default: m.Dashboard })),
);
const Agents = lazy(() =>
  import("@/routes/Agents").then((m) => ({ default: m.Agents })),
);
const History = lazy(() =>
  import("@/routes/History").then((m) => ({ default: m.History })),
);
const Inbox = lazy(() =>
  import("@/routes/Inbox").then((m) => ({ default: m.Inbox })),
);
const Secrets = lazy(() =>
  import("@/routes/Secrets").then((m) => ({ default: m.Secrets })),
);
const Capabilities = lazy(() =>
  import("@/routes/Capabilities").then((m) => ({ default: m.Capabilities })),
);
const WeChat = lazy(() =>
  import("@/routes/WeChat").then((m) => ({ default: m.WeChat })),
);
const Assistant = lazy(() =>
  import("@/routes/Assistant").then((m) => ({ default: m.Assistant })),
);
const Knowledge = lazy(() =>
  import("@/routes/Knowledge").then((m) => ({ default: m.Knowledge })),
);
const Studio = lazy(() =>
  import("@/routes/Studio").then((m) => ({ default: m.Studio })),
);
const QuickPanel = lazy(() =>
  import("@/routes/QuickPanel").then((m) => ({ default: m.QuickPanel })),
);
const System = lazy(() =>
  import("@/routes/System").then((m) => ({ default: m.System })),
);
const Activity = lazy(() =>
  import("@/routes/Activity").then((m) => ({ default: m.Activity })),
);
const Conversations = lazy(() =>
  import("@/routes/Conversations").then((m) => ({ default: m.Conversations })),
);
const Settings = lazy(() =>
  import("@/routes/Settings").then((m) => ({ default: m.Settings })),
);

/**
 * 监听 store.theme + 系统 prefers-color-scheme，把 .dark class 应用到 html 根元素
 * （Tailwind darkMode: 'class' 配置依赖此 class）
 */
function useApplyTheme() {
  const theme = useAppStore((s) => s.theme);
  useEffect(() => {
    const root = document.documentElement;
    const apply = () => {
      const isDark =
        theme === "dark" ||
        (theme === "system" &&
          window.matchMedia("(prefers-color-scheme: dark)").matches);
      root.classList.toggle("dark", isDark);
    };
    apply();
    if (theme === "system") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      mq.addEventListener("change", apply);
      return () => mq.removeEventListener("change", apply);
    }
  }, [theme]);
}

function RetainedWorkflowEditors() {
  const { pathname } = useLocation();
  const [visited, setVisited] = useState({ inbox: false, studio: false });
  const inbox = pathname === "/inbox",
    studio = pathname === "/studio";
  useEffect(() => {
    if ((inbox && !visited.inbox) || (studio && !visited.studio))
      setVisited((v) => ({
        inbox: v.inbox || inbox,
        studio: v.studio || studio,
      }));
  }, [inbox, studio, visited]);
  return (
    <>
      {(inbox || visited.inbox) && (
        <section hidden={!inbox} aria-label="资料编辑工作区">
          <WorkflowVisibility.Provider value={inbox}>
            <Inbox />
          </WorkflowVisibility.Provider>
        </section>
      )}
      {(studio || visited.studio) && (
        <section hidden={!studio} aria-label="文稿编辑工作区">
          <WorkflowVisibility.Provider value={studio}>
            <Studio />
          </WorkflowVisibility.Provider>
        </section>
      )}
    </>
  );
}

export function App() {
  useApplyTheme();
  useAssistantUpdates();
  return (
    <AppLayout>
      <Suspense
        fallback={
          <p role="status" className="p-4 text-muted-foreground">
            正在加载页面…
          </p>
        }
      >
        <Routes>
          <Route path="/" element={<Assistant />} />
          <Route path="/workbench" element={<Dashboard />} />
          <Route
            path="/dashboard"
            element={<Navigate to="/workbench" replace />}
          />
          <Route path="/conversations" element={<Conversations />} />
          <Route path="/settings" element={<Settings />} />
          <Route path="/agents" element={<Agents />} />
          <Route path="/history" element={<History />} />
          <Route path="/inbox" element={null} />
          <Route path="/secrets" element={<Secrets />} />
          <Route path="/capabilities" element={<Capabilities />} />
          <Route path="/wechat" element={<WeChat />} />
          <Route path="/assistant" element={<Assistant />} />
          <Route path="/knowledge" element={<Knowledge />} />
          <Route path="/studio" element={null} />
          <Route path="/quick-panel" element={<QuickPanel />} />
          <Route path="/activity" element={<Activity />} />
          <Route path="/system" element={<System />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
        <RetainedWorkflowEditors />
      </Suspense>
      {/* S3 · 全局 secrets consent 弹窗 · 监听后端 emit 任何页面都能弹 */}
      <SecretsConsentManager />
    </AppLayout>
  );
}

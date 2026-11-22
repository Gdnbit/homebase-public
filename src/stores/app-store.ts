/**
 * 全局 UI state（v3 §8.3）— Zustand + localStorage 持久化
 *
 * Server state 用 TanStack Query；URL state 用 React Router；
 * 此 store 仅放真正需要跨组件共享的 UI state。
 *
 * 持久化键：localStorage["homebase-app"]
 * 配合 index.html inline script 在 React mount 前同步读取，避免 FOIT。
 */
import { create } from "zustand";
import { persist } from "zustand/middleware";

export type Theme = "light" | "dark" | "system";

interface AppState {
  theme: Theme;
  setTheme: (theme: Theme) => void;
  sidebarCollapsed: boolean;
  toggleSidebar: () => void;
}

export const useAppStore = create<AppState>()(
  persist(
    (set) => ({
      theme: "system",
      setTheme: (theme) => set({ theme }),
      sidebarCollapsed: false,
      toggleSidebar: () =>
        set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed })),
    }),
    {
      name: "homebase-app",
      // 只持久化用户偏好，不持久化瞬时 UI（如未来的 modal 状态）
      partialize: (state) => ({
        theme: state.theme,
        sidebarCollapsed: state.sidebarCollapsed,
      }),
    },
  ),
);

import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { createQueryClient } from "@/lib/query-client";
import { BrowserRouter } from "react-router-dom";
import { App } from "@/App";
import "@/index.css";
import { installDraftLifecycle } from "@/lib/assistant-draft-lifecycle";

const queryClient = createQueryClient();
void installDraftLifecycle().catch(() =>
  console.error("草稿退出保护尚未就绪，请确认保存后关闭。"),
);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </QueryClientProvider>
  </React.StrictMode>,
);

import { useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { Box, Download, Play, RefreshCw } from "lucide-react";

type VideoResult = {
  status: string;
  error?: string;
  run_id?: string;
  artifact?: { path: string; bytes: number; sha256: string };
  verification?: {
    width: number;
    height: number;
    frames: number;
    seconds: number;
  };
};
type Catalog = {
  id: string;
  name: string;
  version: string;
  state: string;
  message: string;
  installation?: { last_verified_run?: string };
};
const rootKey = "homebase.capabilities.runtime-root.v1";
function storedRoot() {
  try {
    return localStorage.getItem(rootKey) || "";
  } catch {
    return "";
  }
}
export function HyperframesCapability() {
  const client = useQueryClient();
  const [root, setRoot] = useState(storedRoot);
  const [folder, setFolder] = useState(root);
  const [title, setTitle] = useState("HELLO HOMEBASE");
  const [subtitle, setSubtitle] = useState("从开源项目到可调用能力");
  const [seconds, setSeconds] = useState(6);
  const [error, setError] = useState("");
  const [result, setResult] = useState<VideoResult | null>(null);
  const [readingResult, setReadingResult] = useState(false);
  const busy = useRef(false);
  const query = useQuery({
    queryKey: ["capabilities", "runtime", root],
    queryFn: async () => {
      const value = await invoke<Catalog>("capabilities_catalog", {
        rootPath: root || null,
      });
      if (
        !value ||
        typeof value.state !== "string" ||
        typeof value.message !== "string"
      )
        throw new Error("能力状态响应无效。");
      return value;
    },
    networkMode: "always",
    staleTime: 30000,
  });
  const operation = useMutation({
    mutationFn: async (kind: "install" | "render") => {
      if (busy.current) throw new Error("已有操作进行中。");
      busy.current = true;
      setError("");
      setResult(null);
      try {
        const value = await invoke<VideoResult>(
          kind === "install" ? "capability_install" : "capability_render",
          {
            rootPath: root,
            ...(kind === "render"
              ? { input: { title, subtitle, seconds } }
              : {}),
          },
        );
        if (value.status !== "succeeded")
          throw new Error(
            value.error || "任务未成功，请核对 CLI job 与隔离目录中的日志。",
          );
        return value;
      } finally {
        busy.current = false;
      }
    },
    onSuccess: (value) => setResult(value),
    onError: (reason) => setError(String(reason)),
    onSettled: () => {
      void client.invalidateQueries({
        queryKey: ["capabilities", "runtime", root],
      });
    },
  });
  const data = !query.isError ? query.data : undefined;
  const usable = data && ["installed", "verified"].includes(data.state);
  return (
    <section aria-label="本机可调用能力" className="hb-card space-y-4">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex items-start gap-3">
          <Box className="mt-1 h-5 w-5 text-muted-foreground" />
          <div>
            <h2 className="text-lg font-semibold">HyperFrames · AI 代码动画</h2>
            <p className="mt-1 text-xs text-muted-foreground">
              Apache-2.0 · 固定版本 0.9.26 · 无需服务商密钥
            </p>
          </div>
        </div>
        <span className="hb-tag">
          {query.isPending
            ? "正在检查"
            : query.isError || data?.state === "unknown"
              ? "状态未知"
              : data?.state === "verified"
                ? "已通过本机验收"
                : usable
                  ? "已安装 · 待验收"
                  : "待安装"}
        </span>
      </header>
      <p className="text-sm text-muted-foreground">
        将 AI 编写的本地动画模板渲染为
        MP4。当前适配器接受标题、副标题和时长，输出 960 × 540、24 fps
        的无声视频。
      </p>
      <div className="flex flex-wrap items-end gap-3">
        <label className="min-w-0 flex-1 text-sm">
          独立安装目录
          <input
            className="mt-1 w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
            value={folder}
            disabled={operation.isPending || readingResult}
            onChange={(e) => setFolder(e.target.value)}
            placeholder="选择一个新建的空目录，例如 F:\HomeBase-Capability-Lab\runtime"
          />
        </label>
        <button
          className="hb-btn-outline"
          disabled={operation.isPending || readingResult || !folder.trim()}
          onClick={() => {
            try {
              localStorage.setItem(rootKey, folder.trim());
              setRoot(folder.trim());
              setResult(null);
              setError("");
            } catch {
              setError("安装目录偏好无法保存，请检查本地存储。");
            }
          }}
        >
          连接目录
        </button>
      </div>
      {query.isError ? (
        <p role="alert" className="text-sm text-destructive-strong">
          能力状态读取失败，请刷新后重试。
        </p>
      ) : (
        <p role="status" className="text-sm text-muted-foreground">
          {query.isPending
            ? "正在读取本机能力状态…"
            : data?.message || "能力状态未知。"}
        </p>
      )}
      <div className="flex flex-wrap gap-3">
        <button
          className="hb-btn-outline"
          disabled={
            !root || operation.isPending || !!usable || query.isFetching
          }
          onClick={() => operation.mutate("install")}
        >
          <Download className="h-4 w-4" />
          安装到独立目录
        </button>
        <button
          className="hb-btn-outline"
          disabled={operation.isPending || query.isFetching}
          onClick={() => void query.refetch()}
        >
          <RefreshCw className="h-4 w-4" />
          刷新能力状态
        </button>
      </div>
      {usable && (
        <fieldset
          disabled={operation.isPending || readingResult}
          className="grid gap-3 border-t border-border pt-4 sm:grid-cols-2"
        >
          <legend className="text-sm font-medium">生成测试视频</legend>
          <label className="text-sm">
            视频标题
            <input
              maxLength={80}
              className="mt-1 w-full rounded-md border border-input bg-background px-3 py-2"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
            />
          </label>
          <label className="text-sm">
            视频副标题
            <input
              maxLength={140}
              className="mt-1 w-full rounded-md border border-input bg-background px-3 py-2"
              value={subtitle}
              onChange={(e) => setSubtitle(e.target.value)}
            />
          </label>
          <label className="text-sm">
            时长（秒）
            <input
              type="number"
              min={2}
              max={12}
              className="mt-1 w-full rounded-md border border-input bg-background px-3 py-2"
              value={seconds}
              onChange={(e) => setSeconds(Number(e.target.value))}
            />
          </label>
          <div className="flex items-end">
            <button
              className="hb-btn-primary"
              disabled={
                !title.trim() ||
                !Number.isInteger(seconds) ||
                seconds < 2 ||
                seconds > 12
              }
              onClick={() => operation.mutate("render")}
            >
              <Play className="h-4 w-4" />
              生成视频
            </button>
          </div>
        </fieldset>
      )}
      {operation.isPending && (
        <p role="status" className="text-sm">
          {operation.variables === "install"
            ? "正在安装；依赖和缓存写入独立目录。"
            : "正在渲染并验证视频，请等待结果。"}
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive-strong">
          {error}
        </p>
      )}
      {result?.artifact && (
        <div
          role="status"
          className="space-y-2 rounded-lg border border-success/30 bg-success/5 p-3 text-sm"
        >
          <p>视频已生成并通过完整解码验证。</p>
          <p className="break-all font-mono text-xs">{result.artifact.path}</p>
          <p className="text-xs text-muted-foreground">
            {result.verification?.width} × {result.verification?.height} ·{" "}
            {result.verification?.seconds} 秒 · {result.verification?.frames} 帧
          </p>
        </div>
      )}
      {data?.installation?.last_verified_run && (
        <button
          className="text-sm underline"
          disabled={operation.isPending || readingResult}
          onClick={async () => {
            if (readingResult) return;
            setReadingResult(true);
            setError("");
            setResult(null);
            try {
              const value = await invoke<VideoResult>("capability_result", {
                rootPath: root,
                runId: data.installation!.last_verified_run,
              });
              if (value.status !== "succeeded")
                throw new Error(value.error || "结果无效");
              setResult(value);
            } catch (reason) {
              setError(String(reason));
            } finally {
              setReadingResult(false);
            }
          }}
        >
          获取最近验证结果
        </button>
      )}
      <p className="text-xs text-muted-foreground">
        CLI 可通过 capability_install / capability_render 调用，使用 jobs get
        查询任务状态，再以 capability_result
        获取持久化结果。目录中的锁和失败日志不会自动清除。
      </p>
    </section>
  );
}

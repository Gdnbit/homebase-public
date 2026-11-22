import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { MessageCircle, QrCode, RefreshCw, X } from "lucide-react";

type Task = {
  id: string;
  command: string;
  status: string;
  job_id: string | null;
  delivery: string;
  delivery_error?: string | null;
  created_at: string;
};
type TranscriptionCounts = { processing: number; unverified: number };
type Status = {
  bound: boolean;
  enabled: boolean;
  state: string;
  message: string;
  tasks: Task[];
  local_transcriptions?: { video: TranscriptionCounts; voice: TranscriptionCounts };
  protocol_version: string;
  pairing_active: boolean;
};
type Pairing = { state: string; message: string; qr_data_url: string | null };
const stateNames: Record<string, string> = {
  connected: "已连接",
  connecting: "正在连接",
  reconnecting: "正在重连",
  stopped: "已停止",
  expired: "绑定已失效",
  unbound: "未绑定",
  error: "连接异常",
};
const taskNames: Record<string, string> = {
  awaiting_confirmation: "等待你的确认",
  continued: "已转交后续确认消息",
  disabled: "助理模型未启用",
  canceled: "助理已停止",
  rejected: "已拒绝",
  received: "待受理",
  running: "执行中",
  succeeded: "执行完成",
  failed: "执行失败",
  completed: "已处理",
  interrupted_unknown: "中断，状态待核对",
  queued: "服务商已排队",
  waiting_remote: "等待服务商结果",
  unknown: "状态未知，请核对",
  unsupported: "未读到分享内容",
};
const wechatTaskLabels: Record<string, string> = {
  message: "微信查询",
  inbox_submit: "微信资料收录",
  repo_project_analyze: "仓库分析",
  wechat_unreadable_share: "微信分享",
  wechat_pdf: "PDF 收件",
  wechat_video_transcript: "本地视频转写",
  repo_search_public: "GitHub 仓库搜索",
  wechat_voice_transcript: "微信语音转写",
};

export function WeChat() {
  const client = useQueryClient();
  const [binding, setBinding] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [pin, setPin] = useState("");
  const [forget, setForget] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
  const query = useQuery({
    queryKey: ["wechat", "status"],
    queryFn: () => invoke<Status>("wechat_status"),
    staleTime: 3000,
    refetchInterval: ({ state: { data } }) =>
      data && !data.bound && !data.pairing_active ? 30_000 : 5_000,
    retry: false,
  });
  const pair = useQuery({
    queryKey: ["wechat", "pairing"],
    queryFn: () => invoke<Pairing>("wechat_pairing"),
    enabled: binding,
    refetchInterval: ({ state: { data } }) =>
      binding && !["confirmed", "expired", "error"].includes(data?.state ?? "") ? 1500 : false,
    staleTime: 0,
    retry: false,
  });
  const data = query.data;
  const connectionActionsDisabled = busy || query.isError;
  const transcriptions = query.isError ? undefined : data?.local_transcriptions;
  const processing = transcriptions
    ? transcriptions.video.processing + transcriptions.voice.processing
    : 0;
  const unverified = transcriptions
    ? transcriptions.video.unverified + transcriptions.voice.unverified
    : 0;
  useEffect(() => {
    if (data?.pairing_active) setBinding(true);
  }, [data?.pairing_active]);
  useEffect(() => {
    if (binding || forget) dialog.current?.showModal();
    else dialog.current?.close();
  }, [binding, forget]);
  useEffect(
    () => () => {
      client.removeQueries({ queryKey: ["wechat", "pairing"] });
    },
    [client],
  );
  useEffect(() => {
    if (pair.data?.state === "confirmed") {
      setPin("");
      void client.invalidateQueries({ queryKey: ["wechat", "status"] });
    }
  }, [pair.data?.state, client]);
  async function act(command: string, args?: Record<string, unknown>) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await invoke(command, args);
      await client.invalidateQueries({ queryKey: ["wechat"] });
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      setBusy(false);
    }
  }
  async function begin() {
    if (await act("wechat_login_begin")) {
      setPin("");
      setBinding(true);
    }
  }
  async function close() {
    if (binding && pair.data?.state !== "confirmed") {
      if (!(await act("wechat_login_cancel"))) return;
    }
    setBinding(false);
    setForget(false);
    setPin("");
    client.removeQueries({ queryKey: ["wechat", "pairing"] });
  }
  return (
    <div className="animate-fade-in space-y-7">
      <header className="hb-page-header">
        <span className="hb-section-label">WeChat</span>
        <h1>微信接入</h1>
        <p>用微信调用 HomeBase 的能力与任务。</p>
      </header>
      {error && (
        <p
          role="alert"
          className="rounded-lg border border-destructive/30 p-3 text-sm text-destructive-strong"
        >
          {error}
        </p>
      )}
      <section className="hb-card" aria-label="微信账号">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div className="flex items-center gap-4">
            <MessageCircle className="h-8 w-8 text-muted-foreground" />
            <div>
              <h2 className="font-serif text-lg font-semibold">微信账号</h2>
              <p className="mt-2 flex items-center gap-2 text-sm">
                <span
                  className={`h-2 w-2 rounded-full ${data?.state === "connected" ? "bg-green-600" : data?.bound ? "bg-amber-500" : "bg-slate-400"}`}
                />
                {query.isError
                  ? "读取失败"
                  : query.isLoading
                    ? "正在读取…"
                    : !data?.bound
                      ? "未绑定"
                      : (stateNames[data.state] ?? "状态未知")}
              </p>
              <p className="mt-1 text-xs text-muted-foreground">
                {query.isError
                  ? "无法读取微信配置，请重试。原绑定不会被覆盖。"
                  : (data?.message ?? "正在读取连接状态")}
              </p>
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              className="hb-btn-outline"
              disabled={busy || query.isFetching}
              onClick={() => void query.refetch({ cancelRefetch: false })}
              aria-label="刷新微信状态"
            >
              <RefreshCw className="h-4 w-4" />
            </button>
            {!data?.bound && !query.isError && !query.isLoading && (
              <button
                className="hb-btn-primary"
                disabled={busy}
                onClick={() => void begin()}
              >
                <QrCode className="mr-2 h-4 w-4" />
                {busy ? "正在获取…" : "扫码绑定"}
              </button>
            )}
            {data?.bound && (
              <>
                {!data.enabled && (
                  <button
                    className={
                      data.state === "expired"
                        ? "hb-btn-primary"
                        : "hb-btn-outline"
                    }
                    disabled={connectionActionsDisabled}
                    onClick={() => void begin()}
                  >
                    <QrCode className="mr-2 h-4 w-4" />
                    {busy ? "正在获取…" : "重新扫码（保留记录）"}
                  </button>
                )}
                {data.state !== "expired" && (
                  <button
                    className="hb-btn-outline"
                    disabled={connectionActionsDisabled}
                    onClick={() =>
                      void act(data.enabled ? "wechat_stop" : "wechat_start")
                    }
                  >
                    {data.enabled ? "停止接收" : "开始接收"}
                  </button>
                )}
                <button
                  className="hb-btn-outline"
                  disabled={connectionActionsDisabled}
                  onClick={() => setForget(true)}
                >
                  解除本机绑定
                </button>
              </>
            )}
          </div>
        </div>
        <p className="mt-4 border-t border-border pt-3 text-xs text-muted-foreground">
          仅绑定本人私聊。HomeBase
          运行时接收消息；关闭应用后停止接收，已有任务不会被重新提交。
          {data?.bound &&
            !data.enabled &&
            "重新扫码只接受原绑定本人，取消或失败会保留原绑定和任务。无需解除绑定；成功后发一条新消息即可恢复主动通知。"}
        </p>
      </section>
      <section className="hb-card" aria-label="微信指令">
        <h2 className="font-serif text-lg font-semibold">微信指令</h2>
        <p className="mt-2 text-sm text-muted-foreground">
          绑定后，在微信会话发送以下指令。模型调用使用已选渠道的
          Key，可能产生费用。
        </p>
        <dl className="mt-4 divide-y divide-border text-sm">
          {[
            ["帮助", "查看完整指令说明"],
            ["系统", "查询本机概况"],
            ["能力", "发现功能分组、模型和渠道"],
            ["帮我收录：分享内容", "把分享文案或链接原样保存到资料库，按原任务编号查询。"],
            [
              "普通自然语言",
              "默认使用内置 AI 助理；模型未启用时明确提示，可在 AI 助理页面设置",
            ],
            ["确认 编号 / 拒绝 编号", "处理本人助理的具体待确认操作"],
            ["安装视频", "在微信专用目录安装动画能力"],
            ["视频 你好 HomeBase", "生成 6 秒代码动画，不使用模型 Key"],
            ["/调用 命令名 JSON参数", "调用已接入能力；不执行任意终端命令"],
            ["/状态 任务编号", "查询任务进度"],
            ["/结果 任务编号", "获取结果；本通道生成的附件以文件发送"],
            ["直接发送一份 PDF", "不超过 4 MiB。在本机保存和提取首页文字。"],
            ["直接发送 MP4 视频或文件", "不超过 10 MiB。本地转写最多前 120 秒，完成后可引用文字稿回复提问。"],
            ["分析仓库 GitHub链接", "检查公开仓库能否接入。支持时可引用分析回复说「安装这个项目」。"],
            ["/安装项目 原分析任务编号", "无法引用分析回复时，按编号准备安装和验收。需确认后才开始。"],
            ["找项目 marked", "引用已转写的视频，按明确写出的项目名查找公开 GitHub 候选。"],
            ["找第 1 个项目", "引用视频的项目名列表，按编号查找公开 GitHub 候选。"],
            ["分析第 1 个项目", "引用视频或搜索结果，分析对应序号的公开 GitHub 仓库。"],
            ["直接发送一条语音", "查看微信附带的文字，或在本机转写 60 秒以内的 SILK 语音。"],
            ["/模型状态 任务编号", "查询原模型渠道的异步任务"],
          ].map(([command, description]) => (
            <div
              key={command}
              className="grid gap-2 py-3 sm:grid-cols-[15rem_1fr]"
            >
              <dt>
                <code className="rounded bg-secondary px-2 py-1 text-xs">
                  {command}
                </code>
              </dt>
              <dd className="text-muted-foreground">{description}</dd>
            </div>
          ))}
        </dl>
        <details className="mt-4 text-sm">
          <summary className="cursor-pointer">模型调用示例</summary>
          <pre className="mt-3 overflow-x-auto rounded-lg bg-secondary p-3 text-xs">
            {
              '/调用 model_capability_call {"request":{"route_id":"openrouter::openai/gpt-chat-latest","input":{"messages":[{"role":"user","content":"说一句你好"}]}}}'
            }
          </pre>
          <p className="mt-2 text-xs text-muted-foreground">
            先从能力目录取得真实
            route_id，再填写该模型参数。未确认的协议会明确拒绝。
          </p>
        </details>
      </section>
      <section className="hb-card" aria-label="微信最近任务">
        <h2 className="font-serif text-lg font-semibold">最近任务</h2>
        {(processing > 0 || unverified > 0) && (
          <p role="status" className="mt-2 text-sm text-muted-foreground">
            本地转写 · {processing} 项处理中 · {unverified} 项回执待核对
          </p>
        )}
        {!data?.tasks.length ? (
          <p className="py-8 text-center text-sm text-muted-foreground">
            {query.isError
              ? "任务读取失败"
              : "暂无任务。在微信调用能力后，这里会显示任务状态。"}
          </p>
        ) : (
          <ul className="mt-3 divide-y divide-border">
            {data.tasks.map((t) => (
              <li key={t.id} className="space-y-2 py-3 text-sm">
                <div className="flex flex-wrap justify-between gap-2">
                  <span>
                    {wechatTaskLabels[t.command] ?? t.command}
                  </span>
                  <span className="text-muted-foreground">
                    {taskNames[t.status] ?? t.status}
                  </span>
                </div>
                <code className="block break-all text-xs text-muted-foreground">
                  {t.id}
                </code>
                <p className="text-xs text-muted-foreground">
                  {new Date(t.created_at).toLocaleString()} · 回复
                  {t.delivery === "accepted"
                    ? "已被微信接口接受"
                    : t.delivery === "suppressed"
                      ? "结果无变化，已安静保留"
                      : t.delivery === "pending"
                        ? "待发送"
                        : t.delivery === "attachment_failed"
                          ? "附件未发送"
                          : t.delivery === "attachment_unknown"
                            ? "附件发送待核对"
                            : t.delivery === "not_sent"
                              ? "未发送"
                              : "状态待核对"}
                </p>
                {t.delivery_error && (
                  <p className="text-xs text-muted-foreground">
                    {t.delivery_error}
                  </p>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>
      <dialog
        ref={dialog}
        onCancel={(e) => {
          e.preventDefault();
          void close();
        }}
        className="max-h-[88vh] w-[calc(100%-2rem)] max-w-md overflow-auto rounded-xl border border-border bg-card p-6 text-foreground shadow-2xl backdrop:bg-black/50"
        aria-label={forget ? "解除本机微信绑定" : "扫码连接微信"}
      >
        <div className="flex items-start justify-between gap-3">
          <h2 className="font-serif text-xl font-semibold">
            {forget ? "解除本机绑定" : "扫码连接微信"}
          </h2>
          <button
            className="rounded p-1 hover:bg-secondary"
            disabled={busy}
            aria-label="关闭微信绑定"
            onClick={() => void close()}
          >
            <X className="h-4 w-4" />
          </button>
        </div>
        {forget ? (
          <>
            <p className="my-5 text-sm text-muted-foreground">
              移除本机加密的微信凭据和该通道任务索引。已生成文件与现有任务会保留；微信端授权请在手机管理。
            </p>
            {query.isError && (
              <p role="alert" className="mb-4 text-sm text-destructive-strong">
                暂时无法核对绑定。请关闭此窗口，刷新微信状态后再试。
              </p>
            )}
            <button
              className="hb-btn-outline"
              disabled={connectionActionsDisabled || !data?.bound}
              onClick={async () => {
                if (await act("wechat_forget")) setForget(false);
              }}
            >
              确认解除本机绑定
            </button>
          </>
        ) : (
          <div className="mt-5 space-y-4">
            {pair.data?.qr_data_url && (
              <img
                src={pair.data.qr_data_url}
                alt="微信绑定二维码，请用本人微信扫码"
                className="mx-auto h-60 w-60 rounded-lg bg-white p-2"
              />
            )}
            <p role="status" className="text-center text-sm">
              {pair.isError
                ? "绑定状态读取失败，请重试"
                : (pair.data?.message ?? "正在读取二维码…")}
            </p>
            {pair.data?.state === "need_pin" && (
              <form
                onSubmit={async (e) => {
                  e.preventDefault();
                  if (await act("wechat_login_pin", { pin })) setPin("");
                }}
                className="space-y-3"
              >
                <label className="block text-sm">
                  手机配对码
                  <input
                    autoComplete="off"
                    inputMode="numeric"
                    type="password"
                    aria-label="手机配对码"
                    value={pin}
                    onChange={(e) => setPin(e.target.value)}
                    maxLength={12}
                    className="mt-2 block w-full rounded-md border border-border bg-background p-3"
                  />
                </label>
                <button
                  className="hb-btn-primary"
                  disabled={busy || !/^\d{4,12}$/.test(pin)}
                >
                  提交配对码
                </button>
              </form>
            )}
            {pair.data && ["expired", "error"].includes(pair.data.state) && (
              <button
                className="hb-btn-outline"
                disabled={busy}
                onClick={() => void begin()}
              >
                重新获取二维码
              </button>
            )}
            <div className="flex justify-end">
              <button
                className="hb-btn-outline"
                disabled={busy}
                onClick={() => void close()}
              >
                {pair.data?.state === "confirmed" ? "完成" : "取消"}
              </button>
            </div>
          </div>
        )}
        {error && (
          <p role="alert" className="mt-3 text-sm text-destructive-strong">
            {error}
          </p>
        )}
      </dialog>
    </div>
  );
}

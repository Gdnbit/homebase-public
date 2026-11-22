import { useEffect, useRef, useState } from "react";
import { Mic, Square, X, RotateCcw, LoaderCircle } from "lucide-react";
import {
  ensureDraft,
  useDraftState,
  voiceRequest,
  savedVoice,
  type VoiceRecord,
} from "@/lib/assistant-drafts";

export function AssistantVoice({
  scope,
  disabled,
  onApplied,
  onBusy,
}: {
  scope: string | null;
  disabled: boolean;
  onApplied?: () => void;
  onBusy: (busy: boolean) => void;
}) {
  const workspace = useDraftState(scope);
  const latest = workspace.voices.at(-1);
  const record =
    latest && !["applied", "dismissed"].includes(latest.phase)
      ? latest
      : undefined;
  const [state, setState] = useState<
    "idle" | "permission" | "recording" | "working" | "checking" | "error"
  >("idle");
  const [error, setError] = useState("");
  const [seconds, setSeconds] = useState(0);
  const recorder = useRef<MediaRecorder | null>(null);
  const stream = useRef<MediaStream | null>(null);
  const audio = useRef<Blob | null>(null);
  const generation = useRef(0);
  const cancelledCaptures = useRef(new Set<number>());
  const elapsed = useRef<ReturnType<typeof setInterval> | null>(null);
  const alive = useRef(true);
  const recordRef = useRef(record);
  recordRef.current = record;
  const pollRef = useRef<{ id: string; generation: number } | null>(null);
  const appliedRef = useRef(onApplied);
  appliedRef.current = onApplied;
  const release = () => {
    recorder.current = null;
    stream.current?.getTracks().forEach((track) => track.stop());
    stream.current = null;
    clearInterval(elapsed.current ?? undefined);
    elapsed.current = null;
  };
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      generation.current++;
      if (recorder.current?.state === "recording") recorder.current.stop();
      release();
      onBusy(false);
      // Navigation stops capture only. Accepted ASR and all durable receipts stay.
    };
  }, []);
  const valid = (g: number) => alive.current && g === generation.current;
  useEffect(() => {
    // A manual workspace refresh can resolve a previously unknown remote state.
    // Do not reset live capture/work or hide a fresh insertion error on the same phase.
    if (
      state === "error" &&
      latest &&
      ["ready", "applied", "dismissed"].includes(latest.phase)
    ) {
      setState("idle");
      setError("");
      onBusy(false);
    }
  }, [latest?.phase]);
  const pause = (delay = 1500) =>
    new Promise((resolve) => setTimeout(resolve, delay));
  const fromReply = (
    reply: Awaited<ReturnType<typeof voiceRequest>>,
    id?: string,
  ) =>
    id
      ? reply.entry?.voices?.find((v) => v.recording_id === id)
      : reply.entry?.voices?.at(-1);

  async function apply(id: string, g = generation.current) {
    setState("working");
    onBusy(true);
    setError("");
    try {
      await voiceRequest(scope, "apply", { recordingId: id });
      if (valid(g)) {
        audio.current = null;
        setState("idle");
        appliedRef.current?.();
      }
    } catch (e) {
      if (valid(g)) {
        await ensureDraft(scope, true);
        if (savedVoice(scope, id)?.phase === "applied") {
          setState("idle");
          setError("");
          appliedRef.current?.();
        } else {
          setError(String(e));
          setState("error");
        }
      }
    } finally {
      if (valid(g)) onBusy(false);
    }
  }
  async function query(id: string, g: number, autoApply = false) {
    if (pollRef.current?.id === id && pollRef.current.generation === g) return;
    pollRef.current = { id, generation: g };
    setState(autoApply ? "working" : "checking");
    if (autoApply) onBusy(true);
    setError("");
    try {
      const until = Date.now() + 180_000;
      let failures = 0;
      await pause();
      while (valid(g) && Date.now() < until) {
        let current: VoiceRecord | undefined;
        try {
          const reply = await voiceRequest(scope, "get", { recordingId: id });
          current = fromReply(reply, id);
          if (!current) throw Error("原录音回执未返回，未重新提交。");
          if (reply.status === "partial")
            throw Error("暂不能读取原任务进度，回执保留。");
          failures = 0;
        } catch (e) {
          if (!valid(g)) return;
          if (++failures >= 3) throw e;
          await pause();
          continue;
        }
        if (!valid(g)) return;
        if (current.phase === "ready") {
          if (autoApply) await apply(id, g);
          else setState("idle");
          return;
        }
        if (!autoApply && current.phase === "unknown" && current.job) {
          // A detached local worker can still publish its original result.
          await pause(3000);
          continue;
        }
        if (
          ["failed", "cancelled", "unknown", "cancel_requested"].includes(
            current.phase,
          )
        ) {
          throw Error(current.message || "原转写状态需要核对，未重新提交。");
        }
        if (["applied", "dismissed"].includes(current.phase)) {
          setState("idle");
          return;
        }
        await pause();
      }
      if (valid(g)) throw Error("转写尚未返回，可查询原任务，不会重复提交。");
    } catch (e) {
      if (valid(g)) {
        setError(String(e));
        setState("error");
      }
    } finally {
      if (pollRef.current?.id === id && pollRef.current.generation === g)
        pollRef.current = null;
      if (autoApply && valid(g)) onBusy(false);
    }
  }
  useEffect(() => {
    if (!workspace.loaded || state !== "idle" || !record) return;
    const recoverable =
      ["submitting", "accepted"].includes(record.phase) ||
      (record.phase === "unknown" && record.job);
    if (!recoverable) return;
    // A loaded receipt is checked once per mount or recording change.
    void query(record.recording_id, generation.current);
  }, [workspace.loaded, record?.recording_id]);

  async function submit(id: string, g: number, autoApply = true) {
    setState("working");
    onBusy(true);
    setError("");
    try {
      const reply = await voiceRequest(scope, "submit", { recordingId: id });
      if (!valid(g)) return;
      const current = fromReply(reply, id);
      if (current?.phase === "unknown")
        throw Error(current.message || "转写受理未知，原请求保留。");
      await query(id, g, autoApply);
    } catch (e) {
      if (valid(g)) {
        setError(String(e));
        setState("error");
        await ensureDraft(scope, true);
      }
    } finally {
      if (valid(g)) onBusy(false);
    }
  }
  async function transcribe(blob: Blob, g: number) {
    setState("working");
    onBusy(true);
    setError("");
    try {
      const bytes = new Uint8Array(await blob.arrayBuffer());
      if (!bytes.length || bytes.length > 10 * 1024 * 1024)
        throw Error("录音为空或超过 10 MiB，请缩短录音。");
      let binary = "";
      for (let i = 0; i < bytes.length; i += 8192)
        binary += String.fromCharCode(...bytes.subarray(i, i + 8192));
      if (!valid(g)) return;
      const reply = await voiceRequest(scope, "begin", {
        audioBase64: btoa(binary),
        mimeType: blob.type,
      });
      const created = fromReply(reply);
      if (!created) throw Error("录音暂存未确认，请核对原输入，不会启动转写。");
      audio.current = null;
      // A late staging response must preserve the owned recording, not discard it.
      if (valid(g)) await submit(created.recording_id, g);
      else if (alive.current && cancelledCaptures.current.delete(g)) {
        // Explicit cancel before the staging response differs from navigation.
        await voiceRequest(scope, "cancel", {
          recordingId: created.recording_id,
        });
        await voiceRequest(scope, "dismiss", {
          recordingId: created.recording_id,
        });
      }
    } catch (e) {
      if (valid(g)) {
        setError(String(e));
        setState("error");
        await ensureDraft(scope, true);
      }
    } finally {
      if (valid(g)) onBusy(false);
    }
  }
  async function start() {
    if (!workspace.loaded || disabled || record) return;
    const g = ++generation.current;
    setState("permission");
    onBusy(true);
    setError("");
    setSeconds(0);
    try {
      if (
        !navigator.mediaDevices?.getUserMedia ||
        typeof MediaRecorder === "undefined"
      )
        throw Error("当前环境不支持录音，请使用文字或文本附件。");
      const media = await navigator.mediaDevices.getUserMedia({ audio: true });
      if (!valid(g)) {
        media.getTracks().forEach((track) => track.stop());
        return;
      }
      stream.current = media;
      const mime = [
        "audio/webm;codecs=opus",
        "audio/ogg;codecs=opus",
        "audio/webm",
      ].find((m) => MediaRecorder.isTypeSupported(m));
      if (!mime) throw Error("当前环境没有可用的录音格式，请使用文字输入。");
      const instance = new MediaRecorder(media, { mimeType: mime });
      const chunks: BlobPart[] = [];
      instance.ondataavailable = (event) => {
        if (event.data.size) chunks.push(event.data);
      };
      instance.onstop = () => {
        // The browser can deliver an old stop event after a new capture starts.
        if (!valid(g) || recorder.current !== instance) return;
        release();
        const blob = new Blob(chunks, { type: mime });
        audio.current = blob;
        void transcribe(blob, g);
      };
      instance.onerror = () => {
        if (!valid(g) || recorder.current !== instance) return;
        release();
        generation.current++;
        setState("error");
        setError("录音中断，请重新录制。");
        onBusy(false);
      };
      recorder.current = instance;
      instance.start();
      setState("recording");
      let count = 0;
      elapsed.current = setInterval(() => {
        setSeconds(++count);
        if (count >= 120 && instance.state === "recording") instance.stop();
      }, 1000);
    } catch (e) {
      if (valid(g)) {
        release();
        setState("error");
        setError(
          e instanceof DOMException && e.name === "NotAllowedError"
            ? "未获得麦克风权限，请在系统或应用设置中允许录音。"
            : String(e),
        );
        onBusy(false);
      }
    }
  }
  async function cancel() {
    cancelledCaptures.current.add(generation.current);
    if (cancelledCaptures.current.size > 32)
      cancelledCaptures.current.delete(
        cancelledCaptures.current.values().next().value!,
      );
    const g = ++generation.current;
    if (recorder.current?.state === "recording") recorder.current.stop();
    release();
    onBusy(false);
    audio.current = null;
    const current = recordRef.current;
    if (!current) {
      setState("idle");
      setError("");
      return;
    }
    setState("working");
    try {
      const reply = await voiceRequest(scope, "cancel", {
        recordingId: current.recording_id,
      });
      if (!valid(g)) return;
      const result = fromReply(reply, current.recording_id);
      if (result?.phase === "cancelled")
        await voiceRequest(scope, "dismiss", {
          recordingId: current.recording_id,
        });
      else throw Error(result?.message || "取消未确认，原回执仍保留。");
      setState("idle");
      setError("");
    } catch (e) {
      if (valid(g)) {
        setState("error");
        setError(String(e));
      }
    }
  }
  async function dismiss() {
    const current = recordRef.current;
    if (!current) return;
    const g = ++generation.current;
    try {
      await voiceRequest(scope, "dismiss", {
        recordingId: current.recording_id,
      });
      if (valid(g)) {
        setState("idle");
        setError("");
        onBusy(false);
      }
    } catch (e) {
      if (valid(g)) {
        setState("error");
        setError(String(e));
      }
    }
  }
  if (state === "idle" && !record)
    return (
      <button
        type="button"
        className="chat-icon"
        aria-label="语音输入"
        title="录音后转为可编辑文字"
        disabled={disabled || !workspace.loaded}
        onClick={() => void start()}
      >
        <Mic size={20} />
      </button>
    );
  return (
    <div className="chat-voice" role="status" aria-live="polite">
      {state === "recording" ? (
        <>
          <span className="chat-record-dot" />
          <span className="tabular-nums">
            {Math.floor(seconds / 60)
              .toString()
              .padStart(2, "0")}
            :{(seconds % 60).toString().padStart(2, "0")}
          </span>
          <span>正在录音</span>
          <button type="button" onClick={() => recorder.current?.stop()}>
            <Square size={13} />
            停止并转写
          </button>
        </>
      ) : (
        <>
          {state === "checking" ? (
            <>
              <LoaderCircle size={16} className="animate-spin" />
              <span>正在查询原转写…</span>
              {record?.phase === "unknown" && record.message && (
                <span className="max-w-sm text-xs">{record.message}</span>
              )}
            </>
          ) : state === "working" || state === "permission" ? (
            <>
              <LoaderCircle size={16} className="animate-spin" />
              <span>
                {state === "permission"
                  ? "等待麦克风权限…"
                  : "正在核对本地转写…"}
              </span>
            </>
          ) : (
            <span className="max-w-sm text-xs">
              {error ||
                record?.message ||
                (record?.phase === "ready"
                  ? "转写已完成，尚未插入"
                  : "有一段未完成录音，原回执已保留")}
            </span>
          )}
          {record?.phase === "ready" && state !== "working" && state !== "checking" && (
            <>
              <details className="max-w-sm text-xs">
                <summary>查看转写文字</summary>
                <p className="mt-2 max-h-32 overflow-auto whitespace-pre-wrap">
                  {record.transcript}
                </p>
              </details>
              <button
                type="button"
                onClick={() => void apply(record.recording_id)}
              >
                插入草稿
              </button>
            </>
          )}
          {state !== "working" && state !== "permission" && (
            <>
              {state !== "checking" && record &&
                !["ready", "cancelled", "failed"].includes(record.phase) && (
                  <button
                    type="button"
                    aria-label="重试转写"
                    onClick={() =>
                      record.phase === "staged"
                        ? void submit(record.recording_id, generation.current)
                        : void query(record.recording_id, generation.current)
                    }
                  >
                    <RotateCcw size={14} />
                    {record.phase === "staged" ? "继续转写" : "查询原转写"}
                  </button>
                )}
              {!record && audio.current && (
                <button
                  type="button"
                  aria-label="重试转写"
                  onClick={() =>
                    void transcribe(audio.current!, generation.current)
                  }
                >
                  重试暂存录音
                </button>
              )}
              {record && (
                <button type="button" onClick={() => void dismiss()}>
                  停止显示（保留回执）
                </button>
              )}
            </>
          )}
        </>
      )}
      <button
        type="button"
        aria-label="取消语音输入"
        onClick={() => void cancel()}
      >
        <X size={16} />
      </button>
    </div>
  );
}

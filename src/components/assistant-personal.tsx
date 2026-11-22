import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { X } from "lucide-react";
import {
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";
import {
  assistantApi,
  needsReminderAttention,
  type PersonalAssistantState,
} from "@/lib/assistant";

const field =
  "w-full rounded-lg border border-border bg-background px-3 py-2 text-sm";
const button =
  "rounded-lg border border-border px-3 py-2 text-sm hover:bg-secondary disabled:opacity-50";
const delivery = new Map([
  ["scheduled", "等待到期"],
  ["pending", "等待发送到微信"],
  ["claimed", "微信发送中"],
  ["accepted", "微信已受理"],
  ["unknown", "发送结果待核对"],
  ["desktop_due", "已到期"],
  ["dismissed", "已确认"],
  ["canceled", "已取消"],
]);
export function AssistantPersonal({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null),
    client = useQueryClient();
  const data = useQuery({
    queryKey: ["assistant", "personal"],
    queryFn: assistantApi.personal,
    staleTime: 60_000,
    retry: false,
  });
  const [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const [notifications, setNotifications] = useState(
    () => localStorage.getItem("homebase.personal.notifications") === "on",
  );
  const notificationError = useQuery<string>({
    queryKey: ["assistant", "personal-notification-error"],
    enabled: false,
    initialData: "",
  });
  const [memory, setMemory] = useState<{
    id: string | null;
    title: string;
    content: string;
    expires: string;
  }>({ id: null, title: "", content: "", expires: "" });
  const [reminder, setReminder] = useState({ text: "", due: "", interval: "" });
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  async function change(fn: () => Promise<unknown>) {
    setBusy(true);
    setError("");
    try {
      await fn();
      await client.invalidateQueries({ queryKey: ["assistant", "personal"] });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  function editMemory(row: PersonalAssistantState["memories"][number]) {
    if (row.expires_at && Number.isNaN(Date.parse(row.expires_at))) {
      setError("原到期日期无效，请重新设置");
      setMemory({
        id: row.id,
        title: row.title,
        content: row.content,
        expires: "",
      });
      return;
    }
    setMemory({
      id: row.id,
      title: row.title,
      content: row.content,
      expires: row.expires_at
        ? new Date(
            new Date(row.expires_at).getTime() -
              new Date(row.expires_at).getTimezoneOffset() * 60000,
          )
            .toISOString()
            .slice(0, 16)
        : "",
    });
  }
  return (
    <dialog
      ref={dialog}
      onCancel={onClose}
      className="max-h-[85vh] w-[min(760px,94vw)] overflow-y-auto rounded-xl border border-border bg-card p-6 text-foreground shadow-xl backdrop:bg-black/35"
    >
      <div className="flex items-center justify-between">
        <h2 className="font-serif text-xl">个人记忆与提醒</h2>
        <button
          className={button}
          onClick={onClose}
          aria-label="关闭个人助理管理"
        >
          <X size={16} />
        </button>
      </div>
      <p className="mt-3 text-sm text-muted-foreground">
        桌面与微信共用，加密保存在本机。记忆不构成操作授权；应用关闭时提醒不运行，重新打开后检查到期项目。
      </p>
      {data.data?.warning && (
        <p role="alert" className="mt-3 text-sm text-destructive">
          {data.data.warning}
        </p>
      )}
      {(error || data.error || notificationError.data) && (
        <p role="alert" className="mt-3 text-sm text-destructive">
          {error || (data.error ? String(data.error) : notificationError.data)}
        </p>
      )}
      <button
        className={`${button} mt-3`}
        disabled={busy}
        onClick={() =>
          void change(async () => {
            if (notifications) {
              localStorage.setItem("homebase.personal.notifications", "off");
              setNotifications(false);
              return;
            }
            const allowed =
              (await isPermissionGranted()) ||
              (await requestPermission()) === "granted";
            localStorage.setItem(
              "homebase.personal.notifications",
              allowed ? "on" : "off",
            );
            if (!allowed) throw Error("未授予系统通知权限，应用内提醒仍然有效");
            setNotifications(true);
            client.setQueryData(
              ["assistant", "personal-notification-error"],
              "",
            );
          })
        }
      >
        {notifications ? "关闭系统提醒" : "启用系统提醒"}
      </button>
      {data.isPending && (
        <p role="status" className="mt-4 text-sm">
          正在读取个人助理数据…
        </p>
      )}
      {data.data && (
        <>
          <section className="mt-5">
            <h3 className="font-serif text-lg">已登记的助理 Skill</h3>
            <div className="mt-3 grid gap-3 sm:grid-cols-3">
              {data.data.skills?.map((skill) => (
                <article
                  key={skill.id}
                  className="rounded-xl border border-border p-3"
                >
                  <strong className="text-sm">{skill.name}</strong>
                  <p className="mt-2 text-xs text-muted-foreground">
                    {skill.description} · v{skill.version}
                  </p>
                </article>
              ))}
            </div>
            <p className="mt-2 text-xs text-muted-foreground">
              按需读取已审查的说明；不会扫描本机其他 Skill 或执行任意脚本。
            </p>
          </section>
          <section className="mt-6">
            <h3 className="font-serif text-lg">长期记忆</h3>
            <p className="mt-1 text-xs text-muted-foreground">
              新任务会有限召回相关偏好，到期记忆不参与检索。同标题内容变化时请编辑原记录。删除会清除该记忆的更新历史，聊天记录单独保留。
            </p>
            <div className="mt-3 grid gap-3 sm:grid-cols-2">
              {data.data.memories.map((row) => (
                <article
                  key={row.id}
                  className="rounded-xl border border-border p-4"
                >
                  <div className="flex justify-between gap-2">
                    <strong className="text-sm">{row.title}</strong>
                    <span className="text-xs text-muted-foreground">
                      {
                        {
                          active: "有效",
                          expired: "已到期",
                          invalid: "日期无效",
                        }[row.status]
                      }
                    </span>
                  </div>
                  <p className="mt-2 whitespace-pre-wrap break-words text-sm">
                    {row.content}
                  </p>
                  {!!row.previous?.length && (
                    <details className="mt-2 text-xs text-muted-foreground">
                      <summary className="cursor-pointer">更新历史（{row.previous.length}）</summary>
                      {row.previous.map((old, i) => (
                        <p key={`${old.updated_at}-${i}`} className="mt-2 whitespace-pre-wrap break-words">
                          {new Date(old.updated_at).toLocaleString()} · {old.content}
                        </p>
                      ))}
                    </details>
                  )}
                  <div className="mt-3 flex gap-2">
                    <button
                      className={button}
                      disabled={busy}
                      onClick={() => editMemory(row)}
                    >
                      编辑
                    </button>
                    <button
                      className={button}
                      disabled={busy}
                      onClick={() =>
                        void change(() =>
                          assistantApi.memoryForget(
                            row.id,
                            data.data!.revision,
                          ),
                        )
                      }
                    >
                      删除记忆
                    </button>
                  </div>
                </article>
              ))}
            </div>
            {!data.data.memories.length && (
              <p className="mt-3 text-sm text-muted-foreground">
                尚无长期记忆。也可以在微信告诉助理“请记住…”，确认后保存。
              </p>
            )}
            <form
              className="mt-4 space-y-3"
              onSubmit={(e) => {
                e.preventDefault();
                void change(async () => {
                  await assistantApi.memorySave({
                    revision: data.data!.revision,
                    id: memory.id,
                    title: memory.title,
                    content: memory.content,
                    expires_at: memory.expires
                      ? new Date(memory.expires).toISOString()
                      : null,
                  });
                  setMemory({ id: null, title: "", content: "", expires: "" });
                });
              }}
            >
              <label className="block text-sm">
                记忆标题
                <input
                  required
                  maxLength={100}
                  className={`${field} mt-1`}
                  value={memory.title}
                  onChange={(e) =>
                    setMemory({ ...memory, title: e.target.value })
                  }
                />
              </label>
              <label className="block text-sm">
                记忆内容
                <textarea
                  required
                  maxLength={2000}
                  className={`${field} mt-1`}
                  value={memory.content}
                  onChange={(e) =>
                    setMemory({ ...memory, content: e.target.value })
                  }
                />
              </label>
              <label className="block text-sm">
                到期时间（可选，本地时间）
                <input
                  type="datetime-local"
                  className={`${field} mt-1`}
                  value={memory.expires}
                  onChange={(e) =>
                    setMemory({ ...memory, expires: e.target.value })
                  }
                />
              </label>
              <div className="flex gap-2">
                <button className={button} disabled={busy}>
                  {memory.id ? "保存记忆修改" : "添加记忆"}
                </button>
                {memory.id && (
                  <button
                    type="button"
                    className={button}
                    onClick={() =>
                      setMemory({
                        id: null,
                        title: "",
                        content: "",
                        expires: "",
                      })
                    }
                  >
                    取消编辑
                  </button>
                )}
              </div>
            </form>
          </section>
          <section className="mt-7 border-t border-border pt-5">
            <h3 className="font-serif text-lg">提醒与重复计划</h3>
            <div className="mt-3 space-y-3">
              {data.data.reminders.map((row) => (
                <article
                  key={row.id}
                  className="rounded-xl border border-border p-4"
                >
                  <p className="text-sm">{row.text}</p>
                  <p className="mt-2 text-xs text-muted-foreground">
                    {new Date(row.due_at).toLocaleString()} ·{" "}
                    {row.source === "wechat" ? "微信" : "桌面"} ·{" "}
                    {delivery.get(row.delivery) ?? "状态待核对"}
                    {row.interval_minutes
                      ? ` · 每 ${row.interval_minutes} 分钟`
                      : ""}
                  </p>
                  <div className="mt-3 flex gap-2">
                    {row.enabled && (
                      <button
                        className={button}
                        disabled={busy}
                        onClick={() =>
                          void change(() =>
                            assistantApi.reminderAck(row.id, true),
                          )
                        }
                      >
                        取消提醒
                      </button>
                    )}
                    {needsReminderAttention(row.delivery) && (
                      <button
                        className={button}
                        disabled={busy}
                        onClick={() =>
                          void change(() =>
                            assistantApi.reminderAck(row.id, false),
                          )
                        }
                      >
                        已知悉
                      </button>
                    )}
                  </div>
                </article>
              ))}
            </div>
            {!data.data.reminders.length && (
              <p className="mt-3 text-sm text-muted-foreground">
                尚无提醒。微信创建的提醒返回原绑定微信；这里新建的提醒显示在桌面。
              </p>
            )}
            <form
              className="mt-4 space-y-3"
              onSubmit={(e) => {
                e.preventDefault();
                void change(async () => {
                  await assistantApi.reminderSave({
                    revision: data.data!.revision,
                    id: null,
                    text: reminder.text,
                    due_at: new Date(reminder.due).toISOString(),
                    interval_minutes: reminder.interval
                      ? Number(reminder.interval)
                      : null,
                    enabled: true,
                  });
                  setReminder({ text: "", due: "", interval: "" });
                });
              }}
            >
              <label className="block text-sm">
                提醒内容
                <input
                  className={`${field} mt-1`}
                  required
                  maxLength={1000}
                  value={reminder.text}
                  onChange={(e) =>
                    setReminder({ ...reminder, text: e.target.value })
                  }
                />
              </label>
              <label className="block text-sm">
                提醒时间（本地时间）
                <input
                  type="datetime-local"
                  required
                  className={`${field} mt-1`}
                  value={reminder.due}
                  onChange={(e) =>
                    setReminder({ ...reminder, due: e.target.value })
                  }
                />
              </label>
              <label className="block text-sm">
                重复间隔（分钟，留空仅一次）
                <input
                  type="number"
                  min={1}
                  max={525600}
                  className={`${field} mt-1`}
                  value={reminder.interval}
                  onChange={(e) =>
                    setReminder({ ...reminder, interval: e.target.value })
                  }
                />
              </label>
              <button className={button} disabled={busy}>
                添加桌面提醒
              </button>
            </form>
          </section>
          <section className="mt-6">
            <h3 className="font-serif text-lg">主动办事</h3>
            <p className="mt-1 text-xs text-muted-foreground">
              在聊天中说清时间、具体事项和次数，确认后到时处理并返回结果。仅使用已启用的免费渠道；应用关闭时不执行，额外权限仍需确认。
            </p>
            <div className="mt-3 grid gap-3 sm:grid-cols-2">
              {data.data.followups?.entries.map((row) => (
                <article key={row.id} className="rounded-xl border border-border p-4">
                  <div className="flex justify-between gap-2">
                    <strong className="text-sm">{row.title}</strong>
                    <span className="text-xs text-muted-foreground">
                      {row.state === "running" && row.runs.at(-1)?.state === "prepared"
                        ? "等待受理"
                        : (({ scheduled: "等待执行", running: "正在处理", completed: "已完成", paused: "需要处理", blocked_model: "配置已变化", missed: "已过执行窗口", canceled: "已取消" } as Record<string, string>)[row.state] ?? "状态待核对")}
                    </span>
                  </div>
                  <p className="mt-2 whitespace-pre-wrap break-words text-sm">{row.instruction}</p>
                  {row.matter_id && <p className="mt-2 text-xs text-muted-foreground">关联事项：{data.data.matters?.entries.find((m) => m.id === row.matter_id)?.title ?? "原事项待核对"}</p>}
                  <p className="mt-2 text-xs text-muted-foreground">
                    {new Date(row.due_at).toLocaleString()} · 已受理 {row.runs.filter((run) => run.turn_id).length}/{row.max_runs} 次
                    {row.interval_minutes ? ` · 每 ${row.interval_minutes} 分钟` : ""}
                  </p>
                  <p className="mt-2 text-xs text-muted-foreground">{row.notice}</p>
                  {row.source === "wechat" && row.notice_delivery && (
                    <p className="mt-2 text-xs text-muted-foreground">
                      微信通知 · {delivery.get(row.notice_delivery) ?? "状态待核对"}
                    </p>
                  )}
                  {row.runs.map((run) => (
                    <p key={run.ordinal} className="mt-2 text-xs text-muted-foreground">
                      第 {run.ordinal} 次：{({ completed: "已完成", awaiting_confirmation: "等你确认", running: "处理中", queued: "已排队", prepared: "准备受理", not_submitted: "未受理", failed: "未完成", needs_input: "需要补充", paused_budget: "暂到预算", canceled: "已停止", interrupted_unknown: "需要核对原任务" } as Record<string, string>)[run.state] ?? "待核对"}
                    </p>
                  ))}
                  {row.enabled && (
                    <button className={`${button} mt-3`} disabled={busy} onClick={() => void change(() => assistantApi.followupCancel(row.id, data.data!.followups!.revision))}>
                      取消后续执行
                    </button>
                  )}
                </article>
              ))}
            </div>
            {!data.data.followups?.entries.length && <p className="mt-3 text-sm text-muted-foreground">尚未安排主动任务。普通提醒只提示，到时办事会实际调用助理。</p>}
          </section>
          <section className="mt-6 border-t border-border pt-5">
            <h3 className="font-serif text-lg">持续事项</h3>
            <p className="mt-1 text-xs text-muted-foreground">在聊天中说“持续跟进这件事”，确认后保存目标、进展和下一步。复查只提醒；原任务的实际状态单独显示，完成一项任务不会自动关闭整件事。</p>
            <div className="mt-3 grid gap-3 sm:grid-cols-2">
              {data.data.matters?.entries.map((row) => (
                <article key={row.id} className="rounded-xl border border-border p-4">
                  <div className="flex justify-between gap-2"><strong className="text-sm">{row.title}</strong><span className="text-xs text-muted-foreground">{({ active: "跟进中", waiting: "等待条件", paused: "已暂停", completed: "已结束", canceled: "已取消" } as Record<string, string>)[row.state] ?? "待核对"}</span></div>
                  <p className="mt-2 whitespace-pre-wrap break-words text-sm">{row.goal}</p>
                  <p className="mt-2 whitespace-pre-wrap break-words text-xs text-muted-foreground">上次确认进展：{row.summary || "尚未记录"}</p>
                  <p className="mt-2 whitespace-pre-wrap break-words text-sm">下一步：{row.next_action || "待决定"}</p>
                  {row.latest_task && <p className="mt-2 text-xs text-muted-foreground">关联任务：{({ completed: "已完成", awaiting_confirmation: "等你确认", running: "处理中", queued: "已排队", waiting_jobs: "等待原结果", needs_input: "需要补充", interrupted_unknown: "结果需核对", failed: "未完成", canceled: "已停止" } as Record<string, string>)[row.latest_task.state] ?? "待核对"}{row.latest_task.progress?.current ? ` · ${row.latest_task.progress.current}` : ""}</p>}
                  {row.review_at && <p className="mt-2 text-xs text-muted-foreground">复查：{new Date(row.review_at).toLocaleString()} · {delivery.get(row.delivery) ?? (row.delivery === "none" ? "未安排" : "状态待核对")}</p>}
                  {!!row.previous.length && <details className="mt-2 text-xs text-muted-foreground"><summary className="cursor-pointer">事项更新历史（{row.previous.length}）</summary>{row.previous.map((old, i) => <p key={`${old.updated_at}-${i}`} className="mt-2 whitespace-pre-wrap break-words">{new Date(old.updated_at).toLocaleString()} · {old.summary} · 下一步：{old.next_action}</p>)}</details>}
                  {needsReminderAttention(row.delivery) && <button className={`${button} mt-3`} disabled={busy} onClick={() => void change(() => assistantApi.matterReviewAck(row.id, data.data!.matters!.revision))}>已知悉本次复查</button>}
                </article>
              ))}
            </div>
            {!data.data.matters?.entries.length && <p className="mt-3 text-sm text-muted-foreground">尚无持续事项。可以在微信创建，桌面会看到同一份记录。</p>}
          </section>
        </>
      )}
      <button
        className={`${button} mt-6`}
        disabled={busy}
        onClick={() => void data.refetch()}
      >
        刷新个人助理数据
      </button>
    </dialog>
  );
}

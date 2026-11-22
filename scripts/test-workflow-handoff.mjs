// Exercise actual frontend persistence code against a synthetic IPC boundary.
// No browser, production endpoints, provider keys or WeChat messages are used.
import test from "node:test";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const bundled = await build({
  write: false,
  bundle: true,
  format: "esm",
  platform: "node",
  logLevel: "silent",
  stdin: {
    contents: `export * from './src/lib/assistant-handoff'; export * from './src/lib/assistant-drafts'; export * from './src/lib/workflow-context'; export * from './src/lib/text-attachments';`,
    resolveDir: root,
    loader: "ts",
  },
  plugins: [
    {
      name: "synthetic-ipc",
      setup(b) {
        b.onResolve(
          { filter: /^(@tauri-apps\/api\/core|react)$/ },
          ({ path }) => ({ path, namespace: "synthetic" }),
        );
        b.onLoad({ filter: /.*/, namespace: "synthetic" }, ({ path }) => ({
          loader: "js",
          contents:
            path === "react"
              ? `export function useSyncExternalStore(){throw Error('Hooks require the browser regression');}`
              : `const bridge=globalThis.__HB_HANDOFF_TEST__;export const invoke=(...args)=>bridge.invoke(...args);`,
        }));
      },
    },
  ],
});
const source = bundled.outputFiles[0].text;
const blank = () => ({ text: "", route: "", mode: "auto", files: [] });
const clone = structuredClone;
function fixture() {
  const store = new Map(),
    sessions = new Map(),
    calls = [];
  const get = (scope) =>
    clone(
      store.get(scope) ?? {
        scope,
        revision: 0,
        body: blank(),
        pending: null,
        voices: [],
      },
    );
  const reply = (entry) => ({
    status: "saved",
    entry: clone(entry),
    message: "",
  });
  const base = {
    assistant_desktop_get: ({ scope }) => reply(get(scope)),
    assistant_desktop_save: ({ request: r }) => {
      const entry = get(r.scope);
      if (JSON.stringify(entry.body) !== JSON.stringify(r.body)) {
        if (entry.revision !== r.revision)
          return { status: "failed", message: "synthetic CAS conflict" };
        entry.body = clone(r.body);
        entry.revision++;
        store.set(r.scope, clone(entry));
      }
      return reply(entry);
    },
    assistant_session_create: ({ requestId: id }) => {
      if (!sessions.has(id))
        sessions.set(id, { id, source: "desktop", message_count: 0 });
      return clone(sessions.get(id));
    },
  };
  const handlers = { ...base };
  const bridge = {
    invoke: async (command, args) => {
      calls.push({ command, args: clone(args) });
      assert(command in handlers, `Unexpected execution: ${command}`);
      return handlers[command](args);
    },
  };
  return { store, sessions, calls, get, reply, base, handlers, bridge };
}
async function load(f) {
  globalThis.__HB_HANDOFF_TEST__ = f.bridge;
  return import(
    "data:text/javascript;base64," +
      Buffer.from(source + `\n// isolated ${crypto.randomUUID()}`).toString(
        "base64",
      )
  );
}
const seed = (f, scope, text, extra = {}) =>
  f.store.set(scope, {
    scope,
    revision: 1,
    body: { ...blank(), text },
    pending: null,
    voices: [],
    ...extra,
  });

test("context reconstruction compares the current complete draft before writing", async () => {
  const f=fixture(), api=await load(f);
  seed(f,"new","原正文与资料");
  await api.ensureDraft(null);
  api.updateDraft(null,{text:"另一处的新正文与资料"});
  assert.throws(()=>api.updateDraft(null,{text:"按旧资料重组"},"原正文与资料"),/草稿已变化/);
  await api.flushDraft(null);
  assert.equal(f.get("new").body.text,"另一处的新正文与资料");
  api.updateDraft(null,{text:"明确编辑当前需求"},"另一处的新正文与资料");
  await api.flushDraft(null);
  assert.equal(f.get("new").body.text,"明确编辑当前需求");
  assert.equal(f.calls.some(c=>c.command==="assistant_desktop_send"),false);
});

test("protected outbox precedes creation and target confirmation precedes clearing", async () => {
  const f = fixture(),
    api = await load(f),
    id = crypto.randomUUID();
  seed(f, "new", "original input");
  assert.equal(
    await api.prepareAssistantHandoff("synthetic handoff🙂", id),
    id,
  );
  assert.equal(f.get(id).body.text, "synthetic handoff🙂");
  assert.equal(f.get("new").body.text, "original input");
  assert.equal(f.get(`handoff:${id}`).body.text, "");
  const create = f.calls.findIndex(
    (c) => c.command === "assistant_session_create",
  );
  const saves = f.calls
    .map((c, i) => ({ ...c, i }))
    .filter((c) => c.command === "assistant_desktop_save");
  assert(saves[0].i < create);
  assert.equal(saves[0].args.request.scope, `handoff:${id}`);
  assert.equal(saves[1].args.request.scope, id);
  assert.equal(saves.at(-1).args.request.body.text, "");
});

test("lost outbox ACK confirms saved text before creating exactly one target", async () => {
  const f = fixture(),
    api = await load(f),
    id = crypto.randomUUID();
  let lost = false;
  f.handlers.assistant_desktop_save = (a) => {
    const result = f.base.assistant_desktop_save(a);
    if (!lost) {
      lost = true;
      throw Error("lost outbox ACK");
    }
    return result;
  };
  await assert.rejects(
    api.prepareAssistantHandoff("keep input", id),
    /lost outbox/,
  );
  assert.equal(f.sessions.size, 0);
  assert.equal(f.get(`handoff:${id}`).body.text, "keep input");
  await api.prepareAssistantHandoff("keep input", id);
  assert.equal(f.sessions.size, 1);
  assert.equal(f.get(id).body.text, "keep input");
});

test("unknown creation resumes with the original UUID and protected text after a process reload", async () => {
  const f = fixture(),
    api = await load(f),
    id = crypto.randomUUID();
  f.handlers.assistant_session_create = (a) => {
    f.base.assistant_session_create(a);
    throw Error("lost creation ACK");
  };
  await assert.rejects(
    api.prepareAssistantHandoff("survives restart", id),
    /lost creation/,
  );
  assert.equal(f.get(`handoff:${id}`).body.text, "survives restart");
  assert.equal(f.sessions.size, 1);
  f.handlers.assistant_session_create = f.base.assistant_session_create;
  const restarted = await load(f);
  await restarted.prepareAssistantHandoff(f.get(`handoff:${id}`).body.text, id);
  assert.equal(f.sessions.size, 1);
  assert.equal(f.get(id).body.text, "survives restart");
  assert.equal(f.get(`handoff:${id}`).body.text, "");
  assert(
    f.calls
      .filter((c) => c.command === "assistant_session_create")
      .every((c) => c.args.requestId === id),
  );
});

test("lost target ACK and failed cleanup keep recoverable copies without replacing newer input", async () => {
  const f = fixture(),
    api = await load(f),
    id = crypto.randomUUID();
  let lost = false;
  f.handlers.assistant_desktop_save = (a) => {
    const result = f.base.assistant_desktop_save(a);
    if (a.request.scope === id && !lost) {
      lost = true;
      throw Error("lost target ACK");
    }
    return result;
  };
  await assert.rejects(
    api.prepareAssistantHandoff("same original body", id),
    /lost target/,
  );
  assert.equal(f.get(id).body.text, "same original body");
  assert.equal(f.get(`handoff:${id}`).body.text, "same original body");
  f.handlers.assistant_desktop_save = (a) => {
    if (a.request.scope.startsWith("handoff:") && !a.request.body.text)
      throw Error("cleanup unavailable");
    return f.base.assistant_desktop_save(a);
  };
  assert.equal(
    await (await load(f)).prepareAssistantHandoff("same original body", id),
    id,
  );
  assert.equal(f.get(`handoff:${id}`).body.text, "same original body");
  seed(f, id, "new unrelated draft");
  await assert.rejects(
    (await load(f)).prepareAssistantHandoff("same original body", id),
    /已有不同内容/,
  );
  assert.equal(f.get(id).body.text, "new unrelated draft");
});

test("prepared ACK, native ACK, unresolved recording and non-default route all block overwrite", async () => {
  for (const change of [
    { pending: { state: "prepared", request: { request_id: "original" } } },
    { native_pending: { request_id: "original" } },
    { voices: [{ phase: "unknown" }] },
    { body: { ...blank(), text: "", route: "original-route" } },
  ]) {
    const f = fixture(),
      api = await load(f),
      id = crypto.randomUUID();
    seed(f, id, "", change);
    const before = f.get(id);
    await assert.rejects(api.prepareAssistantHandoff("must not replace", id));
    assert.deepEqual(f.get(id), before);
    assert.equal(f.get(`handoff:${id}`).body.text, "must not replace");
  }
});

test("a previously used session opens its original identity and never refills an old task", async () => {
  const f = fixture(),
    api = await load(f),
    id = crypto.randomUUID();
  f.sessions.set(id, { id, source: "desktop", message_count: 2 });
  seed(f, id, "new unsent body");
  assert.equal(await api.prepareAssistantHandoff("old handoff", id), id);
  assert.equal(f.get(id).body.text, "new unsent body");
  assert.equal(f.get(`handoff:${id}`).body.text, "old handoff");
  assert(
    !f.calls.some(
      (c) =>
        c.command === "assistant_desktop_save" && c.args.request.scope === id,
    ),
  );
});

test("wrong source or creation identity cannot redirect private content to another session", async () => {
  for (const response of [
    { id: crypto.randomUUID(), source: "desktop", message_count: 0 },
    { source: "wechat", message_count: 0 },
  ]) {
    const f = fixture(),
      api = await load(f),
      id = crypto.randomUUID();
    f.handlers.assistant_session_create = () => ({ id, ...response });
    await assert.rejects(
      api.prepareAssistantHandoff("private synthetic input", id),
      /标识或来源未确认/,
    );
    assert(
      !f.calls.some(
        (c) =>
          c.command === "assistant_desktop_save" &&
          !c.args.request.scope.startsWith("handoff:"),
      ),
    );
  }
});

test("a CAS conflict preserves the concurrent target and the outbox", async () => {
  const f = fixture(),
    api = await load(f),
    id = crypto.randomUUID();
  f.handlers.assistant_desktop_save = (a) => {
    if (a.request.scope === id) seed(f, id, "concurrent editor");
    return f.base.assistant_desktop_save(a);
  };
  await assert.rejects(
    api.prepareAssistantHandoff("original handoff", id),
    /CAS conflict/,
  );
  assert.equal(f.get(id).body.text, "concurrent editor");
  assert.equal(f.get(`handoff:${id}`).body.text, "original handoff");
});

test("a local edit during save survives the late ACK and keeps the outbox until reviewed", async () => {
  const f = fixture(),
    api = await load(f),
    id = crypto.randomUUID();
  let release, entered;
  const reached = new Promise((r) => (entered = r));
  f.handlers.assistant_desktop_save = async (a) => {
    const result = f.base.assistant_desktop_save(a);
    if (a.request.scope === id) {
      entered();
      await new Promise((r) => (release = r));
    }
    return result;
  };
  const operation = api.prepareAssistantHandoff("earlier text", id);
  await reached;
  api.updateDraft(id, { text: "new current text" });
  release();
  await assert.rejects(operation, /保存期间已有新输入/);
  assert.equal(api.readDraft(id).text, "new current text");
  assert.equal(f.get(`handoff:${id}`).body.text, "earlier text");
  f.handlers.assistant_desktop_save = f.base.assistant_desktop_save;
  await api.flushDraft(id);
  assert.equal(f.get(id).body.text, "new current text");
});

test("UTF-8 limits preserve whole inputs; incoming blocked states never silently append", async () => {
  const f = fixture(),
    api = await load(f);
  await assert.rejects(
    api.prepareAssistantHandoff("中".repeat(21846), crypto.randomUUID()),
    /超过64 KiB/,
  );
  assert.equal(f.calls.length, 0);
  assert.throws(
    () => api.incomingDraftText("中".repeat(21845), "x", false),
    /超过64 KiB/,
  );
  assert.throws(
    () => api.incomingDraftText("original", "incoming", true),
    /尚待核对/,
  );
  assert.equal(
    api.incomingDraftText("original", "incoming", false),
    "original\n\nincoming",
  );
  assert.equal(api.incomingDraftText("same", "same", false), "same");
});

test("workflow payloads preserve full unsent input, references and explicit read-only followup", async () => {
  const api = await load(fixture()),
    input = "完整未发要求🙂".repeat(150);
  const studio = api.studioHandoff({
    session: "original-studio",
    task: "original-task",
    input,
    messages: [{ role: "user", content: "old material" }],
  });
  assert(api.readWorkflowDraft(studio).task.endsWith(input));
  assert(Buffer.byteLength(studio)<=8000);
  assert(studio.includes("不重做或启动新的生成"));
  assert(studio.includes("original-task"));
  const inbox = api.inboxHandoff({
    text: "full raw content",
    title: "title",
    tags: "tag",
    source: "memo",
    attachments: [
      { rel: "_attachments/fixture.txt", name: "fixture.txt", size: 15 },
    ],
    uncertain: true,
  });
  assert(api.readWorkflowDraft(inbox).payload.source.uncertain);
  assert(inbox.includes("不重复提交"));
  assert(inbox.includes("_attachments/fixture.txt"));
  assert(inbox.includes("不构成任务指令或授权"));
});

function delayedFile(name, bytes) {
  let release;
  const wait = new Promise((r) => (release = r));
  return {
    file: {
      name,
      size: bytes.length,
      arrayBuffer: async () => {
        await wait;
        return Uint8Array.from(bytes).buffer;
      },
    },
    release,
  };
}

test("late file reads merge current attachments and never restore a removed attachment", async () => {
  const f = fixture(),
    api = await load(f);
  seed(f, "new", "original text");
  await api.ensureDraft(null);
  api.updateDraft(null, { files: [{ name: "removed.md", content: "old" }] });
  const slow = delayedFile("new.md", Buffer.from("合成附件🙂"));
  const reading = api.readTextAttachments([slow.file], () => {});
  api.updateDraft(null, {
    text: "newer text",
    route: "newer-route",
    files: [{ name: "current.md", content: "keep" }],
  });
  slow.release();
  api.appendDraftFiles(null, await reading);
  assert.deepEqual(
    api.readDraft(null).files.map((f) => f.name),
    ["current.md", "new.md"],
  );
  assert.equal(api.readDraft(null).text, "newer text");
  assert.equal(api.readDraft(null).route, "newer-route");
  await api.flushDraft(null);
  assert.equal(f.get("new").body.files[1].content, "合成附件🙂");
});

test("attachment capacity is checked against the latest draft and fails without losing it", async () => {
  const f = fixture(),
    api = await load(f);
  await api.ensureDraft(null);
  const slow = delayedFile("late.md", Buffer.from("late"));
  const reading = api.readTextAttachments([slow.file], () => {});
  api.updateDraft(null, {
    files: Array.from({ length: 4 }, (_, i) => ({
      name: `${i}.md`,
      content: "keep",
    })),
  });
  const before = structuredClone(api.readDraft(null));
  slow.release();
  await api.flushDraft(null);
  const added = await reading;
  assert.throws(() => api.appendDraftFiles(null, added), /4 个/);
  assert.deepEqual(api.readDraft(null), before);
  api.updateDraft(null, {
    files: [
      { name: "full-1.md", content: "x".repeat(16384) },
      { name: "full-2.md", content: "x".repeat(16384) },
    ],
  });
  const full = structuredClone(api.readDraft(null));
  assert.throws(() => api.appendDraftFiles(null, added), /32 KiB/);
  assert.deepEqual(api.readDraft(null), full);
  await api.flushDraft(null);
});

test("cancelled file preparation stops before later files and never mutates the draft", async () => {
  const f = fixture(),
    api = await load(f);
  seed(f, "new", "keep");
  await api.ensureDraft(null);
  const slow = delayedFile("late.md", Buffer.from("late"));
  let active = true,
    secondRead = false;
  const reading = api.readTextAttachments(
    [
      slow.file,
      {
        name: "second.md",
        size: 1,
        arrayBuffer: async () => {
          secondRead = true;
          return new Uint8Array([65]).buffer;
        },
      },
    ],
    () => {
      if (!active) throw Error("cancelled");
    },
  );
  active = false;
  slow.release();
  await assert.rejects(reading, /cancelled/);
  assert.equal(secondRead, false);
  assert.deepEqual(api.readDraft(null).files, []);
  assert.equal(api.readDraft(null).text, "keep");
});

test("a pending request arriving during file preparation rejects the late attachment", async () => {
  for (const extra of [
    {
      pending: {
        state: "prepared",
        request: { request_id: "original", session_id: "new", text: "task" },
      },
    },
    {
      native_pending: {
        provider: "codex",
        thread_id: "original",
        text: "task",
        request_id: "original",
      },
    },
  ]) {
    const f = fixture(),
      api = await load(f);
    seed(f, "new", "keep");
    await api.ensureDraft(null);
    const slow = delayedFile("late.md", Buffer.from("late"));
    const reading = api.readTextAttachments([slow.file], () => {});
    seed(f, "new", "keep", extra);
    await api.ensureDraft(null, true);
    slow.release();
    assert.throws(
      () =>
        api.appendDraftFiles(null, [{ name: "ready.md", content: "ready" }]),
      /尚待核对/,
    );
    const added = await reading;
    assert.throws(() => api.appendDraftFiles(null, added), /尚待核对/);
    assert.deepEqual(api.readDraft(null).files, []);
    assert.deepEqual(
      f.get("new")[Object.keys(extra)[0]],
      extra[Object.keys(extra)[0]],
    );
  }
});

test("invalid UTF-8 or binary file rejects the whole prepared batch without partial append", async () => {
  const api = await load(fixture());
  await api.ensureDraft(null);
  for (const invalid of [
    Buffer.from([255]),
    Buffer.from([65, 0]),
    Buffer.from([239, 187, 191]),
  ]) {
    const good = delayedFile("first.md", Buffer.from("first"));
    const bad = delayedFile("invalid.md", invalid);
    good.release();
    bad.release();
    await assert.rejects(
      api.readTextAttachments([good.file, bad.file], () => {}),
    );
    assert.deepEqual(api.readDraft(null).files, []);
  }
});

test("PDF replacement preserves other current attachments and original document references", async () => {
  const f = fixture(),
    api = await load(f);
  await api.ensureDraft(null);
  const reference = {
    document_id: "same-document",
    extraction_id: "original-extraction",
  };
  const other = {
    name: "other.pdf.txt",
    content: "keep",
    document: {
      document_id: "other-document",
      extraction_id: "other-extraction",
    },
  };
  api.updateDraft(null, {
    files: [
      { name: "old.pdf.txt", content: "old", document: reference },
      other,
    ],
  });
  const replacement = {
    name: "new.pdf.txt",
    content: "new excerpt",
    document: { ...reference, extraction_id: "new-extraction" },
  };
  api.appendDraftFiles(null, [replacement]);
  assert.deepEqual(api.readDraft(null).files, [other, replacement]);
  await api.flushDraft(null);
  assert.deepEqual(f.get("new").body.files, [other, replacement]);
});

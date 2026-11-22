import { test, expect, type Page } from "@playwright/test";

const id="opaque_receipt_test",scope=`native:codex:${id}`;
async function enter(page:Page){
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.evaluate(({id,scope})=>{
    const p=(window as any).__performance;
    if(!p.desktopStore.has(scope))p.desktopStore.set(scope,{scope,revision:1,body:{text:"合成待发原稿",mode:"auto",route:"",files:[]},pending:null,native_pending:null,voices:[]});
    p.nativeSends=[];
    p.values.conversation_native=({request:r}:any)=>{
      if(r.action==="list")return {data:[]};
      if(r.action==="read")return {thread:{id,turns:[]}};
      if(r.action==="status")return {id,state:"idle",items:{}};
      if(r.action==="send"){p.nativeSends.push(r);return {state:"accepted",thread_id:id,turn:{id:"original-turn"}};}
      throw Error("Unexpected synthetic native mutation");
    };
  },{id,scope});
  await page.getByLabel("更多对话操作").click();
  await page.getByRole("button",{name:"未发送与待核对",exact:true}).click();
  await page.getByRole("button",{name:/codex · opaque_r/}).click();
  await expect(page.getByLabel("给 Codex 发消息",{exact:true})).toBeEnabled();
}
async function reloadProtected(page:Page){
  const seed=await page.evaluate(()=>{const p=(window as any).__performance;return {desktop:Object.fromEntries(p.desktopStore),receipts:Object.fromEntries(p.nativeReceipts)};});
  await page.addInitScript(seed=>{(window as any).__desktopSeed=seed.desktop;(window as any).__nativeReceiptSeed=seed.receipts;},seed);
  await enter(page);
}
test.beforeEach(async({page})=>{
  await page.route("**/*",r=>new URL(r.request().url()).hostname==="127.0.0.1"?r.continue():r.abort());
});

test("edits during an explicit native draft save retry are protected when its reply arrives", async ({page}) => {
  await page.clock.install();
  await enter(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const save = p.values.assistant_desktop_save;
    p.values.assistant_desktop_save = async (args: any) => {
      const reply = save(args);
      if (args.request.body.text === "NATIVE FIRST") throw Error("ISOLATED_NATIVE_SAVE_ACK_LOST");
      if (args.request.body.text === "NATIVE SECOND") await new Promise<void>(resolve => { p.releaseDraftSave = resolve; });
      return reply;
    };
  });
  const input = page.getByLabel("给 Codex 发消息", {exact:true});
  await input.fill("NATIVE FIRST");
  await page.clock.runFor(950);
  const retry = page.getByRole("button", {name:"重试保存", exact:true});
  await expect(retry).toBeVisible();
  await input.fill("NATIVE SECOND");
  await retry.click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releaseDraftSave)).toBe("function");
  await input.fill("NATIVE THIRD");
  await page.evaluate(() => (window as any).__performance.releaseDraftSave());
  await expect(page.getByLabel("原生草稿保存状态")).toContainText("草稿已受保护保存");
  expect(await page.evaluate(scope => {
    const p = (window as any).__performance;
    return {text:p.desktopStore.get(scope).body.text, sends:p.nativeSends, saves:p.calls.filter((c:string) => c === "assistant_desktop_save").length};
  }, scope)).toEqual({text:"NATIVE THIRD", sends:[], saves:3});
});

test("native session resumes Enter after an interrupted composition loses focus", async ({ page }) => {
  await enter(page);
  const input = page.getByLabel("给 Codex 发消息", { exact: true });
  const draft = "只交给原 Codex 会话的草稿";
  await input.fill(draft);
  await input.dispatchEvent("compositionstart");
  await page.getByRole("button", { name: "刷新原生会话", exact: true }).focus();
  await expect(input).not.toBeFocused();
  await expect(input).toHaveValue(draft);
  expect(await page.evaluate(() => (window as any).__performance.nativeSends)).toEqual([]);

  await input.press("Enter");
  await expect.poll(() => page.evaluate(() => (window as any).__performance.nativeSends.map((request: any) => ({ id: request.thread_id, text: request.text })))).toEqual([{ id, text: draft }]);
});

test("protected preparation failure prevents native send and keeps editable input",async({page})=>{
  await enter(page);
  await page.evaluate(()=>{(window as any).__performance.values.assistant_native_send=()=>({status:"failed",code:"NATIVE_PREPARE_FAILED",message:"合成磁盘不可写，未执行"});});
  await page.getByRole("button",{name:"发送",exact:true}).click();
  await expect(page.getByRole("alert")).toContainText("未执行");
  await expect(page.getByLabel("给 Codex 发消息",{exact:true})).toHaveValue("合成待发原稿");
  expect(await page.evaluate(()=>(window as any).__performance.nativeSends)).toEqual([]);
  await expect(page.getByLabel("原生发送回执")).toHaveCount(0);
});

test("a lost response recovers the durable original acknowledgement without resending",async({page})=>{
  await enter(page);
  await page.evaluate(()=>{const p=(window as any).__performance,original=p.values.assistant_native_send;p.values.assistant_native_send=async(args:any)=>{await original(args);throw Error("合成ACK响应丢失");};});
  await page.getByRole("button",{name:"发送",exact:true}).click();
  await expect(page.getByRole("alert")).toContainText("响应丢失");
  await page.getByRole("button",{name:"查询原请求",exact:true}).click();
  await expect(page.getByLabel("原生发送回执")).toHaveCount(0);
  await expect(page.getByLabel("给 Codex 发消息",{exact:true})).toHaveValue("");
  expect(await page.evaluate(()=>(window as any).__performance.nativeSends.length)).toBe(1);
});

test("restart queries an unknown original, explicit retry retains the same identity and changed text cannot replace it",async({page})=>{
  await enter(page);
  await page.evaluate(()=>{const p=(window as any).__performance;p.values.conversation_native=({request:r}:any)=>{
    if(r.action==="send"){p.nativeSends.push(r);throw Error("合成提交响应未知");}
    if(r.action==="read")return {thread:{id:r.thread_id,turns:[]}};
    if(r.action==="status")return {id:r.thread_id,state:"idle",items:{}};
    return {data:[]};
  };});
  await page.getByRole("button",{name:"发送",exact:true}).click();
  await expect(page.getByLabel("原生发送回执")).toBeVisible();
  const original=await page.evaluate(scope=>(window as any).__performance.desktopStore.get(scope).native_pending,scope);
  await reloadProtected(page);
  await expect(page.getByLabel("原生草稿保存状态")).toContainText("尚未查到原生请求");
  expect(await page.evaluate(()=>(window as any).__performance.nativeSends)).toEqual([]);
  const input=page.getByLabel("给 Codex 发消息",{exact:true});
  await input.fill("后来编辑的新稿");
  await expect(page.getByLabel("原生草稿保存状态")).toContainText("草稿已受保护保存");
  await page.getByRole("button",{name:"发送",exact:true}).click();
  await expect(page.getByRole("alert")).toContainText("原请求尚待核对");
  expect(await page.evaluate(()=>(window as any).__performance.nativeSends)).toEqual([]);
  await page.getByRole("button",{name:"重试原请求",exact:true}).click();
  await expect(page.getByLabel("原生发送回执")).toHaveCount(0);
  await expect(input).toHaveValue("后来编辑的新稿");
  const sent=await page.evaluate(()=>(window as any).__performance.nativeSends);
  expect(sent).toHaveLength(1);expect(sent[0].request_id).toBe(original.request_id);expect(sent[0].text).toBe(original.text);
});

test("refresh settles the original receipt and preserves a newer draft without mutation",async({page})=>{
  const request={provider:"codex",thread_id:id,text:"已经提交的原文",request_id:"persisted-original"};
  await page.addInitScript(({scope,request})=>{(window as any).__desktopSeed={[scope]:{scope,revision:4,body:{text:"新的未发内容",mode:"auto",route:"",files:[]},native_pending:request,pending:null,voices:[]}};},{scope,request});
  await enter(page);
  await expect(page.getByLabel("原生草稿保存状态")).toContainText("尚未查到");
  await page.evaluate(request=>{(window as any).__performance.nativeReceipts.set(`codex:${request.request_id}`,{request,result:{state:"accepted",thread_id:request.thread_id,turn:{id:"original-turn"}}});},request);
  await page.getByRole("button",{name:"刷新原生会话",exact:true}).click();
  await expect(page.getByLabel("原生发送回执")).toHaveCount(0);
  await expect(page.getByLabel("给 Codex 发消息",{exact:true})).toHaveValue("新的未发内容");
  expect(await page.evaluate(()=>(window as any).__performance.nativeSends)).toEqual([]);
});

test("native version conflicts require explicit comparison",async({page})=>{
  await enter(page);
  await page.evaluate(scope=>{const p=(window as any).__performance,e=p.desktopStore.get(scope);e.body.text="另一窗口原稿";e.revision++;},scope);
  const input=page.getByLabel("给 Codex 发消息",{exact:true});await input.fill("本窗口输入");
  await expect(page.getByRole("alert")).toContainText("另一个窗口");
  await page.getByRole("button",{name:"查看已保存版本",exact:true}).click();
  await expect(page.getByLabel("已保存原生正文")).toHaveValue("另一窗口原稿");
  await expect(input).toHaveValue("本窗口输入");
  await page.getByRole("button",{name:"使用已保存版本",exact:true}).click();
  await expect(input).toHaveValue("另一窗口原稿");
  expect(await page.evaluate(()=>(window as any).__performance.nativeSends)).toEqual([]);
});

test("adopting a stored version cannot erase an unresolved native identity",async({page})=>{
  const request={provider:"codex",thread_id:id,text:"未知原生消息",request_id:"must-keep-original"};
  await page.addInitScript(({scope,request})=>{(window as any).__desktopSeed={[scope]:{scope,revision:2,body:{text:request.text,mode:"auto",route:"",files:[]},pending:null,native_pending:request,voices:[]}};},{scope,request});
  await enter(page);
  await expect(page.getByLabel("原生草稿保存状态")).toContainText("尚未查到");
  await page.evaluate(scope=>{const e=(window as any).__performance.desktopStore.get(scope);e.revision++;e.body.text="另一窗口版本";},scope);
  await page.getByLabel("给 Codex 发消息",{exact:true}).fill("本地未保存输入");
  await expect(page.getByRole("alert")).toContainText("另一个窗口");
  await page.getByRole("button",{name:"查看已保存版本",exact:true}).click();
  await page.getByRole("button",{name:"使用已保存版本",exact:true}).click();
  await expect(page.getByRole("alert")).toContainText("不能通过替换草稿清除其回执");
  await expect(page.getByLabel("给 Codex 发消息",{exact:true})).toHaveValue("本地未保存输入");
  expect(await page.evaluate(scope=>(window as any).__performance.desktopStore.get(scope).native_pending.request_id,scope)).toBe(request.request_id);
});

test("settling an ACK during a remote-body conflict does not grant permission to overwrite that body",async({page})=>{
  const request={provider:"codex",thread_id:id,text:"原文待核对",request_id:"ack-with-remote-edit"};
  await page.addInitScript(({scope,request})=>{(window as any).__desktopSeed={[scope]:{scope,revision:2,body:{text:request.text,mode:"auto",route:"",files:[]},pending:null,native_pending:request,voices:[]}};},{scope,request});
  await enter(page);
  await expect(page.getByLabel("原生草稿保存状态")).toContainText("尚未查到");
  await page.evaluate(({scope,request})=>{
    const p=(window as any).__performance,e=p.desktopStore.get(scope);e.body.text="另一窗口的新草稿";e.revision++;
    p.nativeReceipts.set(`codex:${request.request_id}`,{request,result:{state:"accepted",thread_id:request.thread_id,turn:{id:"original-turn"}}});
  },{scope,request});
  const input=page.getByLabel("给 Codex 发消息",{exact:true});await input.fill("本地未保存的新草稿");
  await page.getByRole("button",{name:"查询原请求",exact:true}).click();
  await expect(page.getByLabel("原生发送回执")).toHaveCount(0);
  await expect(input).toHaveValue("本地未保存的新草稿");
  await expect(page.getByRole("alert")).toContainText("另一窗口");
  await page.getByRole("button",{name:"重试保存",exact:true}).click();
  expect(await page.evaluate(scope=>(window as any).__performance.desktopStore.get(scope).body.text,scope)).toBe("另一窗口的新草稿");
});

for (const mismatch of ["draft scope", "accepted thread", "pending request"] as const) {
  test(`native receipt with a different ${mismatch} preserves the original request`, async ({ page }) => {
    const request = { provider: "codex", thread_id: id, text: "原来的待核对消息", request_id: "original-native-receipt" };
    await page.addInitScript(({ scope, request }) => {
      (window as any).__desktopSeed = { [scope]: { scope, revision: 2, body: { text: request.text, mode: "auto", route: "", files: [] },
        native_pending: request, pending: null, voices: [] } };
    }, { scope, request });
    await enter(page);
    await expect(page.getByLabel("原生草稿保存状态")).toContainText("尚未查到");
    await page.evaluate(({ scope, request, mismatch }) => {
      const p = (window as any).__performance;
      p.readOriginalReceipt = p.values.assistant_native_resolve;
      p.values.assistant_native_resolve = () => {
        const entry = structuredClone(p.desktopStore.get(scope));
        entry.revision++;
        entry.body.text = "外来的草稿正文";
        const foreign = { ...request, request_id: "other-native-receipt" };
        let native_result = { state: "accepted", thread_id: request.thread_id, assistant_session_id: "other-shared-session" };
        if (mismatch === "draft scope") {
          entry.scope = "native:gemini-cli:other";
          entry.native_pending = { ...foreign, provider: "gemini-cli", thread_id: "other" };
        } else if (mismatch === "accepted thread") {
          entry.native_pending = null;
          native_result.thread_id = "other";
        } else {
          entry.native_pending = foreign;
          native_result = { ...native_result, state: "unknown" };
        }
        return { status: "fixture", entry, native_result };
      };
    }, { scope, request, mismatch });
    const editor = page.getByLabel("给 Codex 发消息", { exact: true });
    await editor.fill("原会话后来编辑的草稿");
    const original = page.getByLabel("原生发送回执");
    await original.getByRole("button", { name: "查询原请求" }).click();
    await expect(page.getByRole("alert")).toContainText("不一致");
    await expect(original).toContainText(request.request_id.slice(0, 12));
    await expect(editor).toHaveValue("原会话后来编辑的草稿");
    await expect(page.getByRole("link", { name: "查看共享执行任务、结果与待确认操作" })).toHaveCount(0);

    await page.evaluate((request) => {
      const p = (window as any).__performance;
      p.values.assistant_native_resolve = p.readOriginalReceipt;
      p.nativeReceipts.set(`codex:${request.request_id}`, { request, result: { state: "accepted", thread_id: request.thread_id,
        turn: { id: "original-turn" }, assistant_session_id: "original-shared-session" } });
    }, request);
    await original.getByRole("button", { name: "查询原请求" }).click();
    await expect(original).toHaveCount(0);
    await expect(editor).toHaveValue("原会话后来编辑的草稿");
    await expect(page.getByRole("link", { name: "查看共享执行任务、结果与待确认操作" })).toHaveAttribute("href", "/assistant?session=original-shared-session");
    expect(await page.evaluate(() => (window as any).__performance.nativeSends)).toEqual([]);
  });
}

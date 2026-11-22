import {test,expect,type Page} from '@playwright/test';
const id='presentation_test',scope=`native:codex:${id}`;
async function open(page:Page,items:any[],metadata:any={},initialStatus: "idle" | "pending" | "error" = "idle", provider = "codex"){
  await page.goto('/evals/ai-control-ui/performance.html');
  await page.evaluate(({id,scope,items,metadata,initialStatus})=>{
    const p=(window as any).__performance;
    p.nativeActions=[];p.creates=0;
    p.nativeReply={thread:{id,turns:[{id:'saved-turn',items}]},...metadata};
    p.nativeStatus={id,state:'idle',items:{},item_order:[]};
    p.nativeStatusBehavior=initialStatus;
    p.desktopStore.set(scope,{scope,revision:1,body:{text:'合成未发草稿',route:'',mode:'auto',files:[]},pending:null,voices:[]});
    p.values.conversation_native=({request:r}:any)=>{p.nativeActions.push(r);
      if(r.action==='read')return r.cursor&&p.older?new Promise(resolve=>p.finishOlder=()=>resolve(p.older)):p.nativeReply;
      if(r.action==='status'){
        if(p.nativeStatusBehavior==='pending')return new Promise(resolve=>p.finishNativeStatus=()=>resolve(p.nativeStatus));
        if(p.nativeStatusBehavior==='error')throw Error('合成原生状态读取失败');
        return p.nativeStatus;
      }
      if(r.action==='list')return {data:[]};
      throw Error('Synthetic native mutation forbidden');
    };
    const createSession=p.values.assistant_session_create;
    p.values.assistant_session_create=(request:any)=>{p.creates++;return createSession(request);};
    p.values.assistant_session_get=({sessionId}:any)=>({id:sessionId,source:'desktop',messages:[],turns:[]});
  },{id,scope:provider === "codex" ? scope : `native:${provider}:${id}`,items,metadata,initialStatus});
  await page.getByLabel('更多对话操作').click();
  await page.getByRole('button',{name:'未发送与待核对',exact:true}).click();
  await page.getByRole('button',{name:new RegExp(`${provider} · presenta`)}).click();
  await expect(page.getByLabel(`给 ${provider === "claude-code" ? "Claude Code" : "Codex"} 发消息`,{exact:true})).toBeEnabled();
}
const longItems=(start:number,end:number)=>Array.from({length:end-start},(_,n)=>({id:`item-${start+n}`,type:'agentMessage',text:`### 消息${start+n}\n\n${'这是隔离的历史文字。'.repeat(24)}`}));
test.beforeEach(async({page})=>{
  await page.route('**/*',r=>new URL(r.request().url()).hostname==='127.0.0.1'?r.continue():r.abort());
});

test("reading a native turn does not offer to stop unseen work", async ({ page }) => {
  await open(page,[],{},"pending");
  const send=page.getByRole("button",{name:"发送",exact:true});
  const stop=page.getByRole("button",{name:"停止",exact:true});
  await expect.poll(()=>page.evaluate(()=>typeof (window as any).__performance.finishNativeStatus)).toBe("function");
  await expect(send).toBeDisabled();
  await expect(stop).toHaveCount(0);
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.nativeStatusBehavior="idle";
    p.finishNativeStatus();
  });
  await expect(send).toBeEnabled();
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.nativeStatusBehavior="pending";
    p.finishNativeStatus=undefined;
  });
  await page.getByRole("button",{name:"刷新原生会话",exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>typeof (window as any).__performance.finishNativeStatus)).toBe("function");
  await expect(send).toBeDisabled();
  await expect(stop).toHaveCount(0);
  await page.evaluate(()=> (window as any).__performance.finishNativeStatus());
  await expect(send).toBeEnabled();
  expect(await page.evaluate(()=> (window as any).__performance.nativeActions.filter((r:any)=>r.action==="interrupt"))).toEqual([]);
});

test("a failed native status read preserves the draft and retry reads only that status", async ({ page }) => {
  await open(page,[{id:"saved-item",type:"agentMessage",text:"保留已读历史"}],{},"error");
  const send=page.getByRole("button",{name:"发送",exact:true});
  const editor=page.getByLabel("给 Codex 发消息",{exact:true});
  await expect(page.getByText("保留已读历史",{exact:true})).toBeVisible();
  await expect(page.getByRole("alert")).toContainText(/原生状态读取失败/);
  await editor.fill("还在编辑的合成草稿");
  await expect(send).toBeDisabled();
  await editor.press("Enter");
  expect(await page.evaluate(()=> (window as any).__performance.nativeActions.filter((r:any)=>["send","interrupt"].includes(r.action)))).toEqual([]);
  const previous=await page.evaluate(()=> {
    const p=(window as any).__performance;
    p.nativeStatusBehavior="idle";
    return p.nativeActions.length;
  });
  await page.getByRole("button",{name:"重查原生状态",exact:true}).click();
  await expect(send).toBeEnabled();
  await expect(editor).toHaveValue("还在编辑的合成草稿");
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(await page.evaluate(previous=> (window as any).__performance.nativeActions.slice(previous).map((r:any)=>r.action),previous)).toEqual(["status"]);
});

test("a failed poll suspends an active native turn until a fresh status is requested", async ({ page }) => {
  await page.clock.install();
  await open(page, [{ id: "kept-item", type: "agentMessage", text: "已确认的历史回复" }]);
  await expect(page.getByRole("button", { name: "发送", exact: true })).toBeEnabled();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.nativeStatus = { ...p.nativeStatus, state: "inProgress", active_turn_id: "original-turn" };
  });
  await page.getByRole("button", { name: "刷新原生会话", exact: true }).click();
  const transcript = page.getByLabel("原生对话消息");
  await expect(transcript.getByText("正在处理你的事情…")).toBeVisible();
  const editor = page.getByLabel("给 Codex 发消息", { exact: true });
  await editor.fill("轮询失败时继续编辑的草稿");
  await page.evaluate(() => { (window as any).__performance.nativeStatusBehavior = "error"; });
  await page.clock.runFor(1800);
  await expect(page.getByText("原生状态读取失败，草稿仍可编辑。")).toBeVisible();
  await expect(page.getByRole("button", { name: "停止", exact: true })).toHaveCount(0);
  const queries = () => page.evaluate(() => (window as any).__performance.nativeActions.filter((request: any) => request.action === "status"));
  const before = (await queries()).length;
  await page.clock.runFor(5000);
  expect(await queries()).toHaveLength(before);
  await expect(transcript.getByText("正在处理你的事情…")).toHaveCount(0);
  await expect(transcript.getByText("原生任务状态待核对")).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.nativeStatusBehavior = "idle";
    p.nativeStatus = { ...p.nativeStatus, state: "completed", active_turn_id: null };
  });
  await page.getByRole("button", { name: "重查原生状态" }).click();
  await expect(transcript.getByText("这轮原生对话已结束")).toBeVisible();
  await expect(transcript.getByText("已确认的历史回复")).toBeVisible();
  await expect(editor).toHaveValue("轮询失败时继续编辑的草稿");
  expect((await queries())[before]).toMatchObject({ action: "status", provider: "codex", thread_id: id });
  expect(await queries()).toHaveLength(before + 1);
  expect(await page.evaluate(() => (window as any).__performance.nativeActions.filter((r: any) => ["send", "interrupt"].includes(r.action)))).toEqual([]);
});

for (const mismatch of ["thread", "provider", "state"] as const) {
  test(`native refresh does not present an unverified ${mismatch} status`, async ({ page }) => {
    await open(page, [{ id: "saved-item", type: "agentMessage", text: "原会话只显示自己的正文" }]);
    const send = page.getByRole("button", { name: "发送", exact: true });
    await expect(send).toBeEnabled();
    await page.evaluate((kind) => {
      const p = (window as any).__performance;
      p.correctStatus = p.nativeStatus;
      p.nativeStatus = {
        ...p.correctStatus,
        ...(kind === "thread" ? { id: "different-native-thread" }
          : kind === "provider" ? { provider: "gemini-cli" } : { state: "unrecognized" }),
        items: { foreign: { id: "foreign", type: "agentMessage", text: "这条正文没有核对过归属" } },
        item_order: ["foreign"],
      };
    }, mismatch);
    await page.getByRole("button", { name: "刷新原生会话", exact: true }).click();
    await expect(page.getByText("原生状态读取失败，草稿仍可编辑。")).toBeVisible();
    await expect(page.getByText("这条正文没有核对过归属")).toHaveCount(0);
    await expect(send).toBeDisabled();
    await expect(page.getByLabel("给 Codex 发消息")).toHaveValue("合成未发草稿");
    await page.evaluate(() => { const p = (window as any).__performance; p.nativeStatus = p.correctStatus; });
    await page.getByRole("button", { name: "重查原生状态" }).click();
    await expect(send).toBeEnabled();
    await expect(page.getByText("原会话只显示自己的正文")).toBeVisible();
  });
}

test("opening a different native draft never queries the previously selected conversation", async ({ page }) => {
  await open(page,[]);
  const secondId="other_draft";
  const secondScope=`native:claude-code:${secondId}`;
  await page.evaluate(({secondId,secondScope})=>{
    const p=(window as any).__performance;
    p.desktopStore.set(secondScope,{scope:secondScope,revision:1,body:{text:"指定的另一份草稿",route:"",mode:"auto",files:[]},pending:null,voices:[]});
    const previous=p.values.conversation_native;
    p.values.conversation_native=(args:any)=>{
      const request=args.request;
      if(request.thread_id===secondId && request.provider==="claude-code") {
        p.nativeActions.push(request);
        if(request.action==="read")return {thread:{id:secondId,turns:[]}};
        if(request.action==="status")return {id:secondId,state:"idle",items:{}};
      }
      return previous(args);
    };
  },{secondId,secondScope});
  await page.getByRole("link",{name:"AI 助理",exact:true}).click();
  await page.getByLabel("更多对话操作").click();
  await page.getByRole("button",{name:"未发送与待核对",exact:true}).click();
  await page.evaluate(()=>{(window as any).__performance.nativeActions=[];});
  await page.getByRole("button",{name:/claude-code · other_dr/}).click();
  await expect(page.getByLabel("给 Claude Code 发消息",{exact:true})).toHaveValue("指定的另一份草稿");
  await expect.poll(()=>page.evaluate(secondId=>(window as any).__performance.nativeActions.some((x:any)=>x.thread_id===secondId&&x.action==="status"),secondId)).toBe(true);
  expect(await page.evaluate(id=>(window as any).__performance.nativeActions.filter((x:any)=>x.thread_id===id),id)).toEqual([]);
});

test("an already accepted native turn resumes polling after reopening and settles without sending", async ({ page }) => {
  await page.clock.install();
  await page.goto("/evals/ai-control-ui/performance.html");
  const provider = "claude-code";
  const threadId = "accepted_after_reopen";
  const acceptedScope = `native:${provider}:${threadId}`;
  const finalReply = "重新打开后读到原会话的合成结果。";
  await page.evaluate(({ provider, threadId, acceptedScope, finalReply }) => {
    const fixture = (window as any).__performance;
    fixture.nativeActions = [];
    fixture.nativeChecks = 0;
    fixture.desktopStore.set(acceptedScope, {
      scope: acceptedScope, revision: 2,
      body: { text: "新草稿仍在", route: "", mode: "auto", files: [] },
      pending: null, native_pending: null, voices: [],
    });
    const finalItem = { id: "original-reply", type: "agentMessage", text: finalReply };
    fixture.values.conversation_native = ({ request }: any) => {
      if (request.action === "list") return { data: [] };
      if (request.provider !== provider || request.thread_id !== threadId)
        throw new Error("Synthetic query requested a different native identity");
      fixture.nativeActions.push(request);
      if (request.action === "read") {
        return { thread: { id: threadId, turns: [{ id: "original-turn", items: fixture.nativeChecks >= 3 ? [finalItem] : [] }] } };
      }
      if (request.action === "status") {
        fixture.nativeChecks++;
        if (fixture.nativeChecks < 3) {
          return { id: threadId, state: fixture.nativeChecks === 1 ? "accepted" : "inProgress", items: {}, item_order: [] };
        }
        return { id: threadId, state: "completed", items: { "original-reply": finalItem }, item_order: ["original-reply"] };
      }
      throw new Error("Unexpected native mutation");
    };
  }, { provider, threadId, acceptedScope, finalReply });
  await page.getByLabel("更多对话操作").click();
  await page.getByRole("button", { name: "未发送与待核对", exact: true }).click();
  await page.getByRole("button", { name: /claude-code · accepted/ }).click();
  const editor = page.getByLabel("给 Claude Code 发消息", { exact: true });
  await expect(editor).toHaveValue("新草稿仍在");
  await expect(page.getByRole("button", { name: "发送", exact: true })).toBeDisabled();
  await expect(page.getByLabel("原生对话消息").getByText(finalReply, { exact: true })).toBeVisible({ timeout: 7000 });
  await expect.poll(() => page.evaluate(() => (window as any).__performance.nativeActions.filter((request: any) => request.action === "read").length)).toBe(2);
  await expect(page.getByRole("button", { name: "发送", exact: true })).toBeEnabled();
  await expect(editor).toHaveValue("新草稿仍在");
  await page.clock.runFor(3500);
  const actions = await page.evaluate(() => (window as any).__performance.nativeActions.map((request: any) => request.action));
  expect(actions.filter((action: string) => action === "status")).toHaveLength(3);
  expect(actions.filter((action: string) => action === "read")).toHaveLength(2);
  expect(actions.every((action: string) => action === "status" || action === "read")).toBe(true);
});

test('SDK input blocks remain reachable without guessing which text is a system instruction',async({page})=>{
  await open(page,[{id:'user-input',type:'userMessage',content:[
    {type:'text',text:'# AGENTS.md instructions\n这是用户真实输入'},
    {type:'text',text:'HomeBase 自动背景快照\n{"synthetic":true}'},
    {type:'text',text:'第二段也可能是用户正文 <script>window.unsafeNative=true</script>'},
  ]}]);
  const pane=page.getByLabel('原生对话消息');
  await expect(pane.getByText('这是用户真实输入',{exact:true})).toBeVisible();
  await expect(pane.getByRole('heading',{name:'AGENTS.md instructions',exact:true})).toBeVisible();
  const context=pane.locator('pre').filter({hasText:'HomeBase 自动背景快照'});
  await expect(context).toBeHidden();
  await pane.getByText('附带原生输入 · 2 段（完整保留）',{exact:true}).click();
  await expect(context).toBeVisible();
  await expect(pane.locator('pre').filter({hasText:'第二段也可能是用户正文'})).toBeVisible();
  expect(await page.evaluate(()=>(window as any).unsafeNative)).toBeUndefined();
  expect(await page.evaluate(()=>(window as any).__performance.nativeActions.filter((r:any)=>['send','create','interrupt'].includes(r.action)))).toEqual([]);
});

test('assistant paragraphs stay visible and unsupported input is explicit without rendering its payload',async({page})=>{
  await open(page,[{id:'a',type:'agentMessage',content:[{type:'output_text',text:'第一段答复'},{type:'output_text',text:'第二段答复'}]},
    {id:'image-user',type:'userMessage',content:[{type:'image',url:'data:image/png;base64,SYNTHETIC_UNSUPPORTED_PAYLOAD'}]}]);
  const pane=page.getByLabel('原生对话消息');
  await expect(pane.getByText('第一段答复',{exact:true})).toBeVisible();
  await expect(pane.getByText('第二段答复',{exact:true})).toBeVisible();
  await expect(pane.getByText('原生客户端未提供文字内容。',{exact:true})).toBeVisible();
  await expect(pane.getByText(/包含 1 个非文本内容，当前不预览/)).toBeVisible();
  await expect(pane.getByText(/SYNTHETIC_UNSUPPORTED_PAYLOAD/)).toHaveCount(0);
});

test('flattened SDK text uses only byte-matched original blocks and preserves them through live snapshots',async({page})=>{
  const original=['原始用户问题','HomeBase 自动背景快照\n合成资料'];
  const item={id:'flat',type:'userMessage',content:[{type:'text',text:original.join('')}],homebase_input_blocks:original};
  await open(page,[item]);const pane=page.getByLabel('原生对话消息');
  await expect(pane.getByText(original[0],{exact:true})).toBeVisible();
  await expect(pane.locator('pre')).toBeHidden();
  await page.evaluate(({id,item})=>{const p=(window as any).__performance;const {homebase_input_blocks,...raw}=item;p.client.setQueryData(['conversations','codex',id,'live'],{id,state:'completed',items:{flat:raw},item_order:['flat']});},{id,item});
  await expect(pane.getByText(original[0],{exact:true})).toBeVisible();
  await pane.getByText('附带原生输入 · 1 段（完整保留）',{exact:true}).click();
  await expect(pane.locator('pre')).toHaveText(original[1]);
  await page.evaluate(id=>{const p=(window as any).__performance;p.client.setQueryData(['conversations','codex',id,'live'],{id,state:'completed',items:{flat:{id:'flat',type:'userMessage',content:[{type:'text',text:'修改后的真实原文'}],homebase_input_blocks:['伪造','资料']}},item_order:['flat']});},id);
  await expect(pane.getByText('修改后的真实原文',{exact:true})).toBeVisible();
  await expect(pane.locator('details')).toHaveCount(0);
});

test('malformed native content has a visible explanation instead of crashing or inventing text',async({page})=>{
  const errors:string[]=[];page.on('pageerror',e=>errors.push(e.message));
  await open(page,[{id:'bad',type:'userMessage',content:{invalid:'synthetic'}}]);
  await expect(page.getByRole('alert')).toContainText('原生消息格式未能确认');
  expect(errors).toEqual([]);
});

test('new messages follow the bottom but do not interrupt reading older messages or react to status-only changes',async({page})=>{
  await page.setViewportSize({width:1440,height:900});await open(page,longItems(0,25));
  const pane=page.getByLabel('原生对话消息');
  await expect.poll(()=>pane.evaluate(e=>e.scrollHeight-e.clientHeight-e.scrollTop)).toBeLessThan(100);
  await pane.evaluate(e=>{e.scrollTop=0;e.dispatchEvent(new Event('scroll',{bubbles:true}));});
  await expect(page.getByLabel('查看原生最新消息')).toContainText('回到最新');
  await page.evaluate(id=>{const p=(window as any).__performance;p.client.setQueryData(['conversations','codex',id,'live'],{...p.nativeStatus,state:'completed'});},id);
  await expect(page.getByLabel('查看原生最新消息')).toContainText('回到最新');
  await page.evaluate(id=>{const p=(window as any).__performance;p.client.setQueryData(['conversations','codex',id,'live'],{id,state:'completed',items:{new:{id:'new',type:'agentMessage',text:'新到的合成回复'}},item_order:['new']});},id);
  await expect(page.getByLabel('查看原生最新消息')).toContainText('查看新消息');
  expect(await pane.evaluate(e=>e.scrollTop)).toBeLessThan(5);
  await expect(pane.getByText('新到的合成回复',{exact:true})).not.toBeInViewport();
  await page.getByLabel('查看原生最新消息').click();
  await expect(pane.getByText('新到的合成回复',{exact:true})).toBeInViewport();
  await expect(page.getByLabel('查看原生最新消息')).toHaveCount(0);
});

test('prepending history preserves the current reading anchor even after scrolling while the read is pending',async({page})=>{
  await page.setViewportSize({width:1440,height:900});await open(page,longItems(20,40),{next_cursor:'older'});
  const pane=page.getByLabel('原生对话消息');
  await page.evaluate(items=>{(window as any).__performance.older={thread:{id:'presentation_test',turns:[{id:'older',items}]},next_cursor:null};},longItems(0,20));
  await pane.evaluate(e=>{e.scrollTop=0;e.dispatchEvent(new Event('scroll',{bubbles:true}));});
  await page.getByRole('button',{name:'读取更早的原生对话',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>typeof (window as any).__performance.finishOlder)).toBe('function');
  await pane.evaluate(e=>{e.scrollTop=150;e.dispatchEvent(new Event('scroll',{bubbles:true}));});
  const anchor=pane.locator('[data-message-id="item-20"]');const before=(await anchor.boundingBox())!.y;
  await page.evaluate(()=>(window as any).__performance.finishOlder());
  await expect(pane.locator('[data-message-id="item-0"]')).toBeAttached();
  await expect.poll(async()=>Math.abs((await anchor.boundingBox())!.y-before)).toBeLessThan(4);
});

const nativeTurns = (start: number, end: number) => longItems(start, end).map((item) => ({ id: `turn-${item.id}`, status: "completed", items: [item] }));

test("refreshing the latest native window preserves earlier pages and their cursor", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await open(page, [], { thread: { id, turns: nativeTurns(30, 50) }, next_cursor: "30" });
  const pane = page.getByLabel("原生对话消息");
  const earlier = page.getByRole("button", { name: "读取更早的原生对话", exact: true });
  const respondToPage = async (from: number, to: number, cursor: string | null) => {
    await page.evaluate(({ id, turns, cursor }) => {
      const p = (window as any).__performance;
      p.finishOlder = undefined;
      p.older = { thread: { id, turns }, next_cursor: cursor };
    }, { id, turns: nativeTurns(from, to), cursor });
    await earlier.click();
    await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.finishOlder)).toBe("function");
    await page.evaluate(() => (window as any).__performance.finishOlder());
    await expect(pane.locator(`[data-message-id="item-${from}"]`)).toBeAttached();
  };
  await respondToPage(10, 30, "10");
  const anchor = pane.locator('[data-message-id="item-18"]');
  await anchor.scrollIntoViewIfNeeded();
  const before = (await anchor.boundingBox())!.y;
  await page.evaluate(({ id, turns }) => {
    const p = (window as any).__performance;
    p.nativeReply = { thread: { id, turns }, next_cursor: "31" };
  }, { id, turns: nativeTurns(31, 51) });
  await page.getByRole("button", { name: "刷新原生会话", exact: true }).click();
  await expect(pane.locator('[data-message-id="item-50"]')).toBeAttached();
  await expect(pane.locator('[data-message-id="item-10"]')).toBeAttached();
  await expect(pane.locator('[data-message-id="item-30"]')).toBeAttached();
  expect(await pane.locator('[data-message-id="item-31"]').count()).toBe(1);
  await expect.poll(async () => Math.abs((await anchor.boundingBox())!.y - before)).toBeLessThan(4);
  await respondToPage(0, 10, null);
  await expect(earlier).toHaveCount(0);
  const reads = await page.evaluate(() => (window as any).__performance.nativeActions.filter((r: any) => r.action === "read").map((r: any) => r.cursor ?? null));
  expect(reads).toEqual([null, "30", null, "10"]);
  await expect(page.getByLabel("给 Codex 发消息")).toHaveValue("合成未发草稿");
});

test("disjoint native history windows preserve both sides and use the new head cursor to fill the gap", async ({ page }) => {
  await open(page, [], { thread: { id, turns: nativeTurns(30, 50) }, next_cursor: "30" });
  const pane = page.getByLabel("原生对话消息");
  const fixture = async (from: number, to: number, cursor: string | null) => {
    await page.evaluate(({ id, turns, cursor }) => {
      const p = (window as any).__performance;
      p.finishOlder = undefined;
      p.older = { thread: { id, turns }, next_cursor: cursor };
    }, { id, turns: nativeTurns(from, to), cursor });
  };
  const readPage = async (name: string) => {
    await page.getByRole("button", { name, exact: true }).click();
    await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.finishOlder)).toBe("function");
    await page.evaluate(() => (window as any).__performance.finishOlder());
  };
  await fixture(10, 30, "10");
  await readPage("读取更早的原生对话");
  await expect(pane.locator('[data-message-id="item-10"]')).toBeAttached();
  await page.evaluate(({ id, turns }) => {
    (window as any).__performance.nativeReply = { thread: { id, turns }, next_cursor: "60" };
  }, { id, turns: nativeTurns(60, 80) });
  await page.getByRole("button", { name: "刷新原生会话", exact: true }).click();
  await expect(pane.locator('[data-message-id="item-79"]')).toBeAttached();
  await expect(pane.locator('[data-message-id="item-10"]')).toBeAttached();
  await expect(pane.locator('[data-message-id="item-55"]')).toHaveCount(0);
  await fixture(40, 60, "40");
  await readPage("补读中间的原生对话");
  await expect(pane.locator('[data-message-id="item-55"]')).toBeAttached();
  await expect(page.getByRole("button", { name: "补读中间的原生对话" })).toHaveCount(0);
  expect(await pane.locator("[data-message-id]").evaluateAll(rows => rows.map(row => row.getAttribute("data-message-id")))).toEqual(
    Array.from({ length: 70 }, (_, index) => `item-${index + 10}`),
  );
  await expect(page.getByRole("button", { name: "读取更早的原生对话" })).toBeVisible();
  const cursors = await page.evaluate(() => (window as any).__performance.nativeActions.filter((r: any) => r.action === "read").map((r: any) => r.cursor ?? null));
  expect(cursors).toEqual([null, "30", null, "60"]);
});

test("native completion refreshes the latest window without losing earlier turns while a page is in flight", async ({ page }) => {
  await page.clock.install();
  await open(page, [], { thread: { id, turns: nativeTurns(30, 50) }, next_cursor: "30" }, "idle", "claude-code");
  const pane = page.getByLabel("原生对话消息");
  const earlier = page.getByRole("button", { name: "读取更早的原生对话", exact: true });
  await page.evaluate(({ id, turns }) => {
    const p = (window as any).__performance;
    p.nativeStatus = { id, provider: "claude-code", state: "inProgress", items: {} };
    p.older = { thread: { id, turns }, next_cursor: "10" };
  }, { id, turns: nativeTurns(10, 30) });
  await page.getByRole("button", { name: "刷新原生会话", exact: true }).click();
  await expect(pane.getByText("正在处理你的事情…")).toBeVisible();
  await earlier.click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.finishOlder)).toBe("function");

  await page.evaluate(({ id, turns, final }) => {
    const p = (window as any).__performance;
    p.nativeReply = { thread: { id, turns }, next_cursor: "31" };
    p.nativeStatus = { id, provider: "claude-code", state: "completed", item_order: [final.id], items: { [final.id]: final } };
  }, { id, turns: nativeTurns(31, 51), final: longItems(50, 51)[0] });
  await page.clock.runFor(1800);
  await expect.poll(() => page.evaluate(() => (window as any).__performance.nativeActions.filter((r: any) => r.action === "read" && !r.cursor).length)).toBe(3);
  await expect(pane.locator('[data-message-id="item-50"]')).toBeAttached();
  await page.evaluate(() => (window as any).__performance.finishOlder());
  await expect(pane.locator('[data-message-id="item-10"]')).toBeAttached();
  await expect(pane.locator('[data-message-id="item-30"]')).toBeAttached();
  await expect(pane.locator('[data-message-id="item-50"]')).toHaveCount(1);
  await expect(page.getByLabel("给 Claude Code 发消息")).toHaveValue("合成未发草稿");
  const commands = await page.evaluate(() => (window as any).__performance.nativeActions.map((r: any) => [r.action, r.cursor ?? null]));
  expect(commands.filter(([action]: string[]) => action === "read")).toEqual([["read", null], ["read", null], ["read", "30"], ["read", null]]);
  expect(commands.every(([action]: string[]) => ["read", "status", "list"].includes(action))).toBe(true);
});

test('partial HomeBase binding metadata preserves readable history and refresh restores the original shared-task link',async({page})=>{
  await open(page,[{id:'a',type:'agentMessage',text:'可读的原生结果'}],{context_link_state:'unavailable',context_link_message:'原生历史可读，但HomeBase会话关联未能核对',status:'partial'});
  await expect(page.getByRole('alert')).toContainText('关联未能核对');
  await expect(page.getByText('可读的原生结果',{exact:true})).toBeVisible();
  await expect(page.getByRole('link',{name:'查看共享执行任务、结果与待确认操作',exact:true})).toHaveCount(0);
  await page.evaluate(()=>{const r=(window as any).__performance.nativeReply;r.context_link_state='linked';r.context_link_message=null;r.assistant_session_id='shared-synthetic';});
  await page.getByRole('button',{name:'刷新原生会话',exact:true}).click();
  await expect(page.getByRole('link',{name:'查看共享执行任务、结果与待确认操作',exact:true})).toHaveAttribute('href','/assistant?session=shared-synthetic');
});

test('handoff keeps original blocks as bounded quoted data and never cuts a new goal or creates an empty session for an invalid goal',async({page})=>{
  const items=Array.from({length:8},(_,n)=>({id:`u${n}`,type:'userMessage',content:[{type:'text',text:`用户第${n}段🙂${'中文材料🙂'.repeat(240)}`},{type:'text',text:`附带第${n}段原生输入`}]}));
  await open(page,items);
  const original=await page.evaluate(()=>JSON.stringify((window as any).__performance.nativeReply));
  await page.getByRole('button',{name:'交接给 HomeBase',exact:true}).click();
  expect(await page.evaluate(()=>(window as any).__performance.nativeActions.filter((r:any)=>r.action==='read').length)).toBe(1);
  const goal=page.getByLabel('交接目标');await goal.fill('中文目标'.repeat(1000));
  await page.getByRole('button',{name:'准备交接草稿',exact:true}).click();
  await expect(page.getByRole('alert')).toContainText('未创建新会话');
  expect(await page.evaluate(()=>(window as any).__performance.creates)).toBe(0);
  await expect(goal).toHaveValue('中文目标'.repeat(1000));
  const valid='请继续归纳蓝色圆点动画🙂';await goal.fill(valid);
  await page.getByRole('button',{name:'准备交接草稿',exact:true}).click();
  const draft=await page.getByLabel('发送消息',{exact:true}).inputValue();
  expect(Buffer.byteLength(draft)).toBeLessThanOrEqual(8000);expect(draft).toContain(valid);expect(draft).not.toContain('\uFFFD');
  const data=JSON.parse(draft.slice(draft.indexOf('\n{')+1));expect(data.partial).toBe(true);expect(data.messages.length).toBeGreaterThan(0);expect(data.messages.some((m:any)=>m.truncated)).toBe(true);
  expect(await page.evaluate(()=>(window as any).__performance.creates)).toBe(1);
  expect(await page.evaluate(()=>JSON.stringify((window as any).__performance.nativeReply))).toBe(original);
  expect(await page.evaluate(()=>(window as any).__performance.nativeActions.filter((r:any)=>['send','create','interrupt'].includes(r.action)))).toEqual([]);
});

test('short extra user text is preserved in a handoff without rereading loaded history',async({page})=>{
  await open(page,[{id:'u',type:'userMessage',content:[{type:'text',text:'第一段真实用户输入'},{type:'text',text:'不能省略的第二段用户输入'}]}]);
  await page.getByRole('button',{name:'交接给 HomeBase',exact:true}).click();
  await page.getByLabel('交接目标').fill('继续核对两段原始输入');
  await page.getByRole('button',{name:'准备交接草稿',exact:true}).click();
  const draft=await page.getByLabel('发送消息',{exact:true}).inputValue();
  expect(draft).toContain('第一段真实用户输入');expect(draft).toContain('不能省略的第二段用户输入');
  expect(await page.evaluate(()=>(window as any).__performance.nativeActions.filter((r:any)=>r.action==='read').length)).toBe(1);
});

test('a failed handoff read is explicit and cancelling carry prepares only the goal without another native read',async({page})=>{
  await open(page,[{id:'a',type:'agentMessage',text:'原有结果'}]);
  await page.evaluate(()=>{
    const p=(window as any).__performance,original=p.values.conversation_native;
    p.values.conversation_native=(args:any)=>{if(args.request.action==='read'){p.nativeActions.push(args.request);throw Error('Synthetic history read failed');}return original(args);};
  });
  await page.getByRole('button',{name:'刷新原生会话',exact:true}).click();
  await page.getByRole('button',{name:'交接给 HomeBase',exact:true}).click();
  const form=page.getByLabel('交接会话');
  await form.getByLabel('交接目标').fill('不带旧内容，准备新的目标');
  await expect(form.getByRole('alert')).toContainText('读取交接内容失败');
  await expect(form.getByRole('button',{name:'准备交接草稿',exact:true})).toBeDisabled();
  const reads=await page.evaluate(()=>(window as any).__performance.nativeActions.filter((r:any)=>r.action==='read').length);
  await form.getByRole('checkbox',{name:/携带最近对话摘录/}).uncheck();
  await expect(form.getByRole('alert')).toHaveCount(0);
  await form.getByRole('button',{name:'准备交接草稿',exact:true}).click();
  await expect(page.getByLabel('发送消息',{exact:true})).toHaveValue('不带旧内容，准备新的目标');
  expect(await page.evaluate(()=>(window as any).__performance.nativeActions.filter((r:any)=>r.action==='read').length)).toBe(reads);
});

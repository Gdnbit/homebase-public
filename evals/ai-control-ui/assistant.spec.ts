async function openAction(page: import("@playwright/test").Page, name: string) {
  const button = page.getByRole("button", { name, exact: true });
  if (!(await button.isVisible())) await page.getByLabel("更多对话操作", { exact: true }).click();
  await button.click();
}
import { test, expect } from "@playwright/test";

test("accepted in-flight changes are identified as changes instead of read-only discussion",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.values.assistant_config_get.settings.enabled=true;
    const turn={id:"change-receipt",session_id:"adaptation-session",state:"completed",text:"补充已记入原任务，会按新要求接着核对。",steps:[],pending:null,jobs:[],artifacts:[],interaction:"chat",parent_turn_id:"original-task",steered_turn_id:"original-task"};
    p.values.assistant_sessions_list=[{id:"adaptation-session",title:"隔离要求修改",source:"desktop",active_turn_id:null}];
    p.values.assistant_session_get={id:"adaptation-session",source:"desktop",messages:[{id:"a-change",role:"assistant",content:turn.text,turn_id:turn.id}],turns:[turn]};
  });
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await expect(page.getByText("补充已记入原任务 · 已受理的操作会先核对结果",{exact:true})).toBeVisible();
  await expect(page.getByText("只读讨论 · 没有修改执行中的任务",{exact:true})).toHaveCount(0);
  await page.getByLabel("消息方式",{exact:true}).selectOption("task");
  await expect(page.getByText("新事情会排队；修改当前要求请用自动判断。",{exact:true})).toBeVisible();
});

test("discussion mode can reply beside a running task without sending another execution request",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.values.assistant_config_get.settings.enabled=true;
    const task={id:"background-task",session_id:"working-session",state:"waiting_jobs",text:"等待旧任务",steps:[],pending:null,jobs:[],artifacts:[],interaction:"task",progress:{goal:"发布动画",completed:["核对素材"],current:"渲染动画",blocked:null,next:"返回结果",completed_stages:1,total_stages:3}};
    p.values.assistant_sessions_list=[{id:"working-session",title:"工作中交流",source:"desktop",active_turn_id:task.id}];
    p.values.assistant_session_get={id:"working-session",source:"desktop",messages:[{id:"u",role:"user",content:"制作动画",turn_id:task.id},{id:"a",role:"assistant",content:"",turn_id:task.id}],turns:[task]};
    p.values.assistant_turn_get=task;
    p.values.assistant_send=({request}:any)=>{
      p.sentDiscussion=request;
      const chat={id:"foreground-chat",session_id:task.session_id,state:"completed",text:"素材已经核对，现在正在渲染；后面再检查文件。",steps:[],pending:null,jobs:[],artifacts:[],interaction:"chat",parent_turn_id:task.id};
      p.values.assistant_session_get.messages.push({id:"u-chat",role:"user",content:request.text,turn_id:chat.id},{id:"a-chat",role:"assistant",content:chat.text,turn_id:chat.id});
      p.values.assistant_session_get.turns.push(chat);
      return {session_id:task.session_id,turn_id:chat.id,state:chat.state};
    };
  });
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await expect(page.getByText("正在做：渲染动画",{exact:true})).toBeVisible();
  await page.getByLabel("消息方式",{exact:true}).selectOption("chat");
  await page.getByLabel("发送消息",{exact:true}).fill("解释一下这个制作安排");
  await page.getByRole("button",{name:"发送",exact:true}).click();
  await expect(page.getByText("素材已经核对，现在正在渲染；后面再检查文件。",{exact:true})).toBeInViewport();
  await expect(page.getByText("只读讨论 · 没有修改执行中的任务",{exact:true})).toBeVisible();
  const data=await page.evaluate(()=>({request:(window as any).__performance.sentDiscussion,background:(window as any).__performance.values.assistant_session_get.turns[0],calls:(window as any).__performance.calls}));
  expect(data.request.mode).toBe("chat");expect(data.background.state).toBe("waiting_jobs");expect(data.calls.filter((c:string)=>c==="assistant_send")).toHaveLength(1);
  if(process.env.HOMEBASE_DIALOGUE_SCREENSHOT)await page.screenshot({path:`${process.env.HOMEBASE_DIALOGUE_SCREENSHOT}/conversation.png`});
});

test("ongoing matters show confirmed notes separately from original task progress and acknowledge only the review",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.values.assistant_personal_get.matters={revision:3,entries:[{id:"matter-one",title:"发布动画",goal:"跟进动画制作与发布审核",summary:"素材已核对",next_action:"等待审核后发布",state:"waiting",review_at:"2026-11-12T00:00:00+08:00",delivery:"desktop_due",source:"wechat",updated_at:"2026-11-13T00:00:00Z",turn_ids:["original-work"],previous:[{summary:"待核对素材",next_action:"核对素材",state:"active",updated_at:"2026-11-12T00:00:00Z"}],latest_task:{turn_id:"original-work",state:"completed",progress:{current:null}}}]};
    p.values.assistant_matter_review_ack=({matterId,revision}:any)=>{p.matterReview={matterId,revision};p.values.assistant_personal_get.matters.entries[0].delivery="dismissed";p.values.assistant_personal_get.matters.revision++;return null;};
    p.values.assistant_personal_get.followups={revision:0,entries:[{id:"linked-check",title:"复查发布动画",instruction:"核对原进展",due_at:"2026-11-14T00:00:00Z",interval_minutes:null,max_runs:1,enabled:true,state:"scheduled",notice:"按约定复查",source:"wechat",matter_id:"matter-one",runs:[]}]};
  });
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  const personalButton = page.getByRole("button", { name: "记忆与提醒", exact: true });
  await expect(personalButton).toHaveAccessibleDescription("有提醒待处理");
  await expect(personalButton.locator('[aria-hidden="true"]')).toBeVisible();
  await personalButton.click();
  await expect(page.getByText("关联事项：发布动画",{exact:true})).toBeVisible();
  const section=page.getByRole("dialog").locator("section").filter({has:page.getByRole("heading",{name:"持续事项",exact:true})});
  await section.scrollIntoViewIfNeeded();await expect(section.getByText("等待条件",{exact:true})).toBeVisible();await expect(section.getByText("关联任务：已完成",{exact:true})).toBeVisible();
  await expect(section.getByText("下一步：等待审核后发布",{exact:true})).toBeVisible();
  await section.getByText("事项更新历史（1）",{exact:true}).click();await expect(section.getByText(/待核对素材/)).toBeVisible();
  if(process.env.HOMEBASE_DIALOGUE_SCREENSHOT)await page.screenshot({path:`${process.env.HOMEBASE_DIALOGUE_SCREENSHOT}/matters.png`});
  await section.getByRole("button",{name:"已知悉本次复查",exact:true}).click();
  await expect(section.getByRole("button",{name:"已知悉本次复查",exact:true})).toHaveCount(0);
  await page.getByRole("button", { name: "关闭个人助理管理" }).click();
  await expect(personalButton).toHaveAccessibleDescription("");
  await expect(personalButton.locator('[aria-hidden="true"]')).toHaveCount(0);
  const data=await page.evaluate(()=>({ack:(window as any).__performance.matterReview,calls:(window as any).__performance.calls}));expect(data.ack).toEqual({matterId:"matter-one",revision:3});expect(data.calls).not.toContain("assistant_send");expect(data.calls).not.toContain("assistant_resume");
});

test("personal panel separates follow-up progress from WeChat delivery without a red dot", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.assistant_personal_get.followups = {
      revision: 2,
      entries: ["pending", "claimed", "accepted", "unknown", "", "constructor"].map((delivery, index) => ({
        id: `followup-${index}`,
        title: `主动任务 ${index}`,
        instruction: "核对进展",
        due_at: "2026-11-14T00:00:00Z",
        interval_minutes: null,
        max_runs: 1,
        enabled: false,
        state: "paused",
        notice: "原任务需要补充材料",
        notice_delivery: delivery,
        source: "wechat",
        runs: [],
      })),
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  const button = page.getByRole("button", { name: "记忆与提醒", exact: true });
  await expect(button).toHaveAccessibleDescription("");
  await button.click();
  const dialog = page.getByRole("dialog");
  for (const [index, label] of ["等待发送到微信", "微信发送中", "微信已受理", "发送结果待核对", "", "状态待核对"].entries()) {
    const card = dialog.getByRole("article").filter({ hasText: `主动任务 ${index}` });
    await expect(card.getByText("原任务需要补充材料", { exact: true })).toBeVisible();
    if (label) {
      await expect(card.getByText(`微信通知 · ${label}`, { exact: true })).toBeVisible();
    } else {
      await expect(card.getByText(/^微信通知 ·/)).toHaveCount(0);
    }
  }
  const calls = await page.evaluate(() => (window as any).__performance.calls as string[]);
  expect(calls).not.toContain("assistant_send");
  expect(calls).not.toContain("assistant_matter_review_ack");
});

test("personal panel counts only accepted follow-up runs", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.assistant_personal_get.followups = {
      revision: 1,
      entries: [{
        id: "synthetic-followup", title: "重连后核对原任务", instruction: "读取原结果",
        due_at: "2026-11-14T00:00:00Z", interval_minutes: 10, max_runs: 3,
        enabled: true, state: "running", notice: "读取原结果", source: "wechat",
        runs: [
          { ordinal: 1, turn_id: "accepted-original", state: "completed" },
          { ordinal: 2, turn_id: null, state: "prepared" },
        ],
      }],
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await page.getByRole("button", { name: "记忆与提醒", exact: true }).click();
  const card = page.getByRole("dialog").getByRole("article").filter({ hasText: "重连后核对原任务" });
  await expect(card.getByText("等待受理", { exact: true })).toBeVisible();
  await expect(card.getByText(/已受理 1\/3 次/)).toBeVisible();
  await expect(card.getByText("第 2 次：准备受理", { exact: true })).toBeVisible();
});

test("new replies preserve reading position and offer a quiet return to latest",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.values.assistant_config_get.settings.enabled=true;
    const messages=Array.from({length:24},(_,i)=>({id:`history-${i}`,role:i%2?"assistant":"user",content:`隔离历史消息${i}：${"用于检查长对话的阅读位置。".repeat(8)}`,turn_id:null}));
    p.values.assistant_sessions_list=[{id:"scroll-session",title:"阅读隔离历史",source:"desktop",active_turn_id:null}];
    p.values.assistant_session_get={id:"scroll-session",source:"desktop",messages,turns:[]};
  });
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  const transcript=page.getByLabel("对话内容",{exact:true});
  await expect.poll(()=>transcript.evaluate(e=>e.scrollTop)).toBeGreaterThan(100);
  await transcript.evaluate(e=>{e.scrollTop=0;e.dispatchEvent(new Event('scroll',{bubbles:true}));});
  await page.evaluate(()=>{(window as any).__performance.values.assistant_session_get.messages.push({id:"new-reply",role:"assistant",content:"新的隔离回复，不打断你阅读旧消息。",turn_id:null});});
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await expect(page.getByRole("button",{name:"回到最新回复",exact:true})).toBeVisible();
  expect(await transcript.evaluate(e=>e.scrollTop)).toBe(0);
  await page.getByRole("button",{name:"回到最新回复",exact:true}).click();
  await expect(page.getByText("新的隔离回复，不打断你阅读旧消息。",{exact:true})).toBeInViewport();
});

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const settings = {
      revision: 0,
      enabled: false,
      route_id: "",
      wechat_enabled: true,
      max_rounds: 8,
      max_tokens: 2048,
      runtime: "pi",
    };
    p.values.assistant_config_get = {
      settings,
      workspace_root: "X:\\isolated\\assistant",
      model_state: "disabled",
      message: "模型暂未启用",
    };
    p.values.assistant_sessions_list = [];
    p.values.assistant_personal_get = {revision:0,skills:[],memories:[],reminders:[]};
    p.values.assistant_session_create = () => {
      const row = {
        id: "fixture-session",
        title: "新对话",
        source: "desktop",
        message_count: 0,
        updated_at: "2026-11-14T00:00:00Z",
        active_turn_id: null,
      };
      p.values.assistant_sessions_list = [row];
      p.values.assistant_session_get = { ...row, messages: [], turns: [] };
      return row;
    };
    p.values.assistant_config_save = ({ settings }: any) => {
      p.values.assistant_config_get.settings = {
        ...settings,
        revision: settings.revision + 1,
      };
      return p.values.assistant_config_get;
    };
  });
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "AI 助理", exact: true }),
  ).toBeVisible();
});

test("shared task plan exposes evidence, budget and a recoverable stop without submitting again",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.values.assistant_config_get.settings.enabled=true;
    const task={plan:{revision:2,goal:"检查隔离统计并交付",items:[{id:"read",title:"读取合成数据",done_when:"读取结果与输入一致",kind:"inspect",state:"completed",note:"内容已核对",evidence:["proof-read"]},{id:"deliver",title:"交付结果",done_when:"登记真实附件",kind:"deliver",state:"pending",note:"",evidence:[]}]},limits:{model_calls_used:8,max_model_calls:8,steps_used:4,max_steps:192,active_seconds:91,max_active_seconds:1800},stop:{kind:"paused_budget",message:"执行预算已用完，结果保留。",next_action:"明确继续可沿用原检查点。",retry_safe:true}};
    const turn={id:"plan-turn",session_id:"plan-session",state:"paused_budget",text:"还需交付结果",runtime:"deepagents",resumable:true,steps:[],pending:null,jobs:[],artifacts:[],task};
    p.values.assistant_sessions_list=[{id:"plan-session",title:"复杂任务",source:"wechat",active_turn_id:null}];
    p.values.assistant_session_get={id:"plan-session",source:"wechat",messages:[{id:"u",role:"user",content:"检查并交付",turn_id:turn.id},{id:"a",role:"assistant",content:turn.text,turn_id:turn.id}],turns:[turn]};
    p.values.assistant_turn_get=turn;
    p.values.assistant_resume=()=>{p.resumedOriginal=true;turn.state="running";task.limits.max_model_calls=16;task.stop=null as any;return {turn_id:turn.id,session_id:turn.session_id,state:turn.state};};
  });
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await page.getByText("执行详情 · 0 步", {exact:true}).click();
  const plan=page.getByLabel("任务计划",{exact:true});
  await expect(plan).toBeVisible();await expect(plan.getByText(/推理 8\/8/)).toBeVisible();
  await plan.getByText("完成条件与证据",{exact:true}).first().click();
  await expect(plan.getByText("proof-read",{exact:true})).toBeVisible();
  await expect(page.getByText("明确继续可沿用原检查点。",{exact:true})).toBeVisible();
  await page.getByRole("button",{name:"继续原任务",exact:true}).click();
  await expect(plan.getByText(/推理 8\/16/)).toBeVisible();
  const data=await page.evaluate(()=>({resumed:(window as any).__performance.resumedOriginal,calls:(window as any).__performance.calls}));
  expect(data.resumed).toBe(true);expect(data.calls).not.toContain("assistant_send");
});

test("personal memory/reminder manager saves references and shows expiry/delivery explicitly",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.values.assistant_personal_get={revision:0,skills:[{id:"fixture",name:"合成 Skill",description:"隔离能力",version:1}],memories:[],reminders:[]};
    p.values.assistant_memory_save=({request}:any)=>{
      p.savedMemory=request;
      p.values.assistant_personal_get={...p.values.assistant_personal_get,revision:1,memories:[{...request,id:"fixture-memory",status:"active",updated_at:"2026-11-12T00:00:00Z"}]};return p.values.assistant_personal_get.memories[0];
    };
    p.values.assistant_reminder_save=({request}:any)=>{
      p.savedReminder=request;
      p.values.assistant_personal_get={...p.values.assistant_personal_get,revision:2,reminders:[{...request,id:"fixture-reminder",source:"desktop",delivery:"desktop_due",last_due_at:request.due_at}]};return {};
    };
  });
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await page.getByRole("button",{name:"记忆与提醒",exact:true}).click();
  const dialog=page.getByRole("dialog");
  await expect(dialog.getByText("合成 Skill",{exact:true})).toBeVisible();
  await dialog.getByLabel("记忆标题",{exact:true}).fill("合成偏好");
  await dialog.getByLabel("记忆内容",{exact:true}).fill("只保存隔离数据");
  await dialog.getByRole("button",{name:"添加记忆",exact:true}).click();
  await expect(dialog.getByText("合成偏好",{exact:true})).toBeVisible();
  await dialog.getByLabel("提醒内容",{exact:true}).fill("隔离提醒");
  await dialog.getByLabel("提醒时间（本地时间）",{exact:true}).fill("2026-11-13T10:00");
  await dialog.getByRole("button",{name:"添加桌面提醒",exact:true}).click();
  await expect(dialog.getByText("隔离提醒",{exact:true})).toBeVisible();
  await expect(dialog.getByText(/桌面 · 已到期/)).toBeVisible();
  const saved=await page.evaluate(()=>(window as any).__performance.savedReminder);
  expect(saved.due_at).toBe("2026-11-13T02:00:00.000Z");expect(saved.revision).toBe(1);
  await page.screenshot({path:"outputs/page-design/assistant/2026-11-12/personal-memory-reminders.png"});
  await dialog.getByRole("button",{name:"关闭个人助理管理"}).click();
  await expect(dialog).toHaveCount(0);
});

test("scheduled work and memory history share the personal panel without dispatching tasks on open", async ({page}) => {
  await page.evaluate(() => {
    const p=(window as any).__performance;
    p.values.assistant_personal_get={revision:3,skills:[],reminders:[],memories:[{id:"memory-current",title:"动画偏好",content:"简短中文",updated_at:"2026-11-13T00:00:00Z",expires_at:null,status:"active",previous:[{content:"旧的合成字幕",updated_at:"2026-11-12T00:00:00Z",expires_at:null}]}],followups:{revision:7,entries:[{id:"scheduled-one",title:"隔离数据检查",instruction:"检查隔离数据并返回文件",due_at:"2026-11-14T02:00:00Z",interval_minutes:60,max_runs:3,enabled:true,state:"scheduled",notice:"本次完成，下次按原计划执行",source:"wechat",runs:[{ordinal:1,turn_id:"existing-turn",state:"completed",scheduled_at:"2026-11-13T02:00:00Z"}]}]}};
    p.values.assistant_followup_cancel=({followupId,revision}:any)=>{
      p.canceledFollowup={followupId,revision};p.values.assistant_personal_get.followups.revision++;
      const row=p.values.assistant_personal_get.followups.entries[0];row.enabled=false;row.state="canceled";row.notice="已取消后续执行，原结果保留";
    };
  });
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await page.getByRole("button",{name:"记忆与提醒",exact:true}).click();
  const dialog=page.getByRole("dialog");
  await expect(dialog.getByRole("heading",{name:"主动办事",exact:true})).toBeVisible();
  await expect(dialog.getByText("隔离数据检查",{exact:true})).toBeVisible();
  await expect(dialog.getByText("简短中文",{exact:true})).toBeVisible();
  await dialog.getByText("更新历史（1）",{exact:true}).click();
  await expect(dialog.getByText(/旧的合成字幕/)).toBeVisible();
  await dialog.getByRole("button",{name:"取消后续执行",exact:true}).click();
  await expect(dialog.getByText("已取消",{exact:true})).toBeVisible();
  const state=await page.evaluate(()=>({canceled:(window as any).__performance.canceledFollowup,calls:(window as any).__performance.calls}));
  expect(state.canceled).toEqual({followupId:"scheduled-one",revision:7});
  expect(state.calls).not.toContain("assistant_send");expect(state.calls).not.toContain("assistant_followup_save");
  await page.screenshot({path:"outputs/personal-agent-continuity/2026-11-13/personal-panel.png"});
});

test("local workspace and MCP grants are explicit, shared and do not change model settings",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.values.assistant_local_config_get={config:{revision:0,enabled:true,workspaces:[],mcp_grants:[]},workspaces:[{id:"scratch",path:"X:/synthetic/scratch",access:"write",auto_write:true}],mcp_servers:[{id:"fixture",name:"合成 MCP",description:"只用于隔离验证",state:"needs_trust",fingerprint:"a".repeat(64)},{id:"homebase-secrets",name:"密钥服务器",description:"不会开放",state:"blocked_secret_export",fingerprint:"b".repeat(64)}],skill_sources:[{id:"fixture"}],terminal:{binary_present:true,notice:"合成执行器，未执行真实命令"}};
    p.values["plugin:dialog|open"]="X:/synthetic/project";
    p.values.assistant_local_config_save=({config}:any)=>{
      p.savedLocalConfig=config;
      p.values.assistant_local_config_get={...p.values.assistant_local_config_get,config:{...config,revision:config.revision+1}};
      return p.values.assistant_local_config_get;
    };
  });
  await openAction(page, "本机能力");
  const dialog=page.getByRole("dialog",{name:"本机能力",exact:true});
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("button",{name:"凭据导出不开放"})).toBeDisabled();
  await dialog.getByRole("button",{name:"添加目录"}).click();
  await expect(dialog.getByText("X:/synthetic/project",{exact:true})).toBeVisible();
  await expect(dialog.getByRole("combobox")).toHaveValue("read");
  await dialog.getByRole("button",{name:"信任此连接"}).click();
  await dialog.getByRole("button",{name:"保存授权"}).click();
  await expect(dialog.getByRole("status")).toHaveText("授权已保存，微信与桌面共用");
  const saved=await page.evaluate(()=>(window as any).__performance.savedLocalConfig);
  expect(saved.workspaces[0].access).toBe("read");expect(saved.workspaces[0].auto_write).toBe(false);
  expect(saved.mcp_grants[0].tools).toEqual([]);expect(saved.mcp_grants[0].resources).toBe(false);
  await page.screenshot({path:"outputs/personal-agent-local/2026-11-13/local-capabilities.png"});
  await dialog.getByRole("button",{name:"关闭本机能力"}).click();
  await openAction(page, "本机能力");
  expect(await page.evaluate(()=>(window as any).__performance.calls.filter((s:string)=>s==='assistant_local_config_get').length)).toBe(1);
  await dialog.getByRole("button",{name:"刷新本机能力"}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).__performance.calls.filter((s:string)=>s==='assistant_local_config_get').length)).toBe(2);
  const calls=await page.evaluate(()=>(window as any).__performance.calls);
  expect(calls).not.toContain("assistant_config_save");expect(calls).not.toContain("shared_mcp_save");expect(calls).not.toContain("secrets_save");
});

test("MCP credentials require an explicit existing reference and HTTP targets are saved without plaintext keys",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.values.assistant_local_config_get={config:{revision:0,enabled:true,workspaces:[],mcp_grants:[],http_servers:[]},workspaces:[{id:"scratch",path:"X:/synthetic/scratch",access:"write",auto_write:true}],mcp_servers:[{id:"fixture-auth",name:"合成认证服务",description:"隔离验证",state:"needs_trust",fingerprint:"a".repeat(64),required_references:["MCP_TOKEN","MODEL_ID"],credential_references:["MCP_TOKEN"]}],credential_references:[{id:"sops:fixture_token",name:"合成已有凭据引用"}],skill_sources:[],terminal:{binary_present:true,notice:"合成执行器"}};
    p.values.assistant_local_config_save=({config}:any)=>{p.savedLocalConfig=config;const result={...p.values.assistant_local_config_get,config:{...config,revision:config.revision+1}};p.values.assistant_local_config_get=result;return result;};
  });
  await openAction(page, "本机能力");
  const dialog=page.getByRole("dialog",{name:"本机能力",exact:true});
  await dialog.getByRole("button",{name:"信任此连接"}).click();
  await expect(dialog.getByLabel("MCP_TOKEN 提供方式").locator('option[value="literal"]')).toHaveCount(0);
  await dialog.getByLabel("MCP_TOKEN 提供方式").selectOption("credential");
  await expect(dialog.getByLabel("MCP_TOKEN 凭据引用")).toHaveValue("");
  await dialog.getByRole("button",{name:"保存授权"}).click();
  await expect(dialog.getByRole("alert")).toContainText("请选择");
  expect(await page.evaluate(()=>(window as any).__performance.savedLocalConfig)).toBeUndefined();
  await dialog.getByLabel("MCP_TOKEN 凭据引用").selectOption("sops:fixture_token");
  await dialog.getByLabel("MODEL_ID 提供方式").selectOption("literal");
  await dialog.getByLabel("MODEL_ID 运行变量").fill("synthetic-model");
  await dialog.getByText("添加 HTTP MCP",{exact:true}).click();
  await dialog.getByLabel("HTTP MCP名称").fill("隔离HTTP");await dialog.getByLabel("HTTP MCP地址").fill("https://example.invalid/mcp");
  await dialog.getByLabel("HTTP MCP认证方式").selectOption("bearer");
  await dialog.getByRole("button",{name:"加入待保存连接"}).click();
  await dialog.getByRole("button",{name:"保存授权"}).click();
  await expect(dialog.getByRole("status")).toHaveText("授权已保存，微信与桌面共用");
  const config=await page.evaluate(()=>(window as any).__performance.savedLocalConfig);
  expect(config.mcp_grants[0].credential_bindings).toEqual([{variable:"MCP_TOKEN",reference:"sops:fixture_token"}]);expect(config.mcp_grants[0].variables.MODEL_ID).toBe("synthetic-model");
  expect(config.http_servers[0].headers.Authorization).toBe("Bearer ${MCP_TOKEN}");
  await page.screenshot({path:"outputs/personal-agent-local/2026-11-13/mcp-references-http.png"});
});

test("busy conversation accepts a followup once and paused queue requires explicit resume",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance;p.values.assistant_config_get.settings.enabled=true;
    const first={id:"queue-first",session_id:"queue-session",state:"running",text:"正在处理隔离任务",steps:[],pending:null,jobs:[],artifacts:[]};
    p.values.assistant_sessions_list=[{id:"queue-session",title:"隔离队列",source:"desktop",active_turn_id:first.id}];
    p.values.assistant_session_get={id:"queue-session",title:"隔离队列",source:"desktop",messages:[{id:"a",role:"assistant",content:"",turn_id:first.id}],turns:[first]};
    p.values.assistant_turn_get=first;
    p.values.assistant_send=({request}:any)=>{
      p.queuedMessages=(p.queuedMessages??0)+1;
      first.state="failed";first.text="合成失败，后续队列已暂停";
      const queued={...first,id:"queue-next",state:"waiting_input_queue",text:"后续消息已排队",queue_paused:true};
      p.values.assistant_turn_get=queued;p.values.assistant_session_get.turns=[first,queued];
      p.values.assistant_session_get.queue_resume_generation=0;
      p.values.assistant_session_get.messages.push({id:"next",role:"assistant",content:"",turn_id:queued.id});
      return {session_id:"queue-session",turn_id:queued.id,state:queued.state};
    };
    p.values.assistant_queue_resume=()=>{p.resumedQueue=true;for(const turn of p.values.assistant_session_get.turns)turn.queue_paused=false;return {resumed:true,queued:1};};
  });
  await page.getByRole("button",{name:"刷新助理"}).click();
  await page.getByLabel("消息方式",{exact:true}).selectOption("task");
  await expect(page.getByRole("button",{name:"加入后续队列",exact:true})).toBeVisible();
  await page.getByLabel("发送消息",{exact:true}).fill("隔离补充消息");
  await page.getByRole("button",{name:"加入后续队列",exact:true}).click();
  await expect(page.getByRole("button",{name:"继续后续消息",exact:true})).toBeVisible();
  await page.getByRole("button",{name:"继续后续消息",exact:true}).click();
  expect(await page.evaluate(()=>(window as any).__performance.queuedMessages)).toBe(1);
  expect(await page.evaluate(()=>(window as any).__performance.resumedQueue)).toBe(true);
});

test("a lost queue release acknowledgement refreshes the original version before an explicit later release", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.assistant_config_get.settings.enabled = true;
    const queued = { id: "queue-original-waiting", session_id: "queue-original-session", state: "waiting_input_queue", text: "隔离队列等待", steps: [], pending: null, jobs: [], artifacts: [], queue_paused: true };
    const original = { id: queued.session_id, title: "隔离队列版本", source: "desktop", queue_resume_generation: 0,
      messages: [{ id: "queue-original-message", role: "assistant", content: queued.text, turn_id: queued.id }], turns: [queued] };
    fixture.values.assistant_sessions_list = [{ id: original.id, title: original.title, source: original.source, active_turn_id: null }];
    fixture.values.assistant_session_get = original;
    fixture.values.assistant_turn_get = queued;
    fixture.queueReleases = [];
    fixture.values.assistant_queue_resume = (args: { sessionId: string; expectedGeneration?: number }) => {
      fixture.queueReleases.push(args);
      if (fixture.queueReleases.length === 1) {
        original.queue_resume_generation = 1;
        throw Error("隔离队列回执丢失");
      }
      queued.queue_paused = false;
      original.queue_resume_generation = 2;
      return { resumed: true, queued: 1 };
    };
  });
  await page.getByRole("button", { name: "刷新助理" }).click();
  const resume = page.getByRole("button", { name: "继续后续消息", exact: true });
  await resume.click();
  await expect(page.getByRole("alert")).toContainText("隔离队列回执丢失");
  expect(await page.evaluate(() => (window as any).__performance.queueReleases)).toEqual([
    { sessionId: "queue-original-session", expectedGeneration: 0 },
  ]);
  await resume.click();
  await expect(resume).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__performance.queueReleases)).toEqual([
    { sessionId: "queue-original-session", expectedGeneration: 0 },
    { sessionId: "queue-original-session", expectedGeneration: 1 },
  ]);
});

test("explicit credential source and free-only setting save references without inference",async({page})=>{
  await page.evaluate(()=>{const p=(window as any).__performance;const original=p.values.model_capabilities_catalog.models[0];p.values.model_capabilities_catalog.models.push({...original,route_id:"openrouter::fixture/free:free",name:"合成免费模型",provider_id:"openrouter",protocol:"chat"});const save=p.values.assistant_config_save;p.values.assistant_config_save=(args:any)=>{p.savedAssistantSettings=args.settings;return save(args);};});
  await openAction(page, "模型设置");
  await page.getByLabel("助理凭据来源").selectOption("api_key_manager");
  await expect(page.getByLabel("助理运行引擎")).toHaveCount(0);
  await page.getByLabel("仅免费模型（零价格上限，禁止付费回退）").check();
  await page.getByLabel("默认模型渠道").selectOption("openrouter::fixture/free:free");
  await page.getByLabel("启用默认模型（桌面和微信共用）").check();
  await page.screenshot({path:"outputs/page-design/assistant/2026-11-10/free-model-settings.png",fullPage:true});
  await page.getByRole("button",{name:"保存设置",exact:true}).click();
  await expect(page.getByText("已启用 · 仅免费模型",{exact:true})).toBeVisible();
  expect(await page.evaluate(()=>(window as any).__performance.savedAssistantSettings.runtime)).toBe("pi");
  const data=await page.evaluate(()=>({settings:(window as any).__performance.savedAssistantSettings,calls:(window as any).__performance.calls}));expect(data.settings.credential_source).toBe("api_key_manager");expect(data.settings.free_only).toBe(true);expect(data.calls).not.toContain("model_capability_call");expect(data.calls).not.toContain("secrets_save");
});

test("disabled default is explicit, settings never invoke a model and navigation reuses cache", async ({
  page,
}) => {
  await expect(page.getByText("模型未启用", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "发送", exact: true }),
  ).toBeDisabled();
  await openAction(page, "模型设置");
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(
    page.getByLabel("启用默认模型（桌面和微信共用）"),
  ).not.toBeChecked();
  await expect(
    page.getByLabel("微信普通消息使用此助理（明确指令继续兼容）"),
  ).toBeChecked();
  await page.getByRole("button", { name: "保存设置", exact: true }).click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  await openAction(page, "新对话");
  await expect.poll(()=>page.evaluate(()=>(window as any).__performance.client.getQueryData(["assistant","sessions"])?.items?.[0]?.id)).toBe("fixture-session");
  const before = await page.evaluate(
    () =>
      (window as any).__performance.calls.filter((s: string) =>
        ["assistant_config_get","assistant_sessions_query","assistant_session_window_get","assistant_personal_get"].includes(s),
      ).length,
  );
  if (!(await page.getByRole("link", { name: "工作台", exact: true }).isVisible())) await page.getByText("更多功能", {exact:true}).click();
  await page.getByRole("link", { name: "工作台", exact: true }).click();
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "AI 助理", exact: true }),
  ).toBeVisible();
  const after = await page.evaluate(
    () =>
      (window as any).__performance.calls.filter((s: string) =>
        ["assistant_config_get","assistant_sessions_query","assistant_session_window_get","assistant_personal_get"].includes(s),
      ).length,
  );
  expect(after).toBe(before);
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("model_capability_call");
  expect(calls).not.toContain("secrets_reveal");
  expect(calls).not.toContain("assistant_send");
  await page.screenshot({
    path: "outputs/page-design/assistant/2026-11-10/implemented-disabled.png",
    fullPage: true,
  });
  if (!(await page.getByRole("link", { name: "工作台", exact: true }).isVisible())) await page.getByText("更多功能", {exact:true}).click();
  await page.getByRole("link", { name: "工作台", exact: true }).click();
  const reads = await page.evaluate(() => (window as any).__performance.calls.filter((s: string) => s === "assistant_sessions_query").length);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_list.push({ id: "wechat-new", title: "微信新会话", source: "wechat", active_turn_id: null, updated_at:"2026-11-14T00:01:00Z" });
    p.emit("assistant:changed", { session_id: "wechat-new", turn_id: "wechat-turn", config: false });
  });
  await expect.poll(() => page.evaluate(() => (window as any).__performance.client.getQueryState(["assistant", "sessions"]).isInvalidated)).toBe(true);
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((s: string) => s === "assistant_sessions_query").length)).toBe(reads);
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(page.getByRole("button", { name: /微信新会话/ })).toBeVisible();
});

test("synthetic chat shows tool confirmation and verified result; completion refreshes without repeating send", async ({
  page,
}) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_config_get.settings.enabled = true;
    p.values.assistant_config_get.settings.route_id = "openai::fixture";
    p.values.assistant_send = ({ request }: any) => {
      const turn = {
        id: "fixture-turn",
        session_id: "fixture-session",
        state: "awaiting_confirmation",
        text: "需要确认具体操作",
        route_id: "openai::fixture",
        steps: [],
        pending: {
          id: "fixture-consent",
          description: "执行隔离配置变更",
          arguments: { id: "synthetic-agent", model: "fixture" },
        },
        jobs: [],
        artifacts: [],
      };
      p.values.assistant_sessions_list = [
        {
          id: "fixture-session",
          title: request.text,
          source: "desktop",
          active_turn_id: turn.id,
        },
      ];
      p.values.assistant_turn_get = turn;
      p.values.assistant_session_get = {
        id: "fixture-session",
        title: request.text,
        source: "desktop",
        messages: [
          { id: "u", role: "user", content: request.text, turn_id: turn.id },
          { id: "a", role: "assistant", content: "", turn_id: turn.id },
        ],
        turns: [turn],
      };
      return {
        session_id: "fixture-session",
        turn_id: turn.id,
        state: turn.state,
      };
    };
    p.values.assistant_confirm = () => {
      const turn = p.values.assistant_turn_get;
      turn.state = "completed";
      turn.text = "隔离任务完成，文件已生成";
      turn.pending = null;
      turn.steps = [
        {
          id: "step",
          name: "homebase_video",
          state: "completed",
          arguments: { title: "fixture" },
          result: { status: "succeeded" },
          job_id: "fixture-job",
        },
      ];
      turn.artifacts = [
        {
          id: "fixture-file",
          name: "fixture.mp4",
          path: "X:\\isolated\\fixture.mp4",
          bytes: 1234,
          sha256: "fixture",
          mime_type: "video/mp4",
        },
      ];
      return {
        session_id: "fixture-session",
        turn_id: turn.id,
        state: turn.state,
      };
    };
    p.values.assistant_artifact_open = null;
  });
  await page.getByRole("button", { name: "刷新助理" }).click();
  await page.getByLabel("发送消息").fill("生成隔离动画并返回文件");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "确认执行", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("synthetic-agent", { exact: false }),
  ).toBeVisible();
  const sessionReads = await page.evaluate(() => (window as any).__performance.calls.filter((s: string) => s === "assistant_session_window_get").length);
  await page.evaluate(() => { const p = (window as any).__performance; for (let i = 0; i < 12; i++) p.emit("assistant:changed", { session_id: "fixture-session", turn_id: "fixture-turn", config: false }); });
  await expect.poll(() => page.evaluate(() => (window as any).__performance.calls.filter((s: string) => s === "assistant_turn_get").length)).toBeGreaterThan(1);
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((s: string) => s === "assistant_session_window_get").length)).toBe(sessionReads);
  await page.getByRole("button", { name: "确认执行", exact: true }).click();
  await expect(
    page.getByText("隔离任务完成，文件已生成", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("fixture.mp4", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "打开结果", exact: true }).click();
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls.filter((s: string) => s === "assistant_send")).toHaveLength(1);
  expect(calls.filter((s: string) => s === "assistant_confirm")).toHaveLength(
    1,
  );
  await page.screenshot({
    path: "outputs/page-design/assistant/2026-11-10/implemented-result.png",
    fullPage: true,
  });
});

test("settings error preserves draft and is visible inside dialog", async ({
  page,
}) => {
  await page.evaluate(() => {
    (window as any).__performance.values.assistant_config_save = () => {
      throw Error("合成保存失败：revision 冲突");
    };
  });
  await openAction(page, "模型设置");
  await page.getByLabel("微信普通消息使用此助理（明确指令继续兼容）").uncheck();
  await page.getByRole("button", { name: "保存设置", exact: true }).click();
  await expect(page.getByRole("dialog").getByRole("alert")).toContainText(
    "revision 冲突",
  );
  await expect(
    page.getByLabel("微信普通消息使用此助理（明确指令继续兼容）"),
  ).not.toBeChecked();
});

test("external resume refreshes a cached terminal conversation and its dormant detail",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.values.assistant_config_get.settings.enabled=true;
    const turn={id:"resumed-turn",session_id:"resumed-session",state:"failed",text:"合成旧失败",steps:[],pending:null,jobs:[],artifacts:[]};
    p.values.assistant_sessions_list=[{id:"resumed-session",title:"隔离恢复",source:"wechat",active_turn_id:null}];
    p.values.assistant_session_get={id:"resumed-session",title:"隔离恢复",source:"wechat",messages:[{id:"a",role:"assistant",content:turn.text,turn_id:turn.id}],turns:[turn]};
  });
  await page.getByRole("button",{name:"刷新助理"}).click();
  await expect(page.getByText("合成旧失败",{exact:true})).toBeVisible();
  const reads=await page.evaluate(()=>(window as any).__performance.calls.filter((s:string)=>s==="assistant_session_window_get").length);
  await page.clock.setFixedTime(new Date());
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    const prior = p.values.assistant_session_get.turns[0];
    p.client.setQueryData(["assistant", "turn", prior.id, prior.session_id], prior);
    const turn={...p.values.assistant_session_get.turns[0],state:"awaiting_confirmation",text:"合成恢复后待确认",pending:{id:"fixture-consent",description:"合成保存",arguments:{title:"隔离测试"},approved:false}};
    p.values.assistant_session_get={...p.values.assistant_session_get,turns:[turn],messages:[{id:"a",role:"assistant",content:turn.text,turn_id:turn.id}]};
    p.values.assistant_turn_get=() => new Promise(resolve => {
      p.returnResumedDetail=()=>resolve(turn);
    });
    for(let i=0;i<12;i++)p.emit("assistant:changed",{session_id:"resumed-session",turn_id:"resumed-turn",config:false});
  });
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.returnResumedDetail)).toBe("function");
  await expect(page.getByText("合成旧失败",{exact:true})).toHaveCount(0);
  await expect(page.getByRole("button",{name:"确认执行",exact:true})).toHaveCount(0);
  await page.evaluate(() => (window as any).__performance.returnResumedDetail());
  await expect(page.getByRole("button",{name:"确认执行",exact:true})).toBeVisible();
  await expect(page.getByText("合成旧失败",{exact:true})).toHaveCount(0);
  const updated=await page.evaluate(()=>(window as any).__performance.calls.filter((s:string)=>s==="assistant_session_window_get").length);
  expect(updated-reads).toBe(1);
  const details = await page.evaluate(() => (window as any).__performance.calls.filter((name: string) => name === "assistant_turn_get"));
  expect(details).toHaveLength(1);
});

test("ambiguous first send reuses the original request even after session discovery", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.attempts = [];
    p.values.assistant_config_get.settings.enabled = true;
    p.values.assistant_send = ({ request }: any) => {
      p.attempts.push(request);
      p.values.assistant_sessions_list = [{ id: "accepted-session", title: "已受理的会话", source: "desktop", active_turn_id: null }];
      p.values.assistant_session_get = { id: "accepted-session", source: "desktop", messages: [], turns: [] };
      if (p.attempts.length === 1) throw Error("合成传输超时，受理情况待核对");
      return { session_id: "accepted-session", turn_id: "accepted-turn", state: "queued" };
    };
  });
  await page.getByRole("button", { name: "刷新助理" }).click();
  await page.getByLabel("发送消息").fill("隔离重试消息");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("传输超时");
  await page.getByRole("button", { name: "刷新助理" }).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).__performance.client.getQueryData(["assistant","sessions"])?.items?.[0]?.id)).toBe("accepted-session");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  const attempts = await page.evaluate(() => (window as any).__performance.attempts);
  expect(attempts).toHaveLength(2); expect(attempts[0]).toEqual(attempts[1]);
});

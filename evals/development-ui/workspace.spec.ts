import { test, expect } from '@playwright/test';
async function open(page:any){await page.goto('/evals/development-ui/index.html');await expect(page.getByRole('button',{name:'运行开发',exact:true})).toBeEnabled();}
async function fill(page:any){await page.getByLabel('项目目录',{exact:true}).fill('F:/Synthetic/项目');await page.getByLabel('开发需求',{exact:true}).fill('合成私有需求，不能放入浏览器存储🙂');}

test('window close does not wait for a read of clean development input',async({page})=>{
  await page.addInitScript(()=>{(window as any).isTauri=true;});
  await open(page);
  await page.evaluate(()=>{(window as any).__developmentWorkspace.holdGet=true;});
  await page.getByRole('button',{name:'刷新开发草稿',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>typeof (window as any).__developmentWorkspace.releaseGet)).toBe('function');
  await expect.poll(()=>page.evaluate(()=>typeof (window as any).__developmentClose.handler)).toBe('number');
  await page.evaluate(()=>(window as any).__developmentClose.request());
  try {
    await expect.poll(()=>page.evaluate(()=>(window as any).__developmentClose.attempts),{timeout:2500}).toBe(1);
    expect(await page.evaluate(()=>(window as any).__development.calls.filter(({command}:any)=>/workspace_(save|prepare|new)$/.test(command)))).toEqual([]);
  } finally {
    await page.evaluate(()=>(window as any).__developmentWorkspace.releaseGet());
  }
});

test('window close waits for an in-progress legacy import to be protected',async({page})=>{
  const original={request_id:'legacy-close-fixture',workspace:'F:/Synthetic/项目',brief:'退出时保留原导入',workflow:'development-local',checks:[],protected_files:[]};
  await page.clock.install();
  await page.addInitScript(value=>{
    (window as any).isTauri=true;
    (window as any).__developmentFlags={holdPrepare:true};
    localStorage.setItem('homebase.development.pending.v1',value);
  },JSON.stringify(original));
  await page.goto('/evals/development-ui/index.html');
  await expect.poll(()=>page.evaluate(()=>typeof (window as any).__developmentWorkspace.releasePrepare)).toBe('function');
  await expect.poll(()=>page.evaluate(()=>typeof (window as any).__developmentClose.handler)).toBe('number');
  await page.evaluate(()=>(window as any).__developmentClose.request());
  await page.clock.runFor(500);
  expect(await page.evaluate(()=>(window as any).__developmentClose.attempts)).toBe(0);
  expect(await page.evaluate(()=>localStorage.getItem('homebase.development.pending.v1'))).toBe(JSON.stringify(original));
  await page.evaluate(()=>(window as any).__developmentWorkspace.releasePrepare());
  await expect.poll(()=>page.evaluate(()=>(window as any).__developmentClose.attempts)).toBe(1);
  expect(await page.evaluate(()=>(window as any).__developmentWorkspace.entry.pending)).toEqual(original);
  expect(await page.evaluate(()=>localStorage.getItem('homebase.development.pending.v1'))).toBeNull();
  expect(await page.evaluate(()=>(window as any).__development.calls.some(({command}:any)=>command==='development_run'))).toBe(false);
});

test('window close waits for a reverted edit while its earlier save is still in flight',async({page})=>{
  await page.clock.install();
  await page.addInitScript(()=>{(window as any).isTauri=true;});
  await open(page);
  await page.evaluate(()=>{(window as any).__developmentWorkspace.saveDelay=1800;});
  const brief=page.getByLabel('开发需求',{exact:true});
  await brief.fill('中间版已发往保存');
  await page.clock.runFor(500);
  await expect.poll(()=>page.evaluate(()=>(window as any).__developmentWorkspace.entry.draft.brief)).toBe('中间版已发往保存');
  await brief.fill('');
  await page.evaluate(()=>(window as any).__developmentClose.request());
  await page.clock.runFor(350);
  expect(await page.evaluate(()=>(window as any).__developmentClose.attempts)).toBe(0);
  await page.evaluate(()=>{(window as any).__developmentWorkspace.saveDelay=0;});
  await page.clock.runFor(1700);
  await expect.poll(()=>page.evaluate(()=>(window as any).__developmentClose.attempts)).toBe(1);
  expect(await page.evaluate(()=>(window as any).__developmentWorkspace.entry.draft.brief)).toBe('');
});

test('unsubmitted raw input is protected and restored without executing',async({page})=>{
  await open(page);await fill(page);await page.getByText('验收设置',{exact:true}).click();await page.getByLabel('验收命令（JSON 参数数组）').fill('[{未写完');
  await expect(page.getByRole('status')).toContainText('已受保护保存');
  const entry=await page.evaluate(()=>(window as any).__developmentWorkspace.entry);expect(entry.draft.checks_json).toBe('[{未写完');expect(entry.draft.brief).toContain('合成私有需求');
  await page.addInitScript(e=>(window as any).__developmentSeed=e,entry);await page.reload();await expect(page.getByLabel('开发需求',{exact:true})).toHaveValue(entry.draft.brief);expect(await page.evaluate(()=>(window as any).__development.calls.filter((c:any)=>c.command==='development_run').length)).toBe(0);
});
test('pending original is protected before IPC and not stored in localStorage',async({page})=>{
  await open(page);await fill(page);await page.evaluate(()=>(window as any).__development.lost=true);await page.getByRole('button',{name:'运行开发',exact:true}).click();await expect(page.getByRole('alert')).toContainText('response lost');
  expect(await page.evaluate(()=>localStorage.getItem('homebase.development.pending.v1'))).toBeNull();const state=await page.evaluate(()=>(window as any).__developmentWorkspace.entry);expect(state.pending.brief).toContain('合成私有需求');
  const commands=await page.evaluate(()=>(window as any).__development.calls.map((c:any)=>c.command));expect(commands.indexOf('development_workspace_prepare')).toBeLessThan(commands.indexOf('development_run'));
  await page.addInitScript(e=>(window as any).__developmentSeed=e,state);await page.reload();await expect(page.getByRole('button',{name:'运行开发',exact:true})).toBeDisabled();await page.getByRole('button',{name:'查询原请求',exact:true}).click();await expect(page.getByText('执行中',{exact:true})).toBeVisible();expect(await page.evaluate(()=>(window as any).__development.calls.some((c:any)=>c.command==='development_run'))).toBe(false);
});
test('lost preparation response is read back and never executes',async({page})=>{
  await open(page);await fill(page);await page.evaluate(()=>(window as any).__developmentWorkspace.losePrepare=true);await page.getByRole('button',{name:'运行开发',exact:true}).click();
  await expect(page.getByRole('alert')).toContainText('prepare response lost');expect(await page.evaluate(()=>(window as any).__development.calls.some((c:any)=>c.command==='development_run'))).toBe(false);await expect(page.getByRole('button',{name:'运行开发',exact:true})).toBeDisabled();await expect(page.getByRole('button',{name:'查询原请求',exact:true})).toBeVisible();
});
test('legacy request imports before removal and never automatically runs',async({page})=>{
  const original={request_id:'legacy-synthetic',workspace:'F:/Synthetic/项目',brief:'合成旧请求🙂',checks:[['node','--test']],protected_files:[]};
  await page.addInitScript(r=>localStorage.setItem('homebase.development.pending.v1',JSON.stringify(r)),original);await page.goto('/evals/development-ui/index.html');await expect(page.getByRole('button',{name:'查询原请求',exact:true})).toBeVisible();expect(await page.evaluate(()=>localStorage.getItem('homebase.development.pending.v1'))).toBeNull();expect(await page.evaluate(()=>(window as any).__developmentWorkspace.entry.pending)).toEqual({...original,workflow:'development-local'});expect(await page.evaluate(()=>(window as any).__development.calls.some((c:any)=>c.command==='development_run'))).toBe(false);
});
test('failed legacy protection keeps the exact browser record and prevents running',async({page})=>{
  const raw=JSON.stringify({request_id:'legacy-failed',workspace:'F:/Synthetic/项目',brief:'合成旧稿，保存失败不能移除',workflow:'development-local',checks:[],protected_files:[]});
  await page.addInitScript(value=>{localStorage.setItem('homebase.development.pending.v1',value);(window as any).__developmentFlags={failSave:true};},raw);await page.goto('/evals/development-ui/index.html');await expect(page.getByRole('alert')).toContainText('DEVELOPMENT_WORKSPACE_SAVE');await expect(page.getByRole('button',{name:'运行开发',exact:true})).toBeDisabled();expect(await page.evaluate(()=>localStorage.getItem('homebase.development.pending.v1'))).toBe(raw);expect(await page.evaluate(()=>(window as any).__development.calls.some((c:any)=>c.command==='development_run'))).toBe(false);
});
test('new request archives original and historical lookup uses its exact task identity',async({page})=>{
  await open(page);await fill(page);await page.getByRole('button',{name:'运行开发',exact:true}).click();await expect(page.getByText('已受理 · 待执行',{exact:true})).toBeVisible();const original=await page.evaluate(()=>(window as any).__developmentWorkspace.entry.pending);
  page.once('dialog',dialog=>void dialog.accept());await page.getByRole('button',{name:'新建开发任务',exact:true}).click();await expect(page.getByRole('button',{name:'运行开发',exact:true})).toBeEnabled();await page.getByText('历史原请求（1）',{exact:true}).click();await page.getByRole('button',{name:`查询历史原请求 ${original.request_id}`,exact:true}).click();
  const expected=await page.evaluate(async id=>'dev-'+Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(id)))).map(v=>v.toString(16).padStart(2,'0')).join('').slice(0,32),original.request_id);await expect.poll(()=>page.evaluate(()=>(window as any).__development.calls.filter((c:any)=>c.command==='development_result').at(-1)?.args.taskId)).toBe(expected);expect(await page.evaluate(()=>(window as any).__developmentWorkspace.entry.previous[0])).toEqual(original);
});
test('late saves retain newer input and blur flushes the final edit',async({page})=>{
  await open(page);await fill(page);await page.evaluate(()=>(window as any).__developmentWorkspace.saveDelay=650);await expect.poll(()=>page.evaluate(()=>(window as any).__development.calls.filter((c:any)=>c.command==='development_workspace_save').length)).toBeGreaterThan(0);await page.getByLabel('开发需求',{exact:true}).fill('后来输入B🙂，不能被A的回执覆盖');await page.evaluate(()=>window.dispatchEvent(new Event('blur')));await expect(page.getByRole('status')).toContainText('已受保护保存');expect(await page.evaluate(()=>(window as any).__developmentWorkspace.entry.draft.brief)).toBe('后来输入B🙂，不能被A的回执覆盖');await expect(page.getByLabel('开发需求',{exact:true})).toHaveValue('后来输入B🙂，不能被A的回执覆盖');
});
test('explicit refresh preserves conflicting local and external drafts until a choice',async({page})=>{
  await open(page);await fill(page);await expect(page.getByRole('status')).toContainText('已受保护保存');await page.evaluate(()=>{const s=(window as any).__developmentWorkspace;s.entry.revision++;s.entry.draft.brief='服务端C';});await page.getByRole('button',{name:'刷新开发草稿',exact:true}).click();await expect(page.getByLabel('开发需求',{exact:true})).toHaveValue('服务端C');
  await page.getByLabel('开发需求',{exact:true}).fill('本地B');await page.evaluate(()=>{const s=(window as any).__developmentWorkspace;s.entry.revision++;s.entry.draft.brief='后来服务端D';});await page.getByRole('button',{name:'刷新开发草稿',exact:true}).click();await expect(page.getByRole('button',{name:'采用已保存的开发草稿',exact:true})).toBeVisible();await expect(page.getByLabel('开发需求',{exact:true})).toHaveValue('本地B');expect(await page.evaluate(()=>(window as any).__developmentWorkspace.entry.draft.brief)).toBe('后来服务端D');await page.getByRole('button',{name:'保留当前输入并保存',exact:true}).click();await expect(page.getByRole('status')).toContainText('已受保护保存');expect(await page.evaluate(()=>(window as any).__developmentWorkspace.entry.draft.brief)).toBe('本地B');
});
test('unknown preparation retains one identity and freezes editing until an explicit same-request retry',async({page})=>{
  await open(page);await fill(page);await page.evaluate(()=>{const s=(window as any).__developmentWorkspace;s.losePrepare=true;s.failGet=true;});await page.getByRole('button',{name:'运行开发',exact:true}).click();await expect(page.getByRole('button',{name:'重试原准备并运行',exact:true})).toBeVisible();await expect(page.getByLabel('开发需求',{exact:true})).toBeDisabled();expect(await page.evaluate(()=>(window as any).__development.calls.some((c:any)=>c.command==='development_run'))).toBe(false);
  const original=await page.evaluate(()=>(window as any).__developmentWorkspace.entry.pending);await page.evaluate(()=>{const s=(window as any).__developmentWorkspace;s.losePrepare=false;s.failGet=false;});await page.getByRole('button',{name:'重试原准备并运行',exact:true}).click();await expect(page.getByText('已受理 · 待执行',{exact:true})).toBeVisible();const calls=await page.evaluate(()=>(window as any).__development.calls.filter((c:any)=>c.command==='development_run'));expect(calls).toHaveLength(1);expect(calls[0].args.request).toEqual(original);
});
test('save failure keeps editable input and blocks execution until explicitly saved',async({page})=>{
  await open(page);await page.evaluate(()=>(window as any).__developmentWorkspace.failSave=true);await fill(page);await expect(page.getByRole('alert')).toContainText('DEVELOPMENT_WORKSPACE_SAVE');await page.getByRole('button',{name:'运行开发',exact:true}).click();expect(await page.evaluate(()=>(window as any).__development.calls.some((c:any)=>c.command==='development_run'))).toBe(false);await expect(page.getByLabel('开发需求',{exact:true})).toHaveValue('合成私有需求，不能放入浏览器存储🙂');await page.evaluate(()=>(window as any).__developmentWorkspace.failSave=false);await page.getByRole('button',{name:'保存开发草稿',exact:true}).click();await expect(page.getByRole('status')).toContainText('已受保护保存');
});
test('a lost development save reply blocks background writes and preserves later changes', async ({page}) => {
  await page.clock.install();
  await open(page);
  await page.evaluate(() => { (window as any).__developmentWorkspace.loseSave = true; });
  const brief = page.getByLabel('开发需求', {exact:true});
  await brief.fill('DURABLE DEVELOPMENT DRAFT');
  await page.clock.runFor(650);
  await expect(page.getByRole('alert')).toContainText('ISOLATED_DEVELOPMENT_SAVE_ACK_LOST');
  await brief.fill('NEWER DEVELOPMENT DRAFT');
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await page.clock.runFor(650);
  const count = () => page.evaluate(() => (window as any).__development.calls.filter((c: any) => c.command === 'development_workspace_save').length);
  expect(await count()).toBe(1);
  await page.getByRole('button', {name:'刷新开发草稿', exact:true}).click();
  await expect(brief).toHaveValue('NEWER DEVELOPMENT DRAFT');
  await expect(page.getByRole('button', {name:'保留当前输入并保存', exact:true})).toHaveCount(0);
  expect(await count()).toBe(1);
  await page.getByRole('button', {name:'保存开发草稿', exact:true}).click();
  await expect(page.getByRole('status')).toContainText('已受保护保存');
  expect(await count()).toBe(2);
  expect(await page.evaluate(() => (window as any).__developmentWorkspace.entry.draft.brief)).toBe('NEWER DEVELOPMENT DRAFT');
  expect(await page.evaluate(() => (window as any).__development.calls.filter((c: any) => /development_(run|workspace_prepare)/.test(c.command)))).toEqual([]);
});
test('legacy import never overwrites an already protected different draft',async({page})=>{
  const raw=JSON.stringify({request_id:'legacy-conflict',workspace:'F:/Synthetic/项目',brief:'旧请求A',workflow:'development-local',checks:[],protected_files:[]});
  await page.addInitScript(value=>{localStorage.setItem('homebase.development.pending.v1',value);(window as any).__developmentSeed={schema_version:1,revision:1,draft:{workspace:'F:/Synthetic/项目',brief:'新受保护草稿B',workflow:'development-local',checks_json:'[]',protected_files_input:''},pending:null,previous:[]};},raw);await page.goto('/evals/development-ui/index.html');await expect(page.getByRole('alert')).toContainText('双方均保留');expect(await page.evaluate(()=>localStorage.getItem('homebase.development.pending.v1'))).toBe(raw);expect(await page.evaluate(()=>(window as any).__developmentWorkspace.entry.draft.brief)).toBe('新受保护草稿B');expect(await page.evaluate(()=>(window as any).__development.calls.some((c:any)=>c.command==='development_run'||c.command==='development_workspace_prepare'))).toBe(false);
});

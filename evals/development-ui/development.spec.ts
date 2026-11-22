import { test, expect } from '@playwright/test';
async function open(page:any){await page.goto('/evals/development-ui/index.html');await expect(page.getByRole('button',{name:'运行开发',exact:true})).toBeEnabled();}
async function fill(page:any){await page.getByLabel('项目目录',{exact:true}).fill('F:\\Synthetic\\项目');await page.getByLabel('开发需求',{exact:true}).fill('只实现一个合成函数并验收。');}
test('parameters are structured and a double click starts one task',async({page})=>{
  await open(page);await fill(page);await page.getByText('验收设置',{exact:true}).click();await page.getByLabel('验收命令（JSON 参数数组）').fill('[["node","--test"]]');await page.getByLabel('协作流程').selectOption('execution-checked');await page.getByRole('button',{name:'运行开发',exact:true}).dblclick();
  await expect(page.getByText('已受理 · 待执行',{exact:true})).toBeVisible();const calls=await page.evaluate(()=>(window as any).__development.calls.filter((c:any)=>c.command==='development_run'));expect(calls).toHaveLength(1);expect(calls[0].args.request.checks).toEqual([['node','--test']]);expect(calls[0].args.request.workflow).toBe('execution-checked');
});
test('lost response keeps original request and reload only queries',async({page})=>{
  await open(page);await fill(page);await page.evaluate(()=>(window as any).__development.lost=true);await page.getByRole('button',{name:'运行开发',exact:true}).click();await expect(page.getByRole('alert')).toContainText('response lost');const saved=await page.evaluate(()=>(window as any).__developmentWorkspace.entry);await page.addInitScript(e=>(window as any).__developmentSeed=e,saved);await page.reload();await expect(page.getByRole('button',{name:'运行开发',exact:true})).toBeDisabled();await page.getByRole('button',{name:'查询原请求',exact:true}).click();await expect(page.getByText('执行中',{exact:true})).toBeVisible();expect(await page.evaluate(()=>(window as any).__development.calls.filter((c:any)=>c.command==='development_run').length)).toBe(0);expect(await page.evaluate(()=>(window as any).__developmentWorkspace.entry.pending)).toEqual(saved.pending);expect(await page.evaluate(()=>localStorage.getItem('homebase.development.pending.v1'))).toBeNull();
});
test('invalid checks never prepare or invoke development',async({page})=>{
  await open(page);await fill(page);await page.getByText('验收设置',{exact:true}).click();await page.getByLabel('验收命令（JSON 参数数组）').fill('["node --test"]');await page.getByRole('button',{name:'运行开发',exact:true}).click();await expect(page.getByRole('alert')).toContainText('二维');expect(await page.evaluate(()=>localStorage.getItem('homebase.development.pending.v1'))).toBeNull();expect(await page.evaluate(()=>(window as any).__development.calls.some((c:any)=>c.command==='development_run'))).toBe(false);
});
test('manual question and unknown result remain distinct from success',async({page})=>{
  await open(page);await fill(page);await page.getByRole('button',{name:'运行开发',exact:true}).click();await page.evaluate(()=>{const s=(window as any).__development;s.phase='awaiting_input';s.questions=[{id:7,title:'继续哪一步？',options:['只完成实现','停止']}];});await page.getByRole('button',{name:'刷新原任务',exact:true}).click();await expect(page.getByText('需要你回答',{exact:true})).toBeVisible();await page.getByRole('button',{name:'只完成实现',exact:true}).click();const answer=await page.evaluate(()=>(window as any).__development.calls.find((c:any)=>c.command==='development_answer'));expect(answer.args.request.option).toBe(1);expect(answer.args.request.card_id).toBe(7);
  await page.evaluate(()=>(window as any).__development.phase='unknown');await page.getByRole('button',{name:'刷新原任务',exact:true}).click();await expect(page.getByText('原状态未确认',{exact:true})).toBeVisible();await expect(page.getByText('宿主验收通过',{exact:true})).toHaveCount(0);
});
test('corrupt saved request is not overwritten, narrow view has no page overflow',async({page})=>{
  await page.addInitScript(()=>localStorage.setItem('homebase.development.pending.v1','{broken'));await page.setViewportSize({width:420,height:900});await page.goto('/evals/development-ui/index.html');await expect(page.getByRole('alert')).toContainText('原记录未覆盖');await expect(page.getByRole('button',{name:'运行开发',exact:true})).toBeDisabled();expect(await page.evaluate(()=>localStorage.getItem('homebase.development.pending.v1'))).toBe('{broken');expect(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth)).toBe(true);
});
test('a late pre-answer observation cannot overwrite the accepted answer',async({page})=>{
  await open(page);await fill(page);await page.getByRole('button',{name:'运行开发',exact:true}).click();
  await page.evaluate(()=>{const s=(window as any).__development;s.phase='awaiting_input';s.questions=[{id:8,title:'合成问题',options:['继续']}];});
  await page.getByRole('button',{name:'刷新原任务',exact:true}).click();await expect(page.getByRole('button',{name:'继续',exact:true})).toBeVisible();
  await page.evaluate(()=>(window as any).__development.resultDelay=600);await page.getByRole('button',{name:'刷新原任务',exact:true}).click();await page.getByRole('button',{name:'继续',exact:true}).click();
  await expect(page.getByText('执行中',{exact:true})).toBeVisible();await page.waitForTimeout(800);await expect(page.getByText('执行中',{exact:true})).toBeVisible();await expect(page.getByRole('button',{name:'继续',exact:true})).toHaveCount(0);
});

import {test,expect} from '@playwright/test';
test.beforeEach(async({page})=>{
  await page.route('**/*',r=>new URL(r.request().url()).hostname==='127.0.0.1'?r.continue():r.abort());
  await page.goto('/evals/ai-control-ui/performance.html');
  await page.evaluate(()=>{
    const f=(window as any).__performance;let project:any=null;let registered=false;
    f.values.repo_projects_list=()=>({status:'succeeded',entries:project?[project]:[],total:project?1:0,next_offset:null});
    f.values.repo_capabilities_list=()=>({status:'succeeded',entries:registered?[{id:'repo.test.fixture',name:'Markdown 转 HTML',description:'Fixture converter',version:'1.2.3',license:'MIT',state:'verified'}]:[],total:registered?1:0,next_offset:null});
    f.values.repo_project_analyze=()=>{project={id:'fixture-project',name:'fixture',repository:'https://github.com/test/fixture',phase:'analyzed',revision:1,source:{supported:true,description:'Fixture converter',license:'MIT',source_commit:'a'.repeat(40),package:{name:'fixture',version:'1.2.3'},profile:{examples:[]},views:[{id:'source',name:'固定源码',url:'https://github.com/test/fixture/tree/'+ 'a'.repeat(40),pinned:true}]}};return {status:'succeeded',project};};
    f.values.repo_project_get=()=>({status:'succeeded',project});
    f.values.repo_project_install=async()=>{project={...project,phase:'installing',operation:{request_id:'fixture-install'}};await new Promise(r=>setTimeout(r,350));registered=true;project={...project,phase:'registered',revision:2,verification:{passed:true,tests:2}};return {status:'succeeded',project};};
    f.values.repo_capability_describe={status:'succeeded',capability:{id:'repo.test.fixture',name:'Markdown 转 HTML',notice:'HTML作为文本返回',input_schema:{type:'object'},examples:[{input:{stdin:'# Hello'}}]},verification:{passed:true}};
    f.values.repo_capability_run={status:'succeeded',stdout:'<script>window.__unsafe=true</script><h1>Hello</h1>',run_id:'fixture-run',artifact:{path:'X:/isolated/result.txt'}};
    f.values.repo_capability_result=f.values.repo_capability_run;
  });
  await page.getByRole('link',{name:'能力库',exact:true}).click();await page.getByRole('tab',{name:'开源项目',exact:true}).click();
});
test('repository analysis to registered invocation survives navigation and renders HTML as text',async({page})=>{
  const section=page.getByRole('region',{name:'GitHub 项目自动接入'});
  await section.getByLabel('仓库地址',{exact:true}).fill('https://github.com/test/fixture');
  await section.getByRole('button',{name:'分析仓库',exact:true}).click();
  await expect(section.getByText('待安装验收',{exact:true}).first()).toBeVisible();
  await section.getByRole('button',{name:'安装、封装并验收',exact:true}).click();
  await expect(section.getByText('已通过 2 条行为验收，已注册为可调用接口。')).toBeVisible();
  await page.getByRole('link',{name:'工作台',exact:true}).click();await page.getByRole('link',{name:'能力库',exact:true}).click();
  await section.getByRole('button',{name:'调用与接口'}).click();
  await page.getByRole('button',{name:'隔离调用',exact:true}).click();
  await expect(page.getByLabel('能力输出')).toContainText('<script>window.__unsafe=true</script>');
  expect(await page.evaluate(()=>(window as any).__unsafe)).toBeUndefined();
  await page.getByRole('button',{name:'查询原任务结果'}).click();
  await page.getByRole('button',{name:'关闭能力调用'}).click();
  await section.getByRole('button',{name:'调用与接口'}).click();
  await expect(page.getByLabel('能力输出')).toContainText('<h1>Hello</h1>');
  await page.getByRole('button',{name:'关闭能力调用'}).click();
  const calls=await page.evaluate(()=>(window as any).__performance.calls as string[]);
  expect(calls.filter(x=>x==='repo_project_install')).toHaveLength(1);expect(calls.filter(x=>x==='repo_capability_run')).toHaveLength(1);
  await page.locator('main').evaluate(e=>e.scrollTo(0,0));
  await page.screenshot({path:'.tmp/repository-capabilities-desktop.png'});
  await page.setViewportSize({width:390,height:844});
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth)).toBe(true);
  await page.screenshot({path:'.tmp/repository-capabilities-mobile.png',fullPage:true});
});
test('pending installation remains single after leaving and reopening the page',async({page})=>{
  const section=page.getByRole('region',{name:'GitHub 项目自动接入'});
  await section.getByLabel('仓库地址',{exact:true}).fill('https://github.com/test/fixture');await section.getByRole('button',{name:'分析仓库',exact:true}).click();
  await expect(section.getByRole('button',{name:'安装、封装并验收',exact:true})).toBeEnabled();
  await page.evaluate(()=>{const f=(window as any).__performance;const prior=f.values.repo_project_install;f.values.repo_project_install=async()=>{await new Promise(r=>setTimeout(r,2200));return prior();};});
  await section.getByRole('button',{name:'安装、封装并验收',exact:true}).click();
  await page.getByRole('link',{name:'工作台',exact:true}).click();await page.getByRole('link',{name:'能力库',exact:true}).click();
  await expect(section.getByRole('button',{name:'安装、封装并验收',exact:true})).toBeDisabled();
  await expect(section.getByText('已通过 2 条行为验收，已注册为可调用接口。')).toBeVisible();
  expect(await page.evaluate(()=>(window as any).__performance.calls.filter((x:string)=>x==='repo_project_install'))).toHaveLength(1);
});
test('manual refresh reports read failure and never starts installation',async({page})=>{
  const section=page.getByRole('region',{name:'GitHub 项目自动接入'});
  await page.evaluate(()=>{(window as any).__performance.values.repo_projects_list=()=>{throw Error('synthetic read failure');};});
  await section.getByRole('button',{name:'刷新',exact:true}).click();
  await expect(section.getByRole('alert')).toContainText('项目读取失败');
  expect(await page.evaluate(()=>(window as any).__performance.calls.filter((x:string)=>/repo_project_install|repo_capability_run/.test(x)))).toEqual([]);
});

test('sandbox preflight failure is shown without replacing the analyzed project or retrying', async ({ page }) => {
  const section = page.getByRole('region', { name: 'GitHub 项目自动接入' });
  const message = '等待 WSL 隔离进程就绪超时，未提交仓库代码。请核对 Linux 发行版能否启动。';
  await page.evaluate(message => {
    (window as any).__performance.values.repo_project_install = () => ({
      status: 'failed', error: { code: 'SANDBOX_NOT_READY', message }
    });
  }, message);
  await section.getByLabel('仓库地址', { exact: true }).fill('https://github.com/test/fixture');
  await section.getByRole('button', { name: '分析仓库', exact: true }).click();
  const install = section.getByRole('button', { name: '安装、封装并验收', exact: true });
  await install.click();
  await expect(section.getByRole('alert')).toContainText(message);
  await expect(section.getByText('待安装验收', { exact: true }).first()).toBeVisible();
  await expect(install).toBeEnabled();
  await section.getByRole('button', { name: '刷新', exact: true }).click();
  const calls = await page.evaluate(() => (window as any).__performance.calls as string[]);
  expect(calls.filter(name => name === 'repo_project_install')).toHaveLength(1);
});

test('the original isolation check can be canceled without starting installation again', async ({ page }) => {
  const section = page.getByRole('region', { name: 'GitHub 项目自动接入' });
  await page.evaluate(() => {
    const f = (window as any).__performance;
    const read = f.values.repo_project_get;
    let operation: string | undefined;
    let canceled: (() => void) | undefined;
    f.values.repo_project_get = () => {
      const current = read();
      return operation ? { ...current, project: { ...current.project, phase: 'preflighting', operation: { request_id: operation } } } : current;
    };
    f.values.repo_project_install = async ({ request }: { request: { request_id: string } }) => {
      operation = request.request_id;
      await new Promise<void>(resolve => { canceled = resolve; });
      operation = undefined;
      return { ...read(), status: 'canceled', message: '已取消隔离检查，下载尚未开始。' };
    };
    f.values.repo_operation_cancel = ({ request }: { request: { operation_id: string } }) => {
      if (request.operation_id !== operation || !canceled) throw Error('operation mismatch');
      canceled();
      return { status: 'succeeded', state: 'cancel_requested' };
    };
  });
  await section.getByLabel('仓库地址', { exact: true }).fill('https://github.com/test/fixture');
  await section.getByRole('button', { name: '分析仓库', exact: true }).click();
  const install = section.getByRole('button', { name: '安装、封装并验收', exact: true });
  await install.click();
  await expect(section.getByText('正在检查隔离环境', { exact: true })).toBeVisible();
  await section.getByRole('button', { name: '取消此操作', exact: true }).click();
  await expect(section.getByText('已取消隔离检查，下载尚未开始。')).toBeVisible();
  await expect(install).toBeEnabled();
  const calls = await page.evaluate(() => (window as any).__performance.calls as string[]);
  expect(calls.filter(name => name === 'repo_project_install')).toHaveLength(1);
  expect(calls.filter(name => name === 'repo_operation_cancel')).toHaveLength(1);
});

test('an offline repository request exposes the host error and never retries after reconnect', async ({ page }) => {
  const section = page.getByRole('region', { name: 'GitHub 项目自动接入' });
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.repo_project_analyze = () => { throw Error('合成宿主网络不可用'); };
    window.dispatchEvent(new Event('offline'));
  });
  await section.getByLabel('仓库地址', { exact: true }).fill('https://github.com/test/fixture');
  await section.getByRole('button', { name: '分析仓库', exact: true }).click();
  await expect(section.getByRole('alert')).toContainText('合成宿主网络不可用');
  await page.evaluate(async () => {
    window.dispatchEvent(new Event('online'));
    await (window as any).__performance.client.resumePausedMutations();
  });
  const called = await page.evaluate(() => (window as any).__performance.calls as string[]);
  expect(called.filter(name => name === 'repo_project_analyze')).toHaveLength(1);
  expect(called).not.toContain('repo_project_install');
});

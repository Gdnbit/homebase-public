import {test,expect} from '@playwright/test';
for(const [provider,label]of [['claude-code','Claude Code'],['gemini-cli','Gemini CLI']]){
  test(`${label} creates, sends, switches pages and continues with the same identity`,async({page})=>{
    await page.route('**/*',r=>new URL(r.request().url()).hostname==='127.0.0.1'?r.continue():r.abort());
    await page.goto('/evals/ai-control-ui/performance.html');
    await page.evaluate(({provider,label})=>{
      const p=(window as any).__performance;p.nativeRequests=[];
      p.values.conversation_providers={providers:[{id:'homebase',name:'HomeBase 助理',interactive:true},{id:provider,name:label,interactive:true}]};
      p.values.assistant_local_config_get={credential_references:[{id:'api_key_manager:fixture',name:'隔离引用'}]};
      p.values.conversation_native=({request:r}:any)=>{p.nativeRequests.push(r);
        if(r.action==='list')return {data:[]};
        if(r.action==='create')return {state:'prepared',thread:{id:'fixture-thread',turns:[]},assistant_session_id:'linked'};
        if(r.action==='read')return {thread:{id:'fixture-thread',turns:[]}};
        if(r.action==='send'){p.nativeSent=true;return {state:'accepted',turn:{id:'turn-1'},assistant_session_id:'linked'};}
        if(r.action==='status')return p.nativeSent?{id:r.thread_id,provider:r.provider,state:'completed',items:{a:{id:'a',type:'agentMessage',text:'同一原生会话已回复'}},item_order:['a']}:{id:r.thread_id,provider:r.provider,state:'idle',items:{},item_order:[]};
        throw Error('Unexpected mutation');};
    },{provider,label});
    await page.getByRole('link',{name:'会话',exact:true}).click();
    await page.getByLabel('新会话使用的 Agent').selectOption(provider);
    await page.getByLabel('原生 Agent 凭据引用').selectOption('api_key_manager:fixture');
    await page.getByRole('button',{name:'新建会话',exact:true}).click();
    await page.getByRole('button',{name:'刷新原生会话',exact:true}).click();
    await page.getByLabel(`给 ${label} 发消息`).fill('核对可用能力');
    await expect(page.getByRole('button',{name:'发送',exact:true})).toBeEnabled();
    await page.getByRole('button',{name:'发送',exact:true}).click();
    await expect(page.getByText('同一原生会话已回复',{exact:true})).toBeVisible();
    await page.getByLabel(`给 ${label} 发消息`).fill('尚未发送的补充');
    await page.getByRole('link',{name:'能力库',exact:true}).click();
    await page.getByRole('link',{name:'会话',exact:true}).click();
    await expect(page.getByLabel(`给 ${label} 发消息`)).toHaveValue('尚未发送的补充');
    const requests=await page.evaluate(()=>(window as any).__performance.nativeRequests);
    const mutations=requests.filter((r:any)=>['create','send','interrupt'].includes(r.action));
    expect(mutations.map((r:any)=>r.action)).toEqual(['create','send']);
    expect(mutations.every((r:any)=>r.provider===provider)).toBe(true);
    expect(mutations[0].credential_ref).toBe('api_key_manager:fixture');
    await page.screenshot({path:`.tmp/native-${provider}.png`,fullPage:true});
  });
}
for(const outcome of ['fresh','stale'])test(`native completion shares an in-flight ${outcome} history refresh and verifies the saved result`,async({page})=>{
  await page.goto('/evals/ai-control-ui/performance.html');
  await page.evaluate(outcome=>{
    const p=(window as any).__performance;p.slowHistory=[];p.historyReads=0;p.nativeWasSent=false;
    p.values.conversation_providers={providers:[{id:'homebase',name:'HomeBase 助理',interactive:true},{id:'gemini-cli',name:'Gemini CLI',interactive:true}]};
    p.values.conversation_native=({request:r}:any)=>{
      if(r.action==='list')return {data:[]};
      if(r.action==='create')return {state:'prepared',thread:{id:'slow-history-thread',turns:[]}};
      if(r.action==='send'){p.nativeWasSent=true;return {state:'accepted',turn:{id:'slow-turn'}};}
      if(r.action==='status')return p.nativeWasSent?{id:r.thread_id,provider:r.provider,state:'completed',items:{a:{id:'a',type:'agentMessage',text:'完成慢历史测试'}},item_order:['a']}:{id:r.thread_id,provider:r.provider,state:'idle',items:{},item_order:[]};
      if(r.action==='read'){
        if(!p.nativeWasSent)return {thread:{id:r.thread_id,turns:[]}};
        p.historyReads++;
        const result={thread:{id:r.thread_id,turns:[{id:'slow-turn',items:[{id:'a',type:'agentMessage',text:'完成慢历史测试'}]}]}};
        if(p.historyReads>1)return result;
        return new Promise(resolve=>p.slowHistory.push(()=>resolve(outcome==='stale'?{thread:{id:r.thread_id,turns:[]}}:result)));
      }
      throw Error('Unexpected fixture operation');
    };
  },outcome);
  await page.getByRole('link',{name:'会话',exact:true}).click();
  await page.getByLabel('新会话使用的 Agent').selectOption('gemini-cli');
  await page.getByRole('button',{name:'新建会话',exact:true}).click();
  await page.getByLabel('给 Gemini CLI 发消息').fill('只测试一次历史刷新');
  await page.getByRole('button',{name:'发送',exact:true}).click();
  await expect(page.getByText('完成慢历史测试',{exact:true})).toBeVisible();
  await page.evaluate(()=>new Promise<void>(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve()))));
  expect(await page.evaluate(()=>(window as any).__performance.historyReads)).toBe(1);
  await page.evaluate(()=>{for(const done of (window as any).__performance.slowHistory)done();});
  await expect.poll(()=>page.evaluate(()=>(window as any).__performance.historyReads)).toBe(outcome==='stale'?2:1);
  await expect.poll(()=>page.evaluate(()=>JSON.stringify((window as any).__performance.client.getQueryData(['conversations','gemini-cli','slow-history-thread'])))).toContain('完成慢历史测试');
  await expect(page.getByText('完成慢历史测试',{exact:true})).toBeVisible();
});

test('old workflows prepare assistant drafts without starting an operation',async({page})=>{
  await page.goto('/evals/ai-control-ui/performance.html');
  await page.getByText('更多开始方式',{exact:true}).click();
  await page.getByRole('button',{name:'帮我写一份 Markdown 草稿，保存后给我预览和文件',exact:true}).click();
  await expect(page.getByLabel('发送消息',{exact:true})).toHaveValue(/Markdown 草稿/);
  expect(await page.evaluate(()=>(window as any).__performance.calls)).not.toContain('assistant_send');
});
test('registered Markdown result previews in a side panel without publishing or executing HTML',async({page})=>{
  await page.goto('/evals/ai-control-ui/performance.html');
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    const turn={id:'preview-turn',session_id:'preview-session',state:'completed',text:'草稿已保存',steps:[],jobs:[],artifacts:[{id:'owned-preview',name:'article.md',path:'X:/isolated/article.md',bytes:64,sha256:'fixture'}]};
    p.values.assistant_sessions_list=[{id:'preview-session',title:'草稿',source:'desktop',active_turn_id:null}];
    p.values.assistant_session_get={id:'preview-session',source:'desktop',messages:[{id:'a',role:'assistant',content:turn.text,turn_id:turn.id}],turns:[turn]};
    p.values.assistant_turn_get=turn;
    p.values.assistant_artifact_preview=({artifactId}:any)=>{if(artifactId!=='owned-preview')throw Error('Unowned result');return {name:'article.md',content:'# 隔离文稿\n\n<script>window.fixtureExecuted=true</script>',sha256:'fixture',markdown:true};};
  });
  await page.getByRole('button',{name:'刷新助理',exact:true}).click();
  await page.getByRole('button',{name:'预览',exact:true}).click();
  await expect(page.getByRole('complementary',{name:'结果预览'}).getByRole('heading',{name:'隔离文稿'})).toBeVisible();
  expect(await page.evaluate(()=>(window as any).fixtureExecuted)).toBeUndefined();
  await page.getByRole('button',{name:'关闭预览',exact:true}).click();
  const calls=await page.evaluate(()=>(window as any).__performance.calls);
  expect(calls.filter((c:string)=>c==='assistant_artifact_preview')).toHaveLength(1);
  expect(calls).not.toContain('assistant_send');expect(calls).not.toContain('assistant_artifact_open');
});

test.describe('independent result preview', () => {
  test.beforeEach(async ({page}) => {
    await page.route('**/*', route => new URL(route.request().url()).hostname === '127.0.0.1' ? route.continue() : route.abort());
    await page.goto('/evals/ai-control-ui/performance.html');
    await page.evaluate(() => {
      const fixture = (window as any).__performance;
      const turn = {id:'isolated-result',session_id:'isolated-session',state:'completed',text:'隔离任务完成',steps:[],jobs:[],
        artifacts:[{id:'result-a',name:'result.txt',path:'X:/isolated/result.txt',bytes:8,sha256:'original',mime_type:'text/plain'}]};
      fixture.values.assistant_sessions_list = [{id:turn.session_id,title:'隔离结果',source:'desktop',active_turn_id:null}];
      fixture.values.assistant_session_get = {id:turn.session_id,source:'desktop',messages:[{id:'result-message',role:'assistant',content:turn.text,turn_id:turn.id}],turns:[turn]};
      fixture.previewCalls = [];
      fixture.values.assistant_artifact_preview = ({artifactId}: any) => {
        fixture.previewCalls.push(artifactId);
        return {name:'result.txt',sha256:'original',content:'原结果正文',markdown:false};
      };
    });
    await page.getByRole('button', {name:'刷新助理',exact:true}).click();
  });

  test('a details failure never disables original preview or opening', async ({page}) => {
    await page.evaluate(() => {
      const fixture = (window as any).__performance;
      fixture.values.assistant_turn_get = () => { throw Error('隔离详情暂不可读'); };
      fixture.openRequests = [];
      fixture.values.assistant_artifact_open = ({artifactId}: any) => {
        if (fixture.openRequests.push(artifactId) === 1) throw Error('隔离打开失败');
      };
    });
    await page.getByText('执行详情 · 0 步', {exact:true}).click();
    await expect(page.getByText(/详情读取失败/)).toBeVisible();
    await page.getByRole('button', {name:'预览',exact:true}).click({timeout:3000});
    await expect(page.getByRole('complementary', {name:'结果预览'})).toContainText('原结果正文');
    await page.getByRole('button', {name:'关闭预览',exact:true}).click();
    const open = page.getByRole('button', {name:'打开结果',exact:true});
    const error = page.getByText('打开结果失败。隔离打开失败', {exact:true});
    await open.click();
    await expect(error).toBeVisible();
    await open.click();
    await expect(error).toHaveCount(0);
    expect(await page.evaluate(() => (window as any).__performance.openRequests)).toEqual(['result-a','result-a']);
  });

  test('preview distinguishes a failed read, a mismatched receipt and empty text', async ({page}) => {
    await page.evaluate(() => {
      const fixture = (window as any).__performance;
      fixture.values.assistant_artifact_preview = ({artifactId}: any) => {
        const attempts = fixture.previewCalls.push(artifactId);
        if (attempts === 1) throw Error('ARTIFACT_UNAVAILABLE: 隔离文件暂不可读');
        return {name:'result.txt',sha256:attempts === 2 ? 'different' : 'original',content:attempts === 2 ? '不匹配的文字' : '',markdown:false};
      };
    });
    const trigger = page.getByRole('button', {name:'预览',exact:true});
    await trigger.click();
    const panel = page.getByRole('complementary', {name:'结果预览'});
    await expect(panel.getByRole('alert')).toContainText('隔离文件暂不可读');
    await panel.getByRole('button', {name:'重试预览',exact:true}).click();
    await expect(panel.getByRole('alert')).toContainText('回执与原产物不匹配');
    await expect(panel.getByText('不匹配的文字', {exact:true})).toHaveCount(0);
    await panel.getByRole('button', {name:'重试预览',exact:true}).click();
    await expect(panel.getByText('结果文件没有可预览的文本。', {exact:true})).toBeVisible();
    await panel.getByRole('button', {name:'关闭预览',exact:true}).press('Escape');
    await expect(panel).toHaveCount(0);
    await expect(trigger).toBeFocused();
    const state = await page.evaluate(() => { const p = (window as any).__performance; return {calls:p.calls,ids:p.previewCalls}; });
    expect(state.ids).toEqual(['result-a','result-a','result-a']);
    expect(state.calls).not.toContain('assistant_send');
    expect(state.calls).not.toContain('assistant_artifact_open');
  });

  test('export keeps the original artifact through cancel, conflict and retry', async ({page}) => {
    await page.evaluate(() => {
      const fixture = (window as any).__performance;
      fixture.exportRequests = [];
      fixture.values.assistant_artifact_export = ({artifactId}: {artifactId:string}) => {
        const attempt = fixture.exportRequests.push(artifactId);
        if (attempt === 1) return new Promise(resolve => { fixture.cancelExport = () => resolve(null); });
        if (attempt === 2) throw Error('所选目录已有同名文件，请选择其他目录。');
        return {name:'result.txt',bytes:8,sha256:attempt === 3 ? 'different' : 'original'};
      };
    });
    const button = page.getByRole('button', {name:'导出副本',exact:true});
    const saved = page.getByText('副本已保存到所选目录。', {exact:true});
    await button.click();
    await expect(page.getByRole('button', {name:'正在导出…',exact:true})).toBeDisabled();
    await expect(page.getByRole('button', {name:'打开结果',exact:true})).toBeEnabled();
    await page.evaluate(() => (window as any).__performance.cancelExport());
    await expect(button).toBeEnabled();
    await expect(saved).toHaveCount(0);
    await button.click();
    await expect(page.getByText('所选目录已有同名文件，请选择其他目录。')).toBeVisible();
    await button.click();
    await expect(page.getByText('导出回执与原产物不匹配，请检查所选目录。')).toBeVisible();
    await expect(saved).toHaveCount(0);
    await button.click();
    await expect(saved).toBeVisible();
    const state = await page.evaluate(() => {
      const fixture = (window as any).__performance;
      return {ids:fixture.exportRequests,calls:fixture.calls};
    });
    expect(state.ids).toEqual(Array(4).fill('result-a'));
    expect(state.calls).not.toContain('assistant_send');
    expect(state.calls).not.toContain('assistant_artifact_open');
  });

  test('timed SRT preview keeps the registered subtitle and its timeline', async ({page}) => {
    await page.evaluate(() => {
      const fixture = (window as any).__performance;
      const [artifact] = fixture.values.assistant_session_get.turns[0].artifacts;
      Object.assign(artifact, {name:'subtitles.srt',path:'X:/isolated/subtitles.srt',mime_type:'application/x-subrip'});
      fixture.values.assistant_artifact_preview = ({artifactId}: {artifactId:string}) => {
        fixture.previewCalls.push(artifactId);
        return {name:'subtitles.srt',sha256:'original',content:'1\n00:00:00,000 --> 00:00:00,600\n跨越切点',markdown:false};
      };
    });
    await page.getByRole('button', {name:'刷新助理',exact:true}).click();
    await page.getByRole('button', {name:'预览',exact:true}).click();
    const preview = page.getByRole('complementary', {name:'结果预览'});
    await expect(preview).toContainText('00:00:00,000 --> 00:00:00,600');
    await expect(preview).toContainText('跨越切点');
    expect(await page.evaluate(() => (window as any).__performance.previewCalls)).toEqual(['result-a']);
  });

  test('image preview renders verified binary data and discards late or mismatched replies', async ({page}) => {
    await page.evaluate(async () => {
      const fixture = (window as any).__performance;
      const bytes = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg=='), char => char.charCodeAt(0));
      const digest = new Uint8Array(await crypto.subtle.digest('SHA-256',bytes));
      const turn = fixture.values.assistant_session_get.turns[0];
      Object.assign(turn.artifacts[0], {
        name:'result.png', bytes:bytes.length, mime_type:'image/png',
        sha256:[...digest].map(b=>b.toString(16).padStart(2,'0')).join(''),
      });
      fixture.imageReads = [];
      fixture.values.assistant_artifact_image_preview = ({artifactId}: {artifactId:string}) => {
        const attempt = fixture.imageReads.push(artifactId);
        if (attempt === 1) {
          const changed = bytes.slice();
          changed[0] ^= 1;
          return changed.buffer;
        }
        if (attempt === 2) return bytes.buffer;
        return new Promise(resolve => { fixture.finishImage = () => resolve(bytes.buffer); });
      };
    });
    await page.getByRole('button', {name:'刷新助理',exact:true}).click();
    const trigger = page.getByRole('button', {name:'预览',exact:true});
    const panel = page.getByRole('complementary', {name:'结果预览'});
    await trigger.click();
    await expect(panel.getByRole('alert')).toContainText('原图片摘要不匹配');
    await panel.getByRole('button', {name:'重试预览',exact:true}).click();
    const image = panel.getByRole('img', {name:'result.png',exact:true});
    await expect(image).toBeVisible();
    await expect.poll(() => image.evaluate((element: HTMLImageElement) => element.naturalWidth)).toBe(1);
    const imageUrl = await image.getAttribute('src');
    expect(imageUrl).toMatch(/^blob:/);
    await panel.getByRole('button', {name:'关闭预览',exact:true}).press('Escape');
    await expect(trigger).toBeFocused();
    expect(await page.evaluate(async url => {
      try { await fetch(url!); return true; } catch { return false; }
    },imageUrl)).toBe(false);
    await trigger.click();
    await expect(panel.getByRole('status')).toContainText('正在读取原结果');
    await panel.getByRole('button', {name:'关闭预览',exact:true}).click();
    const observed = await page.evaluate(async () => {
      const fixture = (window as any).__performance;
      fixture.finishImage();
      await new Promise(resolve => setTimeout(resolve, 0));
      return {reads:fixture.imageReads,calls:fixture.calls};
    });
    await expect(panel).toHaveCount(0);
    expect(observed.reads).toEqual(['result-a','result-a','result-a']);
    expect(observed.calls).not.toContain('assistant_artifact_open');
    expect(observed.calls).not.toContain('assistant_send');
  });

  test('a cancelled preview cannot reopen when the original reply arrives late', async ({page}) => {
    await page.evaluate(() => {
      const fixture = (window as any).__performance;
      fixture.values.assistant_artifact_preview = () => new Promise(resolve => {
        fixture.finishPreview = () => resolve({name:'result.txt',sha256:'original',content:'迟到的原结果',markdown:false});
      });
    });
    await page.getByRole('button', {name:'预览',exact:true}).click();
    const panel = page.getByRole('complementary', {name:'结果预览'});
    await expect(panel.getByRole('status')).toContainText('正在读取原结果');
    await panel.getByRole('button', {name:'关闭预览',exact:true}).click();
    await page.evaluate(async () => { (window as any).__performance.finishPreview(); await new Promise(resolve => setTimeout(resolve, 10)); });
    await expect(panel).toHaveCount(0);
    await expect(page.getByText('迟到的原结果', {exact:true})).toHaveCount(0);
  });
});
test('six native turns keep earlier replies and refresh history once per completed turn',async({page})=>{
  await page.goto('/evals/ai-control-ui/performance.html');
  await page.evaluate(()=>{
    const p=(window as any).__performance;const turns:any[]=[];p.nativeReads=0;
    p.values.conversation_providers={providers:[{id:'homebase',name:'HomeBase 助理',interactive:true},{id:'gemini-cli',name:'Gemini CLI',interactive:true}]};
    p.values.assistant_local_config_get={credential_references:[]};
    p.values.conversation_native=({request:r}:any)=>{
      if(r.action==='list')return {data:[]};
      if(r.action==='create')return {thread:{id:'six-turns',turns:[]}};
      if(r.action==='send'){const n=turns.length+1;turns.push({id:`turn-${n}`,status:'completed',items:[{id:`u-${n}`,type:'userMessage',text:r.text},{id:`a-${n}`,type:'agentMessage',text:`第${n}轮回复`}]});return {state:'accepted',turn:{id:`turn-${n}`}};}
      if(r.action==='read'){p.nativeReads++;return {thread:{id:'six-turns',turns:structuredClone(turns)}};}
      if(r.action==='status'){const items=turns.slice(-4).flatMap(t=>t.items);return {id:r.thread_id,provider:r.provider,state:turns.length?'completed':'idle',item_order:items.map(i=>i.id),items:Object.fromEntries(items.map(i=>[i.id,i]))};}
      throw Error('Unexpected native action');};
  });
  await page.getByRole('link',{name:'会话',exact:true}).click();await page.getByLabel('新会话使用的 Agent').selectOption('gemini-cli');await page.getByRole('button',{name:'新建会话',exact:true}).click();
  for(let n=1;n<=6;n++){await page.getByLabel('给 Gemini CLI 发消息').fill(`第${n}轮要求`);await page.getByRole('button',{name:'发送',exact:true}).click();await expect(page.getByText(`第${n}轮回复`,{exact:true})).toBeVisible();await expect.poll(()=>page.evaluate(()=>(window as any).__performance.nativeReads)).toBe(n);}
  await expect(page.getByText('第1轮回复',{exact:true})).toBeAttached();
  await page.getByRole('link',{name:'能力库',exact:true}).click();await page.getByRole('link',{name:'会话',exact:true}).click();
  await expect(page.getByText('第1轮回复',{exact:true})).toBeAttached();expect(await page.evaluate(()=>(window as any).__performance.nativeReads)).toBe(6);
  await page.getByRole('button',{name:'刷新原生会话',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).__performance.nativeReads)).toBe(7);
  await expect(page.getByText('第1轮回复',{exact:true})).toBeAttached();
});

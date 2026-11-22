import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {runTerminal} from '../local/terminal.mjs';
const parent=process.env.HOMEBASE_LOCAL_TEST_LAB;
const executable=process.env.HOMEBASE_BWRAP_EXE;
const hash=process.env.HOMEBASE_BWRAP_SHA256;
const enabled=!!(parent&&executable&&hash&&process.platform==='win32');
async function fixture(){
  await fs.mkdir(parent,{recursive:true});const root=await fs.mkdtemp(path.join(parent,'terminal-'));const working=path.join(root,'workspace');await fs.mkdir(working);
  return {root,working,context:{default_workspace:'fixture',protected_paths:[],workspaces:[{id:'fixture',path:working,access:'write'}],sandbox:{kind:'wsl-bubblewrap',executable,sha256:hash,distro:'Ubuntu'}}};
}
test('a missing bundled executor reports an unavailable sandbox',async t=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),'homebase-terminal-'));
  t.after(()=>fs.rm(root,{recursive:true,force:true}));
  const context={default_workspace:'fixture',workspaces:[{id:'fixture',path:root,access:'write'}],
    sandbox:{kind:'wsl-bubblewrap',executable:path.join(root,'missing-bwrap'),sha256:'0'.repeat(64)}};
  await assert.rejects(runTerminal(context,{command:'printf SYNTHETIC'}),
    {code:'SANDBOX_UNAVAILABLE',message:/缺少隔离执行器/});
});
test('real WSL sandbox mounts only test workspace, masks credentials and isolates network',{skip:!enabled},async()=>{
  const f=await fixture();await fs.writeFile(path.join(f.working,'.env'),'SYNTHETIC NOT A REAL KEY');
  await fs.writeFile(path.join(f.root,'outside.txt'),'SYNTHETIC SENTINEL');
  const command=`python3 - <<'PY'\nfrom pathlib import Path\nimport socket\nassert not Path('/mnt/c').exists()\nassert not Path('/mnt/f').exists()\ntry:\n Path('/workspace/.env').read_text()\n raise AssertionError('credential was readable')\nexcept PermissionError:\n pass\ns=socket.socket();s.settimeout(0.3)\ntry:\n s.connect(('1.1.1.1',53))\n raise AssertionError('network escaped')\nexcept OSError:\n pass\nPath('/workspace/result.txt').write_text('隔离输出',encoding='utf8')\nprint('SANDBOX_VERIFIED')\nPY`;
  const result=await runTerminal(f.context,{command,timeout_ms:15000});
  assert.equal(result.status,'succeeded',result.stderr);assert.match(result.stdout,/SANDBOX_VERIFIED/);
  assert.equal(await fs.readFile(path.join(f.working,'result.txt'),'utf8'),'隔离输出');
  assert.equal(await fs.readFile(path.join(f.root,'outside.txt'),'utf8'),'SYNTHETIC SENTINEL');
});
test('read-only mount and deadline return explicit failure without changing the sentinel',{skip:!enabled},async()=>{
  const f=await fixture();await fs.writeFile(path.join(f.working,'sentinel.txt'),'unchanged');
  const readOnly=await runTerminal(f.context,{command:"printf changed > sentinel.txt",write:false,timeout_ms:5000});assert.equal(readOnly.status,'failed');
  assert.equal(await fs.readFile(path.join(f.working,'sentinel.txt'),'utf8'),'unchanged');
  const timed=await runTerminal(f.context,{command:'sleep 5',timeout_ms:300});assert.equal(timed.status,'timed_out');
});
test('cancel kills the sandbox process group before delayed writes can occur',{skip:!enabled},async()=>{
  const f=await fixture();const controller=new AbortController();
  const result=await runTerminal(f.context,{command:"python3 -u -c \"import time;from pathlib import Path;print('READY',flush=True);time.sleep(3);Path('late.txt').write_text('must not happen')\"",timeout_ms:10000},{signal:controller.signal,onEvent:e=>{if(e.text.includes('READY'))controller.abort();}});
  assert.equal(result.status,'canceled');await new Promise(r=>setTimeout(r,3500));
  await assert.rejects(fs.access(path.join(f.working,'late.txt')));
});

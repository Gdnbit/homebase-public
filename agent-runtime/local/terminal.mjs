import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import {spawn} from 'node:child_process';
import readline from 'node:readline';
import {fileURLToPath} from 'node:url';
import {workspace, normalizeRelative, secretName, protectedName, limits, fail} from './policy.mjs';

const supervisor=fileURLToPath(new URL('./sandbox-launch.py',import.meta.url));
const ignored=new Set(['node_modules','.git','target','.venv','venv','__pycache__','.homebase-trash']);
export function toWslPath(value) {
  const match=/^([a-zA-Z]):[\\/](.*)$/.exec(value);
  if(!match)fail('WSL_PATH_UNSUPPORTED','WSL执行只接受本地磁盘目录，暂不支持UNC共享');
  return `/mnt/${match[1].toLowerCase()}/${match[2].replaceAll('\\','/')}`;
}
async function masksFor(root) {
  const pending=[''], masks=[];let visited=0;
  while(pending.length){
    const directory=pending.shift();
    for(const entry of await fs.readdir(path.join(root,directory),{withFileTypes:true})){
      if(++visited>10000)fail('SANDBOX_SCAN_LIMIT','工作区非缓存项超过10000，请选择更具体的执行目录');
      const relative=[directory,entry.name].filter(Boolean).join('/');
      if(entry.isSymbolicLink())fail('SANDBOX_LINK_DENIED','执行目录中含链接，请选择没有链接的目录，未沿链接挂载');
      if(secretName(entry.name)||protectedName(entry.name)||ignored.has(entry.name)){
        masks.push({path:relative,directory:entry.isDirectory()});continue;
      }
      if(entry.isDirectory())pending.push(relative);
      else if(entry.isFile()&&(await fs.stat(path.join(root,relative))).nlink>1)fail('SANDBOX_LINK_DENIED','执行目录含硬链接，未将其暴露给命令');
    }
  }
  return masks;
}
export async function runTerminal(context,request,{signal,onEvent=()=>{}}={}) {
  if(signal?.aborted)return {status:'canceled',stdout:'',stderr:'',exit_code:null,elapsed_ms:0};
  const backend=context.sandbox;
  if(!backend||backend.kind!=='wsl-bubblewrap')fail('SANDBOX_UNAVAILABLE','隔离终端未配置；不会退回无隔离的宿主执行');
  if(typeof request.command!=='string'||!request.command.trim()||request.command.length>16000)fail('COMMAND_INVALID','命令为空或过长');
  const timeout=request.timeout_ms??60000;
  if(!Number.isInteger(timeout)||timeout<100||timeout>300000)fail('TIMEOUT_INVALID','终端超时须为100至300000毫秒');
  const root=await workspace(context,request.workspace_id,request.write!==false);
  const cwd=normalizeRelative(request.cwd||'').join('/');
  const actualHash=crypto.createHash('sha256').update(await fs.readFile(backend.executable)).digest('hex');
  if(actualHash!==backend.sha256)fail('SANDBOX_CHANGED','隔离执行器文件与已核验指纹不一致，未运行');
  const masks=await masksFor(root.path);
  const environment=Object.fromEntries(['SystemRoot','WINDIR','COMSPEC','PATH','PATHEXT'].filter(k=>process.env[k]).map(k=>[k,process.env[k]]));
  environment.WSLENV='';
  const child=spawn(backend.wsl_command||'wsl.exe',['-d',backend.distro||'Ubuntu','-u','root','--exec','/usr/bin/python3',toWslPath(supervisor)],{windowsHide:true,env:environment,stdio:['pipe','pipe','pipe']});
  return new Promise((resolve,reject)=>{
    let stdout='',stderr='',truncated=false,final=null,done=false;
    const started=Date.now();
    function finish(error,value){if(done)return;done=true;clearTimeout(timer);signal?.removeEventListener('abort',cancel);child.stdin.end();if(error)reject(error);else resolve(value);}
    function cancel(){child.stdin.end();}
    const timer=setTimeout(()=>{cancel();const kill=setTimeout(()=>child.kill(),3000);kill.unref();},timeout+15000);
    signal?.addEventListener('abort',cancel,{once:true});
    child.on('error',()=>finish(Object.assign(new Error('无法启动WSL隔离执行器'),{code:'SANDBOX_START_FAILED'})));
    child.stdin.on('error',()=>{});
    child.stderr.on('data',chunk=>{stderr=(stderr+String(chunk)).slice(-65536);});
    readline.createInterface({input:child.stdout}).on('line',line=>{
      if(Buffer.byteLength(line)>128*1024){cancel();return;}
      try{
        const event=JSON.parse(line);
        if(event.event==='complete'){final=event;return;}
        if(!['stdout','stderr'].includes(event.event)||typeof event.text!=='string'){cancel();return;}
        const previous=event.event==='stdout'?stdout:stderr;
        const next=previous+event.text;if(Buffer.byteLength(next)>65536)truncated=true;
        if(event.event==='stdout')stdout=next.slice(-32768);else stderr=next.slice(-32768);
        onEvent(event);
      }catch{cancel();}
    });
    child.on('close',()=>{
      if(!final)return finish(Object.assign(new Error('隔离执行器未返回终态；结果未知，不自动重跑'),{code:'EXECUTION_UNKNOWN'}));
      finish(null,{...final,stdout,stderr,output_truncated:truncated,elapsed_ms:Date.now()-started,masked_paths:masks.length,workspace_id:root.id,cwd:'/workspace'+(cwd?'/'+cwd:'')});
    });
    if(signal?.aborted){cancel();return;}
    child.stdin.write(JSON.stringify({executable:toWslPath(backend.executable),workspace:toWslPath(root.path),write:request.write!==false,cwd:cwd?'/'+cwd:'',command:request.command,timeout_ms:timeout,masks})+'\n');
  });
}

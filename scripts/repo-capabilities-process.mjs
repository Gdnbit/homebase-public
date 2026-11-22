import fs from 'node:fs';
import path from 'node:path';
import {spawn,execFileSync} from 'node:child_process';
import readline from 'node:readline';
import {fileURLToPath} from 'node:url';
import {digest,fail,noLinks} from './repo-capabilities-store.mjs';

export function cleanEnvironment(root){const env={};for(const key of ['SystemRoot','WINDIR','COMSPEC','PATH','PATHEXT']){const actual=Object.keys(process.env).find(k=>k.toLowerCase()===key.toLowerCase());if(actual)env[key]=process.env[actual];}for(const dir of ['home','tmp','cache'])fs.mkdirSync(path.join(root,dir),{recursive:true});Object.assign(env,{HOME:path.join(root,'home'),USERPROFILE:path.join(root,'home'),APPDATA:path.join(root,'home','AppData','Roaming'),LOCALAPPDATA:path.join(root,'home','AppData','Local'),TEMP:path.join(root,'tmp'),TMP:path.join(root,'tmp'),WSLENV:'',CI:'1',GIT_CONFIG_NOSYSTEM:'1',GIT_CONFIG_GLOBAL:path.join(root,'home','empty-gitconfig'),GIT_TERMINAL_PROMPT:'0',npm_config_cache:path.join(root,'cache'),npm_config_userconfig:path.join(root,'home','.npmrc'),npm_config_globalconfig:path.join(root,'home','global-npmrc'),npm_config_registry:'https://registry.npmjs.org',npm_config_ignore_scripts:'true',npm_config_audit:'false',npm_config_fund:'false'});return env;}
export function npmCli(){for(const p of [path.join(path.dirname(process.execPath),'node_modules/npm/bin/npm-cli.js'),path.join(path.dirname(process.execPath),'../node_modules/npm/bin/npm-cli.js')])if(fs.existsSync(p))return p;fail('NPM_UNAVAILABLE','当前Node安装缺少受信任npm CLI，未回退shell执行');}
function kill(child){if(!child.pid||child.exitCode!==null||child.signalCode!==null)return;if(process.platform==='win32'){try{execFileSync(path.join(process.env.SystemRoot||'C:/Windows','System32/taskkill.exe'),['/PID',String(child.pid),'/T','/F'],{stdio:'ignore',windowsHide:true,timeout:3000});}catch{child.kill();}}else child.kill('SIGKILL');}
export function processRun(program,args,{cwd,root,timeout=180000,signal,onSpawn=()=>{}}){return new Promise((resolve,reject)=>{
  if(signal?.aborted)return reject(Object.assign(new Error('已取消'),{code:'CANCELED'}));
  const child=spawn(program,args,{cwd,env:cleanEnvironment(root),windowsHide:true,stdio:['ignore','pipe','pipe']});onSpawn(child.pid);
  let output='',expired=false,canceled=false;const timer=setTimeout(()=>{expired=true;kill(child);},timeout);
  const cancel=()=>{canceled=true;kill(child);};signal?.addEventListener('abort',cancel,{once:true});
  const append=c=>{output+=c.toString('utf8');if(Buffer.byteLength(output)>1024*1024){expired=true;kill(child);}};child.stdout.on('data',append);child.stderr.on('data',append);
  child.once('error',()=>{clearTimeout(timer);signal?.removeEventListener('abort',cancel);reject(Object.assign(new Error('工具进程无法启动'),{code:'PROCESS_START_FAILED'}));});
  child.once('close',code=>{clearTimeout(timer);signal?.removeEventListener('abort',cancel);if(canceled)return reject(Object.assign(new Error('本次操作已取消'),{code:'CANCELED'}));if(expired)return reject(Object.assign(new Error('工具超时或输出超过上限，已停止本次进程树'),{code:'PROCESS_LIMIT'}));if(code!==0){const network=/ECONNRESET|ETIMEDOUT|EAI_AGAIN|ENOTFOUND|ECONNREFUSED/.test(output);return reject(Object.assign(new Error(network?'npm公开下载网络连接失败；未执行项目代码或注册能力。可读取当前项目版本后手动重试安装。':`安装进程退出码 ${Number.isInteger(code)?code:'未知'}；未执行项目代码或注册能力。`),{code:'INSTALL_FAILED'}));}resolve(output);});
});}
export function toWsl(file){const real=fs.realpathSync.native(noLinks(file)).replace(/^\\\\\?\\/,'');const m=/^([a-zA-Z]):[\\/](.*)$/.exec(real);if(!m)fail('WSL_PATH_INVALID','隔离执行只支持本地磁盘目录');return `/mnt/${m[1].toLowerCase()}/${m[2].replaceAll('\\','/')}`;}
function trustedSandbox(context) {
  if(context.sandbox?.kind!=='wsl-bubblewrap'||!fs.existsSync(context.sandbox.executable))fail('SANDBOX_UNAVAILABLE','隔离执行器不可用，未回退宿主执行');
  if(digest(fs.readFileSync(context.sandbox.executable))!==context.sandbox.sha256)fail('SANDBOX_CHANGED','隔离执行器摘要变化，未执行项目代码');
  return context.sandbox;
}

function sandboxProcess(config, workRoot, request, {
  signal, onSpawn = () => {}, launch = spawn, startupTimeout = 12000
} = {}) {
  const error = (code, message) => Object.assign(new Error(message), { code, homebase: true });
  const unavailable = () => error('SANDBOX_NOT_READY', '等待 WSL 隔离进程就绪超时，未提交仓库代码。请核对 Linux 发行版能否启动。');
  const unknown = () => error('EXECUTION_UNKNOWN', '隔离请求已发出，但未能核实结果。请查询原操作，不自动重跑。');
  const canceled = () => error('CANCELED', '等待隔离进程期间已取消，未执行仓库代码。');
  if (signal?.aborted) return Promise.reject(canceled());
  fs.mkdirSync(workRoot,{recursive:true});noLinks(workRoot);
  const supervisor=fileURLToPath(new URL('./repo-capabilities-sandbox.py',import.meta.url));
  const command = toWsl(supervisor);
  const mode = request === null ? ['--probe-v1',toWsl(config.executable),config.sha256] : ['--handshake-v1'];
  const startup = ['-d',config.distro||'Ubuntu','-u','root','--exec','/usr/bin/python3',command,...mode];
  const child = launch(config.wsl_command||'wsl.exe',startup,{env:cleanEnvironment(workRoot),windowsHide:true,stdio:['pipe','pipe','pipe']});
  return new Promise((resolve, reject) => {
    let stdout = '', stderr = '', final = null;
    let bootStderr = Buffer.alloc(0);
    let ready = false, submitted = false, settled = false, executionTimer, executionGuard;
    const started = Date.now();
    const input = child.stdin;
    const lines = readline.createInterface({input:child.stdout});
    const bootTimer = setTimeout(() => stop(reportedWslFailure() ?? unavailable()), startupTimeout);

    function reportedWslFailure(line = '') {
      // WSL can emit UTF-16LE when redirected. Only classify allowlisted Windows
      // identifiers, with no raw process output in the resulting error.
      const text = `${line}\n${bootStderr.toString('utf8')}`.replaceAll('\0', '');
      if (/\b(?:WSL_E_DISTRO_NOT_FOUND|0x80040302)\b/i.test(text)) {
        return error('SANDBOX_WSL_DISTRO_MISSING', 'WSL 未找到配置的 Linux 发行版，未提交仓库代码。');
      }
      if (/\b(?:WSL_E_VIRTUAL_MACHINE_PLATFORM_REQUIRED|HCS_E_HYPERV_NOT_INSTALLED|0x80370102)\b/i.test(text)) {
        return error('SANDBOX_WSL_PLATFORM_UNAVAILABLE', 'Windows 报告无法启动 WSL 2 的虚拟机平台，未提交仓库代码。');
      }
      if (/\b(?:WSL_E_WSL_OPTIONAL_COMPONENT_REQUIRED|0x8007019e)\b/i.test(text)) {
        return error('SANDBOX_WSL_COMPONENT_DISABLED', 'Windows 报告 WSL 所需组件未启用，未提交仓库代码。');
      }
      return null;
    }

    function finish(reason, result) {
      if (settled) return;
      settled = true;
      clearTimeout(bootTimer);
      clearTimeout(executionTimer);
      clearTimeout(executionGuard);
      signal?.removeEventListener('abort', abort);
      lines.close();
      input.end();
      if (reason) reject(reason); else resolve(result);
    }
    function stop(reason) {
      finish(reason);
      kill(child);
    }
    function abort() {
      if (submitted) input.end();
      else stop(canceled());
    }
    function handleLine(line) {
      if (Buffer.byteLength(line) > 128*1024) throw Error('frame too large');
      const message = JSON.parse(line);
      // The install probe is complete only after the fixed sandbox has run its runtime check.
      if (request === null && !ready && message.event === 'unavailable' && message.protocol === 2) {
        const problems = {
          SANDBOX_CHANGED: '隔离执行器摘要变化，未执行项目代码。',
          SANDBOX_RUNTIME_UNAVAILABLE: '隔离环境缺少可运行的 Node.js 20 或更高版本，未下载或执行项目代码。',
          SANDBOX_ISOLATION_UNAVAILABLE: 'WSL 已响应，但无法启动受保护的隔离环境，未下载或执行项目代码。',
        };
        if (!Object.hasOwn(problems, message.code)) throw Error('unknown startup failure');
        return stop(error(message.code, problems[message.code]));
      }
      if (!ready && message.event === 'ready' && message.protocol === (request === null ? 2 : 1)) {
        ready = true;
        if (request === null) return input.end();
        onSpawn(child.pid);
        submitted = true;
        clearTimeout(bootTimer);
        executionTimer = setTimeout(() => input.end(), request.timeout_ms + 10000);
        executionGuard = setTimeout(() => stop(unknown()), request.timeout_ms + 15000);
        input.write(JSON.stringify(request) + '\n');
        return;
      }
      if (!submitted || final) throw Error('unexpected frame');
      if (message.event === 'complete' && ['succeeded','failed','canceled','timed_out','output_limit','unknown'].includes(message.status)) {
        if (message.status === 'succeeded' && message.exit_code !== 0) throw Error('invalid completion');
        final = message;
      } else if (typeof message.text === 'string' && ['stdout','stderr'].includes(message.event)) {
        if (message.event === 'stdout') stdout += message.text; else stderr += message.text;
        if (Buffer.byteLength(stdout)+Buffer.byteLength(stderr)>1024*1024) throw Error('output limit');
      } else {
        throw Error('invalid frame');
      }
    }
    lines.on('line', line => {
      if (settled) return;
      try { handleLine(line); }
      catch {
        stop(submitted ? unknown() : reportedWslFailure(line) ?? error(
          'SANDBOX_PROTOCOL_INVALID', 'WSL 隔离监督进程返回了无法验证的就绪信息，未提交仓库代码。',
        ));
      }
    });
    input.on('error',()=>{});
    child.stderr.on('data', chunk => {
      if (!ready && !settled && bootStderr.length < 8192) {
        bootStderr = Buffer.concat([bootStderr, chunk.subarray(0, 8192 - bootStderr.length)]);
      }
    });
    child.once('error',()=>finish(submitted ? unknown() : error('SANDBOX_START_FAILED','WSL 隔离进程无法启动，未提交仓库代码。')));
    child.once('close',code => {
      if (request === null && ready && code === 0) return finish(null);
      if (!submitted) return finish(reportedWslFailure() ?? error(
        'SANDBOX_BOOT_EXITED', 'WSL 隔离监督进程未完成就绪检查便退出，未提交仓库代码。',
      ));
      if (!final || final.status === 'unknown' || code !== 0) return finish(unknown());
      finish(null,{...final,stdout,stderr,elapsed_ms:Date.now()-started});
    });
    signal?.addEventListener('abort',abort,{once:true});
    if (signal?.aborted) abort();
  });
}

export function probeSandbox(context, workRoot, options) {
  return sandboxProcess(trustedSandbox(context), workRoot, null, options);
}

export async function sandbox(context,packageRoot,workRoot,entry,stdin,args=[],{timeout=20000,...options}={}){
  const config=trustedSandbox(context);
  fs.mkdirSync(workRoot,{recursive:true});
  const request={bwrap:toWsl(config.executable),bwrap_sha256:config.sha256,package_root:toWsl(packageRoot),work_root:toWsl(workRoot),entry,stdin,args,timeout_ms:timeout};
  return sandboxProcess(config,workRoot,request,options);
}

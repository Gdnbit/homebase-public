import {digest,fail,text} from './repo-capabilities-store.mjs';

export function repository(value){
  text(value,1024);let url;try{url=new URL(value.trim());}catch{fail('REPOSITORY_INVALID','请输入公开 GitHub 仓库主页地址');}
  if(url.protocol!=='https:'||url.port||url.username||url.password||url.search||url.hash||!['github.com','deepwiki.com','gitingest.com','gitmcp.io'].includes(url.hostname))fail('REPOSITORY_INVALID','仅接受不含认证信息的公开 GitHub 仓库或已知仓库视图地址');
  const parts=url.pathname.replace(/\/$/,'').split('/').filter(Boolean);if(parts.length!==2)fail('REPOSITORY_INVALID','请使用 owner/repository 仓库主页；固定提交在 revision 字段填写');
  const owner=parts[0],repo=parts[1].replace(/\.git$/,'');if(![owner,repo].every(p=>/^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,99}$/.test(p)&&!p.includes('..')))fail('REPOSITORY_INVALID','仓库名称格式无效');
  return {owner,repo,url:`https://github.com/${owner}/${repo}`};
}
export function views(repo,commit){return [{id:'source',name:'固定源码',url:`${repo.url}/tree/${commit}`,pinned:true},{id:'wiki',name:'DeepWiki 文档',url:`https://deepwiki.com/${repo.owner}/${repo.repo}`,pinned:false},{id:'digest',name:'Gitingest AI 文本',url:`https://gitingest.com/${repo.owner}/${repo.repo}`,pinned:false},{id:'mcp',name:'GitMCP 文档',url:`https://gitmcp.io/${repo.owner}/${repo.repo}`,pinned:false}];}
const repoPart = /^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,99}$/;
const validSearchPart = part => repoPart.test(part) && !part.includes('..');

function searchCandidate(item) {
  const full = typeof item?.full_name === 'string' ? item.full_name.split('/') : [];
  if (full.length !== 2 || !full.every(validSearchPart) || item.private !== false
      || typeof item.archived !== 'boolean' || !Number.isSafeInteger(item.stargazers_count)
      || item.stargazers_count < 0) return null;
  const repository_url = `https://github.com/${full.join('/')}`;
  if (item.html_url !== repository_url || repository(repository_url).url !== repository_url) return null;
  return { repository_url, stars: item.stargazers_count, archived: item.archived };
}

export async function searchRepositories(query, {get=publicJson}={}) {
  const parts = typeof query === 'string' ? query.trim().split('/') : [];
  if (!parts.length || parts.length > 2 || !parts.every(validSearchPart)) {
    fail('REPOSITORY_SEARCH_INVALID','请使用 GitHub 仓库名或 owner/repo，例如 marked 或 markedjs/marked。');
  }
  if (parts.length === 2) {
    const fullName = parts.join('/');
    let item;
    try { item = await get(`https://api.github.com/repos/${fullName}`); }
    catch (error) {
      if (error.code === 'SOURCE_HTTP_ERROR' && error.http_status === 404) return {status:'succeeded', candidates:[], partial:false};
      throw error;
    }
    const candidate = searchCandidate(item);
    if (!candidate || item.full_name.toLowerCase() !== fullName.toLowerCase()) {
      fail('SOURCE_INVALID','返回的公开仓库身份不匹配，请核对仓库名。');
    }
    return {status:'succeeded', candidates:[candidate], partial:false};
  }
  const queryName = parts[0];
  const url = new URL('https://api.github.com/search/repositories');
  url.search = new URLSearchParams({q:`${queryName} in:name is:public`,sort:'stars',order:'desc',per_page:'30'});
  const result = await get(url.href);
  if (!Array.isArray(result?.items) || result.items.length > 30 || typeof result.incomplete_results !== 'boolean'
      || !Number.isSafeInteger(result.total_count) || result.total_count < result.items.length) {
    fail('SOURCE_INVALID','GitHub 搜索响应无法核对。');
  }
  const candidates = new Map();
  for (const item of result.items) {
    if (typeof item?.name !== 'string' || item.name.toLowerCase() !== queryName.toLowerCase()) continue;
    const candidate = searchCandidate(item);
    if (!candidate || item.full_name.split('/')[1].toLowerCase() !== queryName.toLowerCase()) {
      fail('SOURCE_INVALID','GitHub 同名候选的公开仓库身份无法核对。');
    }
    const key = candidate.repository_url.toLowerCase();
    if (!candidates.has(key)) candidates.set(key,candidate);
  }
  return {status:'succeeded', candidates:[...candidates.values()].slice(0,5),
    partial:result.incomplete_results || result.total_count > result.items.length || candidates.size > 5};
}
const allowedHosts=new Set(['api.github.com','registry.npmjs.org']);
export async function publicJson(url,{maximum=2*1024*1024,fetcher=fetch}={}){
  const target=new URL(url);if(target.protocol!=='https:'||target.port||target.username||target.password||!allowedHosts.has(target.hostname))fail('SOURCE_HOST_DENIED','来源主机不在公共仓库/包元数据范围');
  for(let attempt=0;attempt<3;attempt++){
    try{
      const r=await fetcher(url,{headers:{accept:'application/json','user-agent':'HomeBase-Repository-Capabilities'},redirect:'error',signal:AbortSignal.timeout(20000)});
      if(!r.ok){if(r.status===403&&r.headers.get('x-ratelimit-remaining')==='0'){const reset=Number(r.headers.get('x-ratelimit-reset'));const at=Number.isFinite(reset)&&reset>0?new Date(reset*1000).toISOString():'稍后';fail('SOURCE_RATE_LIMITED',`GitHub公开API额度已用尽，请在 ${at} 后刷新分析；未安装或注册`,{http_status:403});}if([429,502,503,504].includes(r.status)&&attempt<2){const wait=Number(r.headers.get('retry-after')||1);if(!Number.isFinite(wait)||wait>5)fail('SOURCE_RATE_LIMITED','公共来源限流，请稍后重新分析；未安装或注册');await new Promise(resolve=>setTimeout(resolve,Math.max(0,wait)*1000));continue;}fail('SOURCE_HTTP_ERROR',`公开来源返回 HTTP ${r.status}，未将其解释为空仓库`,{http_status:r.status});}
      if(Number(r.headers.get('content-length'))>maximum)fail('SOURCE_LIMIT','来源响应超过分析上限');
      const chunks=[];let size=0;for await(const c of r.body){size+=c.length;if(size>maximum)fail('SOURCE_LIMIT','来源响应超过分析上限');chunks.push(c);}
      return JSON.parse(Buffer.concat(chunks).toString('utf8'));
    }catch(e){if(e.code)throw e;if(attempt===2)fail('SOURCE_UNAVAILABLE','公共来源连接或JSON读取失败，未安装或注册');await new Promise(resolve=>setTimeout(resolve,500*(attempt+1)));}
  }
}
function content(value){if(value.type!=='file'||value.encoding!=='base64'||typeof value.content!=='string'||value.size>160000)fail('SOURCE_INVALID','公开文件内容或大小无效');return Buffer.from(value.content,'base64').toString('utf8');}
function binPath(value){return typeof value==='string'&&value.length<200&&!value.includes('\\')&&!value.startsWith('/')&&!value.split('/').some(p=>p==='..'||p==='')&&/^[a-zA-Z0-9_.\/-]+$/.test(value);}
function repoFromPackage(value){let u=typeof value==='string'?value:value?.url;if(typeof u!=='string')return null;u=u.replace(/^git\+/,'').replace(/\.git$/,'');try{return repository(u).url.toLowerCase();}catch{return null;}}
export function profileFor(pkg){
  const markdown=/markdown/i.test([pkg.description,...(Array.isArray(pkg.keywords)?pkg.keywords:[])].join(' '));
  if(markdown)return {id:'markdown-html-v1',name:'Markdown 转 HTML',input_schema:{type:'object',properties:{stdin:{type:'string',maxLength:65536}},required:['stdin'],additionalProperties:false},output_schema:{type:'object',properties:{stdout:{type:'string'},artifact:{type:'object'}},required:['stdout','artifact'],additionalProperties:true},examples:[{input:{stdin:'# HomeBase\n\n**Isolated validation**\n'},contains:['<h1>HomeBase</h1>','<strong>Isolated validation</strong>']},{input:{stdin:'- one\n- two\n'},contains:['<ul>','<li>one</li>','<li>two</li>']}],notice:'HTML作为文本返回，不执行、不嵌入页面；原项目不负责HTML净化。'};
  return {id:'stdin-text-v1',name:pkg.description?.slice(0,100)||pkg.name,input_schema:{type:'object',properties:{stdin:{type:'string',maxLength:65536}},required:['stdin'],additionalProperties:false},output_schema:{type:'object',properties:{stdout:{type:'string'},artifact:{type:'object'}},required:['stdout','artifact'],additionalProperties:true},examples:[],notice:'已发现通用stdin/stdout CLI；须提供至少一条明确输入与预期输出验收用例后才会注册。'};
}
export async function analyzeSource(input,{get=publicJson}={}){
  const repo=repository(input.repository_url);const revision=input.revision||'HEAD';text(revision,100);if(!/^[a-zA-Z0-9_.\/-]+$/.test(revision)||revision.includes('..'))fail('REVISION_INVALID','固定版本或提交格式无效');
  const base=`https://api.github.com/repos/${repo.owner}/${repo.repo}`;
  const metadata=await get(base);if(metadata.private||metadata.full_name?.toLowerCase()!==`${repo.owner}/${repo.repo}`.toLowerCase())fail('REPOSITORY_PRIVATE_OR_MOVED','仅接入当前公开仓库，重定向或私有来源需另行处理');
  const commit=await get(`${base}/commits/${encodeURIComponent(revision)}`);if(!/^[a-f0-9]{40}$/.test(commit.sha||''))fail('SOURCE_INVALID','未获得固定提交');
  let packageSource;try{packageSource=await get(`${base}/contents/package.json?ref=${commit.sha}`);}catch(e){if(e.code!=='SOURCE_HTTP_ERROR'||e.http_status!==404)throw e;return {version:1,repository:repo,source_commit:commit.sha,description:metadata.description||'',license:metadata.license?.spdx_id||'unknown',supported:false,state:'needs_adapter',reason:'根目录没有可识别的npm包；该类型需要新增驱动，不会执行README中的安装命令。',views:views(repo,commit.sha)};}
  let pkg;try{pkg=JSON.parse(content(packageSource));}catch{fail('SOURCE_INVALID','package.json无法完整解析');}
  if(typeof pkg.name!=='string'||! /^(?:@[a-z0-9_.-]+\/)?[a-z0-9][a-z0-9_.-]*$/.test(pkg.name))fail('PACKAGE_INVALID','未发现有效npm包名');
  const release=await get(`https://registry.npmjs.org/${encodeURIComponent(pkg.name)}/latest`);
  if(release.name!==pkg.name)fail('PACKAGE_IDENTITY_MISMATCH','包元数据身份不一致，未安装');
  if(repoFromPackage(release.repository)!==repo.url.toLowerCase())fail('PACKAGE_REPOSITORY_MISMATCH','发布包的仓库声明与输入地址不一致，未安装');
  if(!/^[0-9]+\.[0-9]+\.[0-9]+(?:-[a-zA-Z0-9.-]+)?$/.test(release.version||'')||! /^[a-f0-9]{40}$/.test(release.gitHead||'')||! /^sha512-[A-Za-z0-9+/=]+$/.test(release.dist?.integrity||''))fail('PACKAGE_PROVENANCE_INVALID','发布包缺少精确版本、gitHead或完整性摘要');
  if(input.revision&&release.gitHead!==commit.sha)fail('RELEASE_REVISION_MISMATCH','所选提交与当前发布包不一致，未改用其他源码；请先分析默认发布版本');
  const published=release.gitHead===commit.sha?commit:await get(`${base}/commits/${release.gitHead}`);
  if(published.sha!==release.gitHead)fail('PACKAGE_PROVENANCE_INVALID','发布提交不属于该仓库');
  const bins=typeof release.bin==='string'?{[pkg.name.split('/').at(-1)]:release.bin}:release.bin||{};
  const cli=Object.entries(bins).filter(([name,target])=>/^[a-zA-Z0-9_.-]+$/.test(name)&&binPath(target));
  let readme='';try{readme=content(await get(`${base}/readme?ref=${release.gitHead}`)).slice(0,18000);}catch(e){if(e.code!=='SOURCE_HTTP_ERROR'||e.http_status!==404)throw e;}
  const profile=profileFor(release);const source={version:1,repository:repo,source_commit:release.gitHead,requested_commit:commit.sha,description:String(release.description||metadata.description||'').slice(0,1000),license:release.license||metadata.license?.spdx_id||'unknown',package:{name:release.name,version:release.version,integrity:release.dist.integrity,git_head:release.gitHead,engines:release.engines||{},unpacked_size:release.dist.unpackedSize||null,bin:cli},readme_excerpt:readme,readme_is_untrusted_data:true,mentioned_credentials:[...new Set(readme.match(/\b[A-Z][A-Z0-9_]{2,}_(?:API_KEY|TOKEN|SECRET)\b/g)||[])].slice(0,16),profile,views:views(repo,release.gitHead),supported:cli.length===1,state:cli.length===1?'analyzed':'needs_adapter',reason:cli.length===1?'已固定官方npm发布来源，可生成stdin/stdout接口。':'没有唯一可自动选择的npm CLI，需要补充驱动或入口选择。',provenance_notice:'npm gitHead为发布元数据声明；包完整性另以registry integrity校验，不将其等同于源码构建证明。'};
  source.analysis_sha256=digest(source);return source;
}

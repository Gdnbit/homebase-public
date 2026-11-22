import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {DatabaseSync} from 'node:sqlite';

export const digest=value=>crypto.createHash('sha256').update(typeof value==='string'||Buffer.isBuffer(value)?value:canonical(value)).digest('hex');
export function canonical(value){return JSON.stringify(value,(_key,v)=>v&&typeof v==='object'&&!Array.isArray(v)?Object.fromEntries(Object.keys(v).sort().map(k=>[k,v[k]])):v);}
export function fail(code,message,details={}){throw Object.assign(new Error(message),{code,homebase:true,...details});}
export function id(value){if(typeof value!=='string'||! /^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,159}$/.test(value)||value.includes('..'))fail('ID_INVALID','编号格式无效');return value;}
export function text(value,max=65536){if(typeof value!=='string'||Buffer.byteLength(value)>max)fail('INPUT_INVALID','文本类型或长度无效');return value;}
export function fields(value,names){if(!value||typeof value!=='object'||Array.isArray(value)||Object.keys(value).some(k=>!names.includes(k)))fail('INPUT_INVALID','参数字段无效，请按接口说明填写');}
export function noLinks(file){let p=path.resolve(file);for(;;){try{const s=fs.lstatSync(p);if(s.isSymbolicLink())fail('LINK_DENIED','受管理路径不能包含目录联接或符号链接');}catch(e){if(e.code!=='ENOENT')throw e;}const parent=path.dirname(p);if(parent===p)break;p=parent;}return path.resolve(file);}
export function inside(root,...parts){const file=path.resolve(root,...parts);if(file===root||!file.toLowerCase().startsWith((root+path.sep).toLowerCase()))fail('PATH_DENIED','路径超出受管理目录');return noLinks(file);}
export function read(file,fallback){try{const s=fs.lstatSync(file);if(!s.isFile()||s.nlink!==1||s.size>2*1024*1024)fail('STATE_INVALID','状态文件类型或大小异常');return JSON.parse(fs.readFileSync(file,'utf8'));}catch(e){if(e.code==='ENOENT'&&fallback!==undefined)return structuredClone(fallback);if(e.code==='STATE_INVALID')throw e;fail('STATE_INVALID','状态文件无法完整读取；原文件未覆盖');}}
export function write(file,value){noLinks(file);fs.mkdirSync(path.dirname(file),{recursive:true});const temporary=file+'.'+crypto.randomUUID()+'.tmp';fs.writeFileSync(temporary,JSON.stringify(value,null,2),{flag:'wx',mode:0o600});fs.renameSync(temporary,file);}
export function alive(pid){if(!Number.isInteger(pid)||pid<=0)return false;try{process.kill(pid,0);return true;}catch(e){return e.code==='EPERM';}}
export function store(context,create=false){
  if(!context||!path.isAbsolute(context.data_root||'')||!path.isAbsolute(context.output_root||''))fail('CONTEXT_INVALID','缺少受信任的能力目录');
  const root=noLinks(context.data_root),output=noLinks(context.output_root);
  if(root===path.parse(root).root||output===path.parse(output).root||root===output)fail('CONTEXT_INVALID','能力目录不能是磁盘根目录');
  const marker=path.join(root,'.homebase-repo-capabilities.json');
  if(create){if(!fs.existsSync(marker)){if(fs.existsSync(root)&&fs.readdirSync(root).length)fail('ROOT_NOT_OWNED','目录已有其他内容，未初始化或覆盖');fs.mkdirSync(root,{recursive:true});write(marker,{version:1,owner:'homebase-repo-capabilities'});}if(read(marker).owner!=='homebase-repo-capabilities')fail('ROOT_NOT_OWNED','能力目录所有权标记无效');}
  else if(fs.existsSync(root)&&(!fs.existsSync(marker)||read(marker).owner!=='homebase-repo-capabilities'))fail('ROOT_NOT_OWNED','能力目录未初始化或标记无效');
  const file=(kind,name)=>inside(root,kind,`${id(name)}.json`);
  const filename=path.join(root,'registry.sqlite');let db=null;
  try{
    if(create){noLinks(filename);db=new DatabaseSync(filename);db.exec("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=3000; CREATE TABLE IF NOT EXISTS records(kind TEXT NOT NULL,id TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(kind,id)); CREATE TABLE IF NOT EXISTS metadata(id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL); INSERT OR IGNORE INTO metadata VALUES(1,0);");}
    else if(fs.existsSync(filename)){noLinks(filename);db=new DatabaseSync(filename,{readOnly:true});db.exec('PRAGMA busy_timeout=3000;');}
    else if(fs.existsSync(root))fail('STATE_INVALID','能力索引缺失，未将其解释为空库');
    if(db)db.prepare('SELECT revision FROM metadata WHERE id=1').get();
    if (create) db.exec(`CREATE TABLE IF NOT EXISTS install_checks (
      operation_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, revision INTEGER NOT NULL,
      fingerprint TEXT NOT NULL, pid INTEGER, stage TEXT NOT NULL CHECK(stage IN ('checking','canceled','admitted'))
    ); CREATE INDEX IF NOT EXISTS install_checks_project ON install_checks(project_id) WHERE pid IS NOT NULL;`);
  }catch{try{db?.close();}catch{}fail('STATE_INVALID','能力索引无法打开，未覆盖或重建原数据');}
  const hasInstallChecks = create || db?.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='install_checks'").get();
  const checkQuery = (sql, key) => {
    if (!db || !hasInstallChecks) return null;
    return db.prepare(sql).get(id(key)) || null;
  };
  const updateCheck = (sql, ...args) => {
    if (!create || !db) fail('STATE_READ_ONLY','当前操作不能修改安装预检');
    try { return db.prepare(sql).run(...args).changes; }
    catch { fail('STATE_IO','安装预检状态无法确认，已停止后续动作'); }
  };
  const index=()=>{if(!db)return {version:1,revision:0,projects:[],capabilities:[]};try{const rows=db.prepare('SELECT kind,value FROM records').all();return {version:1,revision:db.prepare('SELECT revision FROM metadata WHERE id=1').get().revision,projects:rows.filter(r=>r.kind==='project').map(r=>JSON.parse(r.value)),capabilities:rows.filter(r=>r.kind==='capability').map(r=>JSON.parse(r.value))};}catch{fail('STATE_INVALID','能力索引无法完整读取，未回退为空库');}};
  const put=(kind,key,value,insertOnly=false)=>{if(!create||!db)fail('STATE_READ_ONLY','当前操作不能修改注册表');try{db.exec('BEGIN IMMEDIATE');const result=db.prepare(insertOnly?'INSERT OR IGNORE INTO records VALUES(?,?,?)':'INSERT INTO records VALUES(?,?,?) ON CONFLICT(kind,id) DO UPDATE SET value=excluded.value').run(kind,key,JSON.stringify(value));db.prepare('UPDATE metadata SET revision=revision+1 WHERE id=1').run();db.exec('COMMIT');return result.changes;}catch{try{db.exec('ROLLBACK');}catch{}fail('STATE_IO','能力索引写入未确认，停止后续动作');}};
  return {root,output,file,index,putProject(p){put('project',id(p.id),p);},insertCapability(c){return put('capability',id(c.id),c,true);},
    installCheck(operation) { return checkQuery('SELECT * FROM install_checks WHERE operation_id=?', operation); },
    projectInstallCheck(project) { return checkQuery('SELECT * FROM install_checks WHERE project_id=? AND pid IS NOT NULL ORDER BY rowid DESC LIMIT 1', project); },
    startInstallCheck(operation, project, fingerprint) {
      updateCheck("INSERT INTO install_checks VALUES (?,?,?,?,?,'checking')", id(operation),id(project.id),project.revision,fingerprint,process.pid);
    },
    cancelInstallCheck(operation, project, pid) {
      return updateCheck("UPDATE install_checks SET stage='canceled' WHERE operation_id=? AND project_id=? AND pid=? AND stage='checking'",id(operation),id(project),pid)===1;
    },
    admitInstallCheck(operation) {
      return updateCheck("UPDATE install_checks SET stage='admitted' WHERE operation_id=? AND pid=? AND stage='checking'",id(operation),process.pid)===1;
    },
    finishInstallCheck(operation, pid) {
      updateCheck("DELETE FROM install_checks WHERE operation_id=? AND pid=? AND stage<>'canceled'",id(operation),pid);
      updateCheck("UPDATE install_checks SET pid=NULL WHERE operation_id=? AND pid=? AND stage='canceled'",id(operation),pid);
    },
    close(){if(db){db.close();db=null;}}};
}
export async function locked(s,operation,fn,resource=operation){
  const acquired=[];
  const release=()=>{for(const {file,handle}of acquired.reverse()){fs.closeSync(handle);fs.unlinkSync(file);}s.close();};
  try{for(const name of new Set([`request:${operation}`,`resource:${resource}`])){const file=inside(s.root,'locks',digest(name)+'.lock');fs.mkdirSync(path.dirname(file),{recursive:true});const handle=fs.openSync(file,'wx',0o600);acquired.push({file,handle});fs.writeFileSync(handle,JSON.stringify({version:1,pid:process.pid,operation,started_at:new Date().toISOString()}));}}catch{release();fail('OPERATION_BUSY','已有能力操作或未核对的中断操作；请查询原任务，不重复提交');}
  try{return await fn();}finally{release();}
}
export function page(entries,query='',offset=0,limit=6){
  text(query,200);
  if(!Number.isInteger(offset)||offset<0||!Number.isInteger(limit)||limit<1||limit>20)fail('INPUT_INVALID','分页参数无效');
  const words=query.toLowerCase().split(/\s+/).filter(Boolean);
  const found=words.length ? entries.filter(entry=>{
    const searchable=JSON.stringify(entry).toLowerCase();
    return words.every(word=>searchable.includes(word));
  }) : entries;
  const end=offset+limit;
  return {entries:found.slice(offset,end),total:found.length,next_offset:end<found.length?end:null};
}

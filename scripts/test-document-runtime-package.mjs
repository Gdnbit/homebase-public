// Fresh, explicitly named staging only. No global installation or profile copy.
import fs from 'node:fs';import path from 'node:path';import crypto from 'node:crypto';import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';import {fileURLToPath} from 'node:url';
const repo=fileURLToPath(new URL('../',import.meta.url)),parent=process.env.HOMEBASE_DOCUMENT_PACKAGE_LAB;
assert(parent&&path.isAbsolute(parent),'Set an absolute HOMEBASE_DOCUMENT_PACKAGE_LAB');
fs.mkdirSync(parent,{recursive:true});const stage=fs.mkdtempSync(path.join(parent,'document-runtime-'));
for(const name of ['worker.mjs','extract.mjs','package.json','package-lock.json','README.md'])fs.copyFileSync(path.join(repo,'document-runtime',name),path.join(stage,name),fs.constants.COPYFILE_EXCL);
fs.copyFileSync(process.execPath,path.join(stage,'node.exe'),fs.constants.COPYFILE_EXCL);
const userconfig=path.join(parent,'empty-user.npmrc'),globalconfig=path.join(parent,'empty-global.npmrc');
for(const file of [userconfig,globalconfig]){if(!fs.existsSync(file))fs.writeFileSync(file,'',{flag:'wx'});assert.equal(fs.readFileSync(file,'utf8'),'');}
const env=Object.fromEntries(Object.entries(process.env).filter(([k])=>['systemroot','windir','comspec','path','pathext'].includes(k.toLowerCase())));
Object.assign(env,{USERPROFILE:parent,HOME:parent,APPDATA:path.join(parent,'AppData'),TEMP:parent,TMP:parent,
  NPM_CONFIG_USERCONFIG:userconfig,NPM_CONFIG_GLOBALCONFIG:globalconfig,NPM_CONFIG_CACHE:path.join(parent,'npm-cache'),NPM_CONFIG_REGISTRY:'https://registry.npmjs.org'});
const install=spawnSync('npm.cmd',['ci','--ignore-scripts','--no-audit','--no-fund'],{cwd:stage,env,shell:true,windowsHide:true,stdio:'inherit',timeout:180000});
assert(!install.error&&install.status===0,'Pinned document dependency install failed');
const test=spawnSync(path.join(stage,'node.exe'),['--test',path.join(repo,'document-runtime/test/extract.test.mjs')],{cwd:stage,env:{...env,HOMEBASE_DOCUMENT_LAB:path.join(parent,'package-test'),HOMEBASE_DOCUMENT_RUNTIME:stage,HOMEBASE_DOCUMENT_NODE:path.join(stage,'node.exe')},windowsHide:true,encoding:'utf8',timeout:120000,maxBuffer:4*1024*1024});
process.stdout.write(test.stdout??'');assert(!test.error&&test.status===0,'Packaged parser suite failed');
const count=Number(/# tests (\d+)/.exec(test.stdout)?.[1]);
assert(count>0&&new RegExp(`# pass ${count}(?:\\r?\\n|$)`).test(test.stdout)&&/# fail 0(?:\r?\n|$)/.test(test.stdout)&&/# skipped 0(?:\r?\n|$)/.test(test.stdout));
fs.mkdirSync(path.join(stage,'licenses'));
for(const [from,to]of [['node_modules/pdfjs-dist/LICENSE','PDF.js-Apache-2.0.txt'],['node_modules/@napi-rs/canvas/LICENSE','Canvas-MIT.txt']])fs.copyFileSync(path.join(stage,from),path.join(stage,'licenses',to),fs.constants.COPYFILE_EXCL);
const digest=file=>crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
function files(dir){return fs.readdirSync(dir,{withFileTypes:true}).flatMap(e=>e.isDirectory()?files(path.join(dir,e.name)):[path.join(dir,e.name)]);}
const hashes=Object.fromEntries(files(stage).map(file=>[path.relative(stage,file).replaceAll('\\','/'),digest(file)]));
const version=spawnSync(path.join(stage,'node.exe'),['--version'],{env,cwd:stage,windowsHide:true,encoding:'utf8',timeout:5000});assert.equal(version.status,0);
const report={stage,passed:true,pinned_install:true,parser_tests:count,node_version:version.stdout.trim(),pdfjs_version:'6.5.438',files:hashes,source_config_copied:false,real_model_calls:0,real_wechat_messages:0};
fs.writeFileSync(path.join(stage,'validation.json'),JSON.stringify(report,null,2),{flag:'wx'});
console.log(JSON.stringify({stage,passed:true,parser_tests:count,files_verified:Object.keys(hashes).length,node_version:report.node_version}));

import fs from 'node:fs';import path from 'node:path';import crypto from 'node:crypto';import assert from 'node:assert/strict';
const source=path.resolve('native-agent-runtime');
const destination=process.argv[2];assert(destination&&path.isAbsolute(destination),'Explicit absolute destination required');
const pkg=JSON.parse(fs.readFileSync(path.join(source,'package.json'),'utf8'));
const lock=JSON.parse(fs.readFileSync(path.join(source,'package-lock.json'),'utf8'));
assert.equal(pkg.dependencies['@google/gemini-cli'],'0.68.0');
assert.deepEqual(Object.keys(pkg.dependencies),['@google/gemini-cli'],'No checkout links or extra runtime dependencies');
assert.equal(lock.packages['node_modules/@google/gemini-cli'].version,'0.68.0');
assert.equal(lock.packages['node_modules/@google/gemini-cli'].integrity,'sha512-eq7AWY+YCijNGKVwyiRjN+GuOdXU/djuOZhX18p+HDmNyUqFdqH7C5V6vDJLl1uffhRPhGijePV+YvY8lymrbQ==');
assert(!fs.existsSync(destination),'Never overwrite a runtime directory');
function checkTree(dir){for(const e of fs.readdirSync(dir,{withFileTypes:true})){const p=path.join(dir,e.name);assert(!fs.lstatSync(p).isSymbolicLink(),`Runtime links are not portable: ${p}`);if(e.isDirectory())checkTree(p);}}
checkTree(source);
fs.mkdirSync(destination,{recursive:true});
for(const name of ['worker.mjs','mcp-bridge.mjs','package.json','package-lock.json','node_modules'])fs.cpSync(path.join(source,name),path.join(destination,name),{recursive:true,errorOnExist:true,force:false});
const hash=file=>crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
function files(dir){return fs.readdirSync(dir,{withFileTypes:true}).flatMap(e=>e.isDirectory()?files(path.join(dir,e.name)):[path.join(dir,e.name)]);}
const hashes=Object.fromEntries(files(destination).map(p=>[path.relative(destination,p).replaceAll('\\','/'),hash(p)]));
fs.writeFileSync(path.join(destination,'validation.json'),JSON.stringify({pinned_install:true,gemini_version:'0.68.0',files:hashes},null,2),{flag:'wx'});
console.log(JSON.stringify({destination,gemini_version:'0.68.0',files:Object.keys(hashes).length}));

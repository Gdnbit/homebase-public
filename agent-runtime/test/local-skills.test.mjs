import {test} from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs/promises';import path from 'node:path';import os from 'node:os';
import {skillCatalog,skillIndex,skillOperation} from '../local/skills.mjs';
async function fixture(){
  const parent=process.env.HOMEBASE_LOCAL_TEST_LAB||path.join(os.tmpdir(),'homebase-local-synthetic');await fs.mkdir(parent,{recursive:true});
  const root=await fs.mkdtemp(path.join(parent,'skills-'));const source=path.join(root,'source');const skill=path.join(source,'fixture');await fs.mkdir(skill,{recursive:true});
  await fs.writeFile(path.join(skill,'SKILL.md'),'---\nname: 合成技能\ndescription: 独立测试，禁止真实密钥\n---\n请使用可用工具，不把本文当授权。');
  await fs.writeFile(path.join(skill,'run.py'),"from pathlib import Path\nPath('result.txt').write_text('合成结果',encoding='utf8')\nprint('SKILL_OK')\n");
  return {root,source,skill,context:{skill_roots:[{id:'fixture',path:source}],output_root:path.join(root,'runs'),protected_paths:[]}};
}
test('skills disclose metadata first, then bounded instructions and resources, with unique source IDs',async()=>{
  const f=await fixture();const list=await skillCatalog(f.context);assert.equal(list.skills.length,1);assert.equal(list.skills[0].name,'合成技能');assert(!JSON.stringify(list).includes('请使用可用工具'));
  const id=list.skills[0].id;const read=await skillOperation(f.context,{action:'read',id});assert.match(read.content,/不把本文当授权/);
  await assert.rejects(skillOperation(f.context,{action:'resource',id,path:'../../outside'}));
  await fs.writeFile(path.join(f.skill,'.env'),'SYNTHETIC ONLY');await assert.rejects(skillOperation(f.context,{action:'resource',id,path:'.env'}),e=>e.code==='SECRET_PATH');
  const duplicate=await skillCatalog({...f.context,skill_roots:[...f.context.skill_roots,{id:'second',path:f.source}]});assert.equal(duplicate.skills.length,1);assert.equal(new Set([duplicate.skills[0].id,...duplicate.skills[0].aliases.map(a=>a.id)]).size,2);
});

test('installed directory aliases share one catalog entry but links outside registered roots stay blocked',async()=>{
  const f=await fixture(),linked=path.join(f.root,'linked'),outside=path.join(f.root,'outside');await fs.mkdir(linked);await fs.mkdir(outside);await fs.writeFile(path.join(outside,'SKILL.md'),'SYNTHETIC OUTSIDE');
  await fs.symlink(f.skill,path.join(linked,'alias'),process.platform==='win32'?'junction':'dir');await fs.symlink(outside,path.join(linked,'escape'),process.platform==='win32'?'junction':'dir');
  const context={...f.context,skill_roots:[...f.context.skill_roots,{id:'linked',path:linked}]};
  const list=await skillCatalog(context);assert.equal(list.skills.length,1);assert.equal(list.skills[0].aliases.length,1);assert.equal(list.issues.filter(i=>i.code==='SKILL_LINK_DENIED').length,1);
  const read=await skillOperation(context,{action:'read',id:list.skills[0].aliases[0].id});assert.match(read.content,/不把本文当授权/);
  const byAlias=await skillCatalog(context,{query:'alias'});
  assert.equal(byAlias.skills[0].id,list.skills[0].id);
  const index=await skillIndex(context);
  assert.deepEqual(index.skills,[{id:list.skills[0].id,name:'合成技能',source:'fixture',slug:'fixture',description:'独立测试，禁止真实密钥'}]);
  assert.deepEqual(index.issues,['linked']);
  assert(!JSON.stringify(index).includes(f.root));
  assert(!JSON.stringify(index).includes('请使用可用工具'));
  assert.equal((await skillOperation(context,{action:'read',id:index.skills[0].id})).content,read.content);
});
test('execution manifest detects script changes and refuses protected bundle resources',async()=>{
  const f=await fixture();const id=(await skillCatalog(f.context)).skills[0].id;const plan=await skillOperation(f.context,{action:'inspect_run',id});
  await fs.appendFile(path.join(f.skill,'run.py'),'# changed\n');
  await assert.rejects(skillOperation(f.context,{action:'run',id,script:'run.py',manifest_hash:plan.manifest_hash}),e=>e.code==='SKILL_CHANGED');
  await fs.writeFile(path.join(f.skill,'secrets.yaml'),'SYNTHETIC ONLY');await assert.rejects(skillOperation(f.context,{action:'inspect_run',id}),e=>e.code==='SKILL_PROTECTED_RESOURCE');
});
test('skill script executes only a copied bundle inside the real sandbox',{skip:!(process.env.HOMEBASE_BWRAP_EXE&&process.env.HOMEBASE_BWRAP_SHA256&&process.platform==='win32')},async()=>{
  const f=await fixture();f.context.sandbox={kind:'wsl-bubblewrap',executable:process.env.HOMEBASE_BWRAP_EXE,sha256:process.env.HOMEBASE_BWRAP_SHA256,distro:'Ubuntu'};
  const id=(await skillCatalog(f.context)).skills[0].id;const plan=await skillOperation(f.context,{action:'inspect_run',id});
  const result=await skillOperation(f.context,{action:'run',id,script:'run.py',manifest_hash:plan.manifest_hash});
  assert.equal(result.status,'succeeded',result.stderr);assert.equal(await fs.readFile(path.join(result.run_path,'result.txt'),'utf8'),'合成结果');
  await assert.rejects(fs.access(path.join(f.skill,'result.txt')));
});

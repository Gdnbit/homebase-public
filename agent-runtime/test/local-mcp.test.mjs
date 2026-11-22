import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {spawn} from 'node:child_process';
import readline from 'node:readline';
import {fileURLToPath} from 'node:url';
import {McpBroker,serverFingerprint} from '../local/mcp.mjs';
const fixture=fileURLToPath(new URL('./fixtures/mcp-local.mjs',import.meta.url));
async function context(def){
  const root=process.env.HOMEBASE_LOCAL_TEST_LAB||path.join(os.tmpdir(),'homebase-local-synthetic');await fs.mkdir(root,{recursive:true});
  return {mcp_profile_root:await fs.mkdtemp(path.join(root,'mcp-')),mcp_servers:[def],references:{FIXTURE_TOKEN:'SYNTHETIC-MCP-SECRET-ONLY'},mcp_grants:[{id:def.id,fingerprint:serverFingerprint(def),tools:['sum','stats'],resources:true,prompts:true}]};
}
async function check(broker,ctx){
  const catalog=await broker.operation(ctx,{action:'list'});assert.equal(catalog.servers[0].state,'granted');assert(!JSON.stringify(catalog).includes('SECRET-ONLY'));
  const [tools,description]=await Promise.all([broker.operation(ctx,{action:'tools',server_id:'fixture'}),broker.operation(ctx,{action:'describe',server_id:'fixture',tool:'sum'})]);assert.equal(tools.tools.length,2);
  await assert.rejects(broker.operation(ctx,{action:'call',server_id:'fixture',tool:'sum',schema_hash:description.tool.schema_hash,arguments:{a:'bad',b:2}}),e=>e.code==='MCP_ARGUMENTS_INVALID');
  const result=await broker.operation(ctx,{action:'call',server_id:'fixture',tool:'sum',schema_hash:description.tool.schema_hash,arguments:{a:2,b:3}});
  assert.equal(result.status,'succeeded');assert.equal(result.result.structuredContent.sum,5);assert(!JSON.stringify(result).includes('SYNTHETIC-MCP-SECRET-ONLY'));
  const stats=await broker.operation(ctx,{action:'describe',server_id:'fixture',tool:'stats'});
  const counters=await broker.operation(ctx,{action:'call',server_id:'fixture',tool:'stats',schema_hash:stats.tool.schema_hash});
  assert.deepEqual(JSON.parse(counters.result.content[0].text),{calls:1,lists:1});
  assert.equal((await broker.operation(ctx,{action:'read_resource',server_id:'fixture',uri:'fixture://notes'})).result.contents[0].text,'SYNTHETIC RESOURCE');
  assert.equal((await broker.operation(ctx,{action:'get_prompt',server_id:'fixture',prompt:'fixture-prompt'})).status,'succeeded');
  await broker.operation(ctx,{action:'tools',server_id:'fixture',refresh:true});
  const refreshed=await broker.operation(ctx,{action:'call',server_id:'fixture',tool:'stats',schema_hash:stats.tool.schema_hash});assert.equal(JSON.parse(refreshed.result.content[0].text).lists,2);
  const revoked={...ctx,mcp_grants:ctx.mcp_grants.map(g=>({...g,tools:[]}))};await assert.rejects(broker.operation(revoked,{action:'call',server_id:'fixture',tool:'sum',schema_hash:description.tool.schema_hash,arguments:{a:1,b:1}}),e=>e.code==='MCP_TOOL_APPROVAL_REQUIRED');
}
test('official MCP stdio client discovers, validates, calls, caches and reads resources/prompts',async()=>{
  const def={id:'fixture',command:process.execPath,args:[fixture],env:{FIXTURE_TOKEN:'${FIXTURE_TOKEN}'}};const ctx=await context(def);const broker=new McpBroker();
  process.env.HOMEBASE_PARENT_SECRET='SYNTHETIC-PARENT-ONLY';
  try{await check(broker,ctx);
    const desc=await broker.operation(ctx,{action:'describe',server_id:'fixture',tool:'sum'});
    const result=await broker.operation(ctx,{action:'call',server_id:'fixture',tool:'sum',schema_hash:desc.tool.schema_hash,arguments:{a:1,b:1}});assert.equal(JSON.parse(result.result.content[0].text).inherited,false);
    const changed={...ctx,mcp_servers:[{...def,args:[fixture,'different-config']}]};await assert.rejects(broker.operation(changed,{action:'tools',server_id:'fixture'}),e=>e.code==='MCP_TRUST_REQUIRED');
    const secretDef={...def,id:'homebase-secrets'};const secret=await context(secretDef);await assert.rejects(broker.operation(secret,{action:'tools',server_id:secretDef.id}),e=>e.code==='MCP_SECRET_EXPORT_BLOCKED');
    await broker.disconnect('fixture');const renewed=await broker.operation(ctx,{action:'tools',server_id:'fixture'});assert.equal(renewed.cached,false);
    const literal={...def,id:'literal-fixture',env:{FIXTURE_TOKEN:'SYNTHETIC-LITERAL-DO-NOT-START'}},literalContext=await context(literal);
    assert.equal((await broker.operation(literalContext,{action:'list'})).servers[0].state,'needs_secure_reference');
    await assert.rejects(broker.operation(literalContext,{action:'tools',server_id:literal.id}),error=>error.code==='MCP_LITERAL_CREDENTIAL_BLOCKED');assert.equal(broker.entries.has(literal.id),false);
  }finally{delete process.env.HOMEBASE_PARENT_SECRET;await broker.close();}
});
test('official MCP Streamable HTTP client uses the same discovery and call contract',async()=>{
  const child=spawn(process.execPath,[fixture,'--http'],{windowsHide:true,stdio:['ignore','pipe','pipe'],env:{SystemRoot:process.env.SystemRoot,PATH:process.env.PATH,FIXTURE_TOKEN:'SYNTHETIC-MCP-SECRET-ONLY'}});
  const timer=setTimeout(()=>child.kill(),20000);
  const port=await new Promise((resolve,reject)=>{readline.createInterface({input:child.stdout}).once('line',line=>resolve(JSON.parse(line).port));child.on('error',reject);child.once('exit',()=>reject(Error('HTTP fixture exited')));});
  const def={id:'fixture',transport:'streamable-http',url:`http://127.0.0.1:${port}/mcp`};const ctx=await context(def);const broker=new McpBroker();
  try{await check(broker,ctx);
    const authDef={...def,id:'auth-fixture',url:`http://127.0.0.1:${port}/auth`},authContext=await context(authDef);
    await assert.rejects(broker.operation(authContext,{action:'tools',server_id:authDef.id}),error=>error.code==='MCP_AUTH_REQUIRED');
  }finally{await broker.close();child.kill();clearTimeout(timer);}
});

test('a catalog change notification during loading prevents invocation with the stale schema',async()=>{
  const def={id:'fixture',command:process.execPath,args:[fixture,'--change-during-list']},ctx=await context(def),broker=new McpBroker();
  try{
    await assert.rejects(broker.operation(ctx,{action:'tools',server_id:'fixture'}),error=>error.code==='MCP_CATALOG_CHANGED');
    const description=await broker.operation(ctx,{action:'describe',server_id:'fixture',tool:'sum'});
    const result=await broker.operation(ctx,{action:'call',server_id:'fixture',tool:'sum',schema_hash:description.tool.schema_hash,arguments:{a:1,b:2}});assert.equal(result.result.structuredContent.sum,3);
  }finally{await broker.close();}
});

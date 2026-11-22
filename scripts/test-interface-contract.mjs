import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import {fileURLToPath} from 'node:url';
import {schemaReader} from './control-schemas.mjs';
import {canonicalArguments, reachableDefinitions, validateSchema} from './control-validation.mjs';
const root=fileURLToPath(new URL('../',import.meta.url));
const catalog=JSON.parse(fs.readFileSync(path.join(root,'control/commands.json'),'utf8'));

test('publishing package uses one protected ZIP and accepts one to six ordered segments', () => {
  const command = catalog.commands.find(entry => entry.name === 'capability_video_publish_package');
  assert.deepEqual(command.requires, ['write', 'private']);
  const inputDefs = reachableDefinitions(command.input_schema, catalog.$defs);
  const input = {
    source: {receipt_id: `wx-${'a'.repeat(64)}`, session_id: '00000000-0000-4000-8000-000000000001', sha256: 'b'.repeat(64)},
    segments: [{startMs: 0, durationMs: 800}], coverAtMs: 100, title: '合成标题', description: '简介草稿',
  };
  assert.deepEqual(validateSchema(command.input_schema, input, inputDefs), []);
  for (const invalid of [{segments: []}, {segments: Array(7).fill(input.segments[0])}, {coverAtMs: 120000}, {title: 'x'.repeat(33)}, {description: ''}, {source: {path: 'video.mp4'}}])
    assert(validateSchema(command.input_schema, {...input, ...invalid}, inputDefs).length);
  const outputDefs = reachableDefinitions(command.output_schema, catalog.$defs);
  const artifact = {path: 'short-video-publish.zip', bytes: 128, sha256: 'c'.repeat(64), mime_type: 'application/zip'};
  assert.deepEqual(validateSchema(command.output_schema, {status: 'succeeded', artifact}, outputDefs), []);
  assert(validateSchema(command.output_schema, {status: 'succeeded', artifact: {...artifact, bytes: 20971521}}, outputDefs).length);
});

test('video assembly optionally returns an SRT alongside the original MP4 contract', () => {
  const command = catalog.commands.find(entry => entry.name === 'capability_assemble_video');
  const inputDefs = reachableDefinitions(command.input_schema, catalog.$defs);
  const input = {
    source: {receipt_id: `wx-${'a'.repeat(64)}`, session_id: '00000000-0000-4000-8000-000000000001', sha256: 'b'.repeat(64)},
    segments: [{startMs: 2000, durationMs: 600}, {startMs: 100, durationMs: 500}],
  };
  assert.deepEqual(validateSchema(command.input_schema, input, inputDefs), []);
  assert.deepEqual(validateSchema(command.input_schema, {...input, includeSubtitles: true}, inputDefs), []);
  assert(validateSchema(command.input_schema, {...input, includeSubtitles: 'true'}, inputDefs).length);
  const outputDefs = reachableDefinitions(command.output_schema, catalog.$defs);
  const result = {status: 'succeeded', segments: input.segments, duration_ms: 1100,
    artifact: {path: 'clip.mp4', bytes: 128, sha256: 'c'.repeat(64), mime_type: 'video/mp4'}};
  const subtitles = {path: 'subtitles.srt', bytes: 128, sha256: 'd'.repeat(64), mime_type: 'application/x-subrip'};
  assert.deepEqual(validateSchema(command.output_schema, result, outputDefs), []);
  assert.deepEqual(validateSchema(command.output_schema, {...result, subtitles}, outputDefs), []);
  assert(validateSchema(command.output_schema, {...result, subtitles: {...subtitles, mime_type: 'text/plain'}}, outputDefs).length);
});

test('video cover exposes a verified source, bounded input and three typed PNGs', () => {
  const cover = catalog.commands.find(entry => entry.name === 'capability_video_cover');
  assert.deepEqual(cover.requires, ['write', 'private']);
  const input = { source: {receipt_id: `wx-${'a'.repeat(64)}`, session_id: '00000000-0000-4000-8000-000000000001', sha256: 'b'.repeat(64)}, atMs: 1250, title: '合成测试' };
  const inputDefinitions = reachableDefinitions(cover.input_schema, catalog.$defs);
  assert.deepEqual(validateSchema(cover.input_schema, input, inputDefinitions), []);
  for (const invalid of [{atMs: 120000}, {title: '长'.repeat(33)}, {source: {path: 'original.mp4'}}])
    assert(validateSchema(cover.input_schema, {...input, ...invalid}, inputDefinitions).length);
  const artifacts = [[1280, 720], [1080, 1440], [1080, 1920]].map(([width, height]) => ({
    path: `cover-${height}.png`, mime_type: 'image/png', bytes: 1024, sha256: 'c'.repeat(64), width, height,
  }));
  const result = {status: 'succeeded', at_ms: 1250, artifacts};
  const outputDefinitions = reachableDefinitions(cover.output_schema, catalog.$defs);
  assert.deepEqual(validateSchema(cover.output_schema, result, outputDefinitions), []);
  assert(validateSchema(cover.output_schema, {...result, artifacts: artifacts.slice(1)}, outputDefinitions).length);
  artifacts[0].width = 1080;
  assert(validateSchema(cover.output_schema, result, outputDefinitions).length);
});

test('bounded session discovery is private and exposes stable cursors and explicit completion',()=>{
  const c=catalog.commands.find(c=>c.name==='assistant_sessions_query');assert(c);assert.deepEqual(c.requires,['private']);
  const defs=reachableDefinitions(c.input_schema,catalog.$defs);const input={request:{query:'合成',source:'wechat',offset:0,limit:40}};
  assert.deepEqual(validateSchema(c.input_schema,input,defs),[]);
  for(const change of [{limit:0},{limit:41},{source:'other'},{offset:10001},{source_key:'forged'}])assert(validateSchema(c.input_schema,{request:{...input.request,...change}},defs).length);
  const output=Object.values(reachableDefinitions(c.output_schema,catalog.$defs));
  assert(output.some(s=>s.properties?.version?.const==='homebase-session-catalog-v1'&&s.properties?.captured_revision&&s.properties?.complete&&s.properties?.next_offset));assert(c.usage.completion.includes('旧assistant_sessions_list暂兼容'));
});

test('original send receipt lookup needs only private read permission and a complete original request',()=>{
  const c=catalog.commands.find(c=>c.name==='assistant_send_receipt_get');assert(c);
  assert.deepEqual(c.requires,['private']);
  const input={request:{session_id:null,text:'合成原文',request_id:'original-id',mode:'chat',route_id:'removed:free',attachments:[]}};
  assert.deepEqual(validateSchema(c.input_schema,input,reachableDefinitions(c.input_schema,catalog.$defs)),[]);
  assert(validateSchema(c.input_schema,{request:{...input.request,secret:'forged'}},reachableDefinitions(c.input_schema,catalog.$defs)).length);
  for(const view of [{version:'homebase-send-receipt-v1',lookup_state:'not_found',receipt:null,message:'未找到不是未执行'}, {version:'homebase-send-receipt-v1',lookup_state:'found',receipt:{session_id:'s',turn_id:'t',state:'interrupted_unknown'},message:'只读'}])assert.deepEqual(validateSchema(c.output_schema,view,reachableDefinitions(c.output_schema,catalog.$defs)),[]);
  assert(c.usage.completion.includes('不能自动重提'));
});

test('assistant continuation can hold the generation observed on the original task', () => {
  const resume = catalog.commands.find(c => c.name === 'assistant_resume');
  const read = catalog.commands.find(c => c.name === 'assistant_turn_get');
  assert(resume && read);
  const input = resume.input_schema;
  for (const args of [{turnId:'original-turn'}, {turnId:'original-turn',expectedGeneration:0}]) {
    assert.deepEqual(validateSchema(input,args,catalog.$defs),[]);
  }
  for (const expectedGeneration of [-1, 0.5, '0']) {
    assert(validateSchema(input,{turnId:'original-turn',expectedGeneration},catalog.$defs).length);
  }
  const output = reachableDefinitions(read.output_schema,catalog.$defs);
  assert(Object.values(output).some(schema => schema.required?.includes('resume_generation')));
  assert(resume.usage.completion.includes('不追加预算'));
});

test('queue continuation accepts the version from either original session view', () => {
  const resume = catalog.commands.find(c => c.name === 'assistant_queue_resume');
  assert(resume);
  for (const args of [{sessionId:'original-session'}, {sessionId:'original-session',expectedGeneration:0}]) {
    assert.deepEqual(validateSchema(resume.input_schema,args,catalog.$defs),[]);
  }
  for (const expectedGeneration of [-1, 0.5, '0']) {
    assert(validateSchema(resume.input_schema,{sessionId:'original-session',expectedGeneration},catalog.$defs).length);
  }
  for (const name of ['assistant_session_get','assistant_session_window_get']) {
    const read = catalog.commands.find(command => command.name === name);
    const definitions = reachableDefinitions(read.output_schema,catalog.$defs);
    assert(Object.values(definitions).some(schema => schema.properties?.queue_resume_generation),name);
  }
});

test('development editor contracts expose private CAS state rather than execution results',()=>{
  const draft={workspace:'F:/Synthetic/project',brief:'synthetic',workflow:'development-local',checks_json:'[unfinished',protected_files_input:''};
  for(const name of ['development_workspace_get','development_workspace_save','development_workspace_prepare','development_workspace_new']){
    const c=catalog.commands.find(c=>c.name===name);assert(c,name);assert(c.requires.includes('private'));assert.equal(c.requires.includes('write'),!name.endsWith('_get'));
    const defs=reachableDefinitions(c.output_schema,catalog.$defs);
    const entry={schema_version:1,revision:0,draft,pending:null,previous:[]};
    assert.deepEqual(validateSchema(c.output_schema,entry,defs),[],name);
    assert(validateSchema(c.output_schema,{capability_id:'homebase.development',task_id:'dev-synthetic',state:'submitted'},defs).length,name);
    const inputDefs=reachableDefinitions(c.input_schema,catalog.$defs);
    if(name.endsWith('_save')){assert.deepEqual(validateSchema(c.input_schema,{request:{revision:0,draft}},inputDefs),[]);assert(validateSchema(c.input_schema,{request:{revision:0,draft:{...draft,api_key:'forged'}}},inputDefs).length);}
  }
});

test('session preparation exposes a canonical optional business UUID distinct from CLI receipts',()=>{
  const command=catalog.commands.find(c=>c.name==='assistant_session_create');assert(command);
  const defs=reachableDefinitions(command.input_schema,catalog.$defs);
  for(const args of [{},{requestId:null},{requestId:'12345678-abcd-4000-8000-123456789012'}])assert.deepEqual(validateSchema(command.input_schema,args,defs),[]);
  for(const requestId of ['not-a-uuid','12345678-ABCD-4000-8000-123456789012',123])assert(validateSchema(command.input_schema,{requestId},defs).length);
  assert(command.requires.includes('write'));assert(command.usage.completion.includes('不发送')||command.usage.summary.includes('不发送'));assert(command.usage.prerequisites.some(s=>s.includes('CLI外层')));
});

test('timeline windows are private read-only and retain bounded complete messages with explicit detail summaries',()=>{
  const command=catalog.commands.find(c=>c.name==='assistant_session_window_get');assert(command);assert(command.requires.includes('private'));assert(!command.requires.includes('write'));
  const defs=reachableDefinitions(command.input_schema,catalog.$defs);
  const request={session_id:'actual-session',before_message_id:null,limit:40};
  assert.deepEqual(validateSchema(command.input_schema,{request},defs),[]);
  for(const change of [{limit:0},{limit:41},{before_message_id:1},{session_id:'x'.repeat(257)},{source_key:'forged'}])assert(validateSchema(command.input_schema,{request:{...request,...change}},defs).length);
  const outputs=Object.values(reachableDefinitions(command.output_schema,catalog.$defs));
  assert(outputs.some(schema=>schema.properties?.version?.const==='homebase-session-window-v1'));
  assert(outputs.some(schema=>schema.properties?.details_omitted&&schema.properties?.step_count&&schema.properties?.turn));
  assert(outputs.some(schema=>schema.properties?.start_index&&schema.properties?.end_index&&schema.properties?.before_message_id));
});

test('Inbox recovery exposes a bound original UUID, private source CAS and distinct publication/artifact states',()=>{
  const id='87562a38-8fb4-4276-bdda-bd0d85bef544';
  const draft={text:'合成原文',title:'',tags_input:'',source_hint:'',attachments:[],attachment_unconfirmed:false};
  for(const name of ['inbox_workspace_get','inbox_workspace_save','inbox_workspace_prepare','inbox_workspace_submit','inbox_workspace_resolve','inbox_workspace_cancel','inbox_submission_get','inbox_submission_cancel']){
    const c=catalog.commands.find(c=>c.name===name);assert(c,name);assert(c.requires.includes('private'));
    assert.equal(c.requires.includes('write'),!['inbox_workspace_get','inbox_submission_get'].includes(name));
    const defs=reachableDefinitions(c.input_schema,catalog.$defs);
    const valid=name.endsWith('_get')&&name.startsWith('inbox_workspace')?{}:name.endsWith('_save')?{request:{revision:1,draft}}:name.endsWith('_prepare')?{request:{revision:1,submission_id:id,snapshot:draft}}:{submissionId:id};
    assert.deepEqual(validateSchema(c.input_schema,valid,defs),[],name);
    if(valid.submissionId)for(const bad of ['guessed',id.toUpperCase(),23])assert(validateSchema(c.input_schema,{submissionId:bad},defs).length);
    if(valid.request){
      assert(validateSchema(c.input_schema,{request:{...valid.request,arbitrary_path:'forbidden'}},defs).length);
      if(name.endsWith('_prepare'))assert(validateSchema(c.input_schema,{request:{...valid.request,submission_id:'guessed'}},defs).length);
    }
  }
  const output=catalog.$defs.inbox_workspace__InboxWorkspaceReply__output;
  for(const key of ['entry','submission','observation','observation_error'])assert(output.required.includes(key));
  const wire=JSON.stringify(catalog.$defs.inbox_receipts__InboxSubmissionState__output);for(const v of ['not_found','prepared','unknown','completed','canceled'])assert(wire.includes(v));
  const files=JSON.stringify(catalog.$defs.inbox_receipts__InboxSubmissionFileState__output);for(const v of ['present','modified','missing','unreadable'])assert(files.includes(v));
});

test('history has private typed bounded pages, chunks and original-snapshot completion rules',()=>{
  const session_id='00000000-0000-4000-8000-000000000001';
  for(const name of ['assistant_history_query','assistant_history_read']) {
    const c=catalog.commands.find(c=>c.name===name);assert(c);assert(c.requires.includes('private'));assert(!c.requires.includes('write'));
    const defs=reachableDefinitions(c.input_schema,catalog.$defs);
    const request=name.endsWith('query')?{session_id,kind:'messages',query:'中文',offset:0,limit:8}:{session_id,snapshot_id:session_id,item_id:'original-message',offset:0,limit:4096};
    assert.deepEqual(validateSchema(c.input_schema,{request},defs),[]);
    for(const invalid of [{...request,limit:0},{...request,limit:name.endsWith('query')?21:524289},{...request,snapshot_id:'guessed'},{...request,offset:-1},{...request,source_key:'forged'}])assert(validateSchema(c.input_schema,{request:invalid},defs).length);
    if(name.endsWith('read'))assert.deepEqual(validateSchema(c.input_schema,{request:{...request,limit:524288}},defs),[]);
    const output=reachableDefinitions(c.output_schema,catalog.$defs);const result=output[c.output_schema.$ref.split('/').at(-1)];
    assert.equal(result.properties.version.const,'homebase-history-v1');
    for(const key of ['session_id','snapshot_id','next_offset','complete'])assert(result.properties[key],key);
    assert(c.usage.prerequisites.some(rule=>rule.includes('9000'))||c.usage.completion.includes('9000'));
    assert(c.usage.completion.includes('任务')||c.usage.prerequisites.some(rule=>rule.includes('任务')));
  }
  const direct=catalog.commands.find(c=>c.name==='assistant_turn_snapshot');
  assert.deepEqual(direct.requires,['private']);
  assert.deepEqual(validateSchema(direct.input_schema,{sessionId:session_id,turnId:'original-turn'}),[]);
  assert(validateSchema(direct.input_schema,{turnId:'original-turn'}).length);
  assert.equal(direct.output_schema.$ref,'#/$defs/assistant_history__HistoryPage__output');
  const inspect = catalog.commands.find(command => command.name === "assistant_turn_inspect");
  const step = catalog.commands.find(command => command.name === "assistant_turn_step_get");
  const scope = { sessionId: session_id, turnId: "original-turn" };
  assert.deepEqual(inspect.requires, ["private"]);
  assert.deepEqual(step.requires, ["private"]);
  assert.deepEqual(validateSchema(inspect.input_schema, scope), []);
  assert(validateSchema(step.input_schema, scope).length);
  assert.deepEqual(validateSchema(step.input_schema, { ...scope, stepId: "original-step" }), []);
  assert.deepEqual(catalog.$defs.assistant__StepSummary__output.required, ["id", "name", "state"]);
  assert.deepEqual(catalog.$defs.assistant__StepView__output.required, ["session_id", "turn_id", "step"]);
});

test('archive backup requires separate write and private authorization and a stable ID', () => {
  const backupId = '00000000-0000-4000-8000-000000000001';
  for (const [name, grants] of [
    ['assistant_archive_backup', ['private','write']],
    ['assistant_archive_backup_get', ['private']],
  ]) {
    const command = catalog.commands.find(item => item.name === name);
    assert.deepEqual([...command.requires].sort(), grants);
    assert.deepEqual(validateSchema(command.input_schema, {backupId}), []);
    assert(validateSchema(command.input_schema, {backupId:'../escape'}).length);
    assert(validateSchema(command.input_schema, {backupId,path:'arbitrary'}).length);
    const output = reachableDefinitions(command.output_schema, catalog.$defs);
    const wire = JSON.stringify(output);
    for (const state of ['not_found','incomplete','published']) assert(wire.includes(state));
    for (const field of ['backup_id','generation','encrypted_bytes']) assert(wire.includes(field));
    assert.match(command.usage.prerequisites.join(' '), /UUID/);
  }
});

test('backup files are privately paginated and recovery requires explicit write authority', () => {
  const backupId = '00000000-0000-4000-8000-000000000001';
  const fileId = '00000000-0000-4000-8000-000000000002';
  const list = catalog.commands.find(item => item.name === 'assistant_archive_backup_files');
  const recover = catalog.commands.find(item => item.name === 'assistant_archive_backup_file_recover');
  assert.deepEqual(list.requires, ['private']);
  assert.deepEqual([...recover.requires].sort(), ['private','write']);
  assert.deepEqual(validateSchema(list.input_schema, {backupId,offset:0}), []);
  assert(validateSchema(list.input_schema, {backupId,offset:-1}).length);
  assert.deepEqual(validateSchema(recover.input_schema, {backupId,fileId}), []);
  assert(validateSchema(recover.input_schema, {backupId,fileId:'../outside'}).length);
  assert(validateSchema(recover.input_schema, {backupId,fileId,path:'arbitrary'}).length);
  const wire = JSON.stringify(reachableDefinitions(list.output_schema, catalog.$defs));
  for (const token of ['saved','unavailable','changed','next_offset','original_id','drafts_indexed','unreadable_drafts']) assert(wire.includes(token));
  const draft = catalog.commands.find(item => item.name === 'assistant_archive_backup_draft_get');
  assert.deepEqual(draft.requires, ['private']);
  assert.deepEqual(validateSchema(draft.input_schema, {backupId,fileId}), []);
  assert(validateSchema(draft.input_schema, {backupId,fileId:'../outside'}).length);
  const draftWire = JSON.stringify(reachableDefinitions(draft.output_schema, catalog.$defs));
  for (const token of ['revision','pending','native_pending','voices']) assert(draftWire.includes(token));
});

test('capability discovery exposes bounded input, typed pages and explicit snapshot/source boundaries',()=>{
  const c=catalog.commands.find(c=>c.name==='capability_index_query');assert(c);
  const defs=reachableDefinitions(c.input_schema,catalog.$defs);
  assert.deepEqual(validateSchema(c.input_schema,{request:{query:'视频',offset:0,limit:10}},defs),[]);
  assert.deepEqual(validateSchema(c.input_schema,{request:{function:'内容分发',offset:0,limit:10}},defs),[]);
  const keys=['e'.repeat(64),'b'.repeat(64),'0'.repeat(64)];
  assert.deepEqual(validateSchema(c.input_schema,{request:{function:'video',entry_keys:keys}},defs),[]);
  for(const request of [{limit:0},{limit:41},{offset:50001},{kind:'untrusted'},{catalog_id:'guessed'},{unexpected:'field'}])
    assert(validateSchema(c.input_schema,{request},defs).length,JSON.stringify(request));
  for (const entry_keys of [[], [keys[0],keys[0]], [...keys,'1'.repeat(64)], ['E'.repeat(64)], ['e'.repeat(63)], [keys[0]+'\n']])
    assert(validateSchema(c.input_schema,{request:{entry_keys}},defs).length,JSON.stringify(entry_keys));
  const output=reachableDefinitions(c.output_schema,catalog.$defs);
  const page=output[c.output_schema.$ref.split('/').at(-1)];
  for(const field of ['catalog_id','snapshot_id','next_offset','groups','groups_has_more','source_complete','issues_total','issues_has_more','page_byte_limit'])assert(page.properties[field],field);
  assert(!page.required.includes('unmatched_entry_keys'));
  assert.deepEqual(validateSchema(page.properties.unmatched_entry_keys,[keys[0]],output),[]);
  assert(c.usage.prerequisites.some(p=>p.includes('snapshot_id')));
});

test('context discovery exposes summary bounds, original task progress and runtime features',()=>{
  const command=catalog.commands.find(c=>c.name==='assistant_context_get');
  const defs=reachableDefinitions(command.output_schema,catalog.$defs);
  const rootSchema=defs[command.output_schema.$ref.split('/').at(-1)];
  for(const field of ['context_byte_limit','summary_only','tasks_total','tasks_has_more','context_has_more','shared_model_defaults'])
    assert(rootSchema.required.includes(field),field);
  const task=defs[rootSchema.properties.tasks.items.$ref.split('/').at(-1)];
  for(const field of ['id','runtime_features','requirements_revision','progress','result_truncated','artifacts_has_more'])
    assert(task.required.includes(field),field);
  assert.equal(task.additionalProperties,false);
});
test('all available interfaces have self-contained schemas, valid templates and operational guidance',()=>{
  for(const c of catalog.commands.filter(c=>c.status==='available')){
    assert.ok(c.input_schema && c.output_schema,c.name);
    assert.ok(c.usage && JSON.stringify(c.usage).length>100,c.name);
    const defs={...reachableDefinitions(c.input_schema,catalog.$defs),...reachableDefinitions(c.output_schema,catalog.$defs)};
    assert.deepEqual(validateSchema(c.input_schema,c.input_example,defs),[],c.name);
  }
});
test('offline validation rejects wrong nested fields, enums, bounds and duplicate aliases',()=>{
  const c=catalog.commands.find(c=>c.name==='task_events_get');
  const defs=reachableDefinitions(c.input_schema,catalog.$defs);
  assert.deepEqual(validateSchema(c.input_schema,canonicalArguments(c,{topic:'studio:event',task_id:'synthetic',limit:1}),defs),[]);
  for(const input of [{topic:'unknown',taskId:'synthetic'},{topic:'studio:event',taskId:'synthetic',limit:101},{topic:'studio:event',taskId:1}])
    assert.ok(validateSchema(c.input_schema,input,defs).length);
  assert.throws(()=>canonicalArguments(c,{taskId:'a',task_id:'b'}));
  const render=catalog.commands.find(c=>c.name==='capability_render');
  assert.ok(validateSchema(render.input_schema,{rootPath:'synthetic',input:{title:'x',seconds:999}},catalog.$defs).length);
});
test('protected native send exposes bounded identity and rejects unsupported account or attachment fields',()=>{
  const c=catalog.commands.find(c=>c.name==='assistant_native_send');
  const defs=reachableDefinitions(c.input_schema,catalog.$defs);
  const valid={request:{provider:'codex',thread_id:'opaque_thread_01',text:'仅合成输入',request_id:'stable-native-id'},revision:0};
  assert.deepEqual(validateSchema(c.input_schema,valid,defs),[]);
  for(const patch of [{provider:'unknown'},{thread_id:'../wrong'},{request_id:'bad\nidentity'},{credential_ref:'unsupported'},{attachments:[]}])
    assert.ok(validateSchema(c.input_schema,{...valid,request:{...valid.request,...patch}},defs).length);
  assert.ok(c.usage.prerequisites.some(p=>p.includes('不启动SDK')));
});
test('Rust DTO changes update schemas; unknown types and serde features fail closed',()=>{
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'hb-schema-'));
  const source=path.join(dir,'src-tauri/src');fs.mkdirSync(source,{recursive:true});
  const file=path.join(source,'fixture.rs');
  fs.writeFileSync(file,'#[derive(Serialize, Deserialize)]\n#[serde(rename_all="camelCase")]\npub struct Sample { pub user_name: String, #[serde(default)] pub enabled: bool, pub optional: Option<String> }');
  const reader=schemaReader(dir);const schema=reader.convert('Sample','src-tauri/src/fixture.rs','input');
  const value=reader.defs[schema.$ref.split('/').at(-1)];
  assert.deepEqual(value.required,['userName']);assert.equal(value.properties.enabled.default,false);
  assert.throws(()=>reader.convert('MissingDto','src-tauri/src/fixture.rs'));
  fs.writeFileSync(file,'#[serde(rename_all="camelCase")]\npub struct Sample { pub display_name: String }');
  const changed=schemaReader(dir);changed.convert('Sample','src-tauri/src/fixture.rs','input');
  assert.notEqual(JSON.stringify(reader.defs),JSON.stringify(changed.defs));
  assert.ok(Object.values(changed.defs)[0].properties.displayName);
  fs.writeFileSync(file,'#[serde(untagged)]\npub enum Bad { Value { value: String } }');
  assert.throws(()=>schemaReader(dir).convert('Bad','src-tauri/src/fixture.rs'));
  // Tiny isolated fixture retained under OS temp; no project or user data cleanup.
});

test('document attachment reference is optional, structured and cannot smuggle paths or partial identities',()=>{
  const command=catalog.commands.find(c=>c.name==='assistant_send');const defs=reachableDefinitions(command.input_schema,catalog.$defs);
  const base={request:{session_id:null,text:'仅合成文档测试',request_id:'stable-document-input',attachments:[{name:'source.txt',content:'合成文字'}]}};
  assert.deepEqual(validateSchema(command.input_schema,base,defs),[]);
  const document={document_id:'75c8874c-109f-4c59-a445-ad1c7077eb0b',extraction_id:'a'.repeat(64),source_name:'source.pdf',source_sha256:'b'.repeat(64),page_from:1,page_to:1,text_from:0,text_to:10,content_sha256:'c'.repeat(64)};
  const withDocument={request:{...base.request,attachments:[{...base.request.attachments[0],document}]}};
  assert.deepEqual(validateSchema(command.input_schema,withDocument,defs),[]);
  for(const invalid of [{...document,path:'C:/private.pdf'},{...document,page_from:'1'},{}]){
    const value={request:{...base.request,attachments:[{...base.request.attachments[0],document:invalid}]}};
    assert(validateSchema(command.input_schema,value,defs).length);
  }
});

test('document runtime errors preserve Serde uppercase wire names in generated schemas',()=>{
  const command=catalog.commands.find(c=>c.name==='assistant_document_result');
  assert(command);
  const errors=Object.values(reachableDefinitions(command.output_schema,catalog.$defs)).map(d=>d.oneOf?.map(v=>v.const)).filter(d=>d?.includes('CLEANUP_UNKNOWN'));
  assert.equal(errors.length,1);
  assert(errors[0].includes('SOURCE_UNAVAILABLE'));
  assert(errors[0].includes('OUTPUT_LIMIT'));
  assert(!errors[0].includes('SourceUnavailable'));
});

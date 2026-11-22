// Real native CLI + local ASR. Only a fresh explicit lab and synthetic speech.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import net from "node:net";
import { chromium } from "@playwright/test";
const parent = process.env.HOMEBASE_VOICE_LAB, binary = process.env.HOMEBASE_VOICE_BINARY;
assert(parent && path.isAbsolute(parent) && binary && path.isAbsolute(binary));
assert(path.resolve(binary).startsWith(path.resolve(parent) + path.sep));
const home = fs.mkdtempSync(path.join(parent, "voice-native-"));
const reportName = process.env.HOMEBASE_VOICE_REPORT_NAME || "voice-recovery-native-validation.json";
assert(/^[a-zA-Z0-9.-]+\.json$/.test(reportName));
const portProbe=net.createServer();await new Promise(resolve=>portProbe.listen(0,"127.0.0.1",resolve));
const debugPort=portProbe.address().port;await new Promise(resolve=>portProbe.close(resolve));
const speech = path.join(home, "synthetic-speech.wav");
const synth = `$ProgressPreference='SilentlyContinue'; [Console]::InputEncoding=[Text.UTF8Encoding]::new($false); $r=[Console]::In.ReadLine()|ConvertFrom-Json; Add-Type -AssemblyName System.Speech; $s=New-Object System.Speech.Synthesis.SpeechSynthesizer; $v=$s.GetInstalledVoices()|Where-Object {$_.VoiceInfo.Culture.Name -eq 'zh-CN'}|Select-Object -First 1; if(-not $v){throw 'Offline Chinese synthesis unavailable'}; $s.SelectVoice($v.VoiceInfo.Name); $s.SetOutputToWaveFile([string]$r.path); $s.Speak('这是一次语音恢复隔离测试。蓝色圆点，简短中文字幕。'); $s.Dispose();`;
const generated = spawnSync("powershell.exe", ["-NoProfile", "-EncodedCommand", Buffer.from(synth, "utf16le").toString("base64")], { input: JSON.stringify({path:speech})+"\n", encoding:"utf8",windowsHide:true,timeout:30000 });
assert.equal(generated.status, 0, "Offline fixture generation failed");
const presets = process.env.HOMEBASE_VOICE_PRESETS || "C:/HomeBase/AIHub/capabilities/asr-presets.json";
fs.mkdirSync(path.join(home, "AIHub/capabilities"), {recursive:true});
fs.copyFileSync(presets,path.join(home,"AIHub/capabilities/asr-presets.json"),fs.constants.COPYFILE_EXCL);
const env = { ...process.env, HOMEBASE_TEST_HOME:home, HOMEBASE_SECRETS_FILE:path.join(home,"secrets.yaml"),HOMEBASE_SECRETS_META:path.join(home,"secrets-meta.json"),
  HOMEBASE_AIHUB_ROOT:path.join(home,"AIHub"),HOMEBASE_MODEL_ROOT:path.join(home,"models"),HOMEBASE_RUNTIME_ROOT:path.join(home,"runtime"),HOMEBASE_MODEL_CACHE:path.join(home,"cache"),HOMEBASE_ASR_OUTPUT_ROOT:path.join(home,"output"),
  HOMEBASE_ASR_RUNNER:process.env.HOMEBASE_VOICE_RUNNER||"C:/HomeBase/AIHub/tools/local-asr/runner/asr_task_runner.py",HOMEBASE_ASR_PYTHON:process.env.HOMEBASE_VOICE_PYTHON||"C:/HomeBase/Tools/Python313/python.exe",
  WEBVIEW2_USER_DATA_FOLDER:path.join(home,"webview-profile"),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${debugPort}`,HF_HUB_OFFLINE:"1",TRANSFORMERS_OFFLINE:"1",PYTHONDONTWRITEBYTECODE:"1" };
delete env.HOMEBASE_CONTROL_ENDPOINT;
for (const key of Object.keys(env)) if (/(?:API_KEY|ACCESS_TOKEN|AUTH_TOKEN|SOPS_AGE_KEY|AGE_SECRET)/i.test(key) || ["HF_TOKEN","HUGGING_FACE_HUB_TOKEN"].includes(key)) delete env[key];
const pause = ms => new Promise(resolve => setTimeout(resolve,ms));
const hash = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
let app,browser;
function cli(args,input,negative=false) {
  const run=spawnSync(process.execPath,["scripts/homebase.mjs",...args],{env,input:input===undefined?undefined:JSON.stringify(input),encoding:"utf8",windowsHide:true,timeout:45000,maxBuffer:4*1024*1024});
  assert(!run.error,run.error?.message);const value=JSON.parse(run.stdout);
  if(!negative)assert(value.ok,JSON.stringify(value.error));else assert.equal(value.ok,false);
  return value;
}
function call(name,args={},negative=false) {
  const value=cli(["call",name,"--input","-","--allow-private","--allow-write","--request-id",crypto.randomUUID(),"--wait-ms","30000"],args,negative);
  assert.equal(value.job.status,"succeeded",JSON.stringify(value.job.error));return value.job.result;
}
async function launch() {
  app=spawn(binary,[],{env,cwd:process.cwd(),windowsHide:true,stdio:"ignore"});
  for(let i=0;i<120;i++){await pause(250);try{if(cli(["doctor"]).ok)return;}catch{}assert(app.exitCode===null,"Isolated app exited");}
  throw Error("Isolated native app unavailable");
}
async function stop() {if(browser){await browser.close();browser=null;}if(app?.exitCode===null){app.kill();await new Promise(resolve=>app.once("exit",resolve));}}
const last=entry=>entry.voices.at(-1);
try {
  await launch();
  const settings=call("assistant_config_get").settings;assert.equal(settings.enabled,false);
  const body={text:"转写前原稿",mode:"auto",route:"",files:[]};
  call("assistant_desktop_save",{request:{scope:"new",revision:0,body}});
  const audio=fs.readFileSync(speech);
  const begun=call("assistant_voice_begin",{scope:"new",audioBase64:audio.toString("base64"),mimeType:"audio/wav"});
  const record=last(begun.entry),id=record.recording_id;assert.equal(record.audio_sha256,hash(audio));assert.equal(record.phase,"staged");
  const root=path.join(home,".homebase/assistant/recordings",id,"transcripts");
  const submitted=call("assistant_voice_submit",{scope:"new",recordingId:id});
  assert.equal(last(submitted.entry).phase,"accepted");const originalJob=last(submitted.entry).job.job_id;
  const repeated=call("assistant_voice_submit",{scope:"new",recordingId:id});
  assert.equal(last(repeated.entry).job.job_id,originalJob);
  assert.equal(call("asr_task_reconcile",{outputRoot:root}).length,1);
  // Crash only this isolated host. The already accepted local runner is not replayed.
  await stop();await launch();
  let ready;
  const until=Date.now()+180000;
  while(Date.now()<until){
    ready=call("assistant_voice_get",{scope:"new",recordingId:id});
    if(last(ready.entry).phase==="ready")break;
    assert(!["failed","cancelled"].includes(last(ready.entry).phase),last(ready.entry).message);
    await pause(1500);
  }
  assert.equal(last(ready.entry).phase,"ready",JSON.stringify(last(ready.entry)));
  const transcript=last(ready.entry).transcript;assert(/隔离|语音/.test(transcript)&&/蓝色|圆点/.test(transcript),"Synthetic speech keywords missing");
  const wrong=call("assistant_voice_apply",{scope:"new",recordingId:id,revision:0},true);assert.equal(wrong.code,"VOICE_APPLY_FAILED");
  browser=await chromium.connectOverCDP(`http://127.0.0.1:${debugPort}`);
  const page=browser.contexts().flatMap(context=>context.pages()).find(page=>!page.url().startsWith("devtools:"));assert(page);
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await page.getByRole("button",{name:"插入草稿",exact:true}).waitFor({state:"visible"});
  await page.waitForFunction(expected=>document.getElementById("assistant-message")?.value===expected,body.text);
  await page.getByText("查看转写文字",{exact:true}).click();
  await page.screenshot({path:path.join(home,"native-voice-ready.png")});
  await page.getByRole("button",{name:"插入草稿",exact:true}).click();
  await page.waitForFunction(expected=>document.getElementById("assistant-message")?.value===expected,body.text+"\n"+transcript);
  const applied={entry:call("assistant_desktop_get",{scope:"new"}).entry};
  assert.equal(applied.entry.body.text,body.text+"\n"+transcript);assert.equal(last(applied.entry).phase,"applied");
  const duplicate=call("assistant_voice_apply",{scope:"new",recordingId:id,revision:0});assert.equal(duplicate.entry.revision,applied.entry.revision);
  assert.equal(duplicate.entry.body.text,applied.entry.body.text);
  assert(!fs.existsSync(path.join(home,".homebase/assistant/recordings",id,"input.wav")));
  const journal=path.join(root,"_jobs",originalJob+".json"),journalHash=hash(fs.readFileSync(journal));
  await stop();await launch();
  const recovered=call("assistant_desktop_get",{scope:"new"}).entry;assert.equal(last(recovered).phase,"applied");assert.equal(recovered.body.text,applied.entry.body.text);
  call("assistant_voice_get",{scope:"new",recordingId:id});
  assert.equal(hash(fs.readFileSync(journal)),journalHash,"Read/restart must not rewrite original ASR state");
  assert.equal(call("asr_task_reconcile",{outputRoot:root}).length,1);
  const session=call("assistant_session_create");
  const second=last(call("assistant_voice_begin",{scope:session.id,audioBase64:audio.toString("base64"),mimeType:"audio/wav"}).entry);
  const secondRaw=path.join(home,".homebase/assistant/recordings",second.recording_id,"input.wav");
  assert(path.resolve(secondRaw).startsWith(path.resolve(home)+path.sep));fs.appendFileSync(secondRaw,"SYNTHETIC-CHANGE");
  const blocked=call("assistant_voice_submit",{scope:session.id,recordingId:second.recording_id},true);assert.equal(blocked.code,"VOICE_SOURCE_CHANGED");
  const cancelled=call("assistant_voice_cancel",{scope:session.id,recordingId:second.recording_id});assert.equal(last(cancelled.entry).phase,"cancelled");
  assert(!fs.existsSync(secondRaw));
  const nativeScope="native:codex:thread_synthetic_opaque_01";
  const nativeBody={...body,text:"原生输入仅本机保存"};call("assistant_desktop_save",{request:{scope:nativeScope,revision:0,body:nativeBody}});
  await stop();await launch();assert.deepEqual(call("assistant_desktop_get",{scope:nativeScope}).entry.body,nativeBody);
  assert.deepEqual(call("assistant_config_get").settings,settings);
  const result={passed:true,binary,binary_sha256:hash(fs.readFileSync(binary)),isolated_home:home,actual_local_asr:true,original_job:originalJob,
    crash_recovery_without_resubmit:true,asr_jobs:1,atomic_insert_deduplicated:true,transcript_characters:transcript.length,transcript_sha256:hash(transcript),
    native_webview_ready_preview:true,inserted_via_native_ui:true,changed_source_blocked:true,staged_cancel_verified:true,native_draft_restart_verified:true,native_opaque_identifier_verified:true,original_asr_journal_unchanged:true,model_settings_unchanged:true,
    real_model_calls:0,real_wechat_test_messages:0};
  fs.writeFileSync(path.join(parent,reportName),JSON.stringify(result,null,2),{flag:"wx"});console.log(JSON.stringify(result));
} finally {await stop();}

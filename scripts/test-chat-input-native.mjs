import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
// Explicit lab and executable are mandatory; never fall back to production.
const parent = process.env.HOMEBASE_CHAT_NATIVE_LAB;
const binary = process.env.HOMEBASE_CHAT_INPUT_BINARY;
assert(
  parent && path.isAbsolute(parent),
  "Set absolute HOMEBASE_CHAT_NATIVE_LAB",
);
assert(
  binary && path.isAbsolute(binary),
  "Set absolute HOMEBASE_CHAT_INPUT_BINARY",
);
assert(
  path.resolve(binary).startsWith(path.resolve(parent) + path.sep),
  "Binary must be staged inside the lab",
);
assert(fs.existsSync(binary), "Build and stage first");
const home = fs.mkdtempSync(path.join(parent, "chat-input-"));
const runner =
  process.env.HOMEBASE_CHAT_ASR_RUNNER ||
  "C:/HomeBase/AIHub/tools/local-asr/runner/asr_task_runner.py";
const python =
  process.env.HOMEBASE_CHAT_ASR_PYTHON || "C:/HomeBase/Tools/Python313/python.exe";
const presets =
  process.env.HOMEBASE_CHAT_ASR_PRESETS ||
  "C:/HomeBase/AIHub/capabilities/asr-presets.json";
const speech = path.join(parent, "isolated-speech.wav");
if (!fs.existsSync(speech)) {
  const code = `$ProgressPreference='SilentlyContinue'; [Console]::InputEncoding=[Text.UTF8Encoding]::new($false); $request=[Console]::In.ReadLine()|ConvertFrom-Json; Add-Type -AssemblyName System.Speech; $s=New-Object System.Speech.Synthesis.SpeechSynthesizer; $voice=$s.GetInstalledVoices()|Where-Object {$_.VoiceInfo.Culture.Name -eq 'zh-CN'}|Select-Object -First 1; if(-not $voice){throw 'A local Chinese synthesis voice is required for this offline fixture'}; if(Test-Path -LiteralPath $request.path){throw 'Never replace a fixture'}; $s.SelectVoice($voice.VoiceInfo.Name); $s.SetOutputToWaveFile([string]$request.path); $s.Speak('这是一次语音输入隔离测试。蓝色圆点，简短中文字幕。'); $s.Dispose();`;
  const generated = spawnSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-EncodedCommand",
      Buffer.from(code, "utf16le").toString("base64"),
    ],
    {
      input: JSON.stringify({ path: speech }) + "\n",
      encoding: "utf8",
      windowsHide: true,
      timeout: 30000,
    },
  );
  assert.equal(
    generated.status,
    0,
    "Could not create the offline speech fixture",
  );
}
fs.mkdirSync(path.join(home, "AIHub/capabilities"), { recursive: true });
fs.copyFileSync(
  presets,
  path.join(home, "AIHub/capabilities/asr-presets.json"),
  fs.constants.COPYFILE_EXCL,
);
const env = {
  ...process.env,
  HOMEBASE_TEST_HOME: home,
  HOMEBASE_SECRETS_FILE: path.join(home, "secrets.yaml"),
  HOMEBASE_SECRETS_META: path.join(home, "secrets-meta.json"),
  HOMEBASE_AIHUB_ROOT: path.join(home, "AIHub"),
  HOMEBASE_MODEL_ROOT: path.join(home, "models"),
  HOMEBASE_RUNTIME_ROOT: path.join(home, "runtime"),
  HOMEBASE_MODEL_CACHE: path.join(home, "cache"),
  HOMEBASE_ASR_OUTPUT_ROOT: path.join(home, "output"),
};
Object.assign(env, {
  HF_HUB_OFFLINE: "1",
  TRANSFORMERS_OFFLINE: "1",
  PYTHONDONTWRITEBYTECODE: "1",
});
delete env.HOMEBASE_CONTROL_ENDPOINT;
const app = spawn(binary, [], {
  cwd: process.cwd(),
  env,
  windowsHide: true,
  stdio: "ignore",
});
const pause = (ms) => new Promise((r) => setTimeout(r, ms));
const hash = (b) => crypto.createHash("sha256").update(b).digest("hex");
function cli(args, input, ok = true) {
  const p = spawnSync(process.execPath, ["scripts/homebase.mjs", ...args], {
    env,
    input: input === undefined ? undefined : JSON.stringify(input),
    encoding: "utf8",
    windowsHide: true,
    timeout: 45000,
    maxBuffer: 4 * 1024 * 1024,
  });
  assert(!p.error, p.error?.message);
  const v = JSON.parse(p.stdout);
  if (ok) assert(v.ok, JSON.stringify(v.error));
  return v;
}
function call(name, args = {}, ok = true) {
  const v = cli(
    [
      "call",
      name,
      "--input",
      "-",
      "--allow-private",
      "--allow-write",
      "--allow-external",
      "--request-id",
      "chat-input-" + crypto.randomUUID(),
      "--wait-ms",
      "30000",
    ],
    args,
    ok,
  );
  if (ok) {
    assert.equal(v.job.status, "succeeded", JSON.stringify(v.job.error));
    return v.job.result;
  }
  return v;
}
try {
  let online = false;
  for (let i = 0; i < 120; i++) {
    await pause(250);
    try {
      if (cli(["doctor"]).ok) {
        online = true;
        break;
      }
    } catch {}
  }
  assert(online, "Isolated native app unavailable");
  const config = call("assistant_config_get");
  assert.equal(config.settings.enabled, false);
  const session = call("assistant_session_create");
  const request = {
    session_id: session.id,
    text: "只总结隔离附件",
    request_id: "chat-input-original",
    attachments: [{ name: "隔离.md", content: "蓝色圆点，简短中文字幕。" }],
  };
  const ack = call("assistant_send", { request });
  assert.equal(ack.state, "disabled");
  assert.equal(call("assistant_send", { request }).turn_id, ack.turn_id);
  const changed = call(
    "assistant_send",
    {
      request: {
        ...request,
        attachments: [{ name: "隔离.md", content: "不同内容" }],
      },
    },
    false,
  );
  assert(changed.ok === false || changed.job.status === "failed");
  const data = call("assistant_session_get", { sessionId: session.id });
  assert.equal(data.messages[0].content, request.text);
  assert.equal(
    data.messages[0].attachments[0].sha256,
    hash(Buffer.from(request.attachments[0].content)),
  );
  assert.deepEqual(call("assistant_config_get").settings, config.settings);
  const wav = fs.readFileSync(speech);
  const stage = call("assistant_voice_stage", {
    audioBase64: wav.toString("base64"),
    mimeType: "audio/wav",
  });
  assert(
    path.resolve(stage.source_path).startsWith(path.resolve(home) + path.sep),
  );
  assert(
    path.resolve(stage.output_root).startsWith(path.resolve(home) + path.sep),
  );
  assert.equal(hash(fs.readFileSync(stage.source_path)), hash(wav));
  assert(!stage.output_root.toLowerCase().includes("wiki"));
  const probe = call("asr_environment_probe", {
    payload: {
      sourcePath: stage.source_path,
      outputRoot: stage.output_root,
      presetId: "asr_zh_balanced",
      engineIds: ["faster_whisper_large_v3_cpu"],
      runnerPath: runner,
      pythonPath: python,
      allowSystemDrive: true,
    },
  });
  let asr = null;
  if (probe.available) {
    const original = call("asr_task_start", {
      payload: {
        sourcePath: stage.source_path,
        outputRoot: stage.output_root,
        presetId: "asr_zh_balanced",
        engineIds: ["faster_whisper_large_v3_cpu"],
        runnerPath: probe.runnerPath,
        pythonPath: probe.pythonPath,
        taskName: "助理语音隔离验证",
        language: "zh",
        fusionMode: "manual",
        allowSystemDrive: true,
      },
    });
    const until = Date.now() + 180000;
    let result;
    let readFailures = 0;
    await pause(1500);
    while (Date.now() < until) {
      try {
        result = call("asr_task_get", {
          taskDir: original.taskDir,
          outputRoot: stage.output_root,
          jobId: original.jobId,
        });
        readFailures = 0;
      } catch (e) {
        if (++readFailures >= 3) throw e;
        await pause(1500);
        continue;
      }
      if (
        [
          "success",
          "completed",
          "succeeded",
          "failed",
          "cancelled",
          "interrupted",
          "partial",
          "unknown",
        ].includes(result.job?.status)
      )
        break;
      await pause(1500);
    }
    assert(
      ["success", "completed", "succeeded"].includes(result?.job?.status),
      "Local ASR must complete explicitly",
    );
    const artifact = result.artifacts.find((a) =>
      /(?:^|\/)transcript\.txt$/i.test(a.relativePath),
    );
    assert(artifact, "No actual transcript");
    const text = call("asr_task_read_artifact", {
      taskDir: original.taskDir,
      outputRoot: stage.output_root,
      relativePath: artifact.relativePath,
    }).content;
    assert(text.trim(), "No recognized text");
    assert(
      /语音|隔离|蓝色|字幕/.test(text),
      "Recognized text did not match the synthetic spoken fixture",
    );
    asr = {
      status: "completed",
      backend_status: result.job.status,
      text_sha256: hash(Buffer.from(text)),
      characters: text.length,
      fixture_keywords_verified: true,
      original_job: original.jobId,
    };
  } else {
    assert(
      probe.error ||
        probe.report?.blockers?.length ||
        probe.report?.ready_engine_count === 0,
      "Unavailable ASR needs a concrete reason",
    );
  }
  call("assistant_voice_discard", { recordingId: stage.recording_id });
  assert(!fs.existsSync(stage.source_path));
  const bad = call(
    "assistant_voice_discard",
    { recordingId: "../not-owned" },
    false,
  );
  assert(bad.ok === false || bad.job.status === "failed");
  const cipher = fs.readFileSync(
    path.join(home, ".homebase/assistant/state.dpapi"),
  );
  assert(!cipher.includes(Buffer.from(request.attachments[0].content)));
  const report = {
    passed: true,
    binary,
    binary_sha256: hash(fs.readFileSync(binary)),
    isolated_home: home,
    attachments_native_roundtrip: true,
    attachments_request_deduplicated: true,
    changed_attachment_rejected: true,
    encrypted_text: true,
    shared_settings_unchanged: true,
    voice_stage_owned: true,
    voice_sha256_verified: true,
    voice_discard_owned: true,
    local_asr_available: probe.available,
    local_asr: asr,
    local_asr_blockers: probe.available
      ? []
      : probe.report?.blockers?.map((b) => b.code) || [
          "environment_unavailable",
        ],
    real_model_calls: 0,
    real_wechat_messages: 0,
  };
  fs.writeFileSync(
    path.join(parent, "chat-input-native-validation.json"),
    JSON.stringify(report, null, 2),
  );
  console.log(JSON.stringify(report));
} finally {
  app.kill();
  await new Promise((r) => app.once("exit", r));
}

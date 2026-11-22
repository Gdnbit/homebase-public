import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { spawn, execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

export const recipe = Object.freeze({
  id: "hyperframes.animation",
  name: "HyperFrames · AI 代码动画",
  repository: "https://github.com/heygen-com/hyperframes",
  version: "0.9.26",
  license: "Apache-2.0",
  description:
    "由 AI 编写的本地动画模板生成 MP4；不是扩散模型文生视频。不使用服务商密钥。",
  input_schema: {
    type: "object",
    additionalProperties: false,
    properties: {
      title: { type: "string", maxLength: 80 },
      subtitle: { type: "string", maxLength: 140 },
      seconds: { type: "integer", minimum: 2, maximum: 12 },
    },
    required: ["title"],
  },
  output_schema: {
    type: "object",
    required: ["run_id", "status", "artifact", "verification"],
  },
});
const project = fileURLToPath(new URL("../", import.meta.url));
const owner = "homebase-capability-runtime-v1";
function fail(message) {
  throw new Error(message);
}
function noLink(target) {
  if (fs.existsSync(target) && fs.lstatSync(target).isSymbolicLink())
    fail("不接受符号链接或目录联接。");
}
export function rootPath(value) {
  if (
    typeof value !== "string" ||
    !path.isAbsolute(value) ||
    value.length > 500
  )
    fail("请指定独立的绝对安装目录。");
  const root = path.resolve(value);
  if (path.dirname(root) === root || root.startsWith("\\\\"))
    fail("不能使用磁盘根目录或网络共享。");
  for (let p = root; path.dirname(p) !== p; p = path.dirname(p)) noLink(p);
  return root;
}
function owned(root) {
  const marker = path.join(root, ".homebase-capability.json");
  noLink(marker);
  if (!fs.existsSync(marker)) fail("此目录不是已初始化的 HomeBase 能力目录。");
  if (fs.readFileSync(marker, "utf8") !== JSON.stringify({ owner }))
    fail("能力目录标记无效，原文件未修改。");
  for (const name of [
    "tools",
    "runtime",
    "runs",
    "home",
    "tmp",
    "cache",
    "installation.json",
  ])
    noLink(path.join(root, name));
}
function initialize(root) {
  if (fs.existsSync(path.join(root, ".homebase-capability.json"))) {
    owned(root);
    return;
  }
  if (fs.existsSync(root) && fs.readdirSync(root).length)
    fail("安装目录不是空目录，禁止覆盖现有数据。");
  fs.mkdirSync(root, { recursive: true });
  fs.writeFileSync(
    path.join(root, ".homebase-capability.json"),
    JSON.stringify({ owner }),
    { flag: "wx" },
  );
}
function readJson(file, max = 1024 * 1024) {
  noLink(file);
  if (fs.statSync(file).size > max) fail("能力记录超过读取上限。");
  try { return JSON.parse(fs.readFileSync(file, "utf8")); }
  catch { fail("能力记录格式无效，原文件未修改。"); }
}
function writeJson(file, value) {
  noLink(file);
  const temp = `${file}.${crypto.randomUUID()}.tmp`;
  fs.writeFileSync(temp, JSON.stringify(value, null, 2));
  fs.renameSync(temp, file);
}
function lock(root) {
  const target = path.join(root, ".operation.lock");
  try {
    fs.writeFileSync(
      target,
      JSON.stringify({
        pid: process.pid,
        started_at: new Date().toISOString(),
      }),
      { flag: "wx" },
    );
  } catch {
    fail(
      "该能力目录已有操作或上次操作中断，请核对任务后重试；不会自动覆盖锁。",
    );
  }
  return () => fs.unlinkSync(target);
}
export function cleanEnvironment(root) {
  const env = {};
  for (const key of ["SystemRoot", "WINDIR", "COMSPEC", "PATH", "PATHEXT"]) {
    const found = Object.keys(process.env).find(
      (name) => name.toLowerCase() === key.toLowerCase(),
    );
    if (found) env[key] = process.env[found];
  }
  Object.assign(env, {
    HOME: path.join(root, "home"),
    USERPROFILE: path.join(root, "home"),
    APPDATA: path.join(root, "home", "AppData", "Roaming"),
    LOCALAPPDATA: path.join(root, "home", "AppData", "Local"),
    TEMP: path.join(root, "tmp"),
    TMP: path.join(root, "tmp"),
    npm_config_cache: path.join(root, "cache", "npm"),
    npm_config_userconfig: path.join(root, "home", ".npmrc"),
    npm_config_globalconfig: path.join(root, "home", "global-npmrc"),
    npm_config_registry: "https://registry.npmjs.org",
    HYPERFRAMES_NO_TELEMETRY: "1",
    HYPERFRAMES_NO_UPDATE_CHECK: "1",
    CI: "1",
    HYPERFRAMES_FFMPEG_PATH: path.join(root, "tools", "ffmpeg.exe"),
    HYPERFRAMES_FFPROBE_PATH: path.join(
      root,
      "runtime",
      "node_modules",
      "@ffprobe-installer",
      "win32-x64",
      "ffprobe.exe",
    ),
  });
  return env;
}
export async function child(program, args, root, cwd, log, timeout = 180000, extraEnv = {}) {
  const env = { ...cleanEnvironment(root), ...extraEnv };
  return await new Promise((resolve, reject) => {
    const handle = spawn(program, args, {
      cwd,
      env,
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "",
      timedOut = false;
    const append = (chunk) => {
      output = (output + chunk.toString("utf8")).slice(-1024 * 1024);
    };
    handle.stdout.on("data", append);
    handle.stderr.on("data", append);
    const timer = setTimeout(() => {
      timedOut = true;
      if (process.platform === "win32" && handle.pid) {
        try {
          execFileSync(
            path.join(
              env.SystemRoot || "C:\\Windows",
              "System32",
              "taskkill.exe",
            ),
            ["/PID", String(handle.pid), "/T", "/F"],
            { windowsHide: true, stdio: "ignore" },
          );
        } catch {
          handle.kill();
        }
      } else handle.kill("SIGKILL");
    }, timeout);
    handle.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    handle.once("close", (code) => {
      clearTimeout(timer);
      if (log) fs.writeFileSync(log, output);
      if (timedOut) return reject(new Error("执行超时，已终止本次子进程树。"));
      if (code !== 0)
        return reject(
          new Error(`子进程退出码 ${code}，详情见本次隔离目录中的日志。`),
        );
      resolve(output);
    });
  });
}
function cli(root) {
  return path.join(
    root,
    "runtime/node_modules/hyperframes/bin/hyperframes.mjs",
  );
}
function installation(root) {
  owned(root);
  const value = readJson(path.join(root, "installation.json"));
  const pkg = readJson(
    path.join(root, "runtime/node_modules/hyperframes/package.json"),
  );
  if (
    value.recipe_id !== recipe.id ||
    pkg.version !== recipe.version ||
    value.version !== recipe.version ||
    !fs.existsSync(cli(root))
  )
    fail("能力安装不完整或版本不匹配，请重新安装。");
  return value;
}
export function catalog(value) {
  const base = {
    ...recipe,
    state: "not_installed",
    message: "尚未安装；请选择独立目录。",
    root_path: value || null,
  };
  if (!value) return base;
  try {
    const root = rootPath(value);
    if (!fs.existsSync(root)) return base;
    if (!fs.existsSync(path.join(root, ".homebase-capability.json")))
      return {
        ...base,
        state: "uninitialized",
        message: "未初始化，安装前会确认目录为空。",
      };
    const installed = installation(root);
    let verified = false;
    if (installed.last_verified_run) {
      const result = resultFor(root, installed.last_verified_run);
      verified = result.status === "succeeded";
    }
    return {
      ...base,
      state: verified ? "verified" : "installed",
      message: verified
        ? "已完成本机视频渲染及结果验证。"
        : "已安装，尚未通过本机视频验收。",
      installation: installed,
    };
  } catch (error) {
    return { ...base, state: "unknown", message: String(error.message) };
  }
}
export async function install(value) {
  const root = rootPath(value);
  initialize(root);
  owned(root);
  const unlock = lock(root);
  try {
    for (const folder of [
      "tools",
      "runtime",
      "runs",
      "home/AppData/Roaming",
      "home/AppData/Local",
      "tmp",
      "cache",
    ])
      fs.mkdirSync(path.join(root, folder), { recursive: true });
    fs.writeFileSync(path.join(root, "home/.npmrc"), "");
    fs.writeFileSync(path.join(root, "home/global-npmrc"), "");
    const runtime = path.join(root, "runtime");
    const expected = {
      private: true,
      dependencies: {
        hyperframes: recipe.version,
        gsap: "3.15.1",
        "@ffprobe-installer/win32-x64": "5.1.0",
      },
    };
    const pkg = path.join(runtime, "package.json");
    if (
      fs.existsSync(pkg) &&
      JSON.stringify(readJson(pkg)) !== JSON.stringify(expected)
    )
      fail("安装目录的 package.json 已被外部修改，停止覆盖。");
    writeJson(pkg, expected);
    const npm = path.join(
      path.dirname(process.execPath),
      "node_modules/npm/bin/npm-cli.js",
    );
    if (!fs.existsSync(npm)) fail("当前 Node 安装缺少 npm CLI。");
    const npmArgs = fs.existsSync(path.join(runtime, "package-lock.json"))
      ? ["ci"]
      : ["install"];
    await child(
      process.execPath,
      [npm, ...npmArgs, "--ignore-scripts", "--no-audit", "--no-fund"],
      root,
      runtime,
      path.join(root, "install.log"),
      600000,
    );
    const lockfile = readJson(
      path.join(runtime, "package-lock.json"),
      5 * 1024 * 1024,
    );
    if (
      lockfile.packages["node_modules/hyperframes"]?.integrity !==
      "sha512-g9+qCbnF9ylmyid4V23PlwXIVy8FUQdM9pBwJuqM12X33zYnwWAEojGlDeiQqgPwO4r/sAnw1PQFFyqAKgQQcA=="
    )
      fail("HyperFrames 包校验与审阅版本不一致。");
    fs.copyFileSync(
      path.join(project, "src-tauri/resources/distribution/tools/ffmpeg.exe"),
      path.join(root, "tools/ffmpeg.exe"),
    );
    // Explicit browser cache HOME keeps downloads out of the user's browser/profile directories.
    const browserOutput = await child(
      process.execPath,
      [cli(root), "browser", "path"],
      root,
      runtime,
      path.join(root, "browser.log"),
      240000,
    );
    const browser = browserOutput
      .trim()
      .split(/\r?\n/)
      .find((line) => fs.existsSync(line.trim()));
    if (!browser) fail("浏览器探测未返回有效路径，请检查 browser.log。");
    const versionOutput = await child(
      process.execPath,
      [cli(root), "--version"],
      root,
      runtime,
      path.join(root, "version.log"),
      30000,
    );
    if (!versionOutput.includes(recipe.version)) fail("CLI 版本未通过检查。");
    const result = {
      recipe_id: recipe.id,
      version: recipe.version,
      repository: recipe.repository,
      license: recipe.license,
      browser_path: browser.trim(),
      installed_at: new Date().toISOString(),
    };
    writeJson(path.join(root, "installation.json"), result);
    return {
      status: "succeeded",
      ...result,
      root_path: root,
      verification: "installation_only",
    };
  } finally {
    unlock();
  }
}
export function videoInput(input) {
  if (
    !input ||
    typeof input !== "object" ||
    Array.isArray(input) ||
    Object.keys(input).some(
      (key) => !["title", "subtitle", "seconds"].includes(key),
    )
  )
    fail("输入只接受 title、subtitle、seconds。");
  const { title, subtitle = "", seconds = 6 } = input;
  if (
    typeof title !== "string" ||
    !title.trim() ||
    title.length > 80 ||
    typeof subtitle !== "string" ||
    subtitle.length > 140 ||
    !Number.isInteger(seconds) ||
    seconds < 2 ||
    seconds > 12
  )
    fail("标题须为 1–80 字，副标题至多 140 字，时长为 2–12 秒整数。");
  return { title: title.trim(), subtitle, seconds };
}
const escape = (text) =>
  text.replace(
    /[&<>"']/g,
    (char) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        char
      ],
  );
export function composition(input) {
  const { title, subtitle, seconds } = videoInput(input);
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'self' 'unsafe-inline' data:; connect-src 'none';"><style>
*{box-sizing:border-box;margin:0}html,body{width:960px;height:540px;overflow:hidden;background:#101820;color:#f6f0e5;font-family:Arial,'Microsoft YaHei',sans-serif}#root{position:relative;width:960px;height:540px;overflow:hidden}#orb{position:absolute;width:460px;height:460px;border:48px solid #e77547;border-radius:50%;left:550px;top:40px}#bar{position:absolute;left:0;top:0;width:18px;height:540px;background:#e77547}#title{position:absolute;left:66px;top:146px;width:740px;font-size:64px;line-height:1.12;overflow-wrap:anywhere;letter-spacing:-2px}#subtitle{position:absolute;left:70px;top:342px;width:640px;font-size:26px;line-height:1.4;color:#cbc4b7}#line{position:absolute;left:70px;top:432px;width:230px;height:5px;background:#e77547}
</style><script src="gsap.min.js"></script></head><body><main id="root" data-composition-id="main" data-start="0" data-duration="${seconds}" data-width="960" data-height="540"><div id="orb" class="clip" data-start="0" data-duration="${seconds}"></div><div id="bar"></div><h1 id="title" class="clip" data-start="0" data-duration="${seconds}">${escape(title)}</h1><p id="subtitle" class="clip" data-start="0" data-duration="${seconds}">${escape(subtitle)}</p><div id="line"></div></main><script>
const tl=gsap.timeline({paused:true});tl.fromTo('#orb',{x:420,scale:.45,rotation:-80},{x:0,scale:1,rotation:0,duration:1.2,ease:'power3.out'},0).fromTo('#title',{y:100,opacity:0},{y:0,opacity:1,duration:.7,ease:'power3.out'},.15).fromTo('#subtitle',{y:30,opacity:0},{y:0,opacity:1,duration:.7},.65).fromTo('#line',{scaleX:0,transformOrigin:'left'},{scaleX:1,duration:.8},.85).to('#orb',{x:80,rotation:45,scale:1.15,duration:${Math.max(1, seconds - 2)},ease:'sine.inOut'},1.3);window.__timelines={main:tl};tl.seek(0);
</script></body></html>`;
}
export function resultFor(rootValue, id) {
  const root = rootPath(rootValue);
  owned(root);
  if (typeof id !== "string" || !/^[a-f0-9-]{36}$/.test(id))
    fail("无效的 run_id。");
  const folder = path.join(root, "runs", id);
  noLink(folder);
  const result = readJson(path.join(folder, "result.json"));
  if (
    result.run_id !== id ||
    result.status !== "succeeded" ||
    result.capability_id !== recipe.id ||
    result.version !== recipe.version ||
    result.verification?.decoded !== true
  )
    fail("该任务没有通过验证的成功结果。");
  const artifact = path.join(folder, "output.mp4");
  noLink(artifact);
  const stat = fs.statSync(artifact);
  if (
    stat.size > 64 * 1024 * 1024 ||
    crypto
      .createHash("sha256")
      .update(fs.readFileSync(artifact))
      .digest("hex") !== result.artifact.sha256
  )
    fail("结果文件缺失、过大或已被外部修改。");
  return { ...result, artifact: { ...result.artifact, path: artifact } };
}
export async function render(rootValue, value) {
  const root = rootPath(rootValue);
  const input = videoInput(value);
  const installed = installation(root);
  const unlock = lock(root);
  const id = crypto.randomUUID();
  const folder = path.join(root, "runs", id);
  try {
    fs.mkdirSync(folder);
    writeJson(path.join(folder, "input.json"), input);
    fs.writeFileSync(path.join(folder, "index.html"), composition(input));
    fs.copyFileSync(
      path.join(root, "runtime/node_modules/gsap/dist/gsap.min.js"),
      path.join(folder, "gsap.min.js"),
    );
    const args = [
      cli(root),
      "render",
      folder,
      "--output",
      path.join(folder, "output.mp4"),
      "--fps",
      "24",
      "--workers",
      "1",
      "--quality",
      "standard",
      "--no-browser-gpu",
    ];
    await child(
      process.execPath,
      args,
      root,
      folder,
      path.join(folder, "render.log"),
      180000,
      { HYPERFRAMES_BROWSER_PATH: installed.browser_path },
    );
    const env = cleanEnvironment(root);
    const probed = await child(
      env.HYPERFRAMES_FFPROBE_PATH,
      [
        "-v",
        "error",
        "-show_entries",
        "stream=codec_type,codec_name,width,height,nb_frames,avg_frame_rate:format=duration",
        "-of",
        "json",
        path.join(folder, "output.mp4"),
      ],
      root,
      folder,
      path.join(folder, "probe.json"),
      30000,
    );
    const metadata = JSON.parse(probed);
    const stream = metadata.streams?.find(
      (stream) => stream.codec_type === "video",
    );
    if (
      !stream ||
      stream.width !== 960 ||
      stream.height !== 540 ||
      Math.abs(Number(metadata.format.duration) - input.seconds) > 0.15 ||
      Number(stream.nb_frames) !== input.seconds * 24
    )
      fail("输出视频尺寸、帧数或时长不符合契约。");
    await child(
      env.HYPERFRAMES_FFMPEG_PATH,
      ["-v", "error", "-i", path.join(folder, "output.mp4"), "-f", "null", "-"],
      root,
      folder,
      path.join(folder, "decode.log"),
      30000,
    );
    const data = fs.readFileSync(path.join(folder, "output.mp4"));
    const result = {
      run_id: id,
      status: "succeeded",
      capability_id: recipe.id,
      version: recipe.version,
      completed_at: new Date().toISOString(),
      artifact: {
        path: path.join(folder, "output.mp4"),
        bytes: data.length,
        sha256: crypto.createHash("sha256").update(data).digest("hex"),
        mime_type: "video/mp4",
      },
      verification: {
        width: stream.width,
        height: stream.height,
        fps: stream.avg_frame_rate,
        frames: Number(stream.nb_frames),
        seconds: Number(metadata.format.duration),
        decoded: true,
        audio: "none",
      },
    };
    writeJson(path.join(folder, "result.json"), result);
    writeJson(path.join(root, "installation.json"), {
      ...installed,
      last_verified_run: id,
    });
    return result;
  } catch (error) {
    if (fs.existsSync(folder))
      writeJson(path.join(folder, "failure.json"), {
        run_id: id,
        status: "failed",
        error: error.message,
      });
    return {
      run_id: id,
      status: "failed",
      error: error.message,
      log_path: path.join(folder, "render.log"),
    };
  } finally {
    unlock();
  }
}
export async function dispatch(action, request) {
  if (action === "catalog") return catalog(request.root_path);
  if (action === "install") return install(request.root_path);
  if (action === "render") return render(request.root_path, request.input);
  if (action === "result") return resultFor(request.root_path, request.run_id);
  fail("不支持的能力操作。");
}
if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    const chunks = [];
    let size = 0;
    for await (const chunk of process.stdin) {
      chunks.push(chunk);
      size += chunk.length;
      if (size > 16000) fail("请求过大。");
    }
    const request = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const result = await dispatch(process.argv[2], request);
    process.stdout.write(JSON.stringify(result));
  } catch (error) {
    process.stdout.write(
      JSON.stringify({ status: "failed", error: error.message }),
    );
    process.exitCode = 1;
  }
}

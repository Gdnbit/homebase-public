import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const publicRoot = "ff29e733da6e1d09b909ca7779b77f444c47f8d4";
const commitId = /^[a-f0-9]{40}$/;
const missingRef = "0".repeat(40);
const generatedDirectory = /(?:^|\/)(?:\.homebase|\.tmp|\.local-work|node_modules|dist|dist-ssr|target|distribution|release|outputs|test-results|playwright-report|private-data|business-data)(?:\/|$)/i;
const privateFile = /(?:^|\/)(?:secrets(?:-meta)?\.ya?ml|secrets-meta\.json|endpoint\.json|mcp-pool\.json|credentials\.json|local\.properties|keystore\.properties)$|\.(?:dpapi|age|sops|pem|key|p12|pfx|jks|keystore|db|sqlite3?|exe|dll|msi|log|zip|7z|rar|tar|gz|tgz|mp4|wav|mp3)(?:-(?:wal|shm|journal))?$|(?:^|\/)agent-runtime\/tools\/bwrap$/i;
const envFile = /(?:^|\/)\.env(?:\.|$)/i;
const envTemplate = /(?:^|\/)\.env(?:\.[a-z0-9-]+)?\.(?:example|template)$/i;

function git(args, optional = false) {
  const result = spawnSync("git", args, {
    cwd: root,
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 64 * 1024 * 1024,
  });
  if (!result.error && result.status === 0) return result.stdout;
  if (optional && !result.error && result.status === 1) return "";
  throw Error("无法读取 Git 暂存区或提交，请核对本机仓库后重试。");
}

function checkPaths(paths) {
  if (paths.split("\0").some((name) =>
    generatedDirectory.test(name) || privateFile.test(name) ||
    (envFile.test(name) && !envTemplate.test(name))
  )) {
    throw Error("发现本机配置、数据或构建产物，请查看 Git 文件清单并改用合成夹具。");
  }
}

function checkAddedText(diff) {
  for (const line of diff.split("\n")) {
    if (!line.startsWith("+") || line.startsWith("+++")) continue;
    if (/\bwxid_[a-z0-9_-]{6,}\b/i.test(line)) {
      throw Error("发现疑似微信账号标识，请替换为合成值。");
    }
    const profile = /(?:^|[^a-z])[a-z]:[\\/]{1,2}users[\\/]{1,2}([a-z0-9_.-]+)/gi;
    if ([...line.matchAll(profile)].some((match) =>
      !/^(?:homebase|example|fixture|synthetic|isolated|public|default)$/i.test(match[1])
    )) {
      throw Error("发现具体 Windows 用户目录，请使用环境变量或合成路径。");
    }
  }
}

function scan(args, input) {
  const binary = git(["config", "--get", "homebase.gitleaksPath"], true).trim() || "gitleaks";
  const result = spawnSync(binary, [
    ...args,
    "--redact=100",
    "--no-banner",
    "--log-level", "error",
    "--exit-code", "23",
  ], { cwd: root, input, stdio: ["pipe", "ignore", "ignore"], windowsHide: true });
  if (result.status === 23) throw Error("发现疑似凭据，提交或推送已停止。");
  if (result.error || result.status !== 0) {
    throw Error("Gitleaks 未成功运行，请安装并设置 homebase.gitleaksPath 后重试。");
  }
}

function staged() {
  const flags = ["diff", "--cached", "--no-ext-diff", "--no-renames", "--diff-filter=ACM"];
  checkPaths(git([...flags, "--name-only", "-z"]));
  checkAddedText(git([...flags, "--no-color", "--unified=0"]));
  scan(["git", "--staged", "."]);
}

function outgoing(refs) {
  const ranges = new Set();
  for (const line of refs.trim().split(/\r?\n/).filter(Boolean)) {
    const fields = line.split(/\s+/);
    if (fields.length !== 4 || !commitId.test(fields[1]) || !commitId.test(fields[3])) {
      throw Error("无法识别 Git 推送引用，已停止推送。");
    }
    const [, localId, , remoteId] = fields;
    if (localId === missingRef) continue;
    const tip = git(["rev-parse", localId + "^{commit}"]).trim();
    if (git(["rev-list", "--max-parents=0", tip]).trim() !== publicRoot) {
      throw Error("提交包含公开版之外的历史。在公开主检出中逐项引入已审查的改动。");
    }
    if (tip !== localId) scan(["stdin"], git(["cat-file", "-p", localId]));
    const base = remoteId === missingRef ? publicRoot : git(["rev-parse", remoteId + "^{commit}"]).trim();
    if (base !== tip) ranges.add(base + ".." + tip);
  }
  for (const range of ranges) {
    const log = ["log", "--no-ext-diff", "--no-renames", "-m", range];
    checkPaths(git([...log, "--format=", "--diff-filter=ACM", "--name-only", "-z"]));
    checkAddedText(git([...log, "--format=", "--no-color", "--unified=0"]));
    scan(["git", "--log-opts=-m " + range, "."]);
    scan(["stdin"], git(["log", "--format=%B", range]));
  }
}

try {
  if (process.argv[2] === "--outgoing") outgoing(readFileSync(0, "utf8"));
  else staged();
  console.log("HomeBase 隐私检查通过。");
} catch (error) {
  console.error("HomeBase 隐私检查未通过。 " + error.message);
  process.exitCode = 1;
}

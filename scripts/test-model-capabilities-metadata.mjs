// Explicit read-only account model-directory check. Never submits inference or uploads materials.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
if (!process.argv.includes("--metadata-only")) throw Error("Requires --metadata-only");
const project = fileURLToPath(new URL("../", import.meta.url));
const pointer = path.join(project, ".tmp/model-capabilities-lab.txt");
const lab = fs.existsSync(pointer) ? fs.readFileSync(pointer, "utf8").trim() : path.join("F:\\HomeBase-Capability-Lab", `model-library-${crypto.randomUUID()}`);
if (!/^F:\\HomeBase-Capability-Lab\\model-library-[A-Za-z0-9T:.-]+$/.test(lab)) throw Error("Unexpected metadata lab path");
fs.mkdirSync(lab, { recursive: true }); fs.writeFileSync(pointer, lab, "utf8");
const cli = path.join(project, "scripts/homebase.mjs");
function save(name, value) { const file = path.join(lab, name); fs.writeFileSync(file, JSON.stringify(value, null, 2), "utf8"); return file; }
function run(args) {
  if (args[0] === "call" && !["model_capabilities_catalog", "model_capabilities_refresh", "model_capability_resolve"].includes(args[1])) throw Error("Metadata check forbids inference and other operations");
  let stdout;
  try { stdout = execFileSync(process.execPath, [cli, ...args], { cwd: project, encoding: "utf8", windowsHide: true, timeout: 35000, stdio: ["ignore", "pipe", "pipe"] }); }
  catch (error) { stdout = error.stdout; if (!stdout) throw Error("CLI transport failed; keep the existing request ID"); }
  return JSON.parse(stdout);
}
let doctor;
for (let i = 0; i < 20; i++) { doctor = run(["doctor"]); if (doctor.ok) break; await new Promise(resolve => setTimeout(resolve, 1500)); }
save("doctor.json", doctor); if (!doctor.ok) throw Error("Doctor did not become ready");
const input = save("refresh-input.json", { providerIds: ["openrouter", "openai", "fal", "elevenlabs", "dashscope"] });
const idFile = path.join(lab, "request-id.json");
const requestId = fs.existsSync(idFile) ? JSON.parse(fs.readFileSync(idFile, "utf8")).id : `model-catalog-read-${crypto.randomUUID()}`;
save("request-id.json", { id: requestId });
const accepted = run(["call", "model_capabilities_refresh", "--allow-private", "--allow-external", "--input", input, "--request-id", requestId, "--detach"]);
save("refresh-accepted.json", accepted); if (!accepted.ok) throw Error("Refresh not accepted");
console.log(JSON.stringify({ step: "refresh", job: accepted.job.id, reused: accepted.reused }));
let result;
for (let i = 0; i < 500; i++) {
  const value = run(["jobs", "get", accepted.job.id, "--allow-private", "--allow-external"]);
  if (value.job.status !== "running") { save("refresh-result.json", value); if (!value.ok) throw Error("Refresh failed; inspect original job"); result = value.result; break; }
  if (i % 10 === 0) console.log(JSON.stringify({ step: "refresh", status: value.job.status }));
  await new Promise(resolve => setTimeout(resolve, 1500));
}
if (!result) throw Error("Wait expired; use saved job ID without resubmission");
const models = [...result.models]; let next = result.next_offset; let page = 0;
while (next !== null) {
  const request = save(`page-${page}-input.json`, { offset: next, limit: 200 });
  const value = run(["call", "model_capabilities_catalog", "--allow-private", "--input", request]);
  if (!value.ok) throw Error("Catalog page failed");
  save(`page-${page++}.json`, value); models.push(...value.result.models); next = value.result.next_offset;
}
save("catalog-complete.json", { ...result, models, next_offset: null });
const route = models.find(m => m.provider_id === "openrouter" && m.publisher === "openai" && m.protocol === "chat") || models.find(m => m.provider_id === "openrouter" && m.protocol === "chat");
if (route) {
  const input = save("resolve-input.json", { request: { route_id: route.route_id, input: route.input_example, output_root: null } });
  const value = run(["call", "model_capability_resolve", "--allow-private", "--input", input]); save("resolve-result.json", value);
  if (!value.ok || value.result.credential.provider_id !== route.provider_id || !value.result.endpoint.startsWith("https://openrouter.ai/")) throw Error("Channel/key binding mismatch");
}
const summary = { lab, hash: doctor.catalog_hash, models: models.length, providers: result.credentials.map(c => ({ id: c.provider_id, state: c.state, source: c.source, ref: c.credential_ref })), sources: result.sources, groups: result.functions.map(f => ({ id: f.id, name: f.name, count: models.filter(m => m.functions.includes(f.id)).length })), realGenerationCalls: 0 };
save("summary.json", summary); console.log(JSON.stringify(summary));

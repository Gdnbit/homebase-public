import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import ts from "typescript";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sourcePath = resolve(repoRoot, "src/lib/asr-portability.ts");
const tempPath = resolve(repoRoot, ".tmp/asr-portability-test.mjs");

const source = await readFile(sourcePath, "utf8");
const output = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
    strict: true,
  },
});

await mkdir(dirname(tempPath), { recursive: true });
await writeFile(tempPath, output.outputText, "utf8");

const {
  createDefaultAsrPortableConfig,
  exportAsrPortableBundle,
  importAsrPortableBundle,
  validateAsrPortableConfig,
} = await import(pathToFileURL(tempPath).href + `?t=${Date.now()}`);

const machine = {
  source: "live",
  preferredModelRoot: "F:\\AIModels",
  preferredModelDrive: "F:",
  disks: [
    { drive: "C:", total_gb: 500, free_gb: 20, writable: true },
    { drive: "E:", total_gb: 500, free_gb: 80, writable: true },
    { drive: "F:", total_gb: 1000, free_gb: 250, writable: true },
  ],
};

const base = createDefaultAsrPortableConfig(machine);
const baseValidation = validateAsrPortableConfig(base, machine);
assert.equal(baseValidation.ready, true);
assert.equal(baseValidation.status, "ready");

const emptyRoot = structuredClone(base);
emptyRoot.roots.model_root = "";
assert.equal(validateAsrPortableConfig(emptyRoot, machine).status, "blocked");

const collidingRoots = structuredClone(base);
collidingRoots.roots.runtime_root = collidingRoots.roots.model_root;
assert.ok(
  validateAsrPortableConfig(collidingRoots, machine).issues.some(
    (issue) => issue.code === "root_collision" && issue.severity === "error",
  ),
);

const nestedOutput = structuredClone(base);
nestedOutput.roots.output_root = `${nestedOutput.roots.model_root}\\outputs`;
assert.ok(
  validateAsrPortableConfig(nestedOutput, machine).issues.some(
    (issue) => issue.code === "output_nested_in_runtime_data",
  ),
);

const systemDrive = structuredClone(base);
systemDrive.roots.model_root = "C:\\AIModels";
const systemDriveValidation = validateAsrPortableConfig(systemDrive, machine);
assert.equal(systemDriveValidation.ready, true);
assert.ok(systemDriveValidation.issues.some((issue) => issue.code === "model_root_on_system_drive"));

const lowSpaceMachine = {
  ...machine,
  disks: [{ drive: "F:", total_gb: 1000, free_gb: 4, writable: true }],
};
assert.ok(
  validateAsrPortableConfig(base, lowSpaceMachine).issues.some(
    (issue) => issue.code === "model_drive_low_space" && issue.severity === "error",
  ),
);

const secretLeak = structuredClone(base);
secretLeak.provider_secret_refs = ["sk-live-secret-value"];
assert.ok(
  validateAsrPortableConfig(secretLeak, machine).issues.some(
    (issue) => issue.code === "secret_value_in_config" && issue.severity === "error",
  ),
);

const disabledDefault = structuredClone(base);
disabledDefault.enabled_preset_ids = ["asr_local_sprint"];
assert.ok(
  validateAsrPortableConfig(disabledDefault, machine).issues.some(
    (issue) => issue.code === "default_preset_disabled" && issue.severity === "error",
  ),
);

const custom = structuredClone(base);
custom.custom_presets = [
  {
    id: "custom_private_review",
    name: "本地复核",
    description: "两个本地引擎交叉校对",
    engine_ids: ["faster_whisper_large_v3_cpu", "sensevoice_small"],
    fusion_mode: "auto",
    scene_id: "auto",
    language: "zh",
  },
];
custom.default_preset_id = "custom_private_review";
assert.equal(validateAsrPortableConfig(custom, machine).ready, true);

const emptyCustom = structuredClone(custom);
emptyCustom.custom_presets[0].engine_ids = [];
assert.ok(
  validateAsrPortableConfig(emptyCustom, machine).issues.some(
    (issue) => issue.code === "custom_preset_engine_count_invalid" && issue.severity === "error",
  ),
);

const duplicateCustom = structuredClone(custom);
duplicateCustom.custom_presets.push({
  ...duplicateCustom.custom_presets[0],
  id: "custom_private_review_2",
});
assert.ok(
  validateAsrPortableConfig(duplicateCustom, machine).issues.some(
    (issue) => issue.code === "custom_preset_name_duplicate" && issue.severity === "error",
  ),
);

const staleEngine = structuredClone(custom);
staleEngine.custom_presets[0].engine_ids = ["provider_model_removed"];
assert.ok(
  validateAsrPortableConfig(staleEngine, machine).issues.some(
    (issue) => issue.code === "custom_preset_engine_unknown" && issue.severity === "warning",
  ),
);

const unnecessaryFusion = structuredClone(custom);
unnecessaryFusion.custom_presets[0].engine_ids = ["sensevoice_small"];
unnecessaryFusion.custom_presets[0].fusion_mode = "llm";
assert.ok(
  validateAsrPortableConfig(unnecessaryFusion, machine).issues.some(
    (issue) => issue.code === "custom_preset_fusion_without_comparison",
  ),
);

const staleModes = structuredClone(custom);
staleModes.custom_presets[0].scene_id = "legacy_conference_room";
staleModes.custom_presets[0].language = "xx";
const staleModeIssues = validateAsrPortableConfig(staleModes, machine).issues;
assert.ok(staleModeIssues.some((issue) => issue.code === "custom_preset_scene_unknown"));
assert.ok(staleModeIssues.some((issue) => issue.code === "custom_preset_language_unknown"));

const bundle = exportAsrPortableBundle(base);
assert.equal(bundle.format, "homebase-asr-portable-profile");
assert.equal(bundle.schema_version, 2);
assert.equal(bundle.config.roots.model_root, "${MODEL_ROOT}");
assert.deepEqual(bundle.config.provider_secret_refs, base.provider_secret_refs);
assert.ok(!JSON.stringify(bundle).includes("sk-live-secret-value"));

const imported = importAsrPortableBundle(bundle, {
  aihub_root: "D:\\HomeBase\\AIHub",
  model_root: "D:\\HomeBaseData\\Models",
  runtime_root: "D:\\HomeBaseData\\Runtimes",
  output_root: "D:\\HomeBaseData\\Wiki\\raw\\audio",
  cache_root: "D:\\HomeBaseData\\Cache",
});
assert.equal(imported.roots.model_root, "D:\\HomeBaseData\\Models");
assert.equal(imported.default_preset_id, base.default_preset_id);
assert.equal(validateAsrPortableConfig(imported).ready, true);

const customImported = importAsrPortableBundle(exportAsrPortableBundle(custom), {
  ...custom.roots,
});
assert.deepEqual(customImported.custom_presets, custom.custom_presets);

const legacyBundle = structuredClone(bundle);
legacyBundle.schema_version = 1;
delete legacyBundle.config.custom_presets;
const migratedLegacy = importAsrPortableBundle(legacyBundle, { ...base.roots });
assert.equal(migratedLegacy.schema_version, 2);
assert.deepEqual(migratedLegacy.custom_presets, []);

assert.throws(() => importAsrPortableBundle({ format: "other" }), /不是 HomeBase/);

console.log("asr-portability tests passed (19 edge cases)");

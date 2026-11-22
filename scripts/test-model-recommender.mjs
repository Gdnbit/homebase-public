import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import ts from "typescript";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sourcePath = resolve(repoRoot, "src/lib/model-recommender.ts");
const tempPath = resolve(repoRoot, ".tmp/model-recommender-test.mjs");

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
  deriveMachineProfile,
  recommendModel,
  recommendModels,
  summarizeRecommendations,
} = await import(pathToFileURL(tempPath).href + `?t=${Date.now()}`);

const cpuLocal = {
  id: "sensevoice-small",
  name: "SenseVoiceSmall",
  category: "asr",
  status: "candidate",
  hardware_fit: "cpu_ok",
  estimated_size_gb: 2,
  min_ram_gb: 8,
  min_logical_cores: 4,
  recommended_vram_gb: 0,
  runtime_requirements: ["python"],
  local_priority: "recommended",
  api_fallback: false,
};

const cpuWithApi = {
  ...cpuLocal,
  id: "faster-whisper-large-v3",
  name: "faster-whisper large-v3",
  estimated_size_gb: 4,
  api_fallback: true,
};

const cudaWithApi = {
  id: "whisperx-pyannote",
  name: "WhisperX + pyannote",
  category: "asr_diarization",
  status: "candidate",
  hardware_fit: "gpu_rich",
  estimated_size_gb: 8,
  min_ram_gb: 16,
  min_logical_cores: 8,
  recommended_vram_gb: 8,
  runtime_requirements: ["python", "cuda"],
  local_priority: "deferred",
  api_fallback: true,
};

const hugeLocal = {
  ...cpuLocal,
  id: "huge-local",
  name: "Huge Local",
  estimated_size_gb: 200,
  local_priority: "optional",
};

function machine(overrides = {}) {
  const base = {
    source: "live",
    probeCompleteness: "complete",
    info: {
      cpu_brand: "Test CPU",
      cpu_logical_cores: 12,
      memory_total_gb: 16,
      gpu_name: null,
    },
    gpu: {
      available: false,
      gpu_name: null,
      error: null,
      probe_state: "absent",
    },
    disks: [
      {
        drive: "D:",
        total_gb: 500,
        free_gb: 120,
        usage_percent: 76,
        warning: false,
        writable: true,
      },
    ],
    pathEntries: [],
    runtimeProbes: {
      python: { state: "available", version: "3.11" },
      cuda: { state: "missing" },
      ffmpeg: { state: "available", version: "7" },
    },
    manifestState: "loaded",
    installedModels: [],
  };
  const runtimeProbes = Object.prototype.hasOwnProperty.call(overrides, "runtimeProbes")
    ? overrides.runtimeProbes === null
      ? undefined
      : { ...base.runtimeProbes, ...overrides.runtimeProbes }
    : base.runtimeProbes;
  return deriveMachineProfile({
    ...base,
    ...overrides,
    info: { ...base.info, ...(overrides.info ?? {}) },
    gpu: { ...base.gpu, ...(overrides.gpu ?? {}) },
    runtimeProbes,
  });
}

function status(model, profile) {
  return recommendModel(model, profile).recommendation_status;
}

const cases = [
  {
    name: "verified install is runnable",
    model: cpuLocal,
    profile: machine({ installedModels: [{ id: cpuLocal.id, state: "installed", path_exists: true }] }),
    expected: "installed",
  },
  {
    name: "probe record is not an install",
    model: cpuLocal,
    profile: machine({ installedModels: [{ id: cpuLocal.id, state: "probed", path_exists: true }] }),
    expected: "partial_install",
  },
  {
    name: "legacy installed id is only probe evidence",
    model: cpuLocal,
    profile: machine({ installedModelIds: [cpuLocal.id], installedModels: [] }),
    expected: "partial_install",
  },
  {
    name: "untrusted manifest cannot prove an install",
    model: cpuLocal,
    profile: machine({
      manifestState: "invalid",
      installedModels: [{ id: cpuLocal.id, state: "installed", path_exists: true }],
    }),
    expected: "partial_install",
  },
  {
    name: "missing installed path is partial",
    model: cpuLocal,
    profile: machine({ installedModels: [{ id: cpuLocal.id, state: "installed", path_exists: false }] }),
    expected: "partial_install",
  },
  {
    name: "corrupt install is partial",
    model: cpuLocal,
    profile: machine({ installedModels: [{ id: cpuLocal.id, state: "corrupt", path_exists: true }] }),
    expected: "partial_install",
  },
  {
    name: "large model is blocked by safe disk reserve",
    model: hugeLocal,
    profile: machine(),
    expected: "space_blocked",
  },
  {
    name: "unwritable only drive is blocked",
    model: cpuLocal,
    profile: machine({ disks: [{ drive: "D:", total_gb: 500, free_gb: 120, writable: false }] }),
    expected: "storage_blocked",
  },
  {
    name: "unwritable drive selects API fallback when available",
    model: cpuWithApi,
    profile: machine({ disks: [{ drive: "D:", total_gb: 500, free_gb: 120, writable: false }] }),
    expected: "api_preferred",
  },
  {
    name: "no disk facts require review for local-only model",
    model: cpuLocal,
    profile: machine({ disks: [] }),
    expected: "needs_review",
  },
  {
    name: "no disk facts choose API when available",
    model: cpuWithApi,
    profile: machine({ disks: [] }),
    expected: "api_preferred",
  },
  {
    name: "system-drive-only can recommend with warning when space is ample",
    model: cpuLocal,
    profile: machine({ disks: [{ drive: "C:", total_gb: 1000, free_gb: 500, writable: true }] }),
    expected: "recommended",
    warning: "系统盘",
  },
  {
    name: "explicit system-drive root warns even when another data drive exists",
    model: cpuLocal,
    profile: machine({
      preferredModelRoot: "C:\\AIModels",
      disks: [
        { drive: "C:", total_gb: 1000, free_gb: 500, writable: true },
        { drive: "D:", total_gb: 1000, free_gb: 900, writable: true },
      ],
    }),
    expected: "recommended",
    warning: "系统盘",
  },
  {
    name: "explicit root uses its own drive capacity instead of the largest drive",
    model: cpuLocal,
    profile: machine({
      preferredModelRoot: "F:\\AIModels",
      disks: [
        { drive: "D:", total_gb: 1000, free_gb: 900, writable: true },
        { drive: "F:", total_gb: 100, free_gb: 5, writable: true },
      ],
    }),
    expected: "space_blocked",
  },
  {
    name: "explicit root without matching disk facts requires review",
    model: cpuLocal,
    profile: machine({
      preferredModelRoot: "F:\\AIModels",
      disks: [{ drive: "D:", total_gb: 1000, free_gb: 900, writable: true }],
    }),
    expected: "needs_review",
  },
  {
    name: "invalid disk capacity is not treated as usable storage",
    model: cpuLocal,
    profile: machine({ disks: [{ drive: "D:", total_gb: 500, free_gb: Number.NaN, writable: true }] }),
    expected: "needs_review",
  },
  {
    name: "unknown RAM does not auto-approve local",
    model: cpuLocal,
    profile: machine({ info: { memory_total_gb: null } }),
    expected: "needs_review",
  },
  {
    name: "PATH presence without a version probe stays unverified",
    model: cpuLocal,
    profile: machine({
      pathEntries: [{ path: "D:\\Tools\\Python311", exists: true }],
      runtimeProbes: null,
    }),
    expected: "needs_review",
  },
  {
    name: "low RAM blocks local-only model",
    model: cpuLocal,
    profile: machine({ info: { memory_total_gb: 4 } }),
    expected: "unsupported",
  },
  {
    name: "low RAM falls back to API",
    model: cpuWithApi,
    profile: machine({ info: { memory_total_gb: 4 } }),
    expected: "api_preferred",
  },
  {
    name: "invalid RAM value does not auto-approve local",
    model: cpuLocal,
    profile: machine({ info: { memory_total_gb: -1 } }),
    expected: "needs_review",
  },
  {
    name: "unknown CPU core count does not auto-approve local",
    model: cpuLocal,
    profile: machine({ info: { cpu_logical_cores: null } }),
    expected: "needs_review",
  },
  {
    name: "low CPU core count blocks local-only model",
    model: cpuLocal,
    profile: machine({ info: { cpu_logical_cores: 2 } }),
    expected: "unsupported",
  },
  {
    name: "low CPU core count selects API fallback",
    model: cpuWithApi,
    profile: machine({ info: { cpu_logical_cores: 2 } }),
    expected: "api_preferred",
  },
  {
    name: "no GPU falls back to API for CUDA model",
    model: cudaWithApi,
    profile: machine(),
    expected: "api_preferred",
  },
  {
    name: "integrated GPU is not treated as discrete VRAM",
    model: cudaWithApi,
    profile: machine({
      info: { gpu_name: "Intel Iris Xe Graphics" },
      gpu: { available: true, gpu_name: "Intel Iris Xe Graphics", probe_state: "available" },
    }),
    expected: "api_preferred",
  },
  {
    name: "unknown GPU state does not guess",
    model: cudaWithApi,
    profile: machine({
      gpu: { available: false, gpu_name: null, probe_state: "unknown", error: "probe failed" },
    }),
    expected: "api_preferred",
  },
  {
    name: "inferred adequate VRAM still requires verification",
    model: cudaWithApi,
    profile: machine({
      info: { gpu_name: "NVIDIA GeForce RTX 4070" },
      gpu: { available: true, gpu_name: "NVIDIA GeForce RTX 4070", probe_state: "available" },
      runtimeProbes: { cuda: { state: "available" } },
    }),
    expected: "api_preferred",
  },
  {
    name: "reported VRAM with runtime mismatch falls back to API",
    model: cudaWithApi,
    profile: machine({
      info: { gpu_name: "NVIDIA RTX 4090" },
      gpu: {
        available: true,
        gpu_name: "NVIDIA RTX 4090",
        dedicated_memory_gb: 24,
        probe_state: "available",
      },
      runtimeProbes: { cuda: { state: "mismatch", detail: "driver/runtime mismatch" } },
    }),
    expected: "api_preferred",
  },
  {
    name: "AMD GPU does not satisfy CUDA requirement",
    model: cudaWithApi,
    profile: machine({
      info: { gpu_name: "AMD Radeon RX 7900 XTX" },
      gpu: {
        available: true,
        gpu_name: "AMD Radeon RX 7900 XTX",
        dedicated_memory_gb: 24,
        probe_state: "available",
      },
    }),
    expected: "api_preferred",
  },
  {
    name: "AMD GPU cannot satisfy CUDA even when a stale probe says available",
    model: cudaWithApi,
    profile: machine({
      info: { gpu_name: "AMD Radeon RX 7900 XTX" },
      gpu: {
        available: true,
        gpu_name: "AMD Radeon RX 7900 XTX",
        dedicated_memory_gb: 24,
        probe_state: "available",
      },
      runtimeProbes: { cuda: { state: "available", version: "12.4" } },
    }),
    expected: "api_preferred",
  },
  {
    name: "installed model with missing runtime is not runnable",
    model: cpuLocal,
    profile: machine({
      installedModels: [{ id: cpuLocal.id, state: "installed", path_exists: true }],
      runtimeProbes: { python: { state: "missing" } },
    }),
    expected: "runtime_mismatch",
  },
  {
    name: "verified install outranks an unknown generic runtime probe",
    model: cpuLocal,
    profile: machine({
      installedModels: [{ id: cpuLocal.id, state: "installed", path_exists: true }],
      runtimeProbes: { python: { state: "unknown" } },
    }),
    expected: "installed",
    warning: "最小推理证据",
  },
  {
    name: "runtime mismatch is explicit for local-only model",
    model: cpuLocal,
    profile: machine({ runtimeProbes: { python: { state: "mismatch" } } }),
    expected: "runtime_mismatch",
  },
  {
    name: "preview profile never auto-approves local-only model",
    model: cpuLocal,
    profile: machine({ source: "preview", probeCompleteness: "preview" }),
    expected: "needs_review",
  },
  {
    name: "missing model size metadata never auto-approves download",
    model: { ...cpuLocal, id: "missing-size", estimated_size_gb: undefined },
    profile: machine(),
    expected: "needs_review",
  },
  {
    name: "invalid model metadata selects API fallback when available",
    model: { ...cpuWithApi, id: "invalid-metadata", min_logical_cores: 0 },
    profile: machine(),
    expected: "api_preferred",
  },
  {
    name: "space shortage selects API fallback when available",
    model: { ...hugeLocal, id: "huge-api", api_fallback: true },
    profile: machine(),
    expected: "api_preferred",
  },
  {
    name: "healthy CPU route is recommended",
    model: cpuLocal,
    profile: machine(),
    expected: "recommended",
  },
];

for (const testCase of cases) {
  const recommendation = recommendModel(testCase.model, testCase.profile);
  assert.equal(
    recommendation.recommendation_status,
    testCase.expected,
    `${testCase.name}: ${recommendation.reason}`,
  );
  assert.ok(recommendation.next_action.length > 0, `${testCase.name}: next action missing`);
  assert.ok(recommendation.confidence, `${testCase.name}: confidence missing`);
  if (testCase.warning) {
    assert.ok(
      recommendation.warnings.some((item) => item.includes(testCase.warning)),
      `${testCase.name}: expected warning ${testCase.warning}`,
    );
  }
}

const localSummary = summarizeRecommendations(recommendModels([cpuLocal, cudaWithApi], machine()));
assert.equal(localSummary.state, "local_ready");
assert.equal(localSummary.primary_model_id, cpuLocal.id);

const apiSummary = summarizeRecommendations(
  recommendModels([cpuWithApi, cudaWithApi], machine({ info: { memory_total_gb: 4 } })),
);
assert.equal(apiSummary.state, "api_ready");

const reviewSummary = summarizeRecommendations(
  recommendModels([cpuLocal], machine({ disks: [] })),
);
assert.equal(reviewSummary.state, "needs_input");

const optionalSummary = summarizeRecommendations(
  recommendModels([{ ...cpuLocal, id: "optional-local", local_priority: "optional" }], machine()),
);
assert.equal(optionalSummary.state, "local_ready");
assert.equal(optionalSummary.primary_model_id, "optional-local");

const emptySummary = summarizeRecommendations([]);
assert.equal(emptySummary.state, "needs_input");
assert.equal(emptySummary.primary_model_id, null);

assert.equal(status(cpuLocal, machine()), "recommended");
console.log(`model-recommender tests passed (${cases.length} edge cases)`);

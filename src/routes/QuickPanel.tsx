import {
  AlertTriangle,
  AudioLines,
  ArrowRightLeft,
  Bot,
  ChevronDown,
  CheckCircle2,
  ClipboardList,
  Copy,
  Cpu,
  Database,
  Download,
  Eye,
  FileAudio,
  FileJson,
  FileText,
  FolderOpen,
  Globe,
  HardDrive,
  Image as ImageIcon,
  Layers3,
  Loader2,
  Mic2,
  MonitorCheck,
  PackageCheck,
  Play,
  Plus,
  Search,
  Save,
  Send,
  Settings2,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Square,
  Terminal,
  Trash2,
  UploadCloud,
  Upload,
  Video,
  WandSparkles,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { HandoffStatus, useAssistantHandoff } from "@/components/workflow-assistant-entry";
import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { startPolling } from "@/lib/poll";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  asrApi,
  distributionApi,
  openrouterApi,
  modelCenterApi,
  secretsApi,
  systemApi,
  type AsrArtifactContent,
  type AsrArtifactSummary,
  type AsrEnvironmentReport,
  type AsrJobEvent,
  type AsrJobRecord,
  type AsrTaskView,
  type DistributionInfo,
  type ModelDownloadEvent,
  type ModelDownloadJob,
  type OpenRouterModelOption,
} from "@/lib/tauri-api";
import {
  deriveMachineProfile,
  recommendModels,
  summarizeRecommendations,
  type MachineProfile,
  type ModelLocalPriority,
  type ModelRecommendation,
  type ModelRecommendationStatus,
  type RecommenderDiskInput,
} from "@/lib/model-recommender";
import {
  ASR_PORTABLE_ENGINE_IDS,
  createDefaultAsrPortableConfig,
  exportAsrPortableBundle,
  importAsrPortableBundle,
  validateAsrPortableConfig,
  type AsrPortableConfig,
  type AsrPortableCustomPreset,
  type AsrPortableRoots,
} from "@/lib/asr-portability";
import { cn, formatLocalDateTime } from "@/lib/utils";

interface ToolItem {
  title: string;
  description: string;
  status: "ready" | "configured" | "soon";
  params: string[];
}

interface ToolGroup {
  id: string;
  title: string;
  description: string;
  icon: React.ReactNode;
  defaultOpen?: boolean;
  tools: ToolItem[];
}

interface AsrPreset {
  id: string;
  name: string;
  trait: string;
  detail: string;
  engines: string[];
  engineIds: string[];
  track: "local" | "cloud" | "hybrid";
  speed: string;
  outputPolicy: string;
  bestFor: string;
  status: "installed" | "candidate" | "api" | "future";
}

type AsrPresetRuntimeState =
  | "ready"
  | "environment_check"
  | "needs_setup"
  | "api_check"
  | "conditional"
  | "unavailable";

interface AsrScene {
  id: string;
  label: string;
  hint: string;
}

interface AsrEngineOption {
  id: (typeof ASR_PORTABLE_ENGINE_IDS)[number];
  label: string;
  trait: string;
  track: "local" | "cloud";
}

interface AsrArtifact {
  label: string;
  path: string;
  description: string;
}

interface ModelDownloadEntry {
  id: string;
  name: string;
  category: "语音" | "OCR" | "文档";
  trait: string;
  status: "ready" | "candidate" | "heavy" | "api";
  hardware_fit: string;
  estimated_size_gb: number;
  min_ram_gb: number;
  min_logical_cores: number;
  recommended_vram_gb: number;
  runtime_requirements: string[];
  local_priority: ModelLocalPriority;
  api_fallback: boolean;
  source: string;
  sourceUrl: string;
  hardwareFit: string;
  sizeNote: string;
  strengths: string[];
  weaknesses: string[];
  probeCommand: string;
  downloadCommand: string;
  invocation: string;
}

interface WorkspaceLoadedFile {
  label: string;
  relPath: string;
  absolutePath: string;
  size: number;
  content: string;
  category: string;
  engineId: string | null;
}

const toolGroups: ToolGroup[] = [
  {
    id: "audio",
    title: "音频",
    description: "转写、纪要、说话人、语音生成",
    icon: <AudioLines className="h-4 w-4" aria-hidden />,
    defaultOpen: true,
    tools: [
      {
        title: "语音转文字",
        description: "本地优先，也可切换云端高质量模式。",
        status: "configured",
        params: ["模式", "语言", "输出格式"],
      },
      {
        title: "会议纪要",
        description: "从录音或逐字稿生成摘要、行动项和决策。",
        status: "configured",
        params: ["纪要模板", "详细程度"],
      },
      {
        title: "语音生成",
        description: "把文本生成朗读音频，走云端 API。",
        status: "soon",
        params: ["声音", "语速", "格式"],
      },
    ],
  },
  {
    id: "image",
    title: "图片",
    description: "识别、OCR、生成、编辑",
    icon: <ImageIcon className="h-4 w-4" aria-hidden />,
    defaultOpen: true,
    tools: [
      {
        title: "图片识别",
        description: "分析图片内容、界面状态和错误截图。",
        status: "soon",
        params: ["分析类型", "输出格式"],
      },
      {
        title: "OCR 提取",
        description: "提取截图、票据、资料图中的文字。",
        status: "soon",
        params: ["语言", "保留版式"],
      },
      {
        title: "图片生成",
        description: "用于文章配图、概念图和素材草稿。",
        status: "soon",
        params: ["风格", "比例", "数量"],
      },
    ],
  },
  {
    id: "document",
    title: "文档",
    description: "PDF、Word、Markdown、网页资料",
    icon: <FileText className="h-4 w-4" aria-hidden />,
    defaultOpen: true,
    tools: [
      {
        title: "文档摘要",
        description: "提炼结构、结论、风险和待办。",
        status: "soon",
        params: ["摘要长度", "输出语言"],
      },
      {
        title: "表格抽取",
        description: "从 PDF/Word/网页中抽取表格和结构化数据。",
        status: "soon",
        params: ["字段", "JSON/Excel"],
      },
      {
        title: "文档对比",
        description: "比较两个版本的差异、风险和需要确认的问题。",
        status: "soon",
        params: ["对比粒度", "报告格式"],
      },
    ],
  },
  {
    id: "video",
    title: "视频",
    description: "视频理解、字幕、片段摘要",
    icon: <Video className="h-4 w-4" aria-hidden />,
    tools: [
      {
        title: "视频摘要",
        description: "上传视频或片段，提取时间线和重点。",
        status: "soon",
        params: ["时间线", "关键帧"],
      },
      {
        title: "视频生成",
        description: "走 API 生成短视频素材。",
        status: "soon",
        params: ["脚本", "比例", "时长"],
      },
    ],
  },
  {
    id: "web",
    title: "联网",
    description: "搜索、抓取、调研、来源整理",
    icon: <Globe className="h-4 w-4" aria-hidden />,
    tools: [
      {
        title: "联网调研",
        description: "搜索并整理带来源的调研结论。",
        status: "soon",
        params: ["来源数量", "时间范围"],
      },
      {
        title: "网页整理",
        description: "抓取网页内容并转成 Markdown 或摘要。",
        status: "soon",
        params: ["保留图片", "输出位置"],
      },
    ],
  },
  {
    id: "homebase",
    title: "HomeBase",
    description: "代理、历史、文章、Wiki、系统",
    icon: <Bot className="h-4 w-4" aria-hidden />,
    tools: [
      {
        title: "查看代理状态",
        description: "检查本机 AI 代理、模型、MCP 和 Skills。",
        status: "ready",
        params: ["代理", "状态范围"],
      },
      {
        title: "历史对话整理",
        description: "读取 Agent 会话并筛出值得沉淀的内容。",
        status: "ready",
        params: ["Agent", "日期范围"],
      },
      {
        title: "文章生成",
        description: "调用专职 Article Agent 准备个人网站草稿。",
        status: "ready",
        params: ["主题", "风格", "发布目标"],
      },
      {
        title: "Wiki 投料",
        description: "把结果保存到 C:\\HomeBase\\Wiki 并交给 Wiki Agent 整理。",
        status: "ready",
        params: ["目录", "处理方式"],
      },
    ],
  },
];

const promptChips = ["整理最近会话", "生成会议纪要", "准备文章", "检查本机能力"];

const asrPresets: AsrPreset[] = [
  {
    id: "asr_local_sprint",
    name: "极速本地",
    trait: "隐私快跑",
    detail: "零云端成本，适合隐私录音和日常粗转写。",
    engines: ["SenseVoice/FunASR", "faster-whisper large-v3"],
    engineIds: ["sensevoice_small", "faster_whisper_large_v3_cpu"],
    track: "local",
    speed: "本机约 1.6x-8.7x realtime",
    outputPolicy: "best_only",
    bestFor: "隐私优先、先拿粗稿、没有云端额度时。",
    status: "candidate",
  },
  {
    id: "asr_zh_balanced",
    name: "中文均衡",
    trait: "中英日韩",
    detail: "加入 SenseVoice/FunASR 后，与 Whisper 互相校验中文会议术语。",
    engines: ["faster-whisper large-v3", "SenseVoice/FunASR"],
    engineIds: ["faster_whisper_large_v3_cpu", "sensevoice_small"],
    track: "local",
    speed: "本地双引擎，速度取决于音频长度",
    outputPolicy: "keep_all_and_merge",
    bestFor: "中文会议、课堂、术语多但不想上云的录音。",
    status: "candidate",
  },
  {
    id: "asr_accuracy_max",
    name: "准确优先",
    trait: "本地+云端",
    detail: "本地最佳和云端最佳并行，保留全部结果并自动融合。",
    engines: ["faster-whisper large-v3", "Deepgram Nova-3", "ElevenLabs Scribe", "AssemblyAI Universal"],
    engineIds: [
      "faster_whisper_large_v3_cpu",
      "deepgram_nova_3",
      "elevenlabs_scribe",
      "assemblyai_universal",
    ],
    track: "hybrid",
    speed: "云端优先返回，本地底稿较慢",
    outputPolicy: "keep_all_compare_and_merge",
    bestFor: "正式会议、重要录音、需要最终可读稿。",
    status: "api",
  },
  {
    id: "asr_speaker_meeting",
    name: "会议说话人",
    trait: "谁说了什么",
    detail: "优先 diarization，输出 speaker turns、逐字稿和会议纪要。",
    engines: ["Deepgram diarization", "AssemblyAI speaker labels", "OpenAI diarize"],
    engineIds: ["deepgram_nova_3", "assemblyai_universal", "openai_gpt_4o_transcribe_diarize"],
    track: "cloud",
    speed: "云端快速，视额度和文件大小",
    outputPolicy: "speaker_turns_and_summary",
    bestFor: "多人会议、交接讨论、需要分清责任与发言人。",
    status: "api",
  },
  {
    id: "asr_realtime_caption",
    name: "实时字幕",
    trait: "低延迟",
    detail: "面向会议旁路字幕和直播场景，优先速度和稳定流式输出。",
    engines: ["Deepgram Nova-3 streaming", "FunASR streaming"],
    engineIds: ["deepgram_nova_3", "sensevoice_small"],
    track: "hybrid",
    speed: "低延迟流式",
    outputPolicy: "streaming_partial_and_final",
    bestFor: "在线会议旁路字幕、边听边记。",
    status: "candidate",
  },
  {
    id: "asr_full_audit",
    name: "全量评测",
    trait: "全部跑一遍",
    detail: "运行所有已启用方案，生成差异报告和融合稿。",
    engines: ["faster-whisper", "SenseVoice", "Deepgram", "AssemblyAI", "ElevenLabs", "OpenAI"],
    engineIds: [
      "faster_whisper_large_v3_cpu",
      "sensevoice_small",
      "deepgram_nova_3",
      "assemblyai_universal",
      "elevenlabs_scribe",
      "openai_gpt_4o_transcribe_diarize",
    ],
    track: "hybrid",
    speed: "最慢但保留最多证据",
    outputPolicy: "keep_all_compare_and_merge",
    bestFor: "首次评估新录音类型，或需要人工比对所有输出。",
    status: "future",
  },
];

const asrEngineOptions: AsrEngineOption[] = [
  {
    id: "faster_whisper_large_v3_cpu",
    label: "faster-whisper large-v3",
    trait: "本地稳态底稿，CPU 可运行",
    track: "local",
  },
  {
    id: "sensevoice_small",
    label: "SenseVoice Small",
    trait: "本地中文与近场语音快跑",
    track: "local",
  },
  {
    id: "deepgram_nova_3",
    label: "Deepgram Nova-3",
    trait: "云端低延迟、多人场景",
    track: "cloud",
  },
  {
    id: "assemblyai_universal",
    label: "AssemblyAI Universal",
    trait: "云端说话人和会议结构",
    track: "cloud",
  },
  {
    id: "elevenlabs_scribe",
    label: "ElevenLabs Scribe",
    trait: "云端多语言与复杂音频复核",
    track: "cloud",
  },
  {
    id: "openai_gpt_4o_transcribe_diarize",
    label: "OpenAI 转写 + 说话人",
    trait: "云端说话人分离专项",
    track: "cloud",
  },
];

const asrScenes: AsrScene[] = [
  { id: "auto", label: "自动判断", hint: "先做音频画像，再选择预处理方案。" },
  { id: "daily_note", label: "日常记录", hint: "单人近场，少降噪，尽量保留原音。" },
  { id: "meeting_noisy", label: "嘈杂会议", hint: "响度标准化、轻量降噪、多人安全分段。" },
  { id: "reverb_room", label: "回音房间", hint: "保守处理回音，使用更长上下文。" },
  { id: "low_volume", label: "音量偏低", hint: "先增益标准化，再做 VAD。" },
];

const asrOutputArtifacts: AsrArtifact[] = [
  {
    label: "原貌还原稿",
    path: "final/original-scene-reconstruction.md",
    description: "按时间和说话人轮次保留口语现场。",
  },
  {
    label: "清洁逐字稿",
    path: "final/transcript.md",
    description: "更适合阅读和后续整理的主稿。",
  },
  {
    label: "会议纪要",
    path: "final/meeting-summary.md",
    description: "摘要、决策、待办和风险点。",
  },
  {
    label: "运行矩阵",
    path: "compare/run-matrix.json",
    description: "每个引擎的成功状态、耗时和输出路径。",
  },
];

const asrPipeline = [
  {
    title: "输入留存",
    detail: "原始录音进入 source，不覆盖、不改名、不写 C 盘。",
    icon: FileAudio,
  },
  {
    title: "自适应预处理",
    detail: "生成 audio-profile、normalized wav、VAD 和分段信息。",
    icon: SlidersHorizontal,
  },
  {
    title: "多引擎运行",
    detail: "按预设运行本地/云端引擎，逐路保存 raw 和 transcript。",
    icon: Layers3,
  },
  {
    title: "对比与定稿",
    detail: "保留差异、融合稿、说话人视图和最终导出文件。",
    icon: CheckCircle2,
  },
];

const modelManagerPython = "C:\\HomeBase\\Tools\\Python313\\python.exe";
const modelManagerScript = "C:\\HomeBase\\AIHub\\tools\\model-manager\\model_manager.py";
const installedModelsManifest = "C:\\HomeBase\\AIHub\\models\\installed-models.json";
const asrPortableStorageKey = "homebase.asr.portable-profile.v1";
const asrPortableConfigEvent = "homebase:asr-portable-config";
const asrOpenTaskEvent = "homebase:asr-open-task";
const customDraftPresetId = "custom_draft";
const asrProviderSecretByEngine: Record<string, string> = {
  openai_gpt_4o_transcribe_diarize: "openai_key",
  deepgram_nova_3: "deepgram_key",
  assemblyai_universal: "assemblyai_key",
  elevenlabs_scribe: "elevenlabs_key",
};
const automatedModelDownloadIds = new Set([
  "faster-whisper-large-v3",
  "sensevoice-small",
  "whisperx-pyannote-diarization",
  "paddleocr-vl-0.9b",
]);

const builtInModels: ModelDownloadEntry[] = [
  {
    id: "faster-whisper-large-v3",
    name: "faster-whisper large-v3",
    category: "语音",
    trait: "稳态底稿",
    status: "ready",
    hardware_fit: "cpu_ok_gpu_optional",
    estimated_size_gb: 4,
    min_ram_gb: 8,
    min_logical_cores: 8,
    recommended_vram_gb: 0,
    runtime_requirements: ["python"],
    local_priority: "recommended",
    api_fallback: true,
    source: "Hugging Face · Systran",
    sourceUrl: "https://huggingface.co/Systran/faster-whisper-large-v3",
    hardwareFit: "CPU INT8 可跑，GPU 可选；适合作为隐私转写基线。",
    sizeNote: "大模型快照，完整下载前先确认 F 盘空间。",
    strengths: ["中文/混合语音稳定", "长音频和时间戳生态成熟", "本地隐私链路的主基线"],
    weaknesses: ["CPU 长录音会慢", "不自带说话人分离"],
    probeCommand: `"${modelManagerPython}" "${modelManagerScript}" probe faster-whisper-large-v3`,
    downloadCommand: `"${modelManagerPython}" "${modelManagerScript}" download faster-whisper-large-v3 --allow-large`,
    invocation:
      "from faster_whisper import WhisperModel\nmodel = WhisperModel(local_path, device=\"cpu\", compute_type=\"int8\")\nsegments, info = model.transcribe(audio_path, language=\"zh\", vad_filter=True)",
  },
  {
    id: "sensevoice-small",
    name: "SenseVoiceSmall / FunASR",
    category: "语音",
    trait: "中文快跑",
    status: "ready",
    hardware_fit: "cpu_ok",
    estimated_size_gb: 2,
    min_ram_gb: 8,
    min_logical_cores: 4,
    recommended_vram_gb: 0,
    runtime_requirements: ["python"],
    local_priority: "recommended",
    api_fallback: false,
    source: "Hugging Face · FunAudioLLM",
    sourceUrl: "https://huggingface.co/FunAudioLLM/SenseVoiceSmall",
    hardwareFit: "CPU 友好；适合做第一路快速本地草稿。",
    sizeNote: "小型 ASR 模型，建议放 C:\\HomeBase\\Models 或 C:\\HomeBase\\Runtimes。",
    strengths: ["中文/CJK 表现好", "速度快", "可扩展语言/情绪/事件标签路线"],
    weaknesses: ["重要会议仍需 Whisper 或云端复核", "依赖版本要隔离"],
    probeCommand: `"${modelManagerPython}" "${modelManagerScript}" probe sensevoice-small`,
    downloadCommand: `"${modelManagerPython}" "${modelManagerScript}" download sensevoice-small --allow-large`,
    invocation:
      "from funasr import AutoModel\nmodel = AutoModel(model=local_path, trust_remote_code=True, device=\"cpu\")\nresult = model.generate(input=audio_path, language=\"zh\", use_itn=True)",
  },
  {
    id: "whisperx-pyannote-diarization",
    name: "WhisperX + pyannote",
    category: "语音",
    trait: "说话人审计",
    status: "heavy",
    hardware_fit: "gpu_rich_preferred",
    estimated_size_gb: 8,
    min_ram_gb: 16,
    min_logical_cores: 8,
    recommended_vram_gb: 8,
    runtime_requirements: ["python", "cuda"],
    local_priority: "deferred",
    api_fallback: true,
    source: "Hugging Face · pyannote",
    sourceUrl: "https://huggingface.co/pyannote/speaker-diarization-3.1",
    hardwareFit: "更适合 GPU/内存冗余机器；用于多人说话人审计。",
    sizeNote: "需要 WhisperX、alignment 和 pyannote 多组资产，可能需要 HF token。",
    strengths: ["说话人轮次", "词级时间戳", "适合多人会议复核"],
    weaknesses: ["本机默认不够宽裕", "授权和 token 流程更复杂"],
    probeCommand: `"${modelManagerPython}" "${modelManagerScript}" probe whisperx-pyannote-diarization`,
    downloadCommand: `"${modelManagerPython}" "${modelManagerScript}" download whisperx-pyannote-diarization --allow-large`,
    invocation: "先跑 WhisperX 转写/对齐，再用 pyannote diarization 生成 compare/speaker-turns.json。",
  },
  {
    id: "paddleocr-pp-ocrv6",
    name: "PaddleOCR / PP-OCRv6",
    category: "OCR",
    trait: "图片文字",
    status: "candidate",
    hardware_fit: "cpu_ok_gpu_optional",
    estimated_size_gb: 2,
    min_ram_gb: 8,
    min_logical_cores: 4,
    recommended_vram_gb: 0,
    runtime_requirements: ["python"],
    local_priority: "recommended",
    api_fallback: true,
    source: "GitHub · PaddlePaddle",
    sourceUrl: "https://github.com/PaddlePaddle/PaddleOCR",
    hardwareFit: "CPU 可用，GPU 可选；适合作为默认 OCR 路线。",
    sizeNote: "模型由 PaddleOCR 安装器或镜像下载，权重仍放 F 盘。",
    strengths: ["中文 OCR 实用", "截图/票据/扫描件快", "适合做文档解析前置层"],
    weaknesses: ["不负责完整 PDF 版式理解", "表格/公式需交给文档解析或 VLM"],
    probeCommand: `"${modelManagerPython}" "${modelManagerScript}" plan paddleocr-pp-ocrv6`,
    downloadCommand: "按 catalog 调用 PaddleOCR 内置下载；权重根目录指向 C:\\HomeBase\\Models\\ocr\\paddleocr",
    invocation: "from paddleocr import PaddleOCR\nocr = PaddleOCR(lang=\"ch\")\nresult = ocr.ocr(image_path)",
  },
  {
    id: "paddleocr-vl-0.9b",
    name: "PaddleOCR-VL 0.9B",
    category: "文档",
    trait: "复杂版式",
    status: "api",
    hardware_fit: "gpu_rich_or_api",
    estimated_size_gb: 8,
    min_ram_gb: 16,
    min_logical_cores: 8,
    recommended_vram_gb: 8,
    runtime_requirements: ["python", "cuda"],
    local_priority: "deferred",
    api_fallback: true,
    source: "Hugging Face · PaddlePaddle",
    sourceUrl: "https://huggingface.co/PaddlePaddle/PaddleOCR-VL",
    hardwareFit: "更适合 API 或更大显存；复杂版式增强路线。",
    sizeNote: "VLM 类模型，完整下载前先评估显存和存储。",
    strengths: ["表格/公式/图表", "复杂页面元素理解", "OCR 难例增强"],
    weaknesses: ["本机 4GB VRAM 不宽裕", "需要单独 benchmark 才能转默认"],
    probeCommand: `"${modelManagerPython}" "${modelManagerScript}" probe paddleocr-vl-0.9b`,
    downloadCommand: `"${modelManagerPython}" "${modelManagerScript}" download paddleocr-vl-0.9b --allow-large`,
    invocation: "按模型卡 Transformers 示例加载；只在 OCR+解析器不够时路由到该 lane。",
  },
  {
    id: "docling",
    name: "Docling",
    category: "文档",
    trait: "通用解析",
    status: "candidate",
    hardware_fit: "cpu_ok",
    estimated_size_gb: 2,
    min_ram_gb: 8,
    min_logical_cores: 4,
    recommended_vram_gb: 0,
    runtime_requirements: ["python"],
    local_priority: "recommended",
    api_fallback: false,
    source: "GitHub · docling-project",
    sourceUrl: "https://github.com/docling-project/docling",
    hardwareFit: "CPU 友好；适合作为 Office/PDF/HTML/EPUB 的第一文档解析候选。",
    sizeNote: "主要是运行时安装，不是大模型快照。",
    strengths: ["多格式文档", "结构化导出", "适合作为昂贵 VLM 前置"],
    weaknesses: ["扫描件质量依赖 OCR 后端", "中文复杂版式要实测"],
    probeCommand: `"${modelManagerPython}" "${modelManagerScript}" plan docling`,
    downloadCommand: "在 C:\\HomeBase\\Runtimes\\document\\docling 建独立运行时后安装。",
    invocation: "用 Docling CLI/Python API 导出 Markdown/JSON，OCR 重的页面转 PaddleOCR lane。",
  },
  {
    id: "mineru",
    name: "MinerU",
    category: "文档",
    trait: "PDF 深解析",
    status: "candidate",
    hardware_fit: "cpu_ok_gpu_optional",
    estimated_size_gb: 8,
    min_ram_gb: 16,
    min_logical_cores: 8,
    recommended_vram_gb: 6,
    runtime_requirements: ["python"],
    local_priority: "optional",
    api_fallback: true,
    source: "GitHub · opendatalab",
    sourceUrl: "https://github.com/opendatalab/MinerU",
    hardwareFit: "CPU 可用、GPU 可选；先作为重解析候选，不默认全量跑。",
    sizeNote: "依赖较重，运行时放 C:\\HomeBase\\Runtimes\\document\\mineru。",
    strengths: ["PDF/图片/Office 到 Markdown/JSON", "科研/结构化材料", "适合内部路由组合"],
    weaknesses: ["依赖重", "需用真实样本 benchmark"],
    probeCommand: `"${modelManagerPython}" "${modelManagerScript}" plan mineru`,
    downloadCommand: "在 C:\\HomeBase\\Runtimes\\document\\mineru 建独立运行时后安装。",
    invocation: "用 MinerU CLI/service 输出 Markdown/JSON 到 C:\\HomeBase\\Wiki\\raw\\documents\\<date>_<task>。",
  },
  {
    id: "marker",
    name: "Marker",
    category: "文档",
    trait: "Markdown 清洁",
    status: "candidate",
    hardware_fit: "cpu_ok_gpu_optional",
    estimated_size_gb: 4,
    min_ram_gb: 8,
    min_logical_cores: 4,
    recommended_vram_gb: 0,
    runtime_requirements: ["python"],
    local_priority: "optional",
    api_fallback: true,
    source: "GitHub · datalab-to",
    sourceUrl: "https://github.com/datalab-to/marker",
    hardwareFit: "CPU/GPU 可选；适合与 Docling/MinerU 做同样样本对比。",
    sizeNote: "运行时和可选缓存放 F 盘；LLM 清理走 API。",
    strengths: ["Markdown/JSON/HTML", "表格/公式/图片抽取", "可选 LLM 清理"],
    weaknesses: ["LLM 模式会增加成本", "需和 MinerU/Docling 同样本对比"],
    probeCommand: `"${modelManagerPython}" "${modelManagerScript}" plan marker`,
    downloadCommand: "在 C:\\HomeBase\\Runtimes\\document\\marker 建独立运行时后安装。",
    invocation: "用 Marker 转 Markdown/JSON/HTML；准确优先时再开启 LLM cleanup。",
  },
];

export function QuickPanel() {
  const [prompt, setPrompt] = useState("");
  const handoff = useAssistantHandoff(() => prompt);
  return (
    <div className="animate-fade-in space-y-8">
      <header className="hb-page-header">
        <span className="hb-section-label">HomeBase Agent</span>
        <h1>快捷面板</h1>
        <p>直接告诉它要做什么，或从下面的分类面板快速选择能力。</p>
      </header>

      <section aria-label="对话框">
        <div className="rounded-2xl border border-border bg-card p-4 shadow-sm">
          <div className="mb-3 flex items-center gap-2 text-sm text-muted-foreground">
            <Sparkles className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
            <span>HomeBase 内置 AI 助理</span>
          </div>
          <div className="flex items-center gap-2 rounded-xl border border-border bg-background px-3 py-3">
            <Search className="h-4 w-4 shrink-0 text-muted-foreground" aria-hidden />
            <input
              aria-label="向 HomeBase Agent 输入任务"
              className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
              placeholder="写下需求，先保存到助理草稿，再确认发送"
              value={prompt}
              onChange={(event) => setPrompt(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) {
                  event.preventDefault();
                  if (prompt.trim() && !handoff.busy) void handoff.prepare();
                }
              }}
            />
            <button type="button" disabled={!prompt.trim() || handoff.busy} onClick={() => void handoff.prepare()} className="rounded-lg bg-primary-600 p-2 text-primary-foreground disabled:opacity-50">
              {handoff.busy ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> : <Send className="h-4 w-4" aria-hidden />}
              <span className="sr-only">交给助理</span>
            </button>
          </div>
          <div className="mt-3 flex flex-wrap gap-2">
            {promptChips.map((chip) => (
              <button key={chip} type="button" className="hb-tag" onClick={() => setPrompt(chip)}>
                {chip}
              </button>
            ))}
          </div>
          <p className="mt-3 text-xs text-muted-foreground">保存为独立草稿后打开助理；由你确认发送。</p>
          <HandoffStatus handoff={handoff} />
        </div>
      </section>

      <OpenRouterModelPicker />

      <ModelDownloadCenter />

      <AsrWorkbench />

      <ResultWorkspace />

      <section aria-label="能力分类">
        <div className="mb-4 flex items-center justify-between gap-4">
          <div>
            <h2 className="font-serif text-lg font-semibold">能力分类</h2>
            <p className="mt-1 text-sm text-muted-foreground">
              展开分类后选择功能，进入后再填写内容和参数。
            </p>
          </div>
          <span className="hidden rounded-full border border-border bg-card px-3 py-1 text-xs text-muted-foreground sm:inline-flex">
            轻量模式
          </span>
        </div>
        <div className="space-y-3">
          {toolGroups.map((group) => (
            <ToolGroupPanel key={group.id} group={group} />
          ))}
        </div>
      </section>
    </div>
  );
}

function OpenRouterModelPicker() {
  const [selectedModelId, setSelectedModelId] = useState("");
  const catalog = useQuery({
    queryKey: ["openrouter", "modelsCatalog"],
    queryFn: () => openrouterApi.modelsCatalog(),
    staleTime: 60 * 60 * 1000,
  });

  const allModels = useMemo(() => {
    return catalog.data?.groups.flatMap((group) => group.models) ?? [];
  }, [catalog.data]);
  const selectedModel =
    allModels.find((model) => model.id === selectedModelId) ?? allModels[0] ?? null;

  return (
    <section aria-label="API 模型选择">
      <details
        className="group rounded-xl border border-border bg-card transition-colors open:border-primary-300/70 dark:open:border-primary-700/70"
        open
      >
        <summary className="flex cursor-pointer list-none items-center gap-3 px-4 py-3">
          <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
            <Sparkles className="h-4 w-4" aria-hidden />
          </span>
          <div className="min-w-0 flex-1">
            <h2 className="text-sm font-semibold">API 模型</h2>
            <p className="truncate text-xs text-muted-foreground">
              OpenRouter 自动拉取模型列表，按品牌分组，并展示价格。
            </p>
          </div>
          <ChevronDown
            className="h-4 w-4 text-muted-foreground transition-transform group-open:rotate-180"
            aria-hidden
          />
        </summary>
        <div className="border-t border-border p-4">
          {catalog.isError ? (
            <div className="rounded-lg border border-destructive/30 bg-destructive/10 p-4 text-sm text-destructive-strong">
              模型列表读取失败：{catalog.error instanceof Error ? catalog.error.message : "未知错误"}
            </div>
          ) : (
            <div className="grid grid-cols-1 gap-4 lg:grid-cols-[1fr_20rem]">
              <div className="space-y-3">
                <label className="block text-xs font-medium uppercase tracking-widest text-muted-foreground">
                  选择模型
                </label>
                <select
                  value={selectedModel?.id ?? ""}
                  onChange={(event) => setSelectedModelId(event.target.value)}
                  disabled={catalog.isLoading || allModels.length === 0}
                  className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm outline-none transition-colors focus:border-primary-500"
                >
                  {catalog.isLoading ? (
                    <option>正在从 OpenRouter 拉取模型...</option>
                  ) : (
                    catalog.data?.groups.map((group) => (
                      <optgroup key={group.provider} label={`${group.label} (${group.models.length})`}>
                        {group.models.map((model) => (
                          <option key={model.id} value={model.id}>
                            {model.name} · {model.price_label}
                          </option>
                        ))}
                      </optgroup>
                    ))
                  )}
                </select>
                <div className="flex flex-wrap items-center gap-2">
                  <button
                    type="button"
                    onClick={() => catalog.refetch()}
                    className="hb-btn-outline py-2"
                    disabled={catalog.isFetching}
                  >
                    <Settings2 className="h-4 w-4" aria-hidden />
                    {catalog.isFetching ? "刷新中" : "刷新列表"}
                  </button>
                  <span className="text-xs text-muted-foreground">
                    {catalog.data
                      ? `共 ${allModels.length} 个模型 · ${formatLocalDateTime(catalog.data.fetched_at)}`
                      : "列表来自 OpenRouter /models API"}
                  </span>
                </div>
              </div>
              <ModelDetail model={selectedModel} />
            </div>
          )}
        </div>
      </details>
    </section>
  );
}

function ModelDetail({ model }: { model: OpenRouterModelOption | null }) {
  if (!model) {
    return (
      <aside className="rounded-lg border border-border bg-background p-4 text-sm text-muted-foreground">
        请选择模型。
      </aside>
    );
  }
  return (
    <aside className="rounded-lg border border-border bg-background p-4">
      <div className="mb-3">
        <h3 className="truncate text-sm font-semibold">{model.name}</h3>
        <p className="mt-1 truncate font-mono text-xs text-muted-foreground">{model.id}</p>
      </div>
      <dl className="space-y-2 text-xs">
        <DetailRow label="价格" value={model.price_label} />
        <DetailRow
          label="上下文"
          value={model.context_length ? `${model.context_length.toLocaleString()} tokens` : "n/a"}
        />
        <DetailRow label="输入" value={model.input_modalities.join(", ") || "n/a"} />
        <DetailRow label="输出" value={model.output_modalities.join(", ") || "n/a"} />
        <DetailRow
          label="能力"
          value={
            model.supported_parameters
              .filter((param) => ["tools", "response_format", "reasoning"].includes(param))
              .join(", ") || "standard"
          }
        />
      </dl>
    </aside>
  );
}

function ModelDownloadCenter() {
  const queryClient = useQueryClient();
  const categories: Array<ModelDownloadEntry["category"] | "全部"> = ["全部", "语音", "OCR", "文档"];
  const [category, setCategory] = useState<ModelDownloadEntry["category"] | "全部">("全部");
  const [selectedModelId, setSelectedModelId] = useState(builtInModels[0]?.id ?? "");
  const [copied, setCopied] = useState("");
  const [downloadJob, setDownloadJob] = useState<ModelDownloadJob | null>(null);
  const [downloadMessage, setDownloadMessage] = useState("");
  const [downloadRecoveryState, setDownloadRecoveryState] = useState<"checking" | "ready" | "error">("checking");
  const [downloadRecoveryNotice, setDownloadRecoveryNotice] = useState("");
  const [downloadRecoveryCheck, setDownloadRecoveryCheck] = useState(0);
  const [isCancellingDownload, setIsCancellingDownload] = useState(false);
  const activeDownloadRef = useRef<string | null>(null);
  const {
    machineQuery,
    machineProfile,
    portableConfig,
    portableManifestPath,
    distributionInfo,
  } =
    usePortableMachineProfile();
  const modelManagerPath = joinWindowsPath(portableConfig.roots.aihub_root, "tools\\model-manager\\model_manager.py");
  const portableCatalogPath = joinWindowsPath(portableConfig.roots.aihub_root, portableConfig.catalog_rel_path);
  const modelJobsRoot = joinWindowsPath(portableConfig.roots.aihub_root, "models\\_jobs");
  const recommendationMachineProfile = useMemo(
    () => applyConfiguredModelRoot(machineProfile, portableConfig.roots.model_root),
    [machineProfile, portableConfig.roots.model_root],
  );
  const recommendations = useMemo(
    () => recommendModels(builtInModels, recommendationMachineProfile),
    [recommendationMachineProfile],
  );
  const recommendationById = useMemo(
    () => new Map(recommendations.map((item) => [item.id, item])),
    [recommendations],
  );
  const recommendationSummary = useMemo(
    () => summarizeRecommendations(recommendations),
    [recommendations],
  );
  const visibleModels = useMemo(
    () => {
      const filtered = category === "全部" ? builtInModels : builtInModels.filter((model) => model.category === category);
      return [...filtered].sort((left, right) => {
        const leftStatus = recommendationById.get(left.id)?.recommendation_status ?? "needs_review";
        const rightStatus = recommendationById.get(right.id)?.recommendation_status ?? "needs_review";
        return recommendationStatusRank(leftStatus) - recommendationStatusRank(rightStatus);
      });
    },
    [category, recommendationById],
  );
  const selectedModel =
    visibleModels.find((model) => model.id === selectedModelId) ?? visibleModels[0] ?? builtInModels[0];
  const selectedRecommendation = selectedModel ? recommendationById.get(selectedModel.id) : null;
  const verifiedInstallCount = machineProfile.installedModels.filter(
    (item) => item.state === "installed" && item.path_exists !== false,
  ).length;
  const partialInstallCount = machineProfile.installedModels.filter(
    (item) => item.state !== "installed" || item.path_exists === false,
  ).length;
  const downloadUnconfirmed = Boolean(downloadJob && (downloadJob.runtime_state === "unknown" || (["queued", "running"].includes(downloadJob.status) && downloadJob.runtime_state !== "managed")));
  const downloadRecoveryBlocked = downloadRecoveryState !== "ready" || Boolean(downloadRecoveryNotice);
  const downloadIsActive = Boolean(downloadJob && ["queued", "running"].includes(downloadJob.status) && !downloadUnconfirmed);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<ModelDownloadEvent>("model-download", (event) => {
      const payload = event.payload;
      if (payload.jobId !== activeDownloadRef.current) return;
      if (payload.kind === "progress" && payload.payload.event === "model_job") {
        setDownloadJob((current) =>
          current
            ? {
                ...current,
                status: String(payload.payload.status ?? current.status),
                phase: String(payload.payload.phase ?? current.phase),
                progress_percent: Number(payload.payload.progress_percent ?? current.progress_percent),
                updated_at: String(payload.payload.at ?? current.updated_at),
              }
            : current,
        );
      } else if (payload.kind === "diagnostic") {
        setDownloadMessage(payload.message);
      } else if (payload.kind === "finished") {
        activeDownloadRef.current = null;
        setDownloadJob((current) =>
          current
            ? {
                ...current,
                status: payload.status,
                phase: payload.status,
                error: payload.message || current.error,
              }
            : current,
        );
        setDownloadMessage(modelDownloadStatusLabel(payload.status));
        void machineQuery.refetch();
      }
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [machineQuery.refetch]);

  useEffect(() => {
    if (!isTauriRuntime()) { setDownloadRecoveryState("ready"); return; }
    let cancelled = false;
    activeDownloadRef.current = null;
    setDownloadJob(null);
    setDownloadMessage("");
    setDownloadRecoveryState("checking");
    setDownloadRecoveryNotice("");
    void queryClient.fetchQuery({
      queryKey: ["model-center", "reconcile", modelJobsRoot],
      queryFn: () => modelCenterApi.downloadReconcile(modelJobsRoot),
      staleTime: 0,
    })
      .then((jobs) => {
        if (cancelled) return;
        setDownloadRecoveryState("ready");
        const unknown = jobs.filter(job => job.runtime_state === "unmanaged" || job.runtime_state === "unknown" || (!job.runtime_state && ["queued", "running"].includes(job.status)));
        setDownloadRecoveryNotice(unknown.length ? `${unknown.length} 个历史下载任务状态未知，原始记录未修改；请先核对任务和结果。` : "");
        const active = jobs.find((job) => job.runtime_state === "managed");
        const restored = active ?? jobs.find((job) => job.runtime_state === "unmanaged" || job.runtime_state === "unknown" || ["queued", "running"].includes(job.status)) ?? jobs[0];
        if (!restored || activeDownloadRef.current) return;
        activeDownloadRef.current = active?.job_id ?? null;
        setDownloadJob(restored);
        setDownloadMessage(restored.runtime_message ?? "任务状态未知：未确认任务归属，原始记录未修改。");
      })
      .catch(() => { if (!cancelled) { setDownloadRecoveryState("error"); setDownloadRecoveryNotice("恢复检查失败：任务状态未知，原始记录未修改，请重试。"); } });
    return () => {
      cancelled = true;
    };
  }, [modelJobsRoot, queryClient, downloadRecoveryCheck]);

  useEffect(() => {
    if (!downloadJob || !downloadIsActive || !isTauriRuntime()) return;
    let disposed = false;
    const stop = startPolling(async () => {
      await modelCenterApi
        .downloadGet(modelJobsRoot, downloadJob.job_id)
        .then((job) => {
          if (disposed || activeDownloadRef.current !== downloadJob.job_id) return;
          setDownloadJob({ ...job, runtime_state: "managed" });
          if (!["queued", "running"].includes(job.status)) {
            activeDownloadRef.current = null;
            void machineQuery.refetch();
          }
        })
        .catch(() => {
          if (!disposed) setDownloadMessage("下载状态查询失败，任务状态暂时未知；稍后自动重试。");
        });
    }, 1500, false);
    return () => { disposed = true; stop(); };
  }, [downloadIsActive, downloadJob?.job_id, modelJobsRoot, machineQuery.refetch]);

  async function copyCommand(text: string, key: string) {
    await copyToClipboard(text);
    setCopied(key);
    window.setTimeout(() => setCopied(""), 1800);
  }

  async function startModelDownload() {
    if (!selectedModel) return;
    if (!isTauriRuntime() || !automatedModelDownloadIds.has(selectedModel.id)) {
      await copyCommand(selectedModel.downloadCommand, `${selectedModel.id}:download`);
      return;
    }
    setDownloadMessage("");
    try {
      const started = await modelCenterApi.downloadStart({
        modelId: selectedModel.id,
        managerPath: modelManagerPath,
        catalogPath: portableCatalogPath,
        manifestPath: portableManifestPath,
        jobsRoot: modelJobsRoot,
        modelRoot: portableConfig.roots.model_root,
        allowLarge: true,
        allowSystemDrive:
          distributionInfo?.allowSystemDrive || isSystemDrivePath(portableConfig.roots.model_root),
      });
      activeDownloadRef.current = started.jobId;
      const createdAt = new Date().toISOString();
      setDownloadJob({
        schema_version: 1,
        job_id: started.jobId,
        model_id: selectedModel.id,
        model_name: selectedModel.name,
        status: "queued",
        phase: "queued",
        progress_percent: 0,
        files_completed: 0,
        files_total: null,
        bytes_expected: null,
        target_dir: portableConfig.roots.model_root,
        created_at: createdAt,
        updated_at: createdAt,
        revision: 0,
        error: null,
        runtime_state: "managed",
      });
      setDownloadMessage("下载任务已启动；完成后还会执行完整性与最小推理验证。 ");
    } catch (error) {
      setDownloadMessage(error instanceof Error ? error.message : "模型下载启动失败。 ");
    }
  }

  async function cancelModelDownload() {
    if (!downloadJob) return;
    setIsCancellingDownload(true);
    try {
      await modelCenterApi.downloadCancel(downloadJob.job_id);
      activeDownloadRef.current = null;
      setDownloadJob((current) =>
        current ? { ...current, status: "cancelled", phase: "cancelled" } : current,
      );
      setDownloadMessage("下载任务已取消；已经进入 Hugging Face 缓存的文件会在重试时复用。 ");
    } catch (error) {
      setDownloadMessage(error instanceof Error ? error.message : "取消模型下载失败。 ");
    } finally {
      setIsCancellingDownload(false);
    }
  }

  return (
    <section aria-label="模型下载中心">
      <details
        className="group rounded-xl border border-border bg-card transition-colors open:border-primary-300/70 dark:open:border-primary-700/70"
        open
      >
        <summary className="flex cursor-pointer list-none items-center gap-3 px-4 py-3">
          <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
            <Download className="h-4 w-4" aria-hidden />
          </span>
          <div className="min-w-0 flex-1">
            <h2 className="text-sm font-semibold">模型下载中心</h2>
            <p className="truncate text-xs text-muted-foreground">
              内置 ASR、OCR、文档解析候选；按本机硬件和存储策略给出快捷下载与调用方式。
            </p>
          </div>
          <ChevronDown
            className="h-4 w-4 text-muted-foreground transition-transform group-open:rotate-180"
            aria-hidden
          />
        </summary>
        <div className="space-y-5 border-t border-border p-4">
          <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
            <div className="rounded-lg border border-border bg-background p-4">
              <div className="mb-2 flex items-center justify-between gap-3">
                <div className="flex items-center gap-2 text-sm font-semibold">
                  <Cpu className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
                  动态推荐
                </div>
                <span
                  className={cn(
                    "shrink-0 rounded-full border px-2 py-0.5 text-[10px]",
                    machineProfile.source === "live"
                      ? "border-success/30 bg-success/10 text-success-strong"
                      : "border-warning/30 bg-warning/10 text-warning-strong",
                  )}
                >
                  {machineProfile.source === "live" ? "本机探测" : "预览数据"}
                </span>
              </div>
              <p className="text-sm font-medium">{recommendationSummary.title}</p>
              <p className="mt-1 text-xs text-muted-foreground">{recommendationSummary.detail}</p>
              <p className="mt-2 text-[11px] text-muted-foreground">
                {recommendationMachineProfile.cpuLabel} · {recommendationMachineProfile.ramGb ? `${Math.round(recommendationMachineProfile.ramGb)}GB RAM` : "RAM 未知"} ·{" "}
                {machineGpuLabel(recommendationMachineProfile)}
              </p>
              <button
                type="button"
                onClick={() => machineQuery.refetch()}
                className="mt-3 hb-btn-outline py-1.5 text-xs"
                disabled={machineQuery.isFetching}
              >
                <Settings2 className="h-3.5 w-3.5" aria-hidden />
                {machineQuery.isFetching ? "刷新中" : "刷新推荐"}
              </button>
            </div>
            <div className="rounded-lg border border-border bg-background p-4">
              <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
                <HardDrive className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
                存储位置
              </div>
              <p className="break-all font-mono text-[11px] text-muted-foreground">
                {recommendationMachineProfile.preferredModelRoot} · {portableConfig.roots.output_root}
              </p>
              <p className="mt-2 text-xs text-muted-foreground">
                推荐盘剩余 {recommendationMachineProfile.preferredFreeGb != null ? `${Math.round(recommendationMachineProfile.preferredFreeGb)}GB` : "未知"}；
                {recommendationMachineProfile.systemDriveOnly ? "当前使用系统盘，下载前必须保留安全余量。" : "模型权重默认避开系统盘。"}
              </p>
            </div>
            <div className="rounded-lg border border-border bg-background p-4">
              <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
                <Database className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
                下载登记
              </div>
              <p className="break-all font-mono text-[11px] text-muted-foreground">{portableManifestPath}</p>
              <p className="mt-2 text-xs text-muted-foreground">
                {manifestStateLabel(machineProfile.manifestState)} · 已验证 {verifiedInstallCount} · 待完成 {partialInstallCount}
              </p>
            </div>
          </div>

          {recommendationMachineProfile.diagnostics.length > 0 ? (
            <div className="flex items-start gap-2 border-l-2 border-warning/60 bg-warning/5 px-3 py-2 text-xs text-muted-foreground">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-warning-strong" aria-hidden />
              <span>{recommendationMachineProfile.diagnostics.slice(0, 3).join("；")}</span>
            </div>
          ) : null}

          <div className="flex flex-wrap gap-2">
            {categories.map((item) => (
              <button
                key={item}
                type="button"
                onClick={() => setCategory(item)}
                className={cn(
                  "rounded-full border px-3 py-1.5 text-xs transition-colors",
                  category === item
                    ? "border-primary-400 bg-primary-50 text-primary-700 dark:bg-primary-950/30 dark:text-primary-300"
                    : "border-border bg-background text-muted-foreground hover:border-primary-300/70",
                )}
              >
                {item}
              </button>
            ))}
          </div>

          <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,1fr)_24rem]">
            <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
              {visibleModels.map((model) => {
                const recommendation = recommendationById.get(model.id);
                return (
                  <button
                    key={model.id}
                    type="button"
                    onClick={() => setSelectedModelId(model.id)}
                    className={cn(
                      "rounded-lg border bg-background p-4 text-left transition-colors",
                      selectedModel?.id === model.id
                        ? "border-primary-400 ring-1 ring-primary-300/50 dark:border-primary-600"
                        : "border-border hover:border-primary-300/70 dark:hover:border-primary-700/70",
                    )}
                  >
                    <div className="mb-2 flex items-start justify-between gap-3">
                      <div className="min-w-0">
                        <h3 className="text-sm font-semibold">{model.name}</h3>
                        <p className="mt-1 text-xs text-muted-foreground">
                          {model.trait} · {model.category}
                        </p>
                      </div>
                      <ModelRecommendationBadge status={recommendation?.recommendation_status ?? "optional"} />
                    </div>
                    <p className="line-clamp-2 text-xs text-muted-foreground">
                      {recommendation?.reason ?? model.hardwareFit}
                    </p>
                    <div className="mt-3 flex flex-wrap gap-1.5">
                      {model.strengths.slice(0, 3).map((item) => (
                        <span key={item} className="rounded-md bg-muted px-2 py-1 text-[11px] text-muted-foreground">
                          {item}
                        </span>
                      ))}
                    </div>
                  </button>
                );
              })}
            </div>

            {selectedModel ? (
              <aside className="space-y-3 rounded-lg border border-border bg-background p-4">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <h3 className="text-sm font-semibold">{selectedModel.name}</h3>
                    <a
                      href={selectedModel.sourceUrl}
                      target="_blank"
                      rel="noreferrer"
                      className="mt-1 block truncate text-xs text-primary-600 underline-offset-2 hover:underline dark:text-primary-400"
                    >
                      {selectedModel.source}
                    </a>
                  </div>
                  <ModelRecommendationBadge status={selectedRecommendation?.recommendation_status ?? "optional"} />
                </div>
                <dl className="space-y-2 text-xs">
                  <DetailRow label="动态建议" value={selectedRecommendation?.reason ?? selectedModel.hardwareFit} />
                  <DetailRow label="下一步" value={selectedRecommendation?.next_action ?? "先运行环境诊断。"} />
                  <DetailRow
                    label="置信度"
                    value={recommendationConfidenceLabel(selectedRecommendation?.confidence ?? "low")}
                  />
                  <DetailRow label="安装位置" value={selectedRecommendation?.preferred_install_root ?? recommendationMachineProfile.preferredModelRoot} />
                  <DetailRow
                    label="空间"
                    value={
                      selectedRecommendation?.estimated_space_required_gb != null
                        ? `约 ${selectedRecommendation.estimated_space_required_gb}GB · ${selectedModel.sizeNote}`
                        : selectedModel.sizeNote
                    }
                  />
                </dl>
                {selectedRecommendation?.warnings.length ? (
                  <div className="rounded-md bg-warning/10 px-3 py-2 text-xs text-warning-strong">
                    {selectedRecommendation.warnings.join("；")}
                  </div>
                ) : null}
                {selectedRecommendation?.blockers.length ? (
                  <div className="rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
                    阻断项：{selectedRecommendation.blockers.join("；")}
                  </div>
                ) : null}
                {selectedRecommendation?.missing_dependencies.length ? (
                  <div className="rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
                    缺少运行时：{selectedRecommendation.missing_dependencies.join(", ")}
                  </div>
                ) : null}
                {selectedRecommendation?.unknown_dependencies.length &&
                selectedRecommendation.recommendation_status !== "installed" ? (
                  <div className="rounded-md bg-warning/10 px-3 py-2 text-xs text-warning-strong">
                    待确认运行时：{selectedRecommendation.unknown_dependencies.join(", ")}
                  </div>
                ) : null}
                <div>
                  <h4 className="mb-2 text-xs font-medium uppercase tracking-widest text-muted-foreground">优势</h4>
                  <ul className="space-y-1 text-xs text-muted-foreground">
                    {selectedModel.strengths.map((item) => (
                      <li key={item}>· {item}</li>
                    ))}
                  </ul>
                </div>
                <div>
                  <h4 className="mb-2 text-xs font-medium uppercase tracking-widest text-muted-foreground">注意</h4>
                  <ul className="space-y-1 text-xs text-muted-foreground">
                    {selectedModel.weaknesses.map((item) => (
                      <li key={item}>· {item}</li>
                    ))}
                  </ul>
                </div>
                {downloadJob ? (
                  <div className="space-y-2 rounded-md border border-border bg-muted/40 px-3 py-2">
                    <div className="flex items-start justify-between gap-3 text-xs">
                      <div className="min-w-0">
                        <p className="truncate font-medium">{downloadJob.model_name}</p>
                        <p className="mt-0.5 text-[11px] text-muted-foreground">
                          {downloadUnconfirmed ? "任务状态未知（保留原记录）" : modelDownloadPhaseLabel(downloadJob.phase)}
                        </p>
                      </div>
                      <span className="shrink-0 font-mono text-[11px]">
                        {Math.max(0, Math.min(100, Math.round(downloadJob.progress_percent)))}%
                      </span>
                    </div>
                    <div className="h-1.5 overflow-hidden rounded-full bg-border" aria-hidden>
                      <div
                        className={cn(
                          "h-full rounded-full transition-[width]",
                          downloadJob.status === "completed" ? "bg-success" : "bg-primary-500",
                        )}
                        style={{ width: `${Math.max(0, Math.min(100, downloadJob.progress_percent))}%` }}
                      />
                    </div>
                    <p className="break-all text-[11px] text-muted-foreground">
                      {downloadJob.current_file || downloadJob.target_dir}
                    </p>
                    {downloadJob.error ? (
                      <p className="text-[11px] text-destructive-strong">{downloadJob.error}</p>
                    ) : null}
                  </div>
                ) : null}
                <div className="space-y-2">
                  <button
                    type="button"
                    onClick={() => void copyCommand(selectedModel.probeCommand, `${selectedModel.id}:probe`)}
                    className="hb-btn-outline w-full justify-center py-2"
                  >
                    <Copy className="h-4 w-4" aria-hidden />
                    {copied === `${selectedModel.id}:probe` ? "已复制 probe 命令" : "复制测试命令"}
                  </button>
                  {downloadIsActive ? (
                    <button
                      type="button"
                      onClick={() => void cancelModelDownload()}
                      className="hb-btn-outline w-full justify-center py-2 text-destructive-strong"
                      disabled={isCancellingDownload}
                    >
                      {isCancellingDownload ? (
                        <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
                      ) : (
                        <Square className="h-4 w-4" aria-hidden />
                      )}
                      取消当前下载
                    </button>
                  ) : (
                    <button
                      type="button"
                      onClick={() => void startModelDownload()}
                      className="hb-btn-outline w-full justify-center py-2"
                      disabled={downloadRecoveryBlocked || downloadUnconfirmed || !canOfferModelDownload(selectedRecommendation?.recommendation_status)}
                    >
                      {selectedRecommendation?.recommendation_status === "installed" ? (
                        <CheckCircle2 className="h-4 w-4" aria-hidden />
                      ) : (
                        <Download className="h-4 w-4" aria-hidden />
                      )}
                      {downloadUnconfirmed ? "先核对已有任务状态" : selectedRecommendation?.recommendation_status === "installed"
                        ? "已安装并通过最小推理"
                        : copied === `${selectedModel.id}:download`
                          ? "已复制下载命令"
                          : !canOfferModelDownload(selectedRecommendation?.recommendation_status)
                            ? "先解决阻断项"
                            : isTauriRuntime() && automatedModelDownloadIds.has(selectedModel.id)
                              ? "下载、校验并登记"
                              : "复制安装说明"}
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => void copyCommand(selectedModel.invocation, `${selectedModel.id}:invoke`)}
                    className="hb-btn-primary w-full justify-center py-2"
                  >
                    <Terminal className="h-4 w-4" aria-hidden />
                    {copied === `${selectedModel.id}:invoke` ? "已复制调用示例" : "复制调用示例"}
                  </button>
                </div>
                {downloadMessage ? <p className="text-xs text-muted-foreground">{downloadMessage}</p> : null}
                {downloadRecoveryNotice && <p role="status" className="text-xs text-warning-strong">{downloadRecoveryNotice}</p>}
                {isTauriRuntime() && <button type="button" className="hb-btn-outline text-xs" disabled={downloadRecoveryState === "checking" || downloadIsActive} onClick={() => setDownloadRecoveryCheck(value => value + 1)}>重新检查下载任务状态</button>}
                <p className="break-all rounded-md bg-muted px-3 py-2 font-mono text-[11px] text-muted-foreground">
                  {portableCatalogPath}
                </p>
              </aside>
            ) : null}
          </div>

          <AsrPortabilityPanel
            key={`${machineProfile.source}:${machineProfile.preferredModelRoot}`}
            machineProfile={machineProfile}
          />
        </div>
      </details>
    </section>
  );
}

function AsrPortabilityPanel({ machineProfile }: { machineProfile: MachineProfile }) {
  const [config, setConfig] = useState<AsrPortableConfig>(() =>
    loadStoredAsrPortableConfig() ?? createHomeBaseAsrPortableConfig(machineProfile),
  );
  const [message, setMessage] = useState("");
  const importInputRef = useRef<HTMLInputElement>(null);
  const validation = useMemo(
    () => validateAsrPortableConfig(config, machineProfile),
    [config, machineProfile],
  );
  const rootFields: Array<{ key: keyof AsrPortableRoots; label: string }> = [
    { key: "aihub_root", label: "AIHub 根目录" },
    { key: "model_root", label: "模型目录" },
    { key: "runtime_root", label: "运行时目录" },
    { key: "output_root", label: "任务输出目录" },
    { key: "cache_root", label: "缓存目录" },
  ];

  function updateRoot(key: keyof AsrPortableRoots, value: string) {
    setConfig((current) => {
      const next = {
        ...current,
        roots: { ...current.roots, [key]: value },
      };
      persistAsrPortableConfig(next);
      return next;
    });
    setMessage("");
  }

  function exportProfile() {
    if (!validation.ready) {
      setMessage("存在阻断项，修正后才能导出迁移配置。");
      return;
    }
    const bundle = exportAsrPortableBundle(config);
    const blob = new Blob([`${JSON.stringify(bundle, null, 2)}\n`], {
      type: "application/json;charset=utf-8",
    });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "homebase-asr-portable-profile.json";
    anchor.click();
    URL.revokeObjectURL(url);
    setMessage("迁移配置已导出；密钥仅保留短名引用。 ");
  }

  function useCurrentMachinePaths() {
    const next = createHomeBaseAsrPortableConfig(machineProfile);
    setConfig(next);
    persistAsrPortableConfig(next);
    setMessage("已恢复为当前机器的默认路径，请再次检查。 ");
  }

  async function importProfile(file: File | undefined) {
    if (!file) return;
    try {
      const parsed = JSON.parse(await file.text()) as unknown;
      const imported = importAsrPortableBundle(parsed);
      setConfig(imported);
      persistAsrPortableConfig(imported);
      setMessage("配置已导入，请按目标机目录重新绑定并检查。 ");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "迁移配置读取失败。 ");
    } finally {
      if (importInputRef.current) importInputRef.current.value = "";
    }
  }

  return (
    <div className="border-t border-border pt-4">
      <details className="group" open={false}>
        <summary className="flex cursor-pointer list-none items-center gap-3 py-1">
          <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
            <ArrowRightLeft className="h-4 w-4" aria-hidden />
          </span>
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <h3 className="text-sm font-semibold">环境与迁移</h3>
              <span
                className={cn(
                  "rounded-full border px-2 py-0.5 text-[10px]",
                  validation.status === "ready"
                    ? "border-success/30 bg-success/10 text-success-strong"
                    : validation.status === "blocked"
                      ? "border-destructive/30 bg-destructive/10 text-destructive-strong"
                      : "border-warning/30 bg-warning/10 text-warning-strong",
                )}
              >
                {portabilityStatusLabel(validation.status)}
              </span>
            </div>
            <p className="mt-1 text-xs text-muted-foreground">
              版本化配置 · 路径可重绑定 · 密钥不随配置导出
            </p>
          </div>
          <ChevronDown
            className="h-4 w-4 text-muted-foreground transition-transform group-open:rotate-180"
            aria-hidden
          />
        </summary>

        <div className="mt-4 space-y-4">
          <div className="grid grid-cols-1 gap-x-3 gap-y-3 md:grid-cols-2">
            {rootFields.map((field) => (
              <label key={field.key} className={field.key === "output_root" ? "space-y-1.5 md:col-span-2" : "space-y-1.5"}>
                <span className="text-xs font-medium text-muted-foreground">{field.label}</span>
                <input
                  value={config.roots[field.key]}
                  onChange={(event) => updateRoot(field.key, event.target.value)}
                  className="w-full rounded-lg border border-border bg-background px-3 py-2 font-mono text-xs outline-none transition-colors focus:border-primary-500"
                  spellCheck={false}
                />
              </label>
            ))}
          </div>

          <div className="grid grid-cols-1 gap-3 lg:grid-cols-[minmax(0,1fr)_18rem]">
            <div className="border-l-2 border-border bg-muted/50 px-3 py-3">
              <div className="mb-2 flex items-center gap-2 text-xs font-medium">
                {validation.status === "ready" ? (
                  <MonitorCheck className="h-4 w-4 text-success-strong" aria-hidden />
                ) : (
                  <AlertTriangle className="h-4 w-4 text-warning-strong" aria-hidden />
                )}
                迁移预检
              </div>
              {validation.issues.length === 0 ? (
                <p className="text-xs text-muted-foreground">目录、方案与密钥引用规则通过。</p>
              ) : (
                <ul className="space-y-1.5 text-xs text-muted-foreground">
                  {validation.issues.slice(0, 6).map((issue) => (
                    <li key={`${issue.code}:${issue.field}`}>
                      <span className={issue.severity === "error" ? "text-destructive-strong" : "text-warning-strong"}>
                        {issue.message}
                      </span>{" "}
                      {issue.fix}
                    </li>
                  ))}
                </ul>
              )}
            </div>

            <div className="flex flex-col justify-between gap-3">
              <div className="space-y-1.5 text-xs text-muted-foreground">
                <div className="flex items-center gap-2">
                  <FileJson className="h-3.5 w-3.5" aria-hidden />
                  schema v{config.schema_version}
                </div>
                <div className="flex items-center gap-2">
                  <PackageCheck className="h-3.5 w-3.5" aria-hidden />
                  {config.enabled_preset_ids.length} 个方案 · {config.provider_secret_refs.length} 个密钥引用
                </div>
              </div>
              <div className="grid grid-cols-2 gap-2">
                <button
                  type="button"
                  onClick={useCurrentMachinePaths}
                  className="hb-btn-outline col-span-2 justify-center px-3 py-2"
                >
                  <MonitorCheck className="h-4 w-4" aria-hidden />
                  使用当前机器路径
                </button>
                <button
                  type="button"
                  onClick={exportProfile}
                  className="hb-btn-outline justify-center px-3 py-2"
                  disabled={!validation.ready}
                >
                  <FileJson className="h-4 w-4" aria-hidden />
                  导出配置
                </button>
                <button
                  type="button"
                  onClick={() => importInputRef.current?.click()}
                  className="hb-btn-outline justify-center px-3 py-2"
                >
                  <Upload className="h-4 w-4" aria-hidden />
                  导入检查
                </button>
                <input
                  ref={importInputRef}
                  type="file"
                  accept="application/json,.json"
                  className="hidden"
                  onChange={(event) => void importProfile(event.target.files?.[0])}
                />
              </div>
            </div>
          </div>

          {message ? <p className="text-xs text-muted-foreground">{message}</p> : null}
        </div>
      </details>
    </div>
  );
}

type AsrUiRun = {
  jobId: string | null;
  taskDir: string | null;
  status: string;
  activeEngine: string | null;
  message: string;
  runtimeState?: "managed" | "unmanaged" | "terminal" | "unknown";
};

const activeAsrStates = new Set(["queued", "preflight", "preprocessing", "running", "fusing"]);

function AsrWorkbench() {
  const queryClient = useQueryClient();
  const [recoveryState, setRecoveryState] = useState<"checking" | "ready" | "error">("checking");
  const [recoveryNotice, setRecoveryNotice] = useState("");
  const [recoveryCheck, setRecoveryCheck] = useState(0);
  const [sourcePath, setSourcePath] = useState("");
  const [taskName, setTaskName] = useState("");
  const [language, setLanguage] = useState("zh");
  const [scene, setScene] = useState("auto");
  const [selectedPresetId, setSelectedPresetId] = useState("asr_accuracy_max");
  const [fusionMode, setFusionMode] = useState<"manual" | "llm" | "auto">("auto");
  const [customPresetName, setCustomPresetName] = useState("");
  const [customPresetDescription, setCustomPresetDescription] = useState("");
  const [customEngineIds, setCustomEngineIds] = useState<string[]>([
    "faster_whisper_large_v3_cpu",
    "sensevoice_small",
  ]);
  const [editingCustomPresetId, setEditingCustomPresetId] = useState<string | null>(null);
  const [planMessage, setPlanMessage] = useState("");
  const [environment, setEnvironment] = useState<AsrEnvironmentReport | null>(null);
  const [environmentPresetId, setEnvironmentPresetId] = useState("");
  const [providerHealth, setProviderHealth] = useState<Record<string, string>>({});
  const [isChecking, setIsChecking] = useState(false);
  const [isStarting, setIsStarting] = useState(false);
  const [isCancelling, setIsCancelling] = useState(false);
  const [run, setRun] = useState<AsrUiRun>({
    jobId: null,
    taskDir: null,
    status: "idle",
    activeEngine: null,
    message: "",
  });
  const activeJobRef = useRef<string | null>(null);
  const { machineProfile, portableConfig, distributionInfo } = usePortableMachineProfile();
  const customPresets = useMemo(
    () => portableConfig.custom_presets.map(asrPortableCustomPresetToPreset),
    [portableConfig.custom_presets],
  );
  const allAsrPresets = useMemo(() => [...asrPresets, ...customPresets], [customPresets]);
  const customEditorPreset = useMemo(
    () =>
      asrPortableCustomPresetToPreset({
        id: editingCustomPresetId ?? customDraftPresetId,
        name: customPresetName.trim() || "未命名自定义组合",
        description: customPresetDescription.trim() || "按当前选择的引擎执行。",
        engine_ids: customEngineIds,
        fusion_mode: fusionMode,
        scene_id: scene,
        language,
      }),
    [customEngineIds, customPresetDescription, customPresetName, editingCustomPresetId, fusionMode, language, scene],
  );
  const recommendations = useMemo(
    () => recommendModels(builtInModels, machineProfile),
    [machineProfile],
  );
  const recommendationById = useMemo(
    () => new Map(recommendations.map((item) => [item.id, item])),
    [recommendations],
  );
  const presetRuntimeState = useMemo(
    () =>
      new Map(
        allAsrPresets.map((preset) => [
          preset.id,
          deriveAsrPresetRuntimeState(preset, recommendationById, machineProfile),
        ]),
      ),
    [allAsrPresets, machineProfile, recommendationById],
  );
  const selectedPreset =
    selectedPresetId === customDraftPresetId || selectedPresetId === editingCustomPresetId
      ? customEditorPreset
      : allAsrPresets.find((preset) => preset.id === selectedPresetId) ?? asrPresets[0];
  const selectedScene = asrScenes.find((item) => item.id === scene) ?? asrScenes[0];
  const taskFolder = makeAsrTaskFolder(taskName, sourcePath, portableConfig.roots.output_root);
  const isEditingSelectedPreset =
    selectedPresetId === customDraftPresetId || selectedPresetId === editingCustomPresetId;
  const selectedRuntimeState =
    selectedPreset.engineIds.length === 0
      ? "unavailable"
      : isEditingSelectedPreset
        ? deriveAsrPresetRuntimeState(selectedPreset, recommendationById, machineProfile)
        : presetRuntimeState.get(selectedPreset.id) ??
          deriveAsrPresetRuntimeState(selectedPreset, recommendationById, machineProfile);
  const runnerPath = `${normalizeWindowsPath(portableConfig.roots.aihub_root).replace(/\\+$/, "")}\\tools\\local-asr\\runner\\asr_task_runner.py`;
  const selectedPresetSignature = `${selectedPreset.id}:${selectedPreset.engineIds.join(",")}`;
  const unknownCustomEngineIds = customEngineIds.filter(
    (engineId) => !asrEngineOptions.some((option) => option.id === engineId),
  );
  const liveRuntimeState =
    environment && environmentPresetId === selectedPresetSignature
      ? environment.available
        ? "ready"
        : "unavailable"
      : selectedRuntimeState;
  const runUnconfirmed = run.runtimeState === "unknown" || (activeAsrStates.has(run.status) && run.runtimeState !== "managed");
  const recoveryBlocked = recoveryState !== "ready" || Boolean(recoveryNotice);
  const runIsActive = activeAsrStates.has(run.status) && !runUnconfirmed;

  useEffect(() => {
    setEnvironment(null);
    setEnvironmentPresetId("");
    setProviderHealth({});
  }, [portableConfig.roots.output_root, selectedPresetSignature, sourcePath]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<AsrJobEvent>("asr-job", (event) => {
      const payload = event.payload;
      if (payload.jobId !== activeJobRef.current) return;
      if (payload.kind === "progress") {
        const progressEvent = String(payload.payload.event ?? "");
        if (progressEvent === "task_state") {
          const status = String(payload.payload.status ?? "running");
          setRun((current) => ({
            ...current,
            status,
            activeEngine:
              typeof payload.payload.active_engine === "string"
                ? payload.payload.active_engine
                : current.activeEngine,
            message: asrTaskStatusLabel(status),
          }));
        } else if (progressEvent === "engine_state") {
          const engineId = String(payload.payload.engine_id ?? "");
          const status = String(payload.payload.status ?? "");
          setRun((current) => ({
            ...current,
            activeEngine: engineId || current.activeEngine,
            message: engineId ? `${asrEngineLabel(engineId)} · ${asrEngineStatusLabel(status)}` : current.message,
          }));
        }
      } else if (payload.kind === "diagnostic") {
        setRun((current) => ({ ...current, message: payload.message }));
      } else if (payload.kind === "finished") {
        activeJobRef.current = null;
        setRun((current) => ({
          ...current,
          taskDir: payload.taskDir,
          status: payload.status,
          activeEngine: null,
          message: payload.message || asrTaskStatusLabel(payload.status),
        }));
        window.dispatchEvent(new CustomEvent(asrOpenTaskEvent, { detail: payload.taskDir }));
      }
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) { setRecoveryState("ready"); return; }
    let cancelled = false;
    activeJobRef.current = null;
    setRun({ jobId: null, taskDir: null, status: "idle", activeEngine: null, message: "" });
    setRecoveryState("checking");
    setRecoveryNotice("");
    void queryClient.fetchQuery({
      queryKey: ["asr", "reconcile", portableConfig.roots.output_root],
      queryFn: () => asrApi.reconcile(portableConfig.roots.output_root),
      staleTime: 0,
    })
      .then((jobs) => {
        if (cancelled) return;
        setRecoveryState("ready");
        const unknown = jobs.filter(job => job.runtime_state === "unmanaged" || job.runtime_state === "unknown" || (!job.runtime_state && activeAsrStates.has(job.status)));
        setRecoveryNotice(unknown.length ? `${unknown.length} 个历史转写任务状态未知，原始记录未修改；请先核对任务和结果。` : "");
        const active = jobs.find((job) => job.runtime_state === "managed");
        const restored = active ?? jobs.find((job) => job.runtime_state === "unmanaged" || job.runtime_state === "unknown" || activeAsrStates.has(job.status)) ?? jobs[0];
        if (!restored || activeJobRef.current) return;
        activeJobRef.current = active?.jobId ?? null;
        setRun({ ...asrRunFromJob(restored), runtimeState: restored.runtime_state ?? "unknown", message: restored.runtime_message ?? "任务状态未知：未确认任务归属，原始记录未修改。" });
      })
      .catch(() => { if (!cancelled) { setRecoveryState("error"); setRecoveryNotice("恢复检查失败：任务状态未知，原始记录未修改，请重试。"); } });
    return () => {
      cancelled = true;
    };
  }, [portableConfig.roots.output_root, queryClient, recoveryCheck]);

  useEffect(() => {
    if (!isTauriRuntime() || !run.jobId || !run.taskDir || !runIsActive) return;
    let disposed = false;
    const stop = startPolling(async () => {
      await asrApi
        .get(run.taskDir!, portableConfig.roots.output_root, run.jobId)
        .then((view) => {
          if (disposed || activeJobRef.current !== run.jobId) return;
          const status = String(view.taskMeta.status ?? view.job?.status ?? run.status);
          setRun((current) => ({
            ...current,
            status,
            message: asrTaskStatusLabel(status),
          }));
          if (!activeAsrStates.has(status)) {
            activeJobRef.current = null;
            window.dispatchEvent(new CustomEvent(asrOpenTaskEvent, { detail: run.taskDir }));
          }
        })
        .catch(() => {
          if (!disposed) setRun(current => ({ ...current, message: "转写状态查询失败，任务状态暂时未知；稍后自动重试。" }));
        });
    }, 1800, false);
    return () => { disposed = true; stop(); };
  }, [portableConfig.roots.output_root, run.jobId, run.taskDir, run.status, runIsActive]);

  function selectAsrPreset(presetId: string) {
    const custom = portableConfig.custom_presets.find((preset) => preset.id === presetId);
    if (!custom) {
      setSelectedPresetId(presetId);
      setEditingCustomPresetId(null);
      return;
    }
    setCustomPresetName(custom.name);
    setCustomPresetDescription(custom.description);
    setCustomEngineIds([...custom.engine_ids]);
    setFusionMode(custom.fusion_mode);
    setScene(asrScenes.some((item) => item.id === custom.scene_id) ? custom.scene_id : "auto");
    setLanguage(["zh", "auto", "en", "ja", "ko"].includes(custom.language) ? custom.language : "auto");
    setEditingCustomPresetId(custom.id);
    setSelectedPresetId(custom.id);
  }

  function beginCustomPreset() {
    setCustomPresetName("");
    setCustomPresetDescription("");
    setCustomEngineIds(["faster_whisper_large_v3_cpu"]);
    setEditingCustomPresetId(null);
    setSelectedPresetId(customDraftPresetId);
    setPlanMessage("已新建临时组合；可直接使用，也可以命名保存。 ");
  }

  function toggleCustomEngine(engineId: string) {
    setCustomEngineIds((current) =>
      current.includes(engineId)
        ? current.filter((item) => item !== engineId)
        : [...current, engineId],
    );
  }

  function useCustomEditorPreset() {
    if (customEngineIds.length === 0) {
      setPlanMessage("自定义组合至少需要一个引擎。 ");
      return;
    }
    setSelectedPresetId(editingCustomPresetId ?? customDraftPresetId);
    setPlanMessage(
      fusionMode === "llm" && customEngineIds.length === 1
        ? "已使用当前组合；单引擎没有版本差异可融合，只会执行单稿整理。 "
        : "已使用当前组合；开始前仍会逐项检查模型、密钥和额度。 ",
    );
  }

  function saveCustomPreset() {
    const name = customPresetName.trim();
    const description = customPresetDescription.trim();
    if (!name || name.length > 40) {
      setPlanMessage("组合名称必须为 1-40 个字符。 ");
      return;
    }
    if (description.length > 160) {
      setPlanMessage("组合说明不能超过 160 个字符。 ");
      return;
    }
    if (customEngineIds.length === 0) {
      setPlanMessage("至少选择一个引擎后再保存。 ");
      return;
    }
    const duplicate = portableConfig.custom_presets.find(
      (preset) =>
        preset.id !== editingCustomPresetId &&
        preset.name.trim().toLocaleLowerCase() === name.toLocaleLowerCase(),
    );
    if (duplicate) {
      setPlanMessage(`组合名称“${name}”已存在，请换一个名称。 `);
      return;
    }
    if (!editingCustomPresetId && portableConfig.custom_presets.length >= 24) {
      setPlanMessage("最多保存 24 个自定义组合，请先删除不再使用的组合。 ");
      return;
    }

    const id =
      editingCustomPresetId ??
      createCustomAsrPresetId(name, portableConfig.custom_presets.map((preset) => preset.id));
    const nextPreset: AsrPortableCustomPreset = {
      id,
      name,
      description,
      engine_ids: [...customEngineIds],
      fusion_mode: fusionMode,
      scene_id: scene,
      language,
    };
    const exists = portableConfig.custom_presets.some((preset) => preset.id === id);
    const nextCustomPresets = exists
      ? portableConfig.custom_presets.map((preset) => (preset.id === id ? nextPreset : preset))
      : [...portableConfig.custom_presets, nextPreset];
    persistAsrPortableConfig({
      ...portableConfig,
      custom_presets: nextCustomPresets,
    });
    setEditingCustomPresetId(id);
    setSelectedPresetId(id);
    setPlanMessage(exists ? "自定义组合已更新。 " : "自定义组合已保存，并会随迁移配置一起导出。 ");
  }

  function deleteCustomPreset(preset: AsrPortableCustomPreset) {
    if (!window.confirm(`删除自定义组合“${preset.name}”？`)) return;
    const nextDefault =
      portableConfig.default_preset_id === preset.id
        ? "asr_accuracy_max"
        : portableConfig.default_preset_id;
    persistAsrPortableConfig({
      ...portableConfig,
      default_preset_id: nextDefault,
      custom_presets: portableConfig.custom_presets.filter((item) => item.id !== preset.id),
    });
    if (selectedPresetId === preset.id || editingCustomPresetId === preset.id) {
      setEditingCustomPresetId(null);
      setSelectedPresetId("asr_accuracy_max");
    }
    setPlanMessage("自定义组合已删除，历史任务结果不受影响。 ");
  }

  async function chooseAudio() {
    setPlanMessage("");
    if (!isTauriRuntime()) {
      setPlanMessage("浏览器预览不能读取本机绝对路径，请手动填写录音路径。 ");
      return;
    }
    const selected = await openDialog({
      multiple: false,
      directory: false,
      filters: [
        {
          name: "音频与视频",
          extensions: ["mp3", "wav", "m4a", "aac", "flac", "ogg", "mp4", "mov", "mkv", "webm"],
        },
      ],
    });
    if (typeof selected === "string") setSourcePath(selected);
  }

  async function checkEnvironment(includeProviderHealth = true) {
    if (selectedPreset.engineIds.length === 0) {
      setPlanMessage("当前组合没有引擎，无法执行环境预检。 ");
      return null;
    }
    if (!isTauriRuntime()) {
      setPlanMessage("浏览器预览不会声明本机环境、模型或 API 可用。 ");
      return null;
    }
    setIsChecking(true);
    setPlanMessage("");
    try {
      const report = await asrApi.probe({
        sourcePath: sourcePath.trim() || null,
        outputRoot: portableConfig.roots.output_root,
        presetId: selectedPreset.id,
        engineIds: selectedPreset.engineIds,
        runnerPath,
        allowSystemDrive:
          distributionInfo?.allowSystemDrive || isSystemDrivePath(portableConfig.roots.output_root),
      });
      setEnvironment(report);
      setEnvironmentPresetId(selectedPresetSignature);
      if (includeProviderHealth) {
        const refs = [
          ...new Set(
            selectedPreset.engineIds
              .map((engineId) => asrProviderSecretByEngine[engineId])
              .filter((item): item is string => Boolean(item)),
          ),
        ];
        if (fusionMode !== "manual" && selectedPreset.engineIds.length > 1) refs.push("openrouter_key");
        const results = await Promise.allSettled(
          [...new Set(refs)].map(async (shortName) => [shortName, await secretsApi.test(shortName)] as const),
        );
        const nextHealth: Record<string, string> = {};
        for (const result of results) {
          if (result.status === "fulfilled") {
            const [shortName, health] = result.value;
            nextHealth[shortName] = health.success ? "valid" : health.status;
          }
        }
        setProviderHealth(nextHealth);
      }
      setPlanMessage(report.available ? "环境预检通过，可以开始转写。 " : report.error || "当前组合存在阻断项。 ");
      return report;
    } catch (error) {
      setPlanMessage(error instanceof Error ? error.message : "环境预检失败。 ");
      return null;
    } finally {
      setIsChecking(false);
    }
  }

  async function startRun() {
    if (!sourcePath.trim()) {
      setPlanMessage("请先选择或填写录音路径。 ");
      return;
    }
    if (!isTauriRuntime()) {
      await prepareRunPlan();
      return;
    }
    setIsStarting(true);
    setPlanMessage("");
    try {
      const probe = await checkEnvironment(true);
      if (!probe?.available) return;
      const started = await asrApi.start({
        sourcePath: sourcePath.trim(),
        outputRoot: portableConfig.roots.output_root,
        taskName: taskName.trim() || null,
        presetId: selectedPreset.id,
        engineIds: selectedPreset.engineIds,
        language,
        fusionMode,
        preprocessProfile: scene === "auto" ? "adaptive" : scene,
        runnerPath,
        allowSystemDrive:
          distributionInfo?.allowSystemDrive || isSystemDrivePath(portableConfig.roots.output_root),
      });
      activeJobRef.current = started.jobId;
      setRun({
        jobId: started.jobId,
        taskDir: started.taskDir,
        status: "queued",
        activeEngine: null,
        message: "任务已进入后台队列。",
        runtimeState: "managed",
      });
      setPlanMessage("任务已启动；切换页面不会丢失磁盘状态。 ");
    } catch (error) {
      setPlanMessage(error instanceof Error ? error.message : "ASR 任务启动失败。 ");
    } finally {
      setIsStarting(false);
    }
  }

  async function cancelRun() {
    if (!run.jobId) return;
    setIsCancelling(true);
    try {
      await asrApi.cancel(run.jobId);
      activeJobRef.current = null;
      setRun((current) => ({ ...current, status: "cancelled", activeEngine: null, message: "任务已取消。" }));
    } catch (error) {
      setPlanMessage(error instanceof Error ? error.message : "取消任务失败。 ");
    } finally {
      setIsCancelling(false);
    }
  }

  async function prepareRunPlan() {
    if (!sourcePath.trim()) {
      setPlanMessage("请先选择或填写录音路径。 ");
      return;
    }
    if (selectedRuntimeState === "unavailable") {
      setPlanMessage("当前方案没有可执行路线，请先处理模型或 API 阻断项。 ");
      return;
    }
    const plan = {
      schema_version: 1,
      source_path: sourcePath.trim(),
      task_name: taskName.trim() || null,
      preset_id: selectedPreset.id,
      language,
      scene,
      task_folder: taskFolder,
      engine_ids: selectedPreset.engineIds,
      engine_labels: selectedPreset.engines,
      fusion_mode: fusionMode,
      runtime_state: selectedRuntimeState,
    };
    await copyToClipboard(`${JSON.stringify(plan, null, 2)}\n`);
    setPlanMessage("运行计划已复制；桌面版可在同一位置直接执行。 ");
  }

  return (
    <section aria-label="语音转文字工作台">
      <details
        className="group rounded-xl border border-border bg-card transition-colors open:border-primary-300/70 dark:open:border-primary-700/70"
        open
      >
        <summary className="flex cursor-pointer list-none items-center gap-3 px-4 py-3">
          <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
            <Mic2 className="h-4 w-4" aria-hidden />
          </span>
          <div className="min-w-0 flex-1">
            <h2 className="text-sm font-semibold">语音转文字工作台</h2>
            <p className="truncate text-xs text-muted-foreground">
              选择录音、方案和输出目标，结果统一进入 {portableConfig.roots.output_root}。
            </p>
          </div>
          <ChevronDown
            className="h-4 w-4 text-muted-foreground transition-transform group-open:rotate-180"
            aria-hidden
          />
        </summary>
        <div className="space-y-5 border-t border-border p-4">
          <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,1.25fr)_minmax(22rem,0.75fr)]">
            <div className="space-y-4">
              <div className="grid grid-cols-1 gap-3 md:grid-cols-[minmax(0,1fr)_13rem]">
                <label className="space-y-1.5">
                  <span className="text-xs font-medium uppercase tracking-widest text-muted-foreground">
                    录音路径
                  </span>
                  <input
                    value={sourcePath}
                    onChange={(event) => setSourcePath(event.target.value)}
                    className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm outline-none transition-colors placeholder:text-muted-foreground focus:border-primary-500"
                    placeholder="C:\HomeBase\Examples\sample.mp3"
                  />
                </label>
                <label className="space-y-1.5">
                  <span className="text-xs font-medium uppercase tracking-widest text-muted-foreground">
                    任务名
                  </span>
                  <input
                    value={taskName}
                    onChange={(event) => setTaskName(event.target.value)}
                    className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm outline-none transition-colors placeholder:text-muted-foreground focus:border-primary-500"
                    placeholder="采购风控会议"
                  />
                </label>
              </div>

              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
                <label className="space-y-1.5">
                  <span className="text-xs font-medium uppercase tracking-widest text-muted-foreground">
                    识别方案
                  </span>
                  <select
                    value={selectedPresetId}
                    onChange={(event) => selectAsrPreset(event.target.value)}
                    className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm outline-none transition-colors focus:border-primary-500"
                  >
                    <optgroup label="内置方案">
                      {asrPresets.map((preset) => (
                        <option key={preset.id} value={preset.id}>
                          {preset.name} · {preset.trait}
                        </option>
                      ))}
                    </optgroup>
                    {customPresets.length > 0 ? (
                      <optgroup label="我的组合">
                        {customPresets.map((preset) => (
                          <option key={preset.id} value={preset.id}>
                            {preset.name} · {preset.engines.length} 路
                          </option>
                        ))}
                      </optgroup>
                    ) : null}
                    {selectedPresetId === customDraftPresetId ? (
                      <option value={customDraftPresetId}>临时组合 · 尚未保存</option>
                    ) : null}
                  </select>
                </label>
                <label className="space-y-1.5">
                  <span className="text-xs font-medium uppercase tracking-widest text-muted-foreground">
                    语言
                  </span>
                  <select
                    value={language}
                    onChange={(event) => setLanguage(event.target.value)}
                    className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm outline-none transition-colors focus:border-primary-500"
                  >
                    <option value="zh">中文优先</option>
                    <option value="auto">自动检测</option>
                    <option value="en">英文</option>
                    <option value="ja">日文</option>
                    <option value="ko">韩文</option>
                  </select>
                </label>
                <label className="space-y-1.5">
                  <span className="text-xs font-medium uppercase tracking-widest text-muted-foreground">
                    场景
                  </span>
                  <select
                    value={scene}
                    onChange={(event) => setScene(event.target.value)}
                    className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm outline-none transition-colors focus:border-primary-500"
                  >
                    {asrScenes.map((item) => (
                      <option key={item.id} value={item.id}>
                        {item.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="space-y-1.5">
                  <span className="text-xs font-medium uppercase tracking-widest text-muted-foreground">
                    结果整理
                  </span>
                  <select
                    value={fusionMode}
                    onChange={(event) => setFusionMode(event.target.value as "manual" | "llm" | "auto")}
                    className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm outline-none transition-colors focus:border-primary-500"
                  >
                    <option value="auto">自动融合，失败则保留主稿</option>
                    <option value="llm">大模型融合</option>
                    <option value="manual">仅保留人工对比</option>
                  </select>
                </label>
              </div>

              <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                {asrPresets.map((preset) => (
                  <button
                    key={preset.id}
                    type="button"
                    onClick={() => selectAsrPreset(preset.id)}
                    className={cn(
                      "rounded-lg border bg-background p-4 text-left transition-colors",
                      selectedPresetId === preset.id
                        ? "border-primary-400 ring-1 ring-primary-300/50 dark:border-primary-600"
                        : "border-border hover:border-primary-300/70 dark:hover:border-primary-700/70",
                    )}
                  >
                    <div className="mb-2 flex items-start justify-between gap-3">
                      <div className="min-w-0">
                        <h3 className="text-sm font-semibold">
                          {preset.name}
                          <span className="ml-2 text-xs font-normal text-muted-foreground">
                            {preset.trait}
                          </span>
                        </h3>
                        <p className="mt-1 line-clamp-2 text-xs text-muted-foreground">
                          {preset.detail}
                        </p>
                      </div>
                      <AsrPresetBadge
                        status={presetRuntimeState.get(preset.id) ?? "environment_check"}
                      />
                    </div>
                    <p className="rounded-md bg-muted px-2 py-1.5 font-mono text-[11px] text-muted-foreground">
                      {preset.engines.join(" + ")}
                    </p>
                  </button>
                ))}
              </div>

              <details className="border-t border-border pt-3">
                <summary className="flex cursor-pointer list-none items-center gap-2 py-1 text-sm font-medium">
                  <SlidersHorizontal className="h-4 w-4 text-muted-foreground" aria-hidden />
                  <span className="flex-1">自定义组合</span>
                  <span className="text-xs font-normal text-muted-foreground">
                    {portableConfig.custom_presets.length}/24 已保存
                  </span>
                  <ChevronDown className="h-4 w-4 text-muted-foreground transition-transform" aria-hidden />
                </summary>
                <div className="space-y-4 pt-3">
                  <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                    {asrEngineOptions.map((engine) => (
                      <label
                        key={engine.id}
                        className={cn(
                          "flex cursor-pointer items-start gap-3 rounded-md border px-3 py-2.5 transition-colors",
                          customEngineIds.includes(engine.id)
                            ? "border-primary-400 bg-primary-50/50 dark:border-primary-700 dark:bg-primary-950/20"
                            : "border-border bg-background hover:border-primary-300/70",
                        )}
                      >
                        <input
                          type="checkbox"
                          checked={customEngineIds.includes(engine.id)}
                          onChange={() => toggleCustomEngine(engine.id)}
                          className="mt-0.5 h-4 w-4 accent-primary-600"
                        />
                        <span className="min-w-0">
                          <span className="block text-xs font-medium">{engine.label}</span>
                          <span className="mt-0.5 block text-[11px] text-muted-foreground">
                            {engine.track === "local" ? "本地" : "云端"} · {engine.trait}
                          </span>
                        </span>
                      </label>
                    ))}
                  </div>

                  {unknownCustomEngineIds.length > 0 ? (
                    <p className="rounded-md bg-warning/10 px-3 py-2 text-xs text-warning-strong">
                      导入组合包含已未知或下架的引擎：{unknownCustomEngineIds.join("、")}。请移除后再运行。
                    </p>
                  ) : null}

                  <div className="grid grid-cols-1 gap-3 md:grid-cols-[minmax(12rem,0.7fr)_minmax(0,1.3fr)]">
                    <label className="space-y-1.5">
                      <span className="text-xs font-medium text-muted-foreground">组合名称</span>
                      <input
                        value={customPresetName}
                        onChange={(event) => setCustomPresetName(event.target.value)}
                        maxLength={40}
                        className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm outline-none focus:border-primary-500"
                        placeholder="例如：中文会议双轨"
                      />
                    </label>
                    <label className="space-y-1.5">
                      <span className="text-xs font-medium text-muted-foreground">适用说明</span>
                      <input
                        value={customPresetDescription}
                        onChange={(event) => setCustomPresetDescription(event.target.value)}
                        maxLength={160}
                        className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm outline-none focus:border-primary-500"
                        placeholder="说明场景、速度或隐私要求"
                      />
                    </label>
                  </div>

                  <div className="flex flex-wrap items-center gap-2">
                    <button type="button" onClick={beginCustomPreset} className="hb-btn-outline px-3 py-2">
                      <Plus className="h-4 w-4" aria-hidden />
                      新建
                    </button>
                    <button type="button" onClick={useCustomEditorPreset} className="hb-btn-outline px-3 py-2">
                      <CheckCircle2 className="h-4 w-4" aria-hidden />
                      使用当前组合
                    </button>
                    <button type="button" onClick={saveCustomPreset} className="hb-btn-primary px-3 py-2">
                      <Save className="h-4 w-4" aria-hidden />
                      {editingCustomPresetId ? "更新组合" : "保存组合"}
                    </button>
                  </div>

                  {portableConfig.custom_presets.length > 0 ? (
                    <div className="divide-y divide-border border-y border-border">
                      {portableConfig.custom_presets.map((preset) => (
                        <div key={preset.id} className="flex items-center gap-2 py-2">
                          <button
                            type="button"
                            onClick={() => selectAsrPreset(preset.id)}
                            className="min-w-0 flex-1 text-left"
                          >
                            <span className="block truncate text-xs font-medium">{preset.name}</span>
                            <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">
                              {preset.engine_ids.map(asrEngineLabel).join(" + ")}
                            </span>
                          </button>
                          <button
                            type="button"
                            onClick={() => deleteCustomPreset(preset)}
                            className="hb-btn-icon text-muted-foreground hover:text-destructive-strong"
                            title={`删除 ${preset.name}`}
                            aria-label={`删除 ${preset.name}`}
                          >
                            <Trash2 className="h-4 w-4" aria-hidden />
                          </button>
                        </div>
                      ))}
                    </div>
                  ) : null}
                </div>
              </details>
            </div>

            <aside className="space-y-3 rounded-lg border border-border bg-background p-4">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <h3 className="text-sm font-semibold">{selectedPreset.name}</h3>
                  <p className="mt-1 text-xs text-muted-foreground">{selectedPreset.bestFor}</p>
                </div>
                <AsrPresetBadge status={liveRuntimeState} />
              </div>
              <dl className="space-y-2 text-xs">
                <DetailRow label="可用性" value={asrPresetRuntimeLabel(liveRuntimeState)} />
                <DetailRow label="轨道" value={asrTrackLabel(selectedPreset.track)} />
                <DetailRow label="速度" value={selectedPreset.speed} />
                <DetailRow label="策略" value={selectedPreset.outputPolicy} />
                <DetailRow label="场景" value={`${selectedScene.label} · ${selectedScene.hint}`} />
              </dl>
              {environment?.report ? (
                <div className="space-y-1.5 rounded-md border border-border bg-muted/40 px-3 py-2">
                  <div className="flex items-center justify-between gap-2 text-xs font-medium">
                    <span>实际执行预检</span>
                    <span className="font-mono text-[10px] text-muted-foreground">
                      {environment.report.ready_engine_count}/{environment.report.engine_count} 可运行
                    </span>
                  </div>
                  {environment.report.engines.map((engine) => (
                    <div key={engine.engine_id} className="flex items-start justify-between gap-2 text-[11px]">
                      <span className="min-w-0 truncate text-muted-foreground">
                        {asrEngineLabel(engine.engine_id)}
                      </span>
                      <span
                        className={cn(
                          "shrink-0",
                          engine.status === "ready" ? "text-success-strong" : "text-destructive-strong",
                        )}
                        title={engine.reason || engine.repair || undefined}
                      >
                        {engine.status === "ready" ? "就绪" : "阻断"}
                      </span>
                    </div>
                  ))}
                  {Object.entries(providerHealth).map(([shortName, status]) => (
                    <div key={shortName} className="flex items-start justify-between gap-2 text-[11px]">
                      <span className="min-w-0 truncate text-muted-foreground">{shortName}</span>
                      <span className={status === "valid" ? "text-success-strong" : "text-warning-strong"}>
                        {asrProviderHealthLabel(status)}
                      </span>
                    </div>
                  ))}
                </div>
              ) : environment?.error ? (
                <p className="rounded-md bg-destructive/10 px-3 py-2 text-[11px] text-destructive-strong">
                  {environment.error}
                </p>
              ) : null}
              <div className="rounded-md bg-muted px-3 py-2">
                <div className="mb-1 flex items-center gap-2 text-xs font-medium">
                  <FolderOpen className="h-3.5 w-3.5" aria-hidden />
                  输出任务包
                </div>
                <p className="break-all font-mono text-[11px] text-muted-foreground">
                  {run.taskDir || taskFolder}
                </p>
              </div>
              <div className="grid grid-cols-2 gap-2">
                <button
                  type="button"
                  onClick={() => void chooseAudio()}
                  className="hb-btn-outline justify-center px-3 py-2"
                >
                  <FolderOpen className="h-4 w-4" aria-hidden />
                  选择音频
                </button>
                <button
                  type="button"
                  onClick={() => void checkEnvironment(true)}
                  className="hb-btn-outline justify-center px-3 py-2"
                  disabled={isChecking || runIsActive || !isTauriRuntime()}
                >
                  {isChecking ? (
                    <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
                  ) : (
                    <MonitorCheck className="h-4 w-4" aria-hidden />
                  )}
                  检查环境
                </button>
                {runIsActive ? (
                  <button
                    type="button"
                    onClick={() => void cancelRun()}
                    className="hb-btn-outline col-span-2 justify-center px-3 py-2 text-destructive-strong"
                    disabled={isCancelling}
                  >
                    {isCancelling ? (
                      <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
                    ) : (
                      <Square className="h-4 w-4" aria-hidden />
                    )}
                    取消任务
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={() => void startRun()}
                    className="hb-btn-primary col-span-2 justify-center px-3 py-2"
                    disabled={recoveryBlocked || runUnconfirmed || isStarting || selectedRuntimeState === "unavailable"}
                  >
                    {isStarting ? (
                      <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
                    ) : (
                      <Play className="h-4 w-4" aria-hidden />
                    )}
                    {runUnconfirmed ? "先核对已有任务状态" : isTauriRuntime() ? "开始转写" : "复制运行计划"}
                  </button>
                )}
              </div>
              {run.status !== "idle" ? (
                <div className="rounded-md border border-border bg-muted/40 px-3 py-2 text-xs">
                  <div className="flex items-center justify-between gap-3">
                    <span className="font-medium">{runUnconfirmed ? "任务状态未知" : asrTaskStatusLabel(run.status)}</span>
                    <AsrLiveStatusBadge status={runUnconfirmed ? "unknown" : run.status} />
                  </div>
                  {run.activeEngine ? (
                    <p className="mt-1 text-muted-foreground">当前：{asrEngineLabel(run.activeEngine)}</p>
                  ) : null}
                  {run.message ? <p className="mt-1 text-muted-foreground">{run.message}</p> : null}
                  {run.taskDir && !runIsActive ? (
                    <button
                      type="button"
                      onClick={() => {
                        window.dispatchEvent(new CustomEvent(asrOpenTaskEvent, { detail: run.taskDir }));
                        setPlanMessage("已把任务目录交给下方结果工作区。 ");
                      }}
                      className="mt-2 inline-flex items-center gap-1.5 text-xs font-medium text-primary-700 hover:underline dark:text-primary-300"
                    >
                      <ArrowRightLeft className="h-3.5 w-3.5" aria-hidden />
                      交给结果工作区
                    </button>
                  ) : null}
                </div>
              ) : null}
              {planMessage ? <p className="text-xs text-muted-foreground">{planMessage}</p> : null}
            </aside>
          </div>

          <div className="grid grid-cols-1 gap-3 lg:grid-cols-4">
            {asrPipeline.map((step) => {
              const Icon = step.icon;
              return (
                <article key={step.title} className="rounded-lg border border-border bg-background p-4">
                  <div className="mb-2 flex items-center gap-2">
                    <Icon className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
                    <h3 className="text-sm font-semibold">{step.title}</h3>
                  </div>
                  <p className="text-xs text-muted-foreground">{step.detail}</p>
                </article>
              );
            })}
          </div>

          <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,1fr)_24rem]">
            <div className="rounded-lg border border-border bg-background p-4">
              <div className="mb-3 flex items-center gap-2">
                <Database className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
                <h3 className="text-sm font-semibold">输出物</h3>
              </div>
              {recoveryNotice && <p role="status" className="text-xs text-warning-strong">{recoveryNotice}</p>}
              {isTauriRuntime() && <button type="button" className="hb-btn-outline text-xs" disabled={recoveryState === "checking" || runIsActive} onClick={() => setRecoveryCheck(value => value + 1)}>重新检查转写任务状态</button>}
              <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
                {asrOutputArtifacts.map((artifact) => (
                  <div key={artifact.path} className="rounded-md bg-muted px-3 py-2">
                    <div className="flex items-center gap-2 text-xs font-medium">
                      <ClipboardList className="h-3.5 w-3.5" aria-hidden />
                      {artifact.label}
                    </div>
                    <p className="mt-1 font-mono text-[11px] text-muted-foreground">
                      {artifact.path}
                    </p>
                    <p className="mt-1 text-[11px] text-muted-foreground">
                      {artifact.description}
                    </p>
                  </div>
                ))}
              </div>
            </div>

            <div className="rounded-lg border border-border bg-background p-4">
              <div className="mb-3 flex items-center gap-2">
                <ShieldCheck className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
                <h3 className="text-sm font-semibold">Agent 接入</h3>
              </div>
              <div className="space-y-2 text-xs text-muted-foreground">
                <p>
                  人用入口固定在 Quick Panel；AI 用入口对齐 MCP，不让 Agent 直接散写输出文件。
                </p>
                <p className="rounded-md bg-muted px-3 py-2 font-mono text-[11px]">
                  local-asr.transcribe_audio(audio_path, preset_id)
                </p>
                <p className="rounded-md bg-muted px-3 py-2 font-mono text-[11px]">
                  C:\HomeBase\AIHub\mcp\servers\local-asr
                </p>
                <p>
                  自动错字词典和正式回归评测暂不启用，后续确认策略后再接。
                </p>
              </div>
            </div>
          </div>
        </div>
      </details>
    </section>
  );
}

function ResultWorkspace() {
  const queryClient = useQueryClient();
  const { portableConfig } = usePortableMachineProfile();
  const outputRoot = portableConfig.roots.output_root;
  const runnerPath = `${normalizeWindowsPath(portableConfig.roots.aihub_root).replace(/\\+$/, "")}\\tools\\local-asr\\runner\\asr_task_runner.py`;
  const [taskFolder, setTaskFolder] = useState("");
  const [contentType, setContentType] = useState<"audio" | "ocr" | "document">("audio");
  const [copied, setCopied] = useState("");
  const [taskHistory, setTaskHistory] = useState<AsrTaskView[]>([]);
  const [taskView, setTaskView] = useState<AsrTaskView | null>(null);
  const [loadedFiles, setLoadedFiles] = useState<WorkspaceLoadedFile[]>([]);
  const [selectedFileRelPath, setSelectedFileRelPath] = useState("");
  const [loadState, setLoadState] = useState<"idle" | "loading" | "loaded" | "error">("idle");
  const [loadMessage, setLoadMessage] = useState("");
  const [historyState, setHistoryState] = useState<"idle" | "loading" | "loaded" | "error">("idle");
  const [isOrganizing, setIsOrganizing] = useState(false);

  useEffect(() => {
    const handleTask = (event: Event) => {
      const nextTaskFolder = (event as CustomEvent<string>).detail;
      if (!nextTaskFolder) return;
      setTaskFolder(nextTaskFolder);
      setContentType("audio");
      void loadAsrTask(nextTaskFolder);
    };
    window.addEventListener(asrOpenTaskEvent, handleTask);
    return () => window.removeEventListener(asrOpenTaskEvent, handleTask);
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let cancelled = false;
    setHistoryState("loading");
    void queryClient.fetchQuery({
      queryKey: ["asr", "history", outputRoot],
      queryFn: () => asrApi.list(outputRoot, 20),
      staleTime: 1_000,
    })
      .then((tasks) => {
        if (cancelled) return;
        setTaskHistory(tasks);
        setHistoryState("loaded");
      })
      .catch(() => {
        if (!cancelled) setHistoryState("error");
      });
    return () => {
      cancelled = true;
    };
  }, [outputRoot, queryClient]);

  const fusionPrompt = useMemo(() => makeFusionPrompt(taskFolder, contentType), [taskFolder, contentType]);
  const selectedLoadedFile = loadedFiles.find((file) => file.relPath === selectedFileRelPath) ?? loadedFiles[0];
  const taskStatus = asrRecordString(taskView?.taskMeta, "status") || taskView?.job?.status || "idle";
  const preprocessProfile = asrRecordString(taskView?.preprocessMeta, "profile");
  const fusionStatus = asrRecordString(taskView?.fusionMeta, "status");
  const summaryStatus = asrRecordString(taskView?.summaryMeta, "status");
  const summaryTitle = asrRecordString(taskView?.summaryMeta, "title");

  async function copyPrompt(key: string, text: string) {
    await copyToClipboard(text);
    setCopied(key);
    window.setTimeout(() => setCopied(""), 1800);
  }

  async function refreshTaskHistory() {
    if (!isTauriRuntime()) return;
    setHistoryState("loading");
    try {
      setTaskHistory(await asrApi.list(outputRoot, 20));
      setHistoryState("loaded");
    } catch {
      setHistoryState("error");
    }
  }

  async function loadArtifact(view: AsrTaskView, artifact: AsrArtifactSummary) {
    setLoadState("loading");
    setLoadMessage("");
    try {
      const content = await asrApi.readArtifact(view.taskDir, artifact.relativePath, outputRoot);
      const loaded = toWorkspaceLoadedFile(content, artifact);
      setLoadedFiles((current) => [loaded, ...current.filter((file) => file.relPath !== loaded.relPath)]);
      setSelectedFileRelPath(loaded.relPath);
      setLoadState("loaded");
      setLoadMessage(`已读取 ${loaded.label}。`);
    } catch (error) {
      setLoadState("error");
      setLoadMessage(error instanceof Error ? error.message : "读取结果文件失败。");
    }
  }

  async function loadAsrTask(taskDir = taskFolder) {
    const normalized = taskDir.trim();
    if (!normalized) {
      setLoadState("error");
      setLoadMessage("请从最近任务中选择一条，或填写任务包目录。");
      return;
    }
    if (!isTauriRuntime()) {
      setLoadState("error");
      setLoadMessage("浏览器预览不能读取本机任务包；请在 HomeBase 桌面版中查看。");
      return;
    }
    setLoadState("loading");
    setLoadMessage("");
    try {
      const view = await asrApi.get(normalized, outputRoot);
      setTaskView(view);
      setTaskFolder(view.taskDir);
      setLoadedFiles([]);
      const preferred = selectAsrDefaultArtifact(view.artifacts);
      if (preferred) {
        await loadArtifact(view, preferred);
      } else {
        setSelectedFileRelPath("");
        setLoadState("loaded");
        setLoadMessage("任务包已读取，但还没有可查看的结果文件。");
      }
      void refreshTaskHistory();
    } catch (error) {
      setTaskView(null);
      setLoadedFiles([]);
      setSelectedFileRelPath("");
      setLoadState("error");
      setLoadMessage(error instanceof Error ? error.message : "读取 ASR 任务包失败。");
    }
  }

  function selectArtifact(artifact: AsrArtifactSummary) {
    const cached = loadedFiles.find((file) => file.relPath === artifact.relativePath);
    if (cached) {
      setSelectedFileRelPath(cached.relPath);
      return;
    }
    if (taskView) void loadArtifact(taskView, artifact);
  }

  function openMeetingSummary() {
    const artifact = taskView?.artifacts.find((item) => item.relativePath === "final/meeting-summary.md");
    if (artifact) selectArtifact(artifact);
  }

  async function organizeTask() {
    if (!taskView || isOrganizing) return;
    if (
      summaryStatus === "success" &&
      !window.confirm("这会再次调用 OpenRouter 并覆盖 final 下的整理稿，但不会修改各引擎原稿。继续吗？")
    ) {
      return;
    }
    setIsOrganizing(true);
    setLoadMessage("正在调用 OpenRouter 整理融合稿、标题、摘要、决定和待办…");
    try {
      const view = await asrApi.organize({
        taskDir: taskView.taskDir,
        outputRoot,
        fusionMode: "auto",
        runnerPath,
      });
      setTaskView(view);
      const summary = view.artifacts.find((item) => item.relativePath === "final/meeting-summary.md");
      if (summary) await loadArtifact(view, summary);
      setLoadMessage(
        asrRecordString(view.summaryMeta, "status") === "success"
          ? "语义整理完成，标题和会议纪要已写回当前任务包。"
          : "逐字稿已保留，但语义整理未成功；可查看整理状态和错误说明。",
      );
      void refreshTaskHistory();
    } catch (error) {
      setLoadState("error");
      setLoadMessage(error instanceof Error ? error.message : "语义整理失败；各引擎原稿未受影响。");
    } finally {
      setIsOrganizing(false);
    }
  }

  return (
    <section aria-label="结果工作台">
      <details
        className="group rounded-xl border border-border bg-card transition-colors open:border-primary-300/70 dark:open:border-primary-700/70"
        open
      >
        <summary className="flex cursor-pointer list-none items-center gap-3 px-4 py-3">
          <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
            <WandSparkles className="h-4 w-4" aria-hidden />
          </span>
          <div className="min-w-0 flex-1">
            <h2 className="text-sm font-semibold">结果工作台</h2>
            <p className="truncate text-xs text-muted-foreground">
              自动接收已完成任务，直接查看各模型、融合稿、场景检测和会议纪要。
            </p>
          </div>
          <ChevronDown
            className="h-4 w-4 text-muted-foreground transition-transform group-open:rotate-180"
            aria-hidden
          />
        </summary>
        <div className="space-y-5 border-t border-border p-4">
          <div className="grid grid-cols-1 gap-3 md:grid-cols-[minmax(0,1fr)_14rem_auto]">
            <label className="space-y-1.5">
              <span className="text-xs font-medium uppercase tracking-widest text-muted-foreground">任务包目录</span>
              <input
                value={taskFolder}
                onChange={(event) => setTaskFolder(event.target.value)}
                className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm outline-none transition-colors placeholder:text-muted-foreground focus:border-primary-500"
                placeholder="C:\\HomeBase\\Wiki\\raw\\audio\\2026-07-08_task-name"
              />
            </label>
            <label className="space-y-1.5">
              <span className="text-xs font-medium uppercase tracking-widest text-muted-foreground">内容类型</span>
              <select
                value={contentType}
                onChange={(event) => setContentType(event.target.value as "audio" | "ocr" | "document")}
                className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm outline-none transition-colors focus:border-primary-500"
              >
                <option value="audio">语音/录音</option>
                <option value="ocr">图片/OCR</option>
                <option value="document">PDF/文档</option>
              </select>
            </label>
            <button
              type="button"
              onClick={() => void loadAsrTask()}
              className="hb-btn-primary self-end justify-center px-4 py-2"
              disabled={loadState === "loading" || contentType !== "audio"}
            >
              {loadState === "loading" ? (
                <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
              ) : (
                <Database className="h-4 w-4" aria-hidden />
              )}
              读取任务包
            </button>
          </div>

          <div className="rounded-lg border border-border bg-background p-4">
            <div className="mb-3 flex items-center justify-between gap-3">
              <div>
                <h3 className="text-sm font-semibold">最近任务</h3>
                <p className="mt-1 text-xs text-muted-foreground">来自当前输出根目录，不再扫描整个 Wiki。</p>
              </div>
              <button
                type="button"
                onClick={() => void refreshTaskHistory()}
                className="hb-btn-outline shrink-0 px-3 py-1.5"
                disabled={!isTauriRuntime() || historyState === "loading"}
              >
                {historyState === "loading" ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden />
                ) : (
                  <Database className="h-3.5 w-3.5" aria-hidden />
                )}
                刷新
              </button>
            </div>
            {taskHistory.length > 0 ? (
              <div className="grid grid-cols-1 gap-2 md:grid-cols-2 xl:grid-cols-4">
                {taskHistory.slice(0, 8).map((task) => {
                  const name = asrRecordString(task.taskMeta, "task_name") || "未命名录音";
                  const status = asrRecordString(task.taskMeta, "status") || task.job?.status || "unknown";
                  const createdAt = asrRecordString(task.taskMeta, "created_at");
                  return (
                    <button
                      key={task.taskDir}
                      type="button"
                      onClick={() => void loadAsrTask(task.taskDir)}
                      className={cn(
                        "min-w-0 rounded-md border px-3 py-2 text-left transition-colors",
                        taskView?.taskDir === task.taskDir
                          ? "border-primary-400 bg-primary-50 dark:bg-primary-950/30"
                          : "border-border bg-muted hover:border-primary-300/70",
                      )}
                    >
                      <span className="block truncate text-xs font-medium">{name}</span>
                      <span className="mt-1 flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
                        <span>{asrTaskStatusLabel(status)}</span>
                        <span>{formatAsrTaskTime(createdAt)}</span>
                      </span>
                    </button>
                  );
                })}
              </div>
            ) : (
              <p className="rounded-md bg-muted px-3 py-2 text-xs text-muted-foreground">
                {historyState === "loading" ? "正在读取任务索引…" : "当前输出根目录还没有可用任务。"}
              </p>
            )}
          </div>

          {taskView ? (
            <div className="grid grid-cols-2 gap-2 lg:grid-cols-4">
              <AsrResultFact label="任务状态" value={asrTaskStatusLabel(taskStatus)} />
              <AsrResultFact label="场景处理" value={asrPreprocessLabel(preprocessProfile)} />
              <AsrResultFact label="融合状态" value={asrFusionStatusLabel(fusionStatus)} />
              <AsrResultFact label="语义整理" value={asrSummaryStatusLabel(summaryStatus)} />
            </div>
          ) : null}

          <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
            <article className="rounded-lg border border-border bg-background p-4">
              <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
                <WandSparkles className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
                软件自动整理
              </div>
              <p className="text-xs text-muted-foreground">
                自动/大模型模式会在转写结束后直接生成融合逐字稿、标题、摘要、决定和待办。
              </p>
              {summaryStatus === "success" ? (
                <div className="mt-3 grid grid-cols-[minmax(0,1fr)_auto] gap-2">
                  <button type="button" onClick={openMeetingSummary} className="hb-btn-primary min-w-0 justify-center py-2">
                    <FileText className="h-4 w-4" aria-hidden />
                    <span className="truncate">{summaryTitle || "查看会议纪要"}</span>
                  </button>
                  <button
                    type="button"
                    onClick={() => void organizeTask()}
                    className="hb-btn-outline justify-center px-3 py-2"
                    disabled={isOrganizing}
                    title="重新调用 OpenRouter 整理当前任务"
                  >
                    {isOrganizing ? (
                      <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
                    ) : (
                      <Globe className="h-4 w-4" aria-hidden />
                    )}
                    重整
                  </button>
                </div>
              ) : (
                <button
                  type="button"
                  onClick={() => void organizeTask()}
                  className="mt-3 hb-btn-primary w-full justify-center py-2"
                  disabled={!taskView || isOrganizing}
                >
                  {isOrganizing ? (
                    <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
                  ) : (
                    <Globe className="h-4 w-4" aria-hidden />
                  )}
                  {isOrganizing ? "整理中" : "调用 API 生成纪要"}
                </button>
              )}
            </article>
            <article className="rounded-lg border border-border bg-background p-4">
              <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
                <Eye className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
                软件内查看
              </div>
              <p className="text-xs text-muted-foreground">
                任务包由专用 ASR API 读取，只暴露结果白名单，不读取源录音或相邻目录。
              </p>
              <button
                type="button"
                onClick={() => void loadAsrTask()}
                className="mt-3 hb-btn-outline w-full justify-center py-2"
                disabled={loadState === "loading" || !taskFolder}
              >
                <Database className="h-4 w-4" aria-hidden />
                {loadState === "loading" ? "读取中" : "刷新当前任务"}
              </button>
            </article>
            <article className="rounded-lg border border-border bg-background p-4">
              <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
                <Copy className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
                Agent 深度复核
              </div>
              <p className="text-xs text-muted-foreground">
                只有复杂语境或需要专项格式时，才把同一任务包交给 Codex、Claude Code 等继续整理。
              </p>
              <button
                type="button"
                onClick={() => void copyPrompt("codex", fusionPrompt)}
                className="mt-3 hb-btn-outline w-full justify-center py-2"
                disabled={!taskFolder}
              >
                <Copy className="h-4 w-4" aria-hidden />
                {copied === "codex" ? "已复制" : "复制复核提示词"}
              </button>
            </article>
          </div>

          {taskView?.artifacts.length ? (
            <div className="grid grid-cols-1 gap-4 xl:grid-cols-[20rem_minmax(0,1fr)]">
              <div className="rounded-lg border border-border bg-background p-4">
                <div className="mb-3 flex items-center gap-2">
                  <Layers3 className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
                  <h3 className="text-sm font-semibold">任务产物</h3>
                </div>
                <div className="max-h-[32rem] space-y-2 overflow-auto pr-1">
                  {taskView.artifacts.map((artifact) => (
                    <button
                      key={artifact.relativePath}
                      type="button"
                      onClick={() => selectArtifact(artifact)}
                      className={cn(
                        "w-full rounded-md border px-3 py-2 text-left text-xs transition-colors",
                        selectedFileRelPath === artifact.relativePath
                          ? "border-primary-400 bg-primary-50 text-primary-700 dark:bg-primary-950/30 dark:text-primary-300"
                          : "border-border bg-muted text-muted-foreground hover:border-primary-300/70",
                      )}
                    >
                      <span className="flex items-center justify-between gap-2">
                        <span className="min-w-0 truncate font-medium">{asrArtifactLabel(artifact)}</span>
                        <span className="shrink-0 text-[10px]">{Math.max(1, Math.ceil(artifact.sizeBytes / 1024))} KB</span>
                      </span>
                      <span className="mt-1 block break-all font-mono text-[11px]">{artifact.relativePath}</span>
                    </button>
                  ))}
                </div>
              </div>
              <div className="rounded-lg border border-border bg-background p-4">
                <div className="mb-3 flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <h3 className="text-sm font-semibold">{selectedLoadedFile?.label ?? "稿件预览"}</h3>
                    <p className="mt-1 break-all font-mono text-[11px] text-muted-foreground">
                      {selectedLoadedFile?.absolutePath}
                    </p>
                  </div>
                  <span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-[10px] text-muted-foreground">
                    {selectedLoadedFile ? `${Math.ceil(selectedLoadedFile.size / 1024)} KB` : "0 KB"}
                  </span>
                </div>
                <pre className="max-h-96 overflow-auto whitespace-pre-wrap rounded-md bg-muted p-3 font-mono text-[11px] text-muted-foreground">
                  {selectedLoadedFile?.content || "选择左侧产物进行读取。"}
                </pre>
              </div>
            </div>
          ) : (
            <div className="rounded-lg border border-border bg-background p-4 text-xs text-muted-foreground">
              尚未选择任务，或该任务还没有可读产物。
            </div>
          )}

          {loadMessage ? (
            <p
              className={cn(
                "rounded-md px-3 py-2 text-xs",
                loadState === "error"
                  ? "bg-destructive/10 text-destructive-strong"
                  : "bg-muted text-muted-foreground",
              )}
            >
              {loadMessage}
            </p>
          ) : null}

          <div className="rounded-lg border border-border bg-background p-4">
            <div className="mb-3 flex items-center justify-between gap-3">
              <div>
                <h3 className="text-sm font-semibold">提示词预览</h3>
                <p className="mt-1 text-xs text-muted-foreground">复制后直接给 Codex/Claude Code 使用。</p>
              </div>
              <button
                type="button"
                onClick={() => void copyPrompt("preview", fusionPrompt)}
                className="hb-btn-outline shrink-0 py-2"
              >
                <Copy className="h-4 w-4" aria-hidden />
                {copied === "preview" ? "已复制" : "复制"}
              </button>
            </div>
            <pre className="max-h-72 overflow-auto whitespace-pre-wrap rounded-md bg-muted p-3 font-mono text-[11px] text-muted-foreground">
              {fusionPrompt}
            </pre>
          </div>
        </div>
      </details>
    </section>
  );
}

function makeAsrTaskFolder(taskName: string, sourcePath: string, outputRoot = "C:\\HomeBase\\Wiki\\raw\\audio") {
  const now = new Date();
  const yyyy = now.getFullYear();
  const mm = String(now.getMonth() + 1).padStart(2, "0");
  const dd = String(now.getDate()).padStart(2, "0");
  const date = `${yyyy}-${mm}-${dd}`;
  const rawName =
    taskName.trim() ||
    sourcePath
      .split(/[\\/]/)
      .pop()
      ?.replace(/\.[^.]+$/, "") ||
    "asr-auto";
  const safeName = rawName
    // Windows filenames cannot contain ASCII control characters.
    // eslint-disable-next-line no-control-regex
    .replace(/[<>:"/\\|?*\u0000-\u001F]/g, "-")
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 48);
  return `${normalizeWindowsPath(outputRoot).replace(/\\+$/, "")}\\${date}_${safeName || "asr-auto"}`;
}

function asrPortableCustomPresetToPreset(preset: AsrPortableCustomPreset): AsrPreset {
  const knownTracks = new Set(
    preset.engine_ids
      .map((engineId) => asrEngineOptions.find((option) => option.id === engineId)?.track)
      .filter((track): track is "local" | "cloud" => Boolean(track)),
  );
  const hasUnknownEngine = preset.engine_ids.some(
    (engineId) => !asrEngineOptions.some((option) => option.id === engineId),
  );
  const track: AsrPreset["track"] =
    hasUnknownEngine || knownTracks.size > 1
      ? "hybrid"
      : knownTracks.has("cloud")
        ? "cloud"
        : "local";
  const outputPolicy = {
    auto: "keep_all_auto_merge",
    llm: "keep_all_llm_merge",
    manual: "keep_all_manual_compare",
  } satisfies Record<AsrPortableCustomPreset["fusion_mode"], string>;

  return {
    id: preset.id,
    name: preset.name,
    trait: `${preset.engine_ids.length} 路${track === "local" ? "本地" : track === "cloud" ? "云端" : "混合"}`,
    detail: preset.description || "按自定义引擎组合执行，并保留每一路原始结果。",
    engines: preset.engine_ids.map(asrEngineLabel),
    engineIds: [...preset.engine_ids],
    track,
    speed: "取决于最慢引擎；各路独立留存",
    outputPolicy: outputPolicy[preset.fusion_mode],
    bestFor: preset.description || "临时专项场景和固定模型组合。",
    status: track === "cloud" ? "api" : "candidate",
  };
}

function createCustomAsrPresetId(name: string, existingIds: string[]) {
  const slug = name
    .normalize("NFKD")
    .toLocaleLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 32);
  const seed = slug.length >= 3 ? slug : Date.now().toString(36);
  const base = `custom_${seed}`;
  const existing = new Set(existingIds);
  if (!existing.has(base)) return base;
  for (let index = 2; index < 1000; index += 1) {
    const candidate = `${base.slice(0, 44)}_${index}`;
    if (!existing.has(candidate)) return candidate;
  }
  return `custom_${Date.now().toString(36)}`;
}

function asrTrackLabel(track: AsrPreset["track"]) {
  const labels = {
    local: "本地",
    cloud: "云端",
    hybrid: "本地 + 云端",
  } satisfies Record<AsrPreset["track"], string>;
  return labels[track];
}

async function copyToClipboard(text: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "true");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand("copy");
  document.body.removeChild(textarea);
}

function normalizeWindowsPath(path: string) {
  return path.trim().replace(/\//g, "\\");
}

function isSystemDrivePath(path: string) {
  return /^c:\\/i.test(normalizeWindowsPath(path));
}

function joinWindowsPath(root: string, relative: string) {
  const normalizedRelative = normalizeWindowsPath(relative);
  if (/^[A-Za-z]:\\/.test(normalizedRelative) || normalizedRelative.startsWith("\\\\")) {
    return normalizedRelative;
  }
  return `${normalizeWindowsPath(root).replace(/\\+$/, "")}\\${normalizedRelative.replace(/^\\+/, "")}`;
}

function modelDownloadStatusLabel(status: string) {
  return (
    {
      queued: "等待下载",
      running: "正在下载和校验",
      completed: "下载与最小推理验证通过",
      verification_failed: "文件已下载，但运行验证未通过",
      failed: "下载失败",
      cancelled: "下载已取消",
      interrupted: "下载已中断，可重新开始并复用缓存",
    }[status] ?? status
  );
}

function modelDownloadPhaseLabel(phase: string) {
  return (
    {
      queued: "等待后台任务",
      preflight: "检查磁盘、目录和下载来源",
      downloading: "下载模型文件",
      verifying: "校验文件完整性并运行最小推理",
      writing_manifest: "写入已安装清单",
      completed: "安装验证完成",
      verification_failed: "最小推理验证失败",
      failed: "任务失败",
      cancelled: "任务已取消",
      interrupted: "任务已中断",
    }[phase] ?? phase
  );
}

function applyConfiguredModelRoot(machine: MachineProfile, modelRoot: string): MachineProfile {
  const normalizedRoot = normalizeWindowsPath(modelRoot).replace(/\\+$/, "");
  const driveMatch = normalizedRoot.match(/^([A-Za-z]):[\\/]/);
  const drive = driveMatch ? `${driveMatch[1].toUpperCase()}:` : null;
  const disk = drive
    ? machine.disks.find((item) => item.drive.trim().toUpperCase() === drive)
    : undefined;
  const diagnostics = [...machine.diagnostics];

  if (!drive && !/^\\\\[^\\]+\\[^\\]+/.test(normalizedRoot) && !normalizedRoot.startsWith("/")) {
    diagnostics.push("配置的模型目录不是绝对路径。 ");
  } else if (drive && !disk) {
    diagnostics.push(`没有检测到模型目录对应的 ${drive} 磁盘。 `);
  }

  return {
    ...machine,
    preferredModelRoot: normalizedRoot || "AIModels",
    preferredModelDrive: drive,
    preferredFreeGb: disk?.free_gb ?? null,
    preferredDiskWritable: disk?.writable ?? null,
    systemDriveOnly: drive === "C:",
    diagnostics: [...new Set(diagnostics.map((item) => item.trim()))],
  };
}

function useAsrPortableConfig(machine: MachineProfile, distribution?: DistributionInfo) {
  const [config, setConfig] = useState<AsrPortableConfig>(() =>
    loadStoredAsrPortableConfig() ?? createHomeBaseAsrPortableConfig(machine, distribution),
  );

  useEffect(() => {
    const onConfigChange = (event: Event) => {
      const detail = (event as CustomEvent<AsrPortableConfig>).detail;
      if (detail) setConfig(detail);
    };
    window.addEventListener(asrPortableConfigEvent, onConfigChange);
    return () => window.removeEventListener(asrPortableConfigEvent, onConfigChange);
  }, []);

  useEffect(() => {
    const stored = loadStoredAsrPortableConfig();
    if (!stored) {
      setConfig(createHomeBaseAsrPortableConfig(machine, distribution));
      return;
    }
    if (distribution?.mode === "packaged") {
      const migrated = rebindLegacyDistributionRoots(stored, distribution);
      if (JSON.stringify(migrated.roots) !== JSON.stringify(stored.roots)) {
        persistAsrPortableConfig(migrated);
      }
    }
  }, [distribution, machine.preferredModelRoot, machine.source]);

  return config;
}

function usePortableMachineProfile() {
  const queryClient = useQueryClient();
  const distributionQuery = useQuery({
    queryKey: ["distribution", "info"],
    queryFn: () => distributionApi.info(),
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  const [manifestProbePath, setManifestProbePath] = useState(() => {
    const stored = loadStoredAsrPortableConfig();
    return stored
      ? joinWindowsPath(stored.roots.aihub_root, stored.manifest_rel_path)
      : installedModelsManifest;
  });
  const machineQuery = useQuery({
    queryKey: ["model-center", "machine-profile", manifestProbePath],
    queryFn: () => getRuntimeMachineProfile(queryClient, manifestProbePath),
  });
  const machineProfile = machineQuery.data ?? fallbackMachineProfile;
  const distributionInfo = distributionQuery.data;
  const portableConfig = useAsrPortableConfig(machineProfile, distributionInfo);
  const portableManifestPath = joinWindowsPath(
    portableConfig.roots.aihub_root,
    portableConfig.manifest_rel_path,
  );

  useEffect(() => {
    if (portableManifestPath !== manifestProbePath) {
      setManifestProbePath(portableManifestPath);
    }
  }, [manifestProbePath, portableManifestPath]);

  return {
    machineQuery,
    machineProfile,
    portableConfig,
    portableManifestPath,
    distributionInfo,
  };
}

function createHomeBaseAsrPortableConfig(
  machine: MachineProfile,
  distribution?: DistributionInfo,
) {
  const config = createDefaultAsrPortableConfig(machine);
  if (distribution) {
    return {
      ...config,
      roots: {
        aihub_root: distribution.roots.aihubRoot,
        model_root: distribution.roots.modelRoot,
        runtime_root: distribution.roots.runtimeRoot,
        output_root: distribution.roots.outputRoot,
        cache_root: distribution.roots.cacheRoot,
      },
    };
  }
  if (machine.source !== "preview") return config;
  return {
    ...config,
    roots: {
      aihub_root: "C:\\HomeBase\\AIHub",
      model_root: "C:\\HomeBase\\Models",
      runtime_root: "C:\\HomeBase\\Runtimes",
      output_root: "C:\\HomeBase\\Wiki\\raw\\audio",
      cache_root: "C:\\HomeBase\\Cache\\homebase-model-cache",
    },
  };
}

function rebindLegacyDistributionRoots(
  config: AsrPortableConfig,
  distribution: DistributionInfo,
): AsrPortableConfig {
  const legacy: AsrPortableRoots = {
    aihub_root: "C:\\HomeBase\\AIHub",
    model_root: "C:\\HomeBase\\Models",
    runtime_root: "C:\\HomeBase\\Runtimes",
    output_root: "C:\\HomeBase\\Wiki\\raw\\audio",
    cache_root: "C:\\HomeBase\\Cache\\homebase-model-cache",
  };
  const packaged: AsrPortableRoots = {
    aihub_root: distribution.roots.aihubRoot,
    model_root: distribution.roots.modelRoot,
    runtime_root: distribution.roots.runtimeRoot,
    output_root: distribution.roots.outputRoot,
    cache_root: distribution.roots.cacheRoot,
  };
  const roots = { ...config.roots };
  for (const key of Object.keys(legacy) as Array<keyof AsrPortableRoots>) {
    if (normalizeWindowsPath(roots[key]).toLowerCase() === legacy[key].toLowerCase()) {
      roots[key] = packaged[key];
    }
  }
  return { ...config, roots };
}

function loadStoredAsrPortableConfig() {
  try {
    const stored = window.localStorage.getItem(asrPortableStorageKey);
    return stored ? importAsrPortableBundle(JSON.parse(stored) as unknown) : null;
  } catch {
    return null;
  }
}

function persistAsrPortableConfig(config: AsrPortableConfig) {
  try {
    window.localStorage.setItem(
      asrPortableStorageKey,
      JSON.stringify(exportAsrPortableBundle(config)),
    );
  } catch {
    // Storage can be disabled; export/import remains available.
  }
  window.dispatchEvent(new CustomEvent(asrPortableConfigEvent, { detail: config }));
}

function toWorkspaceLoadedFile(
  content: AsrArtifactContent,
  artifact: AsrArtifactSummary,
): WorkspaceLoadedFile {
  return {
    label: asrArtifactLabel(artifact),
    relPath: content.relativePath,
    absolutePath: content.absolutePath,
    size: content.sizeBytes,
    content: content.content,
    category: artifact.category,
    engineId: artifact.engineId,
  };
}

function selectAsrDefaultArtifact(artifacts: AsrArtifactSummary[]) {
  const priorities = [
    "final/transcript.md",
    "final/multi-engine-fused-transcript.md",
    "final/meeting-summary.md",
    "final/original-scene-reconstruction.md",
    "compare/alignment-notes.md",
  ];
  for (const relativePath of priorities) {
    const artifact = artifacts.find((item) => item.relativePath === relativePath);
    if (artifact) return artifact;
  }
  return artifacts.find((item) => item.relativePath.endsWith("/transcript.md")) ?? artifacts[0];
}

function asrArtifactLabel(artifact: AsrArtifactSummary) {
  const labels: Record<string, string> = {
    "preprocess/audio-profile.json": "原始音频画像",
    "preprocess/preprocessing.meta.json": "自适应预处理记录",
    "compare/run-matrix.json": "多引擎运行矩阵",
    "compare/alignment-notes.md": "多模型差异说明",
    "compare/fusion.meta.json": "融合执行记录",
    "final/transcript.md": "最终逐字稿",
    "final/multi-engine-fused-transcript.md": "多模型融合稿",
    "final/original-scene-reconstruction.md": "原始场景还原稿",
    "final/meeting-summary.md": "会议纪要",
    "final/title-summary.json": "标题与纪要结构",
    "exports/transcript.txt": "纯文本导出",
    "exports/transcript.md": "Markdown 导出",
    "exports/transcript.srt": "SRT 字幕",
    "exports/transcript.vtt": "VTT 字幕",
  };
  if (labels[artifact.relativePath]) return labels[artifact.relativePath];
  if (artifact.engineId) {
    const fileName = artifact.relativePath.split("/").pop() ?? artifact.relativePath;
    const runLabels: Record<string, string> = {
      "transcript.md": "逐字稿",
      "transcript.json": "结构化逐字稿",
      "segments.json": "时间片段",
      "speaker-turns.json": "说话人轮次",
      "subtitles.srt": "字幕",
      "run.meta.json": "运行记录",
      "raw.json": "原始响应",
    };
    return `${asrEngineLabel(artifact.engineId)} · ${runLabels[fileName] ?? fileName}`;
  }
  return artifact.relativePath;
}

function asrRecordString(value: Record<string, unknown> | null | undefined, key: string) {
  const result = value?.[key];
  return typeof result === "string" ? result : "";
}

function formatAsrTaskTime(value: string) {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function asrPreprocessLabel(profile: string) {
  if (!profile) return "等待检测";
  return asrScenes.find((item) => item.id === profile)?.label ?? profile;
}

function asrFusionStatusLabel(status: string) {
  return (
    {
      success: "大模型融合完成",
      manual: "保留主稿，人工对比",
      fallback_manual: "融合失败，已保留主稿",
    } satisfies Record<string, string>
  )[status] ?? "等待融合";
}

function asrSummaryStatusLabel(status: string) {
  return (
    {
      success: "标题与纪要已生成",
      not_requested: "手动模式，未调用大模型",
      unavailable: "整理失败，原稿已保留",
    } satisfies Record<string, string>
  )[status] ?? "等待整理";
}

function AsrResultFact({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 rounded-md border border-border bg-background px-3 py-2">
      <div className="text-[10px] uppercase tracking-widest text-muted-foreground">{label}</div>
      <div className="mt-1 truncate text-xs font-medium" title={value}>
        {value}
      </div>
    </div>
  );
}

const fallbackMachineProfile = deriveMachineProfile({
  source: "preview",
  probeCompleteness: "preview",
  info: {
    cpu_brand: "未连接本机诊断",
    cpu_logical_cores: null,
    memory_total_gb: null,
    gpu_name: null,
  },
  gpu: {
    available: false,
    gpu_name: null,
    error: "浏览器预览未执行 GPU 探测。",
    probe_state: "unknown",
  },
  disks: [],
  pathEntries: [],
  manifestState: "unavailable",
  installedModels: [],
});

async function getRuntimeMachineProfile(queryClient: QueryClient, manifestPath = installedModelsManifest): Promise<MachineProfile> {
  if (!isTauriRuntime()) {
    return fallbackMachineProfile;
  }

  const [infoResult, metricsResult, gpuResult, disksResult, pathResult, manifestResult] = await Promise.allSettled([
    queryClient.fetchQuery({ queryKey: ["system", "info"], queryFn: systemApi.info }),
    queryClient.fetchQuery({ queryKey: ["system", "metrics"], queryFn: systemApi.metrics }),
    queryClient.fetchQuery({ queryKey: ["system", "gpu"], queryFn: systemApi.gpu }),
    queryClient.fetchQuery({ queryKey: ["system", "disks"], queryFn: systemApi.disks }),
    queryClient.fetchQuery({ queryKey: ["system", "path-list"], queryFn: systemApi.pathList }),
    modelCenterApi.manifest(manifestPath),
  ]);
  const info = infoResult.status === "fulfilled" ? infoResult.value : null;
  const metrics = metricsResult.status === "fulfilled" ? metricsResult.value : null;
  const gpu = gpuResult.status === "fulfilled"
    ? gpuResult.value
    : {
        available: false,
        gpu_name: info?.gpu_name ?? null,
        gpu_percent: null,
        gpu_warning: false,
        error: "GPU 探测命令失败。",
      };
  const disks = disksResult.status === "fulfilled" ? disksResult.value : [];
  const pathEntries = pathResult.status === "fulfilled" ? pathResult.value : [];
  const manifest = manifestResult.status === "fulfilled" ? manifestResult.value : null;
  const normalizedDisks: RecommenderDiskInput[] = disks.map((disk) => ({
      drive: disk.drive,
      free_gb: disk.free_gb,
      total_gb: disk.total_gb,
      usage_percent: disk.usage_percent,
      warning: disk.warning,
    }));
  const failedProbeCount = [infoResult, metricsResult, gpuResult, disksResult, pathResult, manifestResult].filter(
    (result) => result.status === "rejected",
  ).length;

  return deriveMachineProfile({
    source: "live",
    probeCompleteness: failedProbeCount === 0 ? "complete" : "partial",
    info: {
      cpu_brand: info?.cpu_brand ?? null,
      cpu_logical_cores: info?.cpu_logical_cores ?? metrics?.cpu_cores ?? null,
      memory_total_gb: info?.memory_total_gb ?? metrics?.memory_total_gb ?? null,
      gpu_name: info?.gpu_name ?? null,
    },
    gpu: {
      ...gpu,
      probe_state: gpu.gpu_name ? "available" : gpu.error ? "unknown" : "absent",
    },
    disks: normalizedDisks,
    pathEntries,
    runtimeProbes:
      pathResult.status === "rejected"
        ? {
            python: { state: "unknown", detail: "PATH 探测失败。" },
            ffmpeg: { state: "unknown", detail: "PATH 探测失败。" },
            cuda: { state: "unknown", detail: "PATH 探测失败。" },
          }
        : undefined,
    manifestState: manifest?.manifest_state ?? "unavailable",
    installedModels: manifest?.models ?? [],
  });
}

function isTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function makeFusionPrompt(taskFolder: string, contentType: "audio" | "ocr" | "document") {
  const typeLabel = {
    audio: "语音/录音",
    ocr: "图片/OCR",
    document: "PDF/文档",
  } satisfies Record<typeof contentType, string>;
  const inputHints = {
    audio:
      "重点读取 source/input.meta.json、preprocess/audio-profile.json、runs/*/transcript.md、runs/*/segments.json、runs/*/speaker-turns.json 和 compare/run-matrix.json。",
    ocr:
      "重点读取 runs/*/content.md、runs/*/blocks.json、source/* 和 compare/run-matrix.json，保留版式、表格和低置信度区域。",
    document:
      "重点读取 runs/*/content.md、runs/*/layout.json、runs/*/tables.json、source/* 和 compare/run-matrix.json，保留章节结构、表格、公式和图片引用。",
  } satisfies Record<typeof contentType, string>;

  return `你是 HomeBase 本机多模型结果整理 Agent。

任务包目录：
${taskFolder}

内容类型：
${typeLabel[contentType]}

请读取这个任务包下的所有可用输出，不要只看单一路径。${inputHints[contentType]}

处理目标：
1. 识别每个模型/引擎各自擅长和不擅长的片段。
2. 对明显错字、同音误识别、断句错误和专有名词错误进行校订。
3. 当多个版本冲突时，优先选择上下文一致、语义完整、与时间线相容的结果。
4. 无法确认的内容保留 [疑似: ...] 或 [听不清]，不要编造。
5. 输出一份综合稿，同时保留差异说明，方便人工复盘。

请写入或更新这些文件：
- final/original-scene-reconstruction.md：尽可能还原原始现场和说话顺序。
- final/multi-engine-fused-transcript.md：多模型融合后的可读主稿。
- final/meeting-summary.md：摘要、决策、行动项、风险、待确认问题。
- compare/alignment-notes.md：不同模型的关键差异、取舍依据和需人工确认片段。

输出要求：
- 使用中文。
- 不要删除原始 runs 目录里的任何文件。
- 在文末列出你实际读取过的文件清单。
- 如果任务包缺少必要文件，先说明缺失项，再基于现有文件完成最大可行整理。`;
}

function ModelRecommendationBadge({ status }: { status: ModelRecommendationStatus }) {
  const config = {
    installed: { label: "已验证", cls: "border-success/30 bg-success/10 text-success-strong" },
    recommended: { label: "推荐", cls: "border-success/30 bg-success/10 text-success-strong" },
    optional: { label: "可选", cls: "border-warning/30 bg-warning/10 text-warning-strong" },
    api_preferred: {
      label: "API优先",
      cls: "border-primary-300/40 bg-primary-50 text-primary-700 dark:bg-primary-950/30 dark:text-primary-300",
    },
    needs_review: { label: "待确认", cls: "border-warning/30 bg-warning/10 text-warning-strong" },
    partial_install: { label: "仅探测", cls: "border-warning/30 bg-warning/10 text-warning-strong" },
    missing_runtime: { label: "缺运行时", cls: "border-destructive/30 bg-destructive/10 text-destructive-strong" },
    runtime_mismatch: { label: "版本不兼容", cls: "border-destructive/30 bg-destructive/10 text-destructive-strong" },
    storage_blocked: { label: "目录不可写", cls: "border-destructive/30 bg-destructive/10 text-destructive-strong" },
    space_blocked: { label: "空间不足", cls: "border-destructive/30 bg-destructive/10 text-destructive-strong" },
    unsupported: { label: "不适合", cls: "border-border bg-muted text-muted-foreground" },
  } satisfies Record<ModelRecommendationStatus, { label: string; cls: string }>;

  return (
    <span className={cn("shrink-0 rounded-full border px-2 py-0.5 text-[10px]", config[status].cls)}>
      {config[status].label}
    </span>
  );
}

function recommendationStatusRank(status: ModelRecommendationStatus) {
  const rank = {
    installed: 0,
    recommended: 1,
    optional: 2,
    api_preferred: 3,
    partial_install: 4,
    needs_review: 5,
    missing_runtime: 6,
    runtime_mismatch: 7,
    storage_blocked: 8,
    space_blocked: 9,
    unsupported: 10,
  } satisfies Record<ModelRecommendationStatus, number>;
  return rank[status];
}

function recommendationConfidenceLabel(confidence: "high" | "medium" | "low") {
  return { high: "高 · 事实充分", medium: "中 · 含条件", low: "低 · 需补探测" }[confidence];
}

function canOfferModelDownload(status: ModelRecommendationStatus | undefined) {
  return status != null && ["recommended", "optional", "partial_install"].includes(status);
}

function manifestStateLabel(state: MachineProfile["manifestState"]) {
  return {
    loaded: "清单已读取",
    missing: "清单不存在",
    invalid: "清单损坏",
    unavailable: "清单未接入",
  }[state];
}

function machineGpuLabel(machine: MachineProfile) {
  if (machine.gpuState === "absent") return "无 GPU";
  if (machine.gpuState === "unknown") return "GPU 未确认";
  const kind = machine.gpuKind === "integrated" ? "核显" : machine.gpuKind === "discrete" ? "独显" : "类型未知";
  const vram = machine.inferredVramGb != null
    ? ` · ${Math.round(machine.inferredVramGb * 10) / 10}GB ${machine.vramConfidence === "inferred" ? "估算显存" : "显存"}`
    : " · 显存未知";
  return `${machine.gpuLabel ?? "GPU"} · ${kind}${vram}`;
}

function portabilityStatusLabel(status: "ready" | "needs_attention" | "blocked") {
  return { ready: "可迁移", needs_attention: "需确认", blocked: "有阻断" }[status];
}

function AsrPresetBadge({ status }: { status: AsrPresetRuntimeState }) {
  const config = {
    ready: { label: "可运行", cls: "border-success/30 bg-success/10 text-success-strong" },
    environment_check: { label: "检查环境", cls: "border-warning/30 bg-warning/10 text-warning-strong" },
    needs_setup: { label: "需安装", cls: "border-warning/30 bg-warning/10 text-warning-strong" },
    api_check: {
      label: "检查 API",
      cls: "border-primary-300/40 bg-primary-50 text-primary-700 dark:bg-primary-950/30 dark:text-primary-300",
    },
    conditional: { label: "条件模式", cls: "border-warning/30 bg-warning/10 text-warning-strong" },
    unavailable: { label: "不可用", cls: "border-destructive/30 bg-destructive/10 text-destructive-strong" },
  } satisfies Record<AsrPresetRuntimeState, { label: string; cls: string }>;

  return <span className={cn("shrink-0 rounded-full border px-2 py-0.5 text-[10px]", config[status].cls)}>{config[status].label}</span>;
}

function deriveAsrPresetRuntimeState(
  preset: AsrPreset,
  recommendationById: Map<string, ModelRecommendation>,
  machine: MachineProfile,
): AsrPresetRuntimeState {
  if (preset.track === "cloud") return "api_check";
  if (machine.source === "preview" || machine.probeCompleteness === "failed") {
    return "environment_check";
  }

  const localModelIdByEngine: Record<string, string> = {
    faster_whisper_large_v3_cpu: "faster-whisper-large-v3",
    sensevoice_small: "sensevoice-small",
  };
  const recommendations = preset.engineIds
    .map((engineId) => localModelIdByEngine[engineId])
    .filter((id): id is string => Boolean(id))
    .map((id) => recommendationById.get(id))
    .filter((item): item is ModelRecommendation => Boolean(item));
  const hasVerifiedLocal = recommendations.some(
    (item) => item.recommendation_status === "installed",
  );
  const hasSetupPath = recommendations.some((item) =>
    [
      "recommended",
      "optional",
      "partial_install",
      "missing_runtime",
      "runtime_mismatch",
      "needs_review",
    ].includes(item.recommendation_status),
  );
  const allBlocked =
    recommendations.length > 0 &&
    recommendations.every((item) =>
      ["storage_blocked", "space_blocked", "unsupported"].includes(
        item.recommendation_status,
      ),
    );

  if (preset.track === "local") {
    if (hasVerifiedLocal) return "ready";
    if (allBlocked) return "unavailable";
    return hasSetupPath ? "needs_setup" : "environment_check";
  }
  if (hasVerifiedLocal) return "conditional";
  return allBlocked ? "api_check" : "conditional";
}

function asrPresetRuntimeLabel(status: AsrPresetRuntimeState) {
  return {
    ready: "本地链路已通过验证",
    environment_check: "先完成本机环境探测",
    needs_setup: "需要完整安装并做最小推理测试",
    api_check: "需要检查密钥、额度和服务状态",
    conditional: "需要本地安装或 API 健康检查",
    unavailable: "当前没有可执行路线",
  }[status];
}

function asrRunFromJob(job: AsrJobRecord): AsrUiRun {
  return {
    jobId: job.jobId,
    taskDir: job.taskDir,
    status: job.status,
    activeEngine: null,
    message: job.error || asrTaskStatusLabel(job.status),
  };
}

function asrTaskStatusLabel(status: string) {
  return (
    {
      idle: "尚未运行",
      queued: "等待执行",
      preflight: "检查环境",
      preprocessing: "处理音频",
      running: "正在转写",
      fusing: "整理多引擎结果",
      success: "全部完成",
      partial: "部分完成",
      failed: "执行失败",
      cancelled: "已取消",
      interrupted: "运行中断",
    }[status] ?? status
  );
}

function asrEngineLabel(engineId: string) {
  return (
    {
      faster_whisper_large_v3_cpu: "faster-whisper large-v3",
      sensevoice_small: "SenseVoice Small",
      openai_gpt_4o_transcribe_diarize: "OpenAI 转写 + 说话人",
      deepgram_nova_3: "Deepgram Nova-3",
      assemblyai_universal: "AssemblyAI Universal",
      elevenlabs_scribe: "ElevenLabs Scribe",
    }[engineId] ?? engineId
  );
}

function asrEngineStatusLabel(status: string) {
  return (
    {
      pending: "等待",
      running: "运行中",
      success: "完成",
      failed: "失败",
      skipped: "跳过",
      blocked: "阻断",
      cancelled: "取消",
    }[status] ?? status
  );
}

function asrProviderHealthLabel(status: string) {
  return (
    {
      valid: "可连接",
      unauthorized: "密钥无效",
      forbidden: "无权限",
      not_found: "服务不可用",
      network_error: "网络失败",
      no_check: "仅已配置",
      unknown: "状态未知",
    }[status] ?? status
  );
}

function AsrLiveStatusBadge({ status }: { status: string }) {
  const active = activeAsrStates.has(status);
  const className =
    status === "success"
      ? "border-success/30 bg-success/10 text-success-strong"
      : status === "partial" || active
        ? "border-warning/30 bg-warning/10 text-warning-strong"
        : status === "cancelled" || status === "interrupted"
          ? "border-border bg-muted text-muted-foreground"
          : "border-destructive/30 bg-destructive/10 text-destructive-strong";
  return (
    <span className={cn("shrink-0 rounded-full border px-2 py-0.5 text-[10px]", className)}>
      {asrTaskStatusLabel(status)}
    </span>
  );
}

function DetailRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-start justify-between gap-3">
      <dt className="shrink-0 text-muted-foreground">{label}</dt>
      <dd className="min-w-0 break-words text-right font-mono text-foreground">{value}</dd>
    </div>
  );
}

function ToolGroupPanel({ group }: { group: ToolGroup }) {
  return (
    <details
      className="group rounded-xl border border-border bg-card transition-colors open:border-primary-300/70 dark:open:border-primary-700/70"
      open={group.defaultOpen}
    >
      <summary className="flex cursor-pointer list-none items-center gap-3 px-4 py-3">
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
          {group.icon}
        </span>
        <div className="min-w-0 flex-1">
          <h3 className="text-sm font-semibold">{group.title}</h3>
          <p className="truncate text-xs text-muted-foreground">{group.description}</p>
        </div>
        <ChevronDown
          className="h-4 w-4 text-muted-foreground transition-transform group-open:rotate-180"
          aria-hidden
        />
      </summary>
      <div className="grid grid-cols-1 gap-3 border-t border-border p-4 md:grid-cols-2 xl:grid-cols-3">
        {group.tools.map((tool) => (
          <ToolCard key={tool.title} tool={tool} />
        ))}
      </div>
    </details>
  );
}

function ToolCard({ tool }: { tool: ToolItem }) {
  const disabled = tool.status === "soon";
  return (
    <article className={cn("rounded-lg border border-border bg-background p-4", disabled && "opacity-75")}>
      <div className="mb-3 flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h4 className="text-sm font-semibold">{tool.title}</h4>
          <p className="mt-1 text-xs text-muted-foreground">{tool.description}</p>
        </div>
        <StatusBadge status={tool.status} />
      </div>
      <div className="mb-4 flex flex-wrap gap-1.5">
        {tool.params.map((param) => (
          <span key={param} className="rounded-md bg-muted px-2 py-1 text-[11px] text-muted-foreground">
            {param}
          </span>
        ))}
      </div>
      <button
        type="button"
        disabled={disabled}
        className={cn(disabled ? "hb-btn-outline opacity-60" : "hb-btn-primary", "w-full justify-center py-2")}
      >
        {disabled ? (
          <>
            <Settings2 className="h-4 w-4" aria-hidden />
            等待接入
          </>
        ) : (
          <>
            <UploadCloud className="h-4 w-4" aria-hidden />
            打开
          </>
        )}
      </button>
    </article>
  );
}

function StatusBadge({ status }: { status: ToolItem["status"] }) {
  const config = {
    ready: { label: "可用", cls: "border-success/30 bg-success/10 text-success-strong" },
    configured: { label: "已配置", cls: "border-warning/30 bg-warning/10 text-warning-strong" },
    soon: { label: "待接入", cls: "border-border bg-muted text-muted-foreground" },
  } satisfies Record<ToolItem["status"], { label: string; cls: string }>;
  return (
    <span className={cn("shrink-0 rounded-full border px-2 py-0.5 text-[10px]", config[status].cls)}>
      {config[status].label}
    </span>
  );
}

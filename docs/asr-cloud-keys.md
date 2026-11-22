---
title: ASR cloud keys
updated: 2026-06-07
---

# ASR Cloud Keys

HomeBase Secrets 面板已注册以下云端 ASR 相关密钥。填入后会写入 `C:\HomeBase\Secrets\secrets.yaml`，由 SOPS 加密保存。

| 服务 | HomeBase short_name | 环境变量名 | 用途 | 申请 / 管理地址 |
|---|---|---|---|---|
| OpenAI | `openai_key` | `OPENAI_API_KEY` | `gpt-4o-transcribe` 等云端转写 | https://platform.openai.com/api-keys |
| Deepgram | `deepgram_key` | `DEEPGRAM_API_KEY` | Nova-3 ASR、实时字幕、说话人分离 | https://console.deepgram.com/ |
| AssemblyAI | `assemblyai_key` | `ASSEMBLYAI_API_KEY` | Universal ASR、diarization、会议类转写 | https://www.assemblyai.com/dashboard/signup |
| ElevenLabs | `elevenlabs_key` | `ELEVENLABS_API_KEY` | Scribe ASR、后续 TTS 能力 | https://elevenlabs.io/app/settings/api-keys |
| Hugging Face | `hf_token` | `HF_TOKEN` | 模型下载加速、pyannote gated model 授权 | https://huggingface.co/settings/tokens |

## Notes

- OpenRouter key 不能替代这些 ASR 专用 API key；OpenRouter 当前用于文本整理、标题、摘要、融合稿。
- `hf_token` 如需用于 pyannote，需要在 Hugging Face 上登录并接受对应 gated model 的使用条款。
- HomeBase 已为这些 key 提供轮换 URL 和基础健康检测；如果服务端接口变更，健康检测可能需要同步更新。

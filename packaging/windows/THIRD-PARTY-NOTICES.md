# Third-Party Notices

HomeBase Windows distribution includes the following redistributable runtimes and tools. Their license files are copied into `distribution/licenses` during packaging.

| Component | Pinned version | Source |
| --- | --- | --- |
| CPython embeddable runtime | 3.13.14 x64 | https://www.python.org/downloads/release/python-31314/ |
| faster-whisper and Python dependencies | Versions in `runtime-requirements.txt` and `python-packages.txt` | https://pypi.org/project/faster-whisper/ |
| FFmpeg essentials build | 8.1.1 | https://www.gyan.dev/ffmpeg/builds/ |
| Mozilla SOPS | 3.12.2 | https://github.com/getsops/sops |
| age-keygen | 1.3.1 | https://github.com/FiloSottile/age |
| silk-wasm (MIT), local WeChat audio decoder | 3.7.1 | https://github.com/idranme/silk-wasm |
| Gemini CLI (Apache-2.0), HomeBase-owned ACP runtime | 0.68.0, optional dependencies in native-agent-runtime/package-lock.json | https://github.com/google-gemini/gemini-cli |

Model weights, API credentials, recordings, transcripts, and user Wiki content are not included in the distribution.

Claude Code and Codex native adapters use separately installed official clients. Their binaries and subscription accounts are not included or migrated by the native Agent runtime package. Install scripts for the pinned Gemini dependency tree are disabled.

from __future__ import annotations

import importlib.util
import json
import math
import os
import struct
import subprocess
import sys
import tempfile
import time
import unittest
import wave
from pathlib import Path


SERVER = Path(r"C:\HomeBase\AIHub\mcp\servers\local-asr\server.py")
REGISTRATION = Path(r"C:\HomeBase\AIHub\mcp\servers.d\local-asr.json")


def load_server():
    spec = importlib.util.spec_from_file_location("homebase_local_asr_mcp", SERVER)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Cannot load {SERVER}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_tone(path: Path) -> None:
    sample_rate = 16_000
    frames = []
    for index in range(int(sample_rate * 0.2)):
        sample = int(32767 * 0.15 * math.sin(2 * math.pi * 440 * index / sample_rate))
        frames.append(struct.pack("<h", sample))
    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(sample_rate)
        handle.writeframes(b"".join(frames))


class LocalAsrMcpContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.server = load_server()

    def test_protocol_initializes_and_lists_canonical_tools(self) -> None:
        requests = "\n".join(
            [
                json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
                json.dumps({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
            ]
        ) + "\n"
        completed = subprocess.run(
            [sys.executable, str(SERVER)],
            input=requests,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=15,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        responses = [json.loads(line) for line in completed.stdout.splitlines() if line.strip()]
        self.assertEqual(responses[0]["result"]["protocolVersion"], "2025-06-18")
        names = {tool["name"] for tool in responses[1]["result"]["tools"]}
        self.assertTrue(
            {
                "plan_transcription",
                "transcribe_audio",
                "list_asr_tasks",
                "get_asr_result",
                "cancel_asr_task",
                "read_asr_artifact",
                "build_fusion_prompt",
                "organize_asr_result",
            }.issubset(names)
        )

    def test_registration_tool_list_matches_live_server(self) -> None:
        registered = set(json.loads(REGISTRATION.read_text(encoding="utf-8"))["tools"])
        live = {tool["name"] for tool in self.server.tools_catalog()}
        self.assertEqual(registered, live)

    def test_fixture_transcription_uses_canonical_task_contract(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "fixture.wav"
            output = root / "tasks"
            write_tone(source)
            started = self.server.start_transcription(
                {
                    "audio_path": str(source),
                    "output_root": str(output),
                    "preset_id": "asr_fixture",
                    "engine_ids": ["fixture_success", "fixture_failure"],
                    "task_name": "MCP fixture",
                    "fusion_mode": "manual",
                    "fixture_mode": True,
                    "allow_system_drive": True,
                }
            )
            deadline = time.time() + 15
            result = None
            while time.time() < deadline:
                result = self.server.get_job(started["job_id"], output)
                if result["status"] not in {"queued", "running"}:
                    break
                time.sleep(0.1)
            self.assertIsNotNone(result)
            self.assertEqual(result["status"], "partial")
            task_dir = Path(started["task_dir"])
            self.assertTrue((task_dir / "task.meta.json").exists())
            self.assertTrue((task_dir / "source" / source.name).exists())
            self.assertTrue((task_dir / "compare" / "run-matrix.json").exists())
            self.assertTrue((task_dir / "final" / "transcript.md").exists())

            organized = self.server.organize_result(
                {
                    "task_dir": str(task_dir),
                    "output_root": str(output),
                    "fusion_mode": "manual",
                }
            )
            self.assertEqual(organized["task_meta"]["status"], "partial")
            summary = json.loads((task_dir / "final" / "title-summary.json").read_text(encoding="utf-8"))
            self.assertEqual(summary["status"], "not_requested")

    def test_result_reader_rejects_path_escape(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ValueError, "artifact"):
                self.server.read_artifact(Path(tmp), "../outside.txt", 1000)

    def test_agent_result_access_is_bound_to_output_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "tasks"
            outside = root / "outside-task"
            (outside / "final").mkdir(parents=True)
            (outside / "task.meta.json").write_text("{}", encoding="utf-8")
            (outside / "compare").mkdir()
            (outside / "compare" / "run-matrix.json").write_text("{}", encoding="utf-8")
            (outside / "final" / "transcript.md").write_text("private", encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "output root"):
                self.server.handle_tool(
                    "read_asr_artifact",
                    {
                        "task_dir": str(outside),
                        "output_root": str(output),
                        "artifact": "transcript",
                    },
                )


if __name__ == "__main__":
    unittest.main()

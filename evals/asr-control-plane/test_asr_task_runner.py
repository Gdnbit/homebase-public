from __future__ import annotations

import importlib.util
import json
import math
import struct
import subprocess
import sys
import tempfile
import unittest
import wave
from pathlib import Path


RUNNER = Path(r"C:\HomeBase\AIHub\tools\local-asr\runner\asr_task_runner.py")


def load_runner():
    spec = importlib.util.spec_from_file_location("asr_task_runner", RUNNER)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Cannot load {RUNNER}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_tone(path: Path, amplitude: float = 0.2, seconds: float = 0.25) -> None:
    sample_rate = 16_000
    frames = []
    for index in range(int(sample_rate * seconds)):
        sample = int(32767 * amplitude * math.sin(2 * math.pi * 440 * index / sample_rate))
        frames.append(struct.pack("<h", sample))
    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(sample_rate)
        handle.writeframes(b"".join(frames))


class RunnerContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.runner = load_runner()

    def test_task_name_preserves_chinese_and_removes_windows_illegal_characters(self) -> None:
        value = self.runner.sanitize_task_name('采购:会议/复盘?  "A"')
        self.assertEqual(value, "采购-会议-复盘-A")

    def test_system_drive_output_is_blocked_by_default(self) -> None:
        with self.assertRaisesRegex(ValueError, "system drive"):
            self.runner.validate_output_root(Path(r"C:\HomeBase-ASR"), allow_system_drive=False)

    def test_low_volume_wav_selects_low_volume_profile(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "quiet.wav"
            write_tone(source, amplitude=0.005)
            profile = self.runner.analyze_wav(source)
            selected = self.runner.select_preprocess_profile(profile, "auto")
            self.assertEqual(selected, "low_volume")

    def test_atomic_json_replaces_state_without_temp_residue(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "state.json"
            self.runner.atomic_write_json(path, {"revision": 1})
            self.runner.atomic_write_json(path, {"revision": 2})
            self.assertEqual(json.loads(path.read_text(encoding="utf-8"))["revision"], 2)
            self.assertEqual(list(path.parent.glob("*.tmp")), [])

    def test_custom_preset_accepts_explicit_engine_combination(self) -> None:
        catalog = self.runner.load_presets()
        preset = self.runner.select_preset(catalog, "custom_private_review")
        engines = self.runner.parse_engines(
            "faster_whisper_large_v3_cpu,sensevoice_small,faster_whisper_large_v3_cpu",
            preset,
        )
        self.assertEqual(preset["id"], "custom_private_review")
        self.assertEqual(
            engines,
            ["faster_whisper_large_v3_cpu", "sensevoice_small"],
        )

    def test_unknown_non_custom_preset_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "unknown preset"):
            self.runner.select_preset(self.runner.load_presets(), "removed_vendor_default")

    def test_model_snapshot_requires_config_and_weights(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            incomplete = root / "snapshots" / "incomplete"
            complete = root / "snapshots" / "complete"
            incomplete.mkdir(parents=True)
            complete.mkdir(parents=True)
            (incomplete / "config.yaml").write_text("model: test", encoding="utf-8")
            (complete / "config.yaml").write_text("model: test", encoding="utf-8")
            (complete / "model.pt").write_bytes(b"weights")
            self.assertEqual(
                self.runner.find_model_snapshot("HOMEBASE_TEST_SNAPSHOT", (root / "snapshots",)),
                complete,
            )

    def test_subprocess_json_parser_ignores_runtime_banner(self) -> None:
        payload = self.runner.parse_last_json_object(
            'funasr version: 1.3.9.\n{"status":"success","text":"ok"}\n'
        )
        self.assertEqual(payload["text"], "ok")
        with self.assertRaisesRegex(ValueError, "no JSON"):
            self.runner.parse_last_json_object("runtime warning only")

    def test_fixture_run_keeps_success_when_sibling_engine_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.wav"
            write_tone(source)
            task_dir = root / "tasks" / "2026-07-16_fixture_job"
            command = [
                sys.executable,
                str(RUNNER),
                "run",
                "--source",
                str(source),
                "--task-dir",
                str(task_dir),
                "--output-root",
                str(root / "tasks"),
                "--task-name",
                "fixture",
                "--preset-id",
                "asr_fixture",
                "--engines",
                "fixture_success,fixture_failure",
                "--fusion-mode",
                "manual",
                "--fixture-mode",
                "--allow-system-drive",
            ]
            completed = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", timeout=30)
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            task_meta = json.loads((task_dir / "task.meta.json").read_text(encoding="utf-8"))
            matrix = json.loads((task_dir / "compare" / "run-matrix.json").read_text(encoding="utf-8"))
            self.assertEqual(task_meta["status"], "partial")
            self.assertEqual({run["status"] for run in matrix["runs"]}, {"success", "failed"})
            self.assertTrue((task_dir / "source" / source.name).exists())
            self.assertTrue((task_dir / "preprocess" / "audio-profile.json").exists())
            self.assertTrue((task_dir / "final" / "transcript.md").exists())
            self.assertTrue((task_dir / "final" / "original-scene-reconstruction.md").exists())
            self.assertTrue((task_dir / "final" / "meeting-summary.md").exists())
            self.assertNotIn("secret", json.dumps(task_meta).lower())

    def test_auto_fusion_writes_structured_title_and_meeting_summary(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            task_dir = Path(tmp) / "task"
            for relative in ("runs/engine_a", "runs/engine_b", "compare", "final", "exports"):
                (task_dir / relative).mkdir(parents=True, exist_ok=True)
            (task_dir / "runs" / "engine_a" / "transcript.md").write_text(
                "明天九点提交报价单。", encoding="utf-8"
            )
            (task_dir / "runs" / "engine_b" / "transcript.md").write_text(
                "明天九点前提交报价单。", encoding="utf-8"
            )
            task_meta = {
                "task_id": "task-1",
                "task_name": "报价会议",
                "created_at": "2026-07-20T00:00:00+08:00",
            }
            runs = [
                {"engine_id": "engine_a", "status": "success", "characters": 10},
                {"engine_id": "engine_b", "status": "success", "characters": 11},
            ]
            calls: list[str] = []

            def fake_llm(prompt: str, model: str, system_prompt: str, max_tokens: int) -> str:
                del model, max_tokens
                calls.append(system_prompt)
                if "结构化会议整理" in system_prompt:
                    return json.dumps(
                        {
                            "title": "报价单提交安排",
                            "summary": "会议确认明天九点前提交报价单。",
                            "topics": ["报价单"],
                            "decisions": ["明天九点前提交"],
                            "action_items": [
                                {"owner": "未明确", "task": "提交报价单", "due": "明天九点前"}
                            ],
                            "risks": [],
                            "uncertainties": ["未明确负责人"],
                        },
                        ensure_ascii=False,
                    )
                return "明天九点前提交报价单。"

            status, _ = self.runner.finalize_outputs(
                task_dir,
                task_meta,
                runs,
                "auto",
                "fixture/model",
                llm_client=fake_llm,
            )

            title_summary = json.loads(
                (task_dir / "final" / "title-summary.json").read_text(encoding="utf-8")
            )
            meeting_summary = (task_dir / "final" / "meeting-summary.md").read_text(encoding="utf-8")
            self.assertEqual(status, "success")
            self.assertEqual(title_summary["status"], "success")
            self.assertEqual(title_summary["title"], "报价单提交安排")
            self.assertIn("提交报价单", meeting_summary)
            self.assertIn("待办", meeting_summary)
            self.assertEqual(len(calls), 2)

    def test_manual_fusion_marks_summary_as_not_requested(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            task_dir = Path(tmp) / "task"
            for relative in ("runs/engine_a", "compare", "final", "exports"):
                (task_dir / relative).mkdir(parents=True, exist_ok=True)
            (task_dir / "runs" / "engine_a" / "transcript.md").write_text(
                "只保留人工核对稿。", encoding="utf-8"
            )
            task_meta = {
                "task_id": "task-2",
                "task_name": "人工核对",
                "created_at": "2026-07-20T00:00:00+08:00",
            }

            def unexpected_llm(*_args, **_kwargs):
                raise AssertionError("manual mode must not call the LLM")

            self.runner.finalize_outputs(
                task_dir,
                task_meta,
                [{"engine_id": "engine_a", "status": "success", "characters": 9}],
                "manual",
                "fixture/model",
                llm_client=unexpected_llm,
            )

            title_summary = json.loads(
                (task_dir / "final" / "title-summary.json").read_text(encoding="utf-8")
            )
            meeting_summary = (task_dir / "final" / "meeting-summary.md").read_text(encoding="utf-8")
            self.assertEqual(title_summary["status"], "not_requested")
            self.assertIn("未调用大模型", meeting_summary)

    def test_extract_json_object_accepts_markdown_code_fence(self) -> None:
        payload = self.runner.extract_json_object('```json\n{"title":"会议"}\n```')
        self.assertEqual(payload["title"], "会议")

    def test_existing_task_can_be_organized_without_rerunning_engines(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            task_dir = Path(tmp) / "task"
            for relative in ("runs/engine_a", "compare", "final", "exports"):
                (task_dir / relative).mkdir(parents=True, exist_ok=True)
            original_transcript = "保留这份引擎原稿。"
            poisoned_output_root = Path(tmp) / "poisoned-output"
            transcript_path = task_dir / "runs" / "engine_a" / "transcript.md"
            transcript_path.write_text(original_transcript, encoding="utf-8")
            task_meta = {
                "task_id": "task-existing",
                "task_name": "现有任务",
                "task_dir": str(task_dir),
                "output_root": str(poisoned_output_root),
                "status": "success",
                "revision": 1,
                "created_at": "2026-07-20T00:00:00+08:00",
                "engine_ids": ["engine_a"],
                "source": {"source_path": "fixture.wav"},
            }
            (task_dir / "task.meta.json").write_text(
                json.dumps(task_meta, ensure_ascii=False), encoding="utf-8"
            )
            (task_dir / "compare" / "run-matrix.json").write_text(
                json.dumps(
                    {
                        "task_id": "task-existing",
                        "runs": [{"engine_id": "engine_a", "status": "success", "characters": 10}],
                    }
                ),
                encoding="utf-8",
            )

            def fake_llm(prompt: str, model: str, system_prompt: str, max_tokens: int) -> str:
                del prompt, model, system_prompt, max_tokens
                return json.dumps(
                    {
                        "title": "现有任务整理",
                        "summary": "已整理。",
                        "topics": [],
                        "decisions": [],
                        "action_items": [],
                        "risks": [],
                        "uncertainties": [],
                    },
                    ensure_ascii=False,
                )

            result = self.runner.organize_existing_task(
                task_dir,
                "auto",
                "fixture/model",
                llm_client=fake_llm,
            )

            self.assertEqual(result["status"], "success")
            self.assertEqual(transcript_path.read_text(encoding="utf-8"), original_transcript)
            self.assertTrue((task_dir / "final" / "meeting-summary.md").is_file())
            self.assertFalse(poisoned_output_root.exists())
            self.assertTrue((task_dir.parent / "_index" / "tasks.jsonl").is_file())
            updated_meta = json.loads((task_dir / "task.meta.json").read_text(encoding="utf-8"))
            self.assertEqual(updated_meta["status"], "success")

    def test_missing_source_returns_structured_failure(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            command = [
                sys.executable,
                str(RUNNER),
                "run",
                "--source",
                str(root / "missing.wav"),
                "--output-root",
                str(root / "tasks"),
                "--engines",
                "fixture_success",
                "--fixture-mode",
                "--allow-system-drive",
            ]
            completed = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", timeout=10)
            self.assertNotEqual(completed.returncode, 0)
            payload = json.loads(completed.stdout.strip().splitlines()[-1])
            self.assertEqual(payload["event"], "failed")
            self.assertEqual(payload["error_code"], "source_missing")


if __name__ == "__main__":
    unittest.main()

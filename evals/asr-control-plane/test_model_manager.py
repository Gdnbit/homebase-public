from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


MANAGER = Path(r"C:\HomeBase\AIHub\tools\model-manager\model_manager.py")


def load_manager():
    spec = importlib.util.spec_from_file_location("homebase_model_manager", MANAGER)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Cannot load {MANAGER}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class ModelManagerContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manager = load_manager()

    def test_system_drive_target_is_blocked_by_default(self) -> None:
        with self.assertRaisesRegex(ValueError, "system drive"):
            self.manager.validate_target_dir(Path(r"C:\AIModels\asr\model"), allow_system_drive=False)

    def test_unverified_snapshot_is_not_installed(self) -> None:
        record = self.manager.manifest_record(
            {
                "id": "sample",
                "name": "Sample",
                "category": "asr",
                "source": {"kind": "fixture"},
                "invocation": {},
            },
            mode="snapshot",
            local_path=r"F:\AIModels\sample",
            verification={
                "completeness": {"status": "passed"},
                "smoke_test": {"status": "failed"},
            },
        )
        self.assertNotEqual(record["status"], "installed")

    def test_atomic_job_state_has_no_temp_residue(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "job.json"
            self.manager.atomic_write_json(path, {"revision": 1})
            self.manager.atomic_write_json(path, {"revision": 2})
            self.assertEqual(json.loads(path.read_text(encoding="utf-8"))["revision"], 2)
            self.assertEqual(list(path.parent.glob("*.tmp")), [])

    def test_sensevoice_json_parser_ignores_runtime_banner(self) -> None:
        payload = self.manager.parse_last_json_object(
            'funasr version: 1.3.9.\n{"status":"success","text":"ok"}\n'
        )
        self.assertEqual(payload["status"], "success")

    def test_fixture_download_is_verified_before_manifest_install(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            catalog = root / "catalog.json"
            manifest = root / "installed-models.json"
            jobs = root / "jobs"
            model_root = root / "models"
            catalog.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "models": [
                            {
                                "id": "fixture-asr",
                                "name": "Fixture ASR",
                                "category": "asr",
                                "estimated_size_gb": 0.001,
                                "source": {
                                    "kind": "fixture",
                                    "fixture_files": {
                                        "config.json": "{}",
                                        "model.bin": "fixture-weights",
                                    },
                                },
                                "download": {"requires_allow_large": False},
                                "verification": {
                                    "required_files": ["config.json", "model.bin"],
                                    "smoke_test": {"kind": "fixture"},
                                },
                                "invocation": {},
                            }
                        ],
                    },
                    ensure_ascii=False,
                ),
                encoding="utf-8",
            )
            job_id = "fixture-job"
            command = [
                sys.executable,
                str(MANAGER),
                "download",
                "fixture-asr",
                "--catalog",
                str(catalog),
                "--manifest",
                str(manifest),
                "--jobs-root",
                str(jobs),
                "--model-root",
                str(model_root),
                "--job-id",
                job_id,
                "--fixture-mode",
                "--allow-system-drive",
            ]
            completed = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", timeout=30)
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            state = json.loads((jobs / f"{job_id}.json").read_text(encoding="utf-8"))
            installed = json.loads(manifest.read_text(encoding="utf-8"))["models"]["fixture-asr"]
            self.assertEqual(state["status"], "completed")
            self.assertEqual(state["progress_percent"], 100)
            self.assertEqual(installed["status"], "installed")
            self.assertEqual(installed["verification"]["smoke_test"]["status"], "passed")
            self.assertTrue((model_root / "asr" / "fixture-asr" / "model.bin").exists())

    def test_preexisting_cancel_request_stops_before_writing_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            catalog = root / "catalog.json"
            manifest = root / "installed-models.json"
            jobs = root / "jobs"
            jobs.mkdir()
            (jobs / "cancelled-job.cancel").write_text("cancel", encoding="utf-8")
            catalog.write_text(
                json.dumps(
                    {
                        "models": [
                            {
                                "id": "fixture-asr",
                                "name": "Fixture ASR",
                                "category": "asr",
                                "estimated_size_gb": 0.001,
                                "source": {"kind": "fixture", "fixture_files": {"config.json": "{}"}},
                                "download": {},
                                "verification": {
                                    "required_files": ["config.json"],
                                    "smoke_test": {"kind": "fixture"},
                                },
                                "invocation": {},
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )
            command = [
                sys.executable,
                str(MANAGER),
                "download",
                "fixture-asr",
                "--catalog",
                str(catalog),
                "--manifest",
                str(manifest),
                "--jobs-root",
                str(jobs),
                "--model-root",
                str(root / "models"),
                "--job-id",
                "cancelled-job",
                "--fixture-mode",
                "--allow-system-drive",
            ]
            completed = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", timeout=30)
            self.assertNotEqual(completed.returncode, 0)
            state = json.loads((jobs / "cancelled-job.json").read_text(encoding="utf-8"))
            self.assertEqual(state["status"], "cancelled")
            self.assertFalse(manifest.exists())


if __name__ == "__main__":
    unittest.main()

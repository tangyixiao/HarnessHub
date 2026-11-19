"""F23 S1 — models.download 状态机测试（spec §2.3 / §3.5）。

事实来源：docs/specs/2026-11-09-f23-model-assets-design.md。
TDD：先于实现提交，RED 证据须为真实失败输出。

executor / info_fetcher / sdk_probe 全部可注入：单元测试不发网络、
不 import 真实 SDK（SDK 缺失降级用例按 spec §6 覆盖）。
"""

from __future__ import annotations

import threading
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from harness_hub_runtime.models.downloads import (
    DISK_BUDGET_BYTES,
    CancelledError,
    DownloadManager,
    DuplicateDownloadError,
)
from harness_hub_runtime.models.info import ModelInfoError

TWO_GIB = 2 * 1024 * 1024 * 1024


def make_info(size_bytes: int | None, files: list[str] | None = None):
    """构造 info_fetcher 的最小返回（鸭子类型：只要求 .size_bytes / .file_count / .files）。"""
    from harness_hub_runtime.models.dto import ModelFileInfo

    names = files if files is not None else ["model.safetensors"]
    infos = tuple(
        ModelFileInfo(path=n, size_bytes=size_bytes if len(names) == 1 else None)
        for n in names
    )
    obj = type(
        "FakeInfo",
        (),
        {
            "size_bytes": size_bytes,
            "file_count": len(names),
            "files": infos,
        },
    )()
    return obj


class FakeExecutor:
    """记录 plan、按脚本推进进度、可阻塞等待取消。"""

    def __init__(self) -> None:
        self.plans: list = []
        self.script: list[tuple] = []  # [(bytes_done, bytes_total, file_done, file_count, current_file)]
        self.block_until_cancel = False
        self.create_part_file = False
        self.raise_error: Exception | None = None
        self.released = threading.Event()

    def __call__(self, plan, progress, cancel_event) -> None:
        self.plans.append(plan)
        part_path: Path | None = None
        if self.create_part_file:
            part_path = plan.target_root / "models--org--name" / "snapshots" / plan.revision / "model.safetensors.part"
            part_path.parent.mkdir(parents=True, exist_ok=True)
            part_path.write_bytes(b"partial")

        for args in self.script:
            if cancel_event.is_set():
                raise CancelledError("cancelled")
            progress(*args)
        if self.block_until_cancel:
            while not cancel_event.is_set():
                if self.released.wait(0.05):
                    break
        if self.create_part_file and cancel_event.is_set():
            raise CancelledError("cancelled")
        if self.raise_error is not None:
            raise self.raise_error


class ManagerTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.info = make_info(1000)
        self.executor = FakeExecutor()
        self.manager = DownloadManager(
            cache_root=self.root,
            info_fetcher=lambda repo_id, revision: self.info,
            sdk_probe=lambda: False,
            executor=self.executor,
        )

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def wait_for(self, manager, download_id, state, timeout=2.0) -> None:
        import time

        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if manager.progress(download_id)["state"] == state:
                return
            time.sleep(0.01)
        self.fail(f"超时等待状态 {state}：{manager.progress(download_id)}")


class StartOutcomeTest(ManagerTestCase):
    def test_start_returns_download_id_and_preparing(self) -> None:
        self.executor.script = [(0, 1000, 0, 1, "model.safetensors")]
        outcome = self.manager.start("org/name", "main")
        self.assertEqual(
            list(outcome.keys()), ["downloadId", "state"], "spec §3.5 立即返回形状"
        )
        self.assertEqual(outcome["state"], "preparing")
        self.assertIsInstance(outcome["downloadId"], str)
        self.wait_for(self.manager, outcome["downloadId"], "downloading")

    def test_progress_shape_matches_contract(self) -> None:
        self.executor.script = []
        outcome = self.manager.start("org/name", "main")
        progress = self.manager.progress(outcome["downloadId"])
        self.assertEqual(
            list(progress.keys()),
            ["repoId", "revision", "state", "bytesDone", "bytesTotal", "fileDone", "fileCount", "currentFile", "lastError"],
        )
        self.assertEqual(progress["repoId"], "org/name")
        self.assertEqual(progress["revision"], "main")
        self.wait_for(self.manager, outcome["downloadId"], "completed")

    def test_duplicate_in_flight_download_is_rejected(self) -> None:
        self.executor.block_until_cancel = True
        first = self.manager.start("org/name", "main")
        self.wait_for(self.manager, first["downloadId"], "downloading")
        with self.assertRaises(DuplicateDownloadError):
            self.manager.start("org/name", "main")
        # 不同 revision 不算重复。
        outcome = self.manager.start("org/name", "v2")
        self.assertEqual(outcome["state"], "preparing")
        self.manager.cancel(first["downloadId"])

    def test_finished_tasks_do_not_block_restart(self) -> None:
        outcome = self.manager.start("org/name", "main")
        self.wait_for(self.manager, outcome["downloadId"], "completed")
        second = self.manager.start("org/name", "main")
        self.assertEqual(second["state"], "preparing")

    def test_invalid_identifiers_rejected_before_any_work(self) -> None:
        with self.assertRaises(ValueError):
            self.manager.start("bad name", "main")
        with self.assertRaises(ValueError):
            self.manager.start("org/name", "..")
        self.assertEqual(self.manager.states(), [])


class BudgetGateTest(ManagerTestCase):
    def test_over_budget_without_confirmation_returns_needs_confirmation(self) -> None:
        self.info = make_info(TWO_GIB + 1)
        outcome = self.manager.start("org/name", "main")
        self.assertEqual(
            outcome,
            {"needsConfirmation": True, "estimatedBytes": TWO_GIB + 1},
            "spec §2.3：> 2 GiB 必须先确认",
        )
        self.assertEqual(self.manager.states(), [], "未确认绝不创建任务")

    def test_at_budget_boundary_needs_no_confirmation(self) -> None:
        self.info = make_info(DISK_BUDGET_BYTES)
        outcome = self.manager.start("org/name", "main")
        self.assertIn("downloadId", outcome, "恰好 2 GiB 不触发门（严格大于）")

    def test_under_budget_starts_directly(self) -> None:
        self.info = make_info(1000)
        outcome = self.manager.start("org/name", "main")
        self.assertIn("downloadId", outcome)

    def test_confirmation_passes_the_gate(self) -> None:
        self.info = make_info(TWO_GIB + 1)
        outcome = self.manager.start("org/name", "main", confirmed_disk_budget=True)
        self.assertIn("downloadId", outcome)

    def test_unknown_size_skips_gate_without_fabricating(self) -> None:
        self.info = make_info(None)
        outcome = self.manager.start("org/name", "main")
        self.assertIn("downloadId", outcome, "拿不到预估按未知处理（不虚报），不拦用户")

    def test_info_failure_propagates_as_model_info_error(self) -> None:
        def broken(repo_id, revision):
            raise ModelInfoError("500")

        self.manager = DownloadManager(
            cache_root=self.root,
            info_fetcher=broken,
            sdk_probe=lambda: False,
            executor=self.executor,
        )
        with self.assertRaises(ModelInfoError):
            self.manager.start("org/name", "main")
        self.assertEqual(self.manager.states(), [])


class ViaSelectionTest(ManagerTestCase):
    def test_sdk_missing_degrades_to_stdlib_honestly(self) -> None:
        outcome = self.manager.start("org/name", "main")
        self.wait_for(self.manager, outcome["downloadId"], "completed")
        plan = self.executor.plans[0]
        self.assertEqual(plan.via, "stdlib", "spec §6：SDK 缺失降级用例")

    def test_sdk_present_uses_huggingface_hub(self) -> None:
        self.manager = DownloadManager(
            cache_root=self.root,
            info_fetcher=lambda repo_id, revision: self.info,
            sdk_probe=lambda: True,
            executor=self.executor,
        )
        outcome = self.manager.start("org/name", "main")
        self.wait_for(self.manager, outcome["downloadId"], "completed")
        self.assertEqual(self.executor.plans[0].via, "huggingface_hub")

    def test_stdlib_plan_lists_files_from_info_and_resolves_url(self) -> None:
        self.info = make_info(3000, files=["a.bin", "sub/b.bin"])
        outcome = self.manager.start("org/name", "main")
        self.wait_for(self.manager, outcome["downloadId"], "completed")
        plan = self.executor.plans[0]
        self.assertEqual(plan.files, ("a.bin", "sub/b.bin"))
        self.assertIn("huggingface.co/org/name/resolve/main/a.bin", plan.source_urls[0])

    def test_file_whitelist_narrows_the_plan(self) -> None:
        self.info = make_info(3000, files=["a.bin", "b.bin"])
        outcome = self.manager.start("org/name", "main", files=["b.bin"])
        self.wait_for(self.manager, outcome["downloadId"], "completed")
        self.assertEqual(self.executor.plans[0].files, ("b.bin",))


class ProgressAndCancelTest(ManagerTestCase):
    def test_executor_progress_is_reflected(self) -> None:
        self.executor.script = [
            (300, 1000, 0, 2, "a.bin"),
            (1000, 2000, 1, 2, "b.bin"),
        ]
        outcome = self.manager.start("org/name", "main")
        download_id = outcome["downloadId"]
        self.wait_for(self.manager, download_id, "completed")
        progress = self.manager.progress(download_id)
        self.assertEqual(progress["state"], "completed")
        self.assertEqual(progress["bytesDone"], 1000)
        self.assertEqual(progress["fileDone"], 1, "脚本最后一次推进按记录呈现")

    def test_running_failure_becomes_failed_with_clipped_error(self) -> None:
        self.executor.raise_error = RuntimeError("e" * 400)
        outcome = self.manager.start("org/name", "main")
        self.wait_for(self.manager, outcome["downloadId"], "failed")
        progress = self.manager.progress(outcome["downloadId"])
        self.assertEqual(progress["state"], "failed")
        self.assertLessEqual(len(progress["lastError"]), 256)
        self.assertIn("RuntimeError", progress["lastError"])

    def test_cancel_is_idempotent_and_cleans_part_files(self) -> None:
        self.executor.block_until_cancel = True
        self.executor.create_part_file = True
        outcome = self.manager.start("org/name", "main")
        download_id = outcome["downloadId"]
        self.wait_for(self.manager, download_id, "downloading")
        first = self.manager.cancel(download_id)
        second = self.manager.cancel(download_id)
        self.wait_for(self.manager, download_id, "cancelled")
        self.assertEqual(first["state"], "cancelled")
        self.assertEqual(second["state"], "cancelled", "幂等")
        part = (
            self.root / "models--org--name" / "snapshots" / "main" / "model.safetensors.part"
        )
        self.assertFalse(part.exists(), "取消后绝不留半截文件冒充完整资产")

    def test_cancel_on_unknown_id_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            self.manager.cancel("no-such-id")

    def test_progress_on_unknown_id_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            self.manager.progress("no-such-id")

    def test_states_feed_models_status_downloads_list(self) -> None:
        self.executor.script = []
        outcome = self.manager.start("org/name", "main")
        self.wait_for(self.manager, outcome["downloadId"], "completed")
        states = self.manager.states()
        self.assertEqual(len(states), 1)
        self.assertEqual(
            list(states[0].keys()), ["repoId", "revision", "state", "via"]
        )
        self.assertEqual(states[0]["state"], "completed")
        self.assertEqual(states[0]["via"], "stdlib")


if __name__ == "__main__":
    unittest.main()

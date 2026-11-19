"""F23 S1 — models.cache.list 缓存扫描测试（spec §3.4 / §2 红线 1）。

事实来源：docs/specs/2026-11-09-f23-model-assets-design.md。
TDD：先于实现提交，RED 证据须为真实失败输出。

零网络红线：缓存扫描只碰文件系统；根路径解析遵循
HUGGINGFACE_HUB_CACHE > HF_HOME(+hub) > ~/.cache/huggingface/hub。
"""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

from harness_hub_runtime.models.cache import (
    MAX_ENTRIES,
    MAX_FILES_PER_REPO,
    resolve_cache_root,
    scan_cache,
)
from harness_hub_runtime.models.dto import CacheScan


class TempCacheTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def make_repo(self, org: str, name: str, revisions: list[str]) -> Path:
        """标准 HF hub 布局：models--<org>--<name>/{refs,snapshots/<rev>/...}。"""
        repo = self.root / f"models--{org}--{name}"
        refs = repo / "refs"
        refs.mkdir(parents=True)
        for revision in revisions:
            (refs / revision).write_text("0" * 40, encoding="utf-8")
        return repo

    def add_snapshot_file(
        self, repo: Path, revision: str, rel_path: str, size: int
    ) -> None:
        target = repo / "snapshots" / revision / rel_path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(b"x" * size)


class ScanLayoutTest(TempCacheTestCase):
    def test_revisions_come_from_refs_and_sizes_are_summed(self) -> None:
        repo = self.make_repo("org", "name", ["main", "v1"])
        self.add_snapshot_file(repo, "main", "config.json", 100)
        self.add_snapshot_file(repo, "main", "weights/model.safetensors", 2000)
        self.add_snapshot_file(repo, "v1", "config.json", 50)

        scan = scan_cache(self.root)
        self.assertIsInstance(scan, CacheScan)
        data = scan.to_json()
        self.assertEqual(list(data.keys()), ["entries", "cacheRoot"])
        self.assertEqual(data["cacheRoot"], str(self.root))
        entries = data["entries"]
        self.assertEqual(len(entries), 1)
        entry = entries[0]
        self.assertEqual(
            list(entry.keys()), ["repoId", "revisions", "sizeBytes", "lastUsedAt"]
        )
        self.assertEqual(entry["repoId"], "org/name")
        self.assertEqual(entry["revisions"], ["main", "v1"])
        self.assertEqual(entry["sizeBytes"], 2150, "snapshots 下文件求和")

    def test_repo_id_reconstruction_keeps_name_dashes(self) -> None:
        self.make_repo("org", "na--me", ["main"])  # 名字内含 -- 的极端形态
        data = scan_cache(self.root).to_json()
        self.assertEqual(data["entries"][0]["repoId"], "org/na--me")

    def test_multiple_repos_sorted_by_repo_id(self) -> None:
        self.make_repo("zeta", "a", ["main"])
        self.make_repo("alpha", "b", ["main"])
        data = scan_cache(self.root).to_json()
        self.assertEqual(
            [entry["repoId"] for entry in data["entries"]], ["alpha/b", "zeta/a"]
        )

    def test_missing_root_is_an_empty_scan_not_an_error(self) -> None:
        missing = self.root / "does-not-exist"
        scan = scan_cache(missing)
        self.assertEqual(scan.to_json()["entries"], [])
        self.assertEqual(scan.to_json()["cacheRoot"], str(missing))

    def test_non_model_dirs_are_ignored(self) -> None:
        (self.root / "datasets--org--set").mkdir()
        (self.root / "unrelated").mkdir()
        self.assertEqual(scan_cache(self.root).to_json()["entries"], [])


class ScanLimitsTest(TempCacheTestCase):
    def test_single_repo_file_cap_is_honest(self) -> None:
        repo = self.make_repo("org", "big", ["main"])
        for i in range(MAX_FILES_PER_REPO + 500):
            # 平铺到多个子目录，避免单目录 2 万+ 文件拖慢测试。
            bucket = i // 500
            self.add_snapshot_file(repo, "main", f"part{bucket}/f{i}.bin", 1)
        data = scan_cache(self.root).to_json()
        entry = data["entries"][0]
        self.assertEqual(
            entry["sizeBytes"],
            MAX_FILES_PER_REPO,
            "超出单仓文件上限的部分不计入（如实截断，不虚报也不崩溃）",
        )

    def test_entries_cap_at_128(self) -> None:
        for i in range(MAX_ENTRIES + 10):
            self.make_repo(f"org{i}", "m", ["main"])
        data = scan_cache(self.root).to_json()
        self.assertEqual(len(data["entries"]), MAX_ENTRIES)


class LastUsedTest(TempCacheTestCase):
    def test_last_used_at_present_or_null_and_rfc3339_shape(self) -> None:
        repo = self.make_repo("org", "name", ["main"])
        self.add_snapshot_file(repo, "main", "a.bin", 4)
        data = scan_cache(self.root).to_json()
        value = data["entries"][0]["lastUsedAt"]
        # refs 文件 mtime 存在 → rfc3339（以 Z 结尾的 UTC）；缺失 → None。
        self.assertIsNotNone(value)
        self.assertTrue(str(value).endswith("Z"), f"rfc3339 UTC：{value}")

    def test_repo_without_refs_reports_null_last_used(self) -> None:
        (self.root / "models--org--bare").mkdir()
        data = scan_cache(self.root).to_json()
        self.assertIsNone(data["entries"][0]["lastUsedAt"])


class CacheRootResolutionTest(unittest.TestCase):
    def _with_env(self, **overrides: str) -> Path:
        env = {
            "HUGGINGFACE_HUB_CACHE": "",
            "HF_HOME": "",
            "HARNESS_HUB_MODELS_ROOT": "",
        }
        env.update(overrides)
        return resolve_cache_root(env)

    def test_precedence_hub_cache_over_hf_home_over_default(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            explicit = self._with_env(HUGGINGFACE_HUB_CACHE=os.path.join(tmp, "hc"))
            self.assertEqual(explicit, Path(tmp) / "hc")

            from_home = self._with_env(HF_HOME=os.path.join(tmp, "home"))
            self.assertEqual(from_home, Path(tmp) / "home" / "hub", "HF_HOME 下挂 hub")

            default = self._with_env()
            self.assertEqual(
                default,
                Path.home() / ".cache" / "huggingface" / "hub",
                "spec §2.2 缺省路径",
            )

    def test_models_root_is_a_download_target_override_not_the_cache(self) -> None:
        # HARNESS_HUB_MODELS_ROOT 只影响下载目标（spec §2.2），不改扫描根。
        with tempfile.TemporaryDirectory() as tmp:
            root = self._with_env(HARNESS_HUB_MODELS_ROOT=os.path.join(tmp, "models"))
            self.assertEqual(root, Path.home() / ".cache" / "huggingface" / "hub")


if __name__ == "__main__":
    unittest.main()

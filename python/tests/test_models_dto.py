"""F23 S1 — models DTO 契约测试（spec §3.2–§3.5 / §2 红线 4）。

事实来源：docs/specs/2026-11-09-f23-model-assets-design.md。
TDD：本文件先于实现提交，RED 证据须为真实失败输出。

键序纪律：to_json() 的键名与顺序即跨语言契约（F22 dto.py 同规则），
Rust 侧 decoder / 前端 normalize 都按这份形状锁定。
"""

from __future__ import annotations

import unittest
from dataclasses import FrozenInstanceError

from harness_hub_runtime.models.dto import (
    DISK_BUDGET_BYTES,
    DOWNLOAD_STATES,
    MAX_FILES,
    MAX_LAST_ERROR,
    CacheEntry,
    CacheScan,
    DownloadProgress,
    ModelDownloadState,
    ModelFileInfo,
    ModelRepoInfo,
    ModelSdkStatus,
    validate_file_list,
    validate_repo_id,
    validate_revision,
)


def _file(path: str, size: int | None) -> ModelFileInfo:
    return ModelFileInfo(path=path, size_bytes=size)


class ModelSdkStatusTest(unittest.TestCase):
    def test_to_json_key_order_frozen(self) -> None:
        data = ModelSdkStatus(huggingface_hub=True).to_json()
        self.assertEqual(list(data.keys()), ["huggingfaceHub"])
        self.assertIs(data["huggingfaceHub"], True)

    def test_frozen_dataclass_rejects_mutation(self) -> None:
        status = ModelSdkStatus(huggingface_hub=False)
        with self.assertRaises(FrozenInstanceError):
            status.huggingface_hub = True  # type: ignore[misc]


class ModelDownloadStateTest(unittest.TestCase):
    def test_to_json_key_order_frozen(self) -> None:
        data = ModelDownloadState(
            repo_id="org/name", revision="main", state="downloading", via="stdlib"
        ).to_json()
        self.assertEqual(list(data.keys()), ["repoId", "revision", "state", "via"])
        self.assertEqual(data["repoId"], "org/name")
        self.assertEqual(data["revision"], "main")
        self.assertEqual(data["state"], "downloading")
        self.assertEqual(data["via"], "stdlib")

    def test_via_may_be_none(self) -> None:
        data = ModelDownloadState(
            repo_id="org/name", revision="main", state="preparing", via=None
        ).to_json()
        self.assertIsNone(data["via"])


class ModelFileInfoTest(unittest.TestCase):
    def test_to_json_key_order_frozen(self) -> None:
        data = ModelFileInfo(path="model.safetensors", size_bytes=123).to_json()
        self.assertEqual(list(data.keys()), ["path", "sizeBytes"])
        self.assertEqual(data["sizeBytes"], 123)

    def test_size_bytes_may_be_none(self) -> None:
        data = ModelFileInfo(path="tokenizer.json", size_bytes=None).to_json()
        self.assertIsNone(data["sizeBytes"])


class ModelRepoInfoTest(unittest.TestCase):
    def test_to_json_key_order_frozen_and_via_is_api(self) -> None:
        info = ModelRepoInfo(
            repo_id="org/name",
            revision="main",
            size_bytes=300,
            file_count=2,
            files=(_file("a.bin", 100), _file("b.bin", 200)),
            files_truncated=False,
            gated=False,
        )
        data = info.to_json()
        self.assertEqual(
            list(data.keys()),
            ["repoId", "revision", "sizeBytes", "fileCount", "files", "filesTruncated", "gated", "via"],
        )
        self.assertEqual(data["via"], "api")
        self.assertEqual(data["files"], [{"path": "a.bin", "sizeBytes": 100}, {"path": "b.bin", "sizeBytes": 200}])

    def test_files_over_limit_are_truncated_honestly(self) -> None:
        files = tuple(_file(f"f{i}.bin", i) for i in range(MAX_FILES + 10))
        info = ModelRepoInfo(
            repo_id="org/name",
            revision="main",
            size_bytes=None,
            file_count=MAX_FILES + 10,
            files=files,
            files_truncated=True,
            gated=False,
        )
        data = info.to_json()
        self.assertEqual(len(data["files"]), MAX_FILES)
        self.assertIs(data["filesTruncated"], True)
        self.assertEqual(data["fileCount"], MAX_FILES + 10)


class CacheEntryTest(unittest.TestCase):
    def test_to_json_key_order_frozen(self) -> None:
        entry = CacheEntry(
            repo_id="org/name",
            revisions=("main", "v1"),
            size_bytes=456,
            last_used_at="2026-11-09T00:00:00Z",
        )
        data = entry.to_json()
        self.assertEqual(list(data.keys()), ["repoId", "revisions", "sizeBytes", "lastUsedAt"])
        self.assertEqual(data["revisions"], ["main", "v1"])

    def test_last_used_at_may_be_none(self) -> None:
        entry = CacheEntry(repo_id="org/name", revisions=(), size_bytes=0, last_used_at=None)
        self.assertIsNone(entry.to_json()["lastUsedAt"])


class CacheScanTest(unittest.TestCase):
    def test_to_json_key_order_frozen(self) -> None:
        scan = CacheScan(
            entries=(
                CacheEntry(repo_id="org/name", revisions=("main",), size_bytes=1, last_used_at=None),
            ),
            cache_root=r"C:\cache\hub",
        )
        data = scan.to_json()
        self.assertEqual(list(data.keys()), ["entries", "cacheRoot"])
        self.assertEqual(data["cacheRoot"], r"C:\cache\hub")


class DownloadProgressTest(unittest.TestCase):
    def test_to_json_key_order_frozen(self) -> None:
        progress = DownloadProgress(
            repo_id="org/name",
            revision="main",
            state="downloading",
            bytes_done=10,
            bytes_total=20,
            file_done=1,
            file_count=3,
            current_file="a.bin",
            last_error=None,
        )
        data = progress.to_json()
        self.assertEqual(
            list(data.keys()),
            [
                "repoId",
                "revision",
                "state",
                "bytesDone",
                "bytesTotal",
                "fileDone",
                "fileCount",
                "currentFile",
                "lastError",
            ],
        )
        self.assertEqual(data["bytesTotal"], 20)

    def test_last_error_clipped_to_256(self) -> None:
        progress = DownloadProgress(
            repo_id="org/name",
            revision="main",
            state="failed",
            bytes_done=0,
            bytes_total=None,
            file_done=0,
            file_count=1,
            current_file=None,
            last_error="x" * (MAX_LAST_ERROR + 50),
        )
        data = progress.to_json()
        self.assertEqual(len(data["lastError"]), MAX_LAST_ERROR)

    def test_bytes_total_may_be_none(self) -> None:
        progress = DownloadProgress(
            repo_id="org/name",
            revision="main",
            state="preparing",
            bytes_done=0,
            bytes_total=None,
            file_done=0,
            file_count=0,
            current_file=None,
            last_error=None,
        )
        self.assertIsNone(progress.to_json()["bytesTotal"])


class FrozenConstantsTest(unittest.TestCase):
    """常量即契约：改动任何一项都是跨语言破坏性变更。"""

    def test_disk_budget_is_2_gib(self) -> None:
        self.assertEqual(DISK_BUDGET_BYTES, 2 * 1024 * 1024 * 1024)

    def test_max_files_is_64(self) -> None:
        self.assertEqual(MAX_FILES, 64)

    def test_max_last_error_is_256(self) -> None:
        self.assertEqual(MAX_LAST_ERROR, 256)

    def test_download_states_frozen(self) -> None:
        self.assertEqual(
            DOWNLOAD_STATES,
            ("idle", "preparing", "downloading", "completed", "failed", "cancelled"),
        )


class ModelIdentifierValidationTest(unittest.TestCase):
    """spec §2 红线 4：^[A-Za-z0-9._/-]+$ 且拒绝 `..` 段（防路径穿越与 header 注入）。"""

    def test_valid_repo_ids_pass(self) -> None:
        for value in ("bert", "org/name", "org/sub/name", "x.y-z_1/Name.v2", "meta-llama/Llama-2-7b"):
            self.assertEqual(validate_repo_id(value), value)

    def test_valid_revisions_pass(self) -> None:
        for value in ("main", "v1.0.0", "commit123", "pr/1", "refs.pr-1"):
            self.assertEqual(validate_revision(value), value)

    def test_invalid_repo_ids_rejected(self) -> None:
        for value in (None, "", 123, "org name", "org/name extra", "a\\b", "..", "org/../name", "../etc"):
            with self.assertRaises(ValueError, msg=f"{value!r} 应被拒绝"):
                validate_repo_id(value)

    def test_invalid_revisions_rejected(self) -> None:
        for value in (None, "", 42, "main branch", "v1 ..v2", "a/../b", "."):
            with self.assertRaises(ValueError, msg=f"{value!r} 应被拒绝"):
                validate_revision(value)

    def test_dot_and_empty_segments_rejected(self) -> None:
        # "." 段与空段同样是路径穿越/歧义向量，与 ".." 一并拒绝。
        for value in ("org/./name", "org//name", "/org/name", "org/name/"):
            with self.assertRaises(ValueError, msg=f"{value!r} 应被拒绝"):
                validate_repo_id(value)

    def test_error_message_mentions_the_field(self) -> None:
        with self.assertRaises(ValueError) as ctx:
            validate_repo_id("bad name")
        self.assertIn("repoId", str(ctx.exception))
        with self.assertRaises(ValueError) as ctx:
            validate_revision("bad rev")
        self.assertIn("revision", str(ctx.exception))


class FileListValidationTest(unittest.TestCase):
    """download.start 的 files 白名单：可选，逐项过同一条校验。"""

    def test_none_passes_through(self) -> None:
        self.assertIsNone(validate_file_list(None))

    def test_valid_list_returns_copy(self) -> None:
        source = ["model.safetensors", "sub/tokenizer.json"]
        result = validate_file_list(source)
        self.assertEqual(result, source)
        self.assertIsNot(result, source, "必须返回新列表对象（副本），入参后续修改不影响结果")

    def test_empty_list_is_rejected(self) -> None:
        # 空白名单语义歧义（全量还是零文件？），fail-closed 拒绝。
        with self.assertRaises(ValueError):
            validate_file_list([])

    def test_non_list_rejected(self) -> None:
        for value in ("a.bin", 3, {"a": 1}, ["a.bin", 2]):
            with self.assertRaises(ValueError):
                validate_file_list(value)

    def test_traversal_segments_rejected(self) -> None:
        for value in ("../config", "a/../../b", "..", "./a", "dir//file"):
            with self.assertRaises(ValueError):
                validate_file_list([value])


if __name__ == "__main__":
    unittest.main()

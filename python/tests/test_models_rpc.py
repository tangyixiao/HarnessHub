"""F23 S1 — rpc 方法表扩展测试（spec §3.1–§3.6，F20 握手零改动锁定）。

事实来源：docs/specs/2026-11-09-f23-model-assets-design.md。
TDD：本文件先于实现提交，RED 证据须为真实失败输出。

注入面（对齐 test_mcp_rpc.py 的 _MCP_REGISTRY 模式）：
* `rpc_server._MODEL_MANAGER` —— 下载状态机（测试换 FakeManager）
* `rpc_server._MODEL_INFO_FETCHER` —— 元数据抓取（测试换 fake，绝不发网络）
* `rpc_server._cache_root` —— 缓存扫描根（测试换临时目录）
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from harness_hub_runtime.models.info import ModelInfoError
from harness_hub_runtime.models.downloads import DuplicateDownloadError
from harness_hub_runtime.rpc import server as rpc_server
from harness_hub_runtime.rpc.server import (
    DEFAULT_METHODS,
    GATEWAY_STATIC_CAPABILITIES,
    GATEWAY_STATIC_METHODS,
    INVALID_REQUEST,
    METHOD_NOT_FOUND,
    handle_request,
)

# F20 三项不变；F22/F23 只允许在末尾追加。
F20_REQUIRED_CAPABILITIES = ["ping", "runtime.info", "shutdown"]
CAPABILITIES_AFTER_F22 = [
    "ping",
    "runtime.info",
    "shutdown",
    "gateway.status",
    "gateway.routing",
    "gateway.streaming",
    "gateway.errors",
    "tokenization.precedence",
    "tokenization.estimate",
    "tokenization.revision",
    "mcp.client",
]
F22_METHODS = [
    "ping",
    "runtime.info",
    "initialize",
    "shutdown",
    "gateway.status",
    "gateway.providers",
    "gateway.route",
    "gateway.normalize.stream",
    "gateway.normalize.error",
    "tokenization.estimate",
    "tokenization.precedence",
    "tokenization.revision",
    "mcp.status",
    "mcp.connect",
    "mcp.disconnect",
    "mcp.tools.list",
    "mcp.tools.call",
]
# F23 追加的 6 个方法（spec §3.1：追加在末尾，顺序冻结）。
MODELS_METHODS = [
    "models.status",
    "models.info",
    "models.cache.list",
    "models.download.start",
    "models.download.progress",
    "models.download.cancel",
]


class FakeManager:
    """实现 DownloadManager 的对外面；记录调用，返回脚本化结果。"""

    def __init__(self) -> None:
        self.cache_root = Path(r"C:\fake\cache")
        self.states_result: list[dict] = []
        self.start_result: dict = {"downloadId": "dl-1", "state": "preparing"}
        self.progress_result: dict = {}
        self.cancel_result: dict = {"state": "cancelled"}
        self.start_error: Exception | None = None
        self.progress_error: Exception | None = None
        self.cancel_error: Exception | None = None
        self.calls: list[tuple] = []

    def states(self):
        return self.states_result

    def start(self, repo_id, revision, files=None, confirmed_disk_budget=False):
        self.calls.append(("start", repo_id, revision, files, confirmed_disk_budget))
        if self.start_error is not None:
            raise self.start_error
        return self.start_result

    def progress(self, download_id):
        self.calls.append(("progress", download_id))
        if self.progress_error is not None:
            raise self.progress_error
        return self.progress_result

    def cancel(self, download_id):
        self.calls.append(("cancel", download_id))
        if self.cancel_error is not None:
            raise self.cancel_error
        return self.cancel_result


class MethodTableContractTest(unittest.TestCase):
    """spec §3.1：追加不重排；GATEWAY_STATIC_* 保持 F22 冻结值不动。"""

    def test_capabilities_appended_in_order(self) -> None:
        self.assertEqual(
            rpc_server.SUPPORTED_CAPABILITIES, CAPABILITIES_AFTER_F22 + ["models.assets"]
        )

    def test_gateway_static_capabilities_untouched(self) -> None:
        self.assertEqual(GATEWAY_STATIC_CAPABILITIES, CAPABILITIES_AFTER_F22)

    def test_gateway_static_methods_untouched(self) -> None:
        self.assertEqual(len(GATEWAY_STATIC_METHODS), 12)
        self.assertEqual(GATEWAY_STATIC_METHODS[:12], F22_METHODS[:12])

    def test_default_methods_appended_at_end(self) -> None:
        self.assertEqual(list(DEFAULT_METHODS.keys()), F22_METHODS + MODELS_METHODS)


class F20HandshakeUnchangedTest(unittest.TestCase):
    def test_initialize_with_f20_required_capabilities_still_passes(self) -> None:
        response = handle_request(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"protocolVersion": "1.0", "requiredCapabilities": F20_REQUIRED_CAPABILITIES},
            },
            DEFAULT_METHODS,
        )
        assert response is not None
        self.assertNotIn("error", response)
        for cap in F20_REQUIRED_CAPABILITIES:
            self.assertIn(cap, response["result"]["capabilities"])

    def test_f23_capability_is_advertised(self) -> None:
        response = handle_request(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"protocolVersion": "1.0", "requiredCapabilities": ["models.assets"]},
            },
            DEFAULT_METHODS,
        )
        assert response is not None
        self.assertNotIn("error", response, "models.assets 必须在能力清单里")

    def test_unknown_method_still_method_not_found(self) -> None:
        response = handle_request({"jsonrpc": "2.0", "id": 3, "method": "models.nope"}, DEFAULT_METHODS)
        assert response is not None
        self.assertEqual(response["error"]["code"], METHOD_NOT_FOUND)


class ModelsHandlerWiringTest(unittest.TestCase):
    def setUp(self) -> None:
        self._previous = rpc_server._MODEL_MANAGER
        self._previous_fetcher = rpc_server._MODEL_INFO_FETCHER
        self.fake = FakeManager()
        rpc_server._MODEL_MANAGER = self.fake

    def tearDown(self) -> None:
        rpc_server._MODEL_MANAGER = self._previous
        rpc_server._MODEL_INFO_FETCHER = self._previous_fetcher

    def _call(self, method: str, params: dict) -> dict:
        response = handle_request({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}, DEFAULT_METHODS)
        assert response is not None
        return response

    # ---------- models.status ----------

    def test_status_shape_is_frozen(self) -> None:
        self.fake.states_result = [
            {"repoId": "org/name", "revision": "main", "state": "completed", "via": "stdlib"}
        ]
        response = self._call("models.status", {})
        self.assertNotIn("error", response)
        result = response["result"]
        self.assertEqual(
            list(result.keys()), ["sdk", "cacheRoot", "downloads", "protocolVersion"]
        )
        self.assertEqual(list(result["sdk"].keys()), ["huggingfaceHub"])
        self.assertIsInstance(result["sdk"]["huggingfaceHub"], bool)
        self.assertEqual(result["cacheRoot"], r"C:\fake\cache")
        self.assertEqual(result["downloads"], self.fake.states_result)
        self.assertEqual(result["protocolVersion"], "1.0")

    def test_status_uses_lazy_real_manager_when_unset(self) -> None:
        rpc_server._MODEL_MANAGER = None
        response = self._call("models.status", {})
        self.assertNotIn("error", response)
        result = response["result"]
        self.assertEqual(result["downloads"], [], "真管理器初始无任务")
        self.assertEqual(result["cacheRoot"], str(rpc_server._MODEL_MANAGER.cache_root))

    # ---------- models.info ----------

    def test_info_passes_through_fetcher_result(self) -> None:
        seen: list[tuple[str, str]] = []

        def fake_fetcher(repo_id: str, revision: str):
            seen.append((repo_id, revision))
            from harness_hub_runtime.models.dto import ModelRepoInfo

            return ModelRepoInfo(
                repo_id=repo_id,
                revision=revision,
                size_bytes=10,
                file_count=1,
                files=(),
                files_truncated=False,
                gated=False,
            )

        rpc_server._MODEL_INFO_FETCHER = fake_fetcher
        response = self._call("models.info", {"repoId": "org/name", "revision": "v1"})
        self.assertNotIn("error", response)
        self.assertEqual(seen, [("org/name", "v1")])
        self.assertEqual(response["result"]["repoId"], "org/name")
        self.assertEqual(response["result"]["via"], "api")

    def test_info_network_failure_is_32004_with_short_message(self) -> None:
        rpc_server._MODEL_INFO_FETCHER = lambda repo_id, revision: (_ for _ in ()).throw(
            ModelInfoError("HTTP 404")
        )
        response = self._call("models.info", {"repoId": "org/missing", "revision": "main"})
        self.assertEqual(response["error"]["code"], rpc_server.MODELS_INFO_FAILED)
        self.assertEqual(rpc_server.MODELS_INFO_FAILED, -32004)
        self.assertLessEqual(len(response["error"]["message"]), 200)

    def test_info_invalid_identifier_is_32602(self) -> None:
        for params in (
            {"repoId": "bad name", "revision": "main"},
            {"repoId": "org/name", "revision": ".."},
            {"revision": "main"},
            {"repoId": "org/name"},
        ):
            response = self._call("models.info", params)
            self.assertEqual(response["error"]["code"], INVALID_REQUEST, f"{params}")

    # ---------- models.cache.list ----------

    def test_cache_list_scans_injected_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            rpc_server._cache_root = lambda: Path(tmp)  # type: ignore[assignment]
            response = self._call("models.cache.list", {})
            self.assertNotIn("error", response)
            result = response["result"]
            self.assertEqual(list(result.keys()), ["entries", "cacheRoot"])
            self.assertEqual(result["entries"], [])
            self.assertEqual(result["cacheRoot"], tmp)

    # ---------- models.download.start ----------

    def test_start_passes_through_manager_outcome(self) -> None:
        response = self._call(
            "models.download.start",
            {"repoId": "org/name", "revision": "main", "confirmedDiskBudget": True},
        )
        self.assertNotIn("error", response)
        self.assertEqual(response["result"], {"downloadId": "dl-1", "state": "preparing"})
        self.assertEqual(
            self.fake.calls[-1], ("start", "org/name", "main", None, True)
        )

    def test_start_needs_confirmation_passes_through(self) -> None:
        self.fake.start_result = {"needsConfirmation": True, "estimatedBytes": 2**31 + 1}
        response = self._call("models.download.start", {"repoId": "org/name", "revision": "main"})
        self.assertNotIn("error", response)
        self.assertEqual(
            response["result"], {"needsConfirmation": True, "estimatedBytes": 2**31 + 1}
        )

    def test_start_duplicate_is_32005(self) -> None:
        self.fake.start_error = DuplicateDownloadError("org/name@main 已有进行中下载")
        response = self._call("models.download.start", {"repoId": "org/name", "revision": "main"})
        self.assertEqual(response["error"]["code"], rpc_server.MODELS_DUPLICATE_DOWNLOAD)
        self.assertEqual(rpc_server.MODELS_DUPLICATE_DOWNLOAD, -32005)

    def test_start_invalid_params_are_32602(self) -> None:
        for params in (
            {"repoId": "bad name", "revision": "main"},
            {"repoId": "org/name", "revision": ".."},
            {"repoId": "org/name", "revision": "main", "files": []},
            {"repoId": "org/name", "revision": "main", "files": "../x"},
        ):
            response = self._call("models.download.start", params)
            self.assertEqual(response["error"]["code"], INVALID_REQUEST, f"{params}")

    # ---------- models.download.progress / cancel ----------

    def test_progress_passes_through_manager_state(self) -> None:
        self.fake.progress_result = {
            "repoId": "org/name",
            "revision": "main",
            "state": "downloading",
            "bytesDone": 10,
            "bytesTotal": 20,
            "fileDone": 0,
            "fileCount": 1,
            "currentFile": "a.bin",
            "lastError": None,
        }
        response = self._call("models.download.progress", {"downloadId": "dl-1"})
        self.assertNotIn("error", response)
        self.assertEqual(response["result"]["state"], "downloading")

    def test_progress_unknown_id_is_32602(self) -> None:
        self.fake.progress_error = ValueError("未知 downloadId")
        response = self._call("models.download.progress", {"downloadId": "nope"})
        self.assertEqual(response["error"]["code"], INVALID_REQUEST)

    def test_cancel_passes_through_and_is_idempotent(self) -> None:
        response = self._call("models.download.cancel", {"downloadId": "dl-1"})
        self.assertNotIn("error", response)
        self.assertEqual(response["result"], {"state": "cancelled"})

    def test_cancel_unknown_id_is_32602(self) -> None:
        self.fake.cancel_error = ValueError("未知 downloadId")
        response = self._call("models.download.cancel", {"downloadId": "nope"})
        self.assertEqual(response["error"]["code"], INVALID_REQUEST)

    def test_download_id_must_be_a_string(self) -> None:
        for method in ("models.download.progress", "models.download.cancel"):
            response = self._call(method, {"downloadId": 42})
            self.assertEqual(response["error"]["code"], INVALID_REQUEST, method)


class RawContentRejectionTest(unittest.TestCase):
    def setUp(self) -> None:
        self._previous = rpc_server._MODEL_MANAGER
        rpc_server._MODEL_MANAGER = FakeManager()

    def tearDown(self) -> None:
        rpc_server._MODEL_MANAGER = self._previous

    def test_all_six_methods_reject_raw_content_keys(self) -> None:
        for method in MODELS_METHODS:
            for key in ("prompt", "messages", "response", "content"):
                response = handle_request(
                    {"jsonrpc": "2.0", "id": 1, "method": method, "params": {key: "原文"}},
                    DEFAULT_METHODS,
                )
                assert response is not None
                self.assertEqual(
                    response["error"]["code"], INVALID_REQUEST, f"{method}/{key}"
                )

    def test_non_dict_params_is_invalid_request(self) -> None:
        for method in MODELS_METHODS:
            response = handle_request(
                {"jsonrpc": "2.0", "id": 1, "method": method, "params": [1, 2]},
                DEFAULT_METHODS,
            )
            assert response is not None
            self.assertEqual(response["error"]["code"], INVALID_REQUEST, method)


if __name__ == "__main__":
    unittest.main()

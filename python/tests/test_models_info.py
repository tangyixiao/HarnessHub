"""F23 S1 — models.info REST 元数据测试（spec §3.3 / §2 红线 1）。

事实来源：docs/specs/2026-11-09-f23-model-assets-design.md。
TDD：先于实现提交，RED 证据须为真实失败输出。

网络红线：models.info 是显式网络动作；传输层可注入（fake opener），
单元测试绝不发真实网络请求。
"""

from __future__ import annotations

import unittest

from harness_hub_runtime.models.dto import MAX_FILES
from harness_hub_runtime.models.info import (
    INFO_TIMEOUT_SECONDS,
    MODEL_INFO_URL_TEMPLATE,
    ModelInfoError,
    fetch_repo_info,
)


class FakeResponse:
    def __init__(self, payload: bytes, status: int = 200) -> None:
        self.payload = payload
        self.status = status

    def read(self) -> bytes:
        return self.payload


class FakeOpenerTestCase(unittest.TestCase):
    """记录 URL / 超时的 fake opener（spec §6：fake transport 注入）。"""

    def setUp(self) -> None:
        self.calls: list[tuple[str, int]] = []
        self.response: FakeResponse | None = None
        self.error: Exception | None = None

    def opener(self, url: str, timeout: float) -> FakeResponse:
        self.calls.append((url, timeout))
        if self.error is not None:
            raise self.error
        assert self.response is not None, "测试未设置响应"
        return self.response

    def seed_json(self, payload: dict) -> None:
        import json

        self.response = FakeResponse(json.dumps(payload).encode("utf-8"))


class SuccessfulFetchTest(FakeOpenerTestCase):
    def test_url_and_timeout_follow_spec(self) -> None:
        self.seed_json({"siblings": [{"rfilename": "config.json", "size": 100}]})
        fetch_repo_info("org/name", "main", opener=self.opener)
        self.assertEqual(len(self.calls), 1)
        url, timeout = self.calls[0]
        self.assertEqual(
            url,
            MODEL_INFO_URL_TEMPLATE.format(repo_id="org/name", revision="main"),
            "REST 端点 = https://huggingface.co/api/models/<repo_id>/revision/<revision>",
        )
        self.assertEqual(timeout, INFO_TIMEOUT_SECONDS)
        self.assertEqual(INFO_TIMEOUT_SECONDS, 10, "spec §3.3：10s 超时")

    def test_shape_and_sizes_are_summed(self) -> None:
        self.seed_json(
            {
                "siblings": [
                    {"rfilename": "config.json", "size": 100},
                    {"rfilename": "model.safetensors", "size": 200},
                ],
                "gated": False,
            }
        )
        info = fetch_repo_info("org/name", "v1", opener=self.opener).to_json()
        self.assertEqual(
            list(info.keys()),
            ["repoId", "revision", "sizeBytes", "fileCount", "files", "filesTruncated", "gated", "via"],
        )
        self.assertEqual(info["repoId"], "org/name")
        self.assertEqual(info["revision"], "v1")
        self.assertEqual(info["sizeBytes"], 300, "siblings 大小求和")
        self.assertEqual(info["fileCount"], 2)
        self.assertIs(info["filesTruncated"], False)
        self.assertIs(info["gated"], False)
        self.assertEqual(info["via"], "api")

    def test_missing_sizes_are_unknown_not_fabricated(self) -> None:
        self.seed_json(
            {
                "siblings": [
                    {"rfilename": "a.bin", "size": 50},
                    {"rfilename": "b.bin"},
                ]
            }
        )
        info = fetch_repo_info("org/name", "main", opener=self.opener)
        self.assertEqual(info.size_bytes, 50, "有已知大小则求已知部分")
        data = info.to_json()
        self.assertIsNone(data["files"][1]["sizeBytes"], "拿不到的按未知，不虚报")

    def test_no_sizes_at_all_is_null(self) -> None:
        self.seed_json({"siblings": [{"rfilename": "a.bin"}, {"rfilename": "b.bin"}]})
        info = fetch_repo_info("org/name", "main", opener=self.opener)
        self.assertIsNone(info.size_bytes)
        self.assertEqual(info.file_count, 2)

    def test_gated_auto_is_truthy(self) -> None:
        self.seed_json({"siblings": [], "gated": "auto"})
        info = fetch_repo_info("org/name", "main", opener=self.opener)
        self.assertIs(info.gated, True, "gated: 'auto' 按真值如实报告")

    def test_over_max_files_is_truncated_honestly(self) -> None:
        siblings = [{"rfilename": f"f{i}.bin", "size": 1} for i in range(MAX_FILES + 5)]
        self.seed_json({"siblings": siblings})
        info = fetch_repo_info("org/name", "main", opener=self.opener)
        self.assertEqual(info.file_count, MAX_FILES + 5, "fileCount 保持真实总数")
        self.assertTrue(info.files_truncated)
        self.assertEqual(len(info.files), MAX_FILES)


class FailureTest(FakeOpenerTestCase):
    def test_http_error_raises_model_info_error(self) -> None:
        import urllib.error

        self.error = urllib.error.HTTPError(
            "url", 404, "Not Found", hdrs=None, fp=None  # type: ignore[arg-type]
        )
        with self.assertRaises(ModelInfoError):
            fetch_repo_info("org/missing", "main", opener=self.opener)

    def test_url_error_and_timeout_raise_model_info_error(self) -> None:
        import urllib.error

        self.error = urllib.error.URLError("connection refused")
        with self.assertRaises(ModelInfoError):
            fetch_repo_info("org/name", "main", opener=self.opener)
        self.error = TimeoutError("timed out")
        with self.assertRaises(ModelInfoError):
            fetch_repo_info("org/name", "main", opener=self.opener)

    def test_error_message_is_short_and_has_no_response_body(self) -> None:
        import urllib.error

        self.error = urllib.error.HTTPError(
            "url", 500, "Server Error", hdrs=None, fp=None  # type: ignore[arg-type]
        )
        try:
            fetch_repo_info("org/name", "main", opener=self.opener)
        except ModelInfoError as error:
            message = str(error)
            self.assertLessEqual(len(message), 200, "短摘要纪律")
            self.assertIn("500", message)

    def test_malformed_json_raises_model_info_error(self) -> None:
        self.response = FakeResponse(b"{not json")
        with self.assertRaises(ModelInfoError):
            fetch_repo_info("org/name", "main", opener=self.opener)

    def test_invalid_identifier_is_rejected_before_any_network(self) -> None:
        with self.assertRaises(ValueError):
            fetch_repo_info("bad name", "main", opener=self.opener)
        with self.assertRaises(ValueError):
            fetch_repo_info("org/name", "../etc", opener=self.opener)
        self.assertEqual(self.calls, [], "校验失败绝不发请求")


if __name__ == "__main__":
    unittest.main()

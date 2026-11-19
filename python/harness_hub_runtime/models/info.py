"""F23 S1 — models.info：HF repo 元数据查询（spec §3.3，显式网络动作）。

* REST 端点：`https://huggingface.co/api/models/<repo_id>/revision/<revision>`，
  stdlib `urllib.request`，10s 超时；失败 → `ModelInfoError`（短摘要，
  绝不回显响应体，server 层映射 -32004）。
* 大小纪律（spec §2.3）：siblings 大小求和；拿不到的按未知（None），不虚报。
* 传输可注入：`opener(url, timeout) -> bytes`（spec §6 fake transport 注入，
  单元测试绝不发真实网络请求）。
* 标识校验先于任何网络（repoId / revision 过 dto 白名单）。
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from typing import Any, Callable

from .dto import MAX_FILES, ModelFileInfo, ModelRepoInfo, validate_repo_id, validate_revision

# spec §3.3 冻结：REST 端点模板与 10s 超时。
MODEL_INFO_URL_TEMPLATE = "https://huggingface.co/api/models/{repo_id}/revision/{revision}"
INFO_TIMEOUT_SECONDS = 10.0

# 错误短摘要上限（-32004 message 纪律：不回显响应体）。
_MAX_ERROR_SUMMARY = 160

# opener 契约：`(url, timeout) -> bytes | response`（带 .read() 的响应对象也接受）。
# 缺省 = urllib.urlopen()。
Opener = Callable[[str, float], Any]


class ModelInfoError(Exception):
    """元数据查询失败（HTTP / 网络 / 响应形状），由 server 层映射为 -32004。"""


def _default_opener(url: str, timeout: float) -> Any:
    return urllib.request.urlopen(url, timeout=timeout)  # noqa: S310 - 固定 HTTPS 端点


def _short_error(error: Exception) -> str:
    """短摘要：类型 + 关键信息，绝不带响应体。"""
    detail = str(error).splitlines()[0] if str(error) else error.__class__.__name__
    return _clip_text(f"{error.__class__.__name__}: {detail}", _MAX_ERROR_SUMMARY)


def _clip_text(text: str, limit: int) -> str:
    return text[:limit]


def fetch_repo_info(
    repo_id: str,
    revision: str,
    *,
    opener: Opener | None = None,
    timeout: float = INFO_TIMEOUT_SECONDS,
) -> ModelRepoInfo:
    """抓取 repo 元数据并归一为 `ModelRepoInfo`（网络，显式）。"""
    repo_id = validate_repo_id(repo_id)
    revision = validate_revision(revision)
    fetch = opener if opener is not None else _default_opener
    url = MODEL_INFO_URL_TEMPLATE.format(repo_id=repo_id, revision=revision)

    try:
        raw = fetch(url, timeout)
        if hasattr(raw, "read"):
            raw = raw.read()
        payload = json.loads(raw.decode("utf-8"))
    except ModelInfoError:
        raise
    except Exception as error:  # noqa: BLE001 - HTTPError/URLError/Timeout/JSON 全收敛
        raise ModelInfoError(_short_error(error)) from error

    if not isinstance(payload, dict):
        raise ModelInfoError("响应不是 JSON 对象")

    siblings = payload.get("siblings")
    if not isinstance(siblings, list):
        siblings = []

    files: list[ModelFileInfo] = []
    known_size_total = 0
    any_known_size = False
    for sibling in siblings:
        if not isinstance(sibling, dict):
            continue
        path = sibling.get("rfilename")
        if not isinstance(path, str) or not path:
            continue
        size = sibling.get("size")
        size_bytes = size if isinstance(size, int) and not isinstance(size, bool) else None
        if size_bytes is not None:
            known_size_total += size_bytes
            any_known_size = True
        files.append(ModelFileInfo(path=path, size_bytes=size_bytes))

    # 拿不到大小按未知处理（不虚报）：有已知部分求和，全未知 → None。
    size_bytes = known_size_total if any_known_size else None
    # fileCount = 真实 sibling 总数（含形状非法被跳过的？不——只数有效项，与 files 一致口径）。
    file_count = len(files)
    gated = bool(payload.get("gated"))

    if len(files) > MAX_FILES:
        # 构造器会截断 + 置位；这里显式传入总数控 fileCount。
        truncated = ModelRepoInfo(
            repo_id=repo_id,
            revision=revision,
            size_bytes=size_bytes,
            file_count=file_count,
            files=tuple(files),
            files_truncated=True,
            gated=gated,
        )
        return truncated

    return ModelRepoInfo(
        repo_id=repo_id,
        revision=revision,
        size_bytes=size_bytes,
        file_count=file_count,
        files=tuple(files),
        files_truncated=False,
        gated=gated,
    )

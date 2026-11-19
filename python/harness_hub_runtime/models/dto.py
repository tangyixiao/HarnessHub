"""F23 S1 — models 契约层冻结 DTO 与常量（spec §2–§3.5）。

全部为 `@dataclass(frozen=True)`；`to_json()` 锁定跨边界的 camelCase 键名与顺序
（对齐 mcp/dto.py 风格，docs/specs/2026-11-09-f23-model-assets-design.md）。

截断 / 诚实纪律：
* `lastError` ≤ 256（`__post_init__` 统一执行，任何构造路径都安全）；
* `files` ≤ 64 —— 超出在构造时截断并置 `filesTruncated=True`，`fileCount`
  保持真实总数（不虚报）；
* 大小未知一律 `None`，绝不编造。

零依赖：只用 dataclasses / typing / __future__。
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any

# ---- 冻结常量（改动任何一项都是跨语言破坏性变更） ----

# spec §2.3 磁盘预算门：预估 > 2 GiB 必须显式 confirmedDiskBudget（对齐 AGENTS.md 规划默认）。
DISK_BUDGET_BYTES = 2 * 1024 * 1024 * 1024
# spec §3.3：files 列表上限，超出截断 + filesTruncated。
MAX_FILES = 64
# spec §3.5：lastError 上限。
MAX_LAST_ERROR = 256
# spec §3.2：下载状态集。
DOWNLOAD_STATES = ("idle", "preparing", "downloading", "completed", "failed", "cancelled")
# 下载通道（spec §3.5）：SDK 在场用 huggingface_hub，缺失退化为 stdlib 直下。
DOWNLOAD_VIAS = ("huggingface_hub", "stdlib")

# spec §2 红线 4：repo_id / revision 的字符白名单（防路径穿越与 header 注入）。
_IDENTIFIER_PATTERN = re.compile(r"^[A-Za-z0-9._/-]+$")


def _clip(text: str, limit: int) -> str:
    """按上限截断字符串（契约纪律，调用方无需再自查）。"""
    return text[:limit]


def _validate_identifier(value: Any, field: str) -> str:
    """repo_id / revision 共用校验：白名单字符 + 拒绝 `.` / `..` / 空段。

    错误信息必须点名字段（repoId / revision），便于上游把参数错误归位。
    """
    if not isinstance(value, str) or not value:
        raise ValueError(f"{field} 必须是非空字符串")
    if not _IDENTIFIER_PATTERN.match(value):
        raise ValueError(f"{field} 含非法字符（只允许 A-Za-z0-9._-/）：{value[:64]}")
    segments = value.split("/")
    for segment in segments:
        if segment in ("", ".", ".."):
            raise ValueError(f"{field} 含非法路径段（拒绝空段 / . / ..）：{value[:64]}")
    return value


def validate_repo_id(value: Any) -> str:
    """校验并原样返回 repoId（spec §2 红线 4）。"""
    return _validate_identifier(value, "repoId")


def validate_revision(value: Any) -> str:
    """校验并原样返回 revision（spec §2 红线 4）。"""
    return _validate_identifier(value, "revision")


def validate_file_list(value: Any) -> list[str] | None:
    """download.start 的 files 白名单：可选；逐项过同一条段规则；返回副本。

    空列表语义歧义（全量还是零文件？），fail-closed 拒绝。
    """
    if value is None:
        return None
    if not isinstance(value, list):
        raise ValueError("files 必须是字符串数组或 null")
    if not value:
        raise ValueError("files 不允许是空数组（省略即全量，如需限文件请逐个列出）")
    validated: list[str] = []
    for item in value:
        if not isinstance(item, str) or not item:
            raise ValueError("files 每一项必须是非空字符串")
        validated.append(_validate_identifier(item, "files"))
    return validated


@dataclass(frozen=True)
class ModelSdkStatus:
    """可选 SDK 可用性（spec §3.2）：探测（find_spec），不 import。"""

    huggingface_hub: bool

    def to_json(self) -> dict[str, Any]:
        return {"huggingfaceHub": self.huggingface_hub}


@dataclass(frozen=True)
class ModelDownloadState:
    """models.status 的 downloads 单项（spec §3.2：{repoId, revision, state, via}）。"""

    repo_id: str
    revision: str
    state: str
    via: str | None

    def __post_init__(self) -> None:
        if self.state not in DOWNLOAD_STATES:
            raise ValueError(f"非法下载状态：{self.state}")
        if self.via is not None and self.via not in DOWNLOAD_VIAS:
            raise ValueError(f"非法下载通道：{self.via}")

    def to_json(self) -> dict[str, Any]:
        return {
            "repoId": self.repo_id,
            "revision": self.revision,
            "state": self.state,
            "via": self.via,
        }


@dataclass(frozen=True)
class ModelFileInfo:
    """文件清单单项：路径 + 已知大小（未知为 None，不虚报）。"""

    path: str
    size_bytes: int | None

    def to_json(self) -> dict[str, Any]:
        return {"path": self.path, "sizeBytes": self.size_bytes}


@dataclass(frozen=True)
class ModelRepoInfo:
    """models.info 归一结果（spec §3.3 形状，键序冻结；via 恒为 "api"）。"""

    repo_id: str
    revision: str
    size_bytes: int | None
    file_count: int
    files: tuple[ModelFileInfo, ...]
    files_truncated: bool
    gated: bool

    def __post_init__(self) -> None:
        # 截断纪律在构造时统一执行：超限即截 + 置位，fileCount 保持真实总数。
        if len(self.files) > MAX_FILES:
            object.__setattr__(self, "files", self.files[:MAX_FILES])
            object.__setattr__(self, "files_truncated", True)

    def to_json(self) -> dict[str, Any]:
        return {
            "repoId": self.repo_id,
            "revision": self.revision,
            "sizeBytes": self.size_bytes,
            "fileCount": self.file_count,
            "files": [file.to_json() for file in self.files],
            "filesTruncated": self.files_truncated,
            "gated": self.gated,
            "via": "api",
        }


@dataclass(frozen=True)
class CacheEntry:
    """models.cache.list 单项（spec §3.4：单仓聚合）。"""

    repo_id: str
    revisions: tuple[str, ...]
    size_bytes: int
    last_used_at: str | None

    def to_json(self) -> dict[str, Any]:
        return {
            "repoId": self.repo_id,
            "revisions": list(self.revisions),
            "sizeBytes": self.size_bytes,
            "lastUsedAt": self.last_used_at,
        }


@dataclass(frozen=True)
class CacheScan:
    """缓存扫描结果（spec §3.4：≤128 项，扫描根如实回显）。"""

    entries: tuple[CacheEntry, ...]
    cache_root: str

    def to_json(self) -> dict[str, Any]:
        return {
            "entries": [entry.to_json() for entry in self.entries],
            "cacheRoot": self.cache_root,
        }


@dataclass(frozen=True)
class DownloadProgress:
    """models.download.progress 归一（spec §3.5 形状，键序冻结）。"""

    repo_id: str
    revision: str
    state: str
    bytes_done: int
    bytes_total: int | None
    file_done: int
    file_count: int
    current_file: str | None
    last_error: str | None

    def __post_init__(self) -> None:
        if self.last_error is not None:
            object.__setattr__(self, "last_error", _clip(self.last_error, MAX_LAST_ERROR))

    def to_json(self) -> dict[str, Any]:
        return {
            "repoId": self.repo_id,
            "revision": self.revision,
            "state": self.state,
            "bytesDone": self.bytes_done,
            "bytesTotal": self.bytes_total,
            "fileDone": self.file_done,
            "fileCount": self.file_count,
            "currentFile": self.current_file,
            "lastError": self.last_error,
        }

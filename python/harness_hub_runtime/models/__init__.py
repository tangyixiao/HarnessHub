"""HF 模型资产平面（F23 S1：HF 模型资产 + 显式下载，spec §0–§3.5）。

Harness Hub 对模型资产只有三类显式能力：元数据查询、缓存发现、显式下载。
transformers / torch 推理执行**永不**进入本包（spec §0 非目标），
`huggingface_hub` 保持 optional（find_spec 探测，绝不顶层 import）。

模块划分：

* `dto.py` —— 冻结 DTO 与常量（预算门、上限、标识校验）。
* `info.py` —— HF REST 元数据（stdlib urllib，10s 超时，opener 可注入）。
* `cache.py` —— 本地缓存只读扫描（零网络）与根路径解析。
* `downloads.py` —— 显式下载状态机（预算门 + .part 纪律 + 取消）。

零新增第三方依赖：元数据 / 缓存 / stdlib 下载全部纯 stdlib。
"""

from __future__ import annotations

from .dto import (
    DISK_BUDGET_BYTES,
    DOWNLOAD_STATES,
    DOWNLOAD_VIAS,
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

__all__: list[str] = [
    "DISK_BUDGET_BYTES",
    "DOWNLOAD_STATES",
    "DOWNLOAD_VIAS",
    "MAX_FILES",
    "MAX_LAST_ERROR",
    "CacheEntry",
    "CacheScan",
    "DownloadProgress",
    "ModelDownloadState",
    "ModelFileInfo",
    "ModelRepoInfo",
    "ModelSdkStatus",
    "validate_file_list",
    "validate_repo_id",
    "validate_revision",
]

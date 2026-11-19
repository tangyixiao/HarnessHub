"""F23 S1 — models.cache.list：本地 HF 缓存只读扫描（spec §3.4，零网络）。

布局（HF hub 约定）::

    <cache_root>/models--<org>--<name>/
      refs/<revision>          # 每个文件名 = 一个 revision 快照指针
      snapshots/<revision>/<path>   # 实际文件

扫描纪律：
* 只读；目录不存在 → 空扫描（不是错误）；
* 单仓文件数上限 20000，超出部分不计入（如实截断，不虚报）；
* entries 上限 128（按 repoId 排序后截取）；
* lastUsedAt 取 refs/ 下最新 mtime 的 rfc3339 UTC（拿不到 → None）。

根路径解析（spec §2.2）：`HUGGINGFACE_HUB_CACHE` > `HF_HOME`(+/hub) >
`~/.cache/huggingface/hub`。`HARNESS_HUB_MODELS_ROOT` 只覆盖下载目标，
不影响扫描根（下载到 override 的资产不会出现在本扫描里，如实行为）。
"""

from __future__ import annotations

import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Mapping

from .dto import CacheEntry, CacheScan

# spec §3.4 冻结上限。
MAX_ENTRIES = 128
MAX_FILES_PER_REPO = 20000

_REPO_DIR_PREFIX = "models--"


def resolve_cache_root(env: Mapping[str, str] | None = None) -> Path:
    """HF 缓存根（扫描用）：HUGGINGFACE_HUB_CACHE > HF_HOME/hub > 缺省路径。"""
    environment = os.environ if env is None else env
    explicit_cache = environment.get("HUGGINGFACE_HUB_CACHE")
    if explicit_cache:
        return Path(explicit_cache)
    hf_home = environment.get("HF_HOME")
    if hf_home:
        return Path(hf_home) / "hub"
    return Path.home() / ".cache" / "huggingface" / "hub"


def resolve_download_root(env: Mapping[str, str] | None = None) -> Path:
    """下载目标根（spec §2.2 写路径锁死）：`HARNESS_HUB_MODELS_ROOT` 显式覆盖，
    否则 = HF 缓存根。任何其他路径在 Rust 下发前就要被拒绝。"""
    environment = os.environ if env is None else env
    override = environment.get("HARNESS_HUB_MODELS_ROOT")
    if override:
        return Path(override)
    return resolve_cache_root(environment)


def _repo_id_from_dir_name(name: str) -> str | None:
    """`models--<org>--<name>` → `org/name`；name 内的 `--` 保留（诚实重建）。"""
    if not name.startswith(_REPO_DIR_PREFIX):
        return None
    remainder = name[len(_REPO_DIR_PREFIX) :]
    parts = remainder.split("--")
    if len(parts) < 2 or not parts[0] or not parts[1]:
        return None
    return f"{parts[0]}/{'--'.join(parts[1:])}"


def _sum_tree_size(root: Path) -> int:
    """snapshots 树求和；文件数超上限即停（超出部分不计入，如实截断）。"""
    total = 0
    seen_files = 0
    stack = [root]
    while stack:
        current = stack.pop()
        try:
            entries = list(current.iterdir())
        except OSError:
            continue
        for entry in entries:
            try:
                if entry.is_dir():
                    stack.append(entry)
                    continue
                if entry.is_file():
                    seen_files += 1
                    if seen_files > MAX_FILES_PER_REPO:
                        return total
                    total += entry.stat().st_size
            except OSError:
                continue
    return total


def _latest_refs_mtime(refs_dir: Path) -> str | None:
    """refs/ 下最新 mtime → rfc3339 UTC（Z 结尾）；拿不到 → None。"""
    try:
        stamps = [entry.stat().st_mtime for entry in refs_dir.iterdir() if entry.is_file()]
    except OSError:
        return None
    if not stamps:
        return None
    latest = datetime.fromtimestamp(max(stamps), tz=timezone.utc)
    return latest.strftime("%Y-%m-%dT%H:%M:%SZ")


def scan_cache(root: Path) -> CacheScan:
    """只读扫描 HF hub 缓存根（零网络，spec §3.4）。"""
    entries: list[CacheEntry] = []
    if root.is_dir():
        try:
            children = sorted(root.iterdir())
        except OSError:
            children = []
        for child in children:
            repo_id = _repo_id_from_dir_name(child.name)
            if repo_id is None or not child.is_dir():
                continue
            refs_dir = child / "refs"
            revisions: tuple[str, ...] = tuple()
            if refs_dir.is_dir():
                try:
                    revisions = tuple(
                        sorted(entry.name for entry in refs_dir.iterdir() if entry.is_file())
                    )
                except OSError:
                    revisions = tuple()
            snapshots_dir = child / "snapshots"
            size_bytes = _sum_tree_size(snapshots_dir) if snapshots_dir.is_dir() else 0
            entries.append(
                CacheEntry(
                    repo_id=repo_id,
                    revisions=revisions,
                    size_bytes=size_bytes,
                    last_used_at=_latest_refs_mtime(refs_dir),
                )
            )

    entries.sort(key=lambda entry: entry.repo_id)
    return CacheScan(entries=tuple(entries[:MAX_ENTRIES]), cache_root=str(root))

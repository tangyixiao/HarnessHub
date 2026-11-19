"""F23 S1 — models.download：显式下载状态机（spec §2.2–§2.3 / §3.5）。

架构归属（spec §1）：Python 拥有连接与传输（这里是下载执行与状态机），
Rust 拥有 IPC DTO 与预算确认门的入口；预算判定所需预估来自 `models.info`
的 siblings 大小求和（拿不到按未知，不虚报）。

状态机：`preparing → downloading → completed | failed | cancelled`。

红线：
* 目标路径锁死 —— 只写 `<target_root>/models--<org>--<name>/snapshots/<revision>/`；
* 半截文件绝不冒充完整资产 —— 进行中写 `<file>.part`，取消/失败即删除；
* 跨重启任务不恢复（进程内状态机，spec §3.5）；
* 取消在文件边界生效（SDK 路径），stdlib 路径在 chunk 级生效。

依赖注入（spec §6）：`info_fetcher` / `sdk_probe` / `executor` 全部可注入，
单元测试不发网络、不 import 真实 SDK。
"""

from __future__ import annotations

import threading
import uuid
import urllib.request
from collections import OrderedDict
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from .dto import (
    DISK_BUDGET_BYTES,
    MAX_LAST_ERROR,
    DownloadProgress,
    ModelDownloadState,
    validate_file_list,
    validate_repo_id,
    validate_revision,
)

# 进行中的状态（重复下载判定，spec §3.5：同一 (repoId, revision) → -32005）。
_IN_FLIGHT_STATES = ("preparing", "downloading")
_TERMINAL_STATES = ("completed", "failed", "cancelled")
# 终态任务保留上限：models.status 的 downloads 列表不会无限增长。
_MAX_KEPT_TASKS = 64
# 单文件下载超时（stdlib 路径）。
_DOWNLOAD_TIMEOUT_SECONDS = 30.0
# chunk 大小（stdlib 路径）。
_CHUNK_SIZE = 64 * 1024


class DuplicateDownloadError(Exception):
    """同一 (repoId, revision) 已有进行中下载。由 server 层映射为 -32005。"""


class CancelledError(Exception):
    """执行器在观测到取消信号时抛出；状态机据此收敛为 cancelled。"""


@dataclass(frozen=True)
class DownloadPlan:
    """一次下载的执行计划（executor 的唯一输入）。"""

    repo_id: str
    revision: str
    files: tuple[str, ...]
    via: str
    target_root: Path
    source_urls: tuple[str, ...]


class _Task:
    """进程内下载任务；字段只在 `_state_lock` 下读写。"""

    def __init__(self, download_id: str, repo_id: str, revision: str, plan: DownloadPlan) -> None:
        self.id = download_id
        self.repo_id = repo_id
        self.revision = revision
        self.plan = plan
        self.state = "preparing"
        self.bytes_done = 0
        self.bytes_total: int | None = None
        self.file_done = 0
        self.file_count = len(plan.files)
        self.current_file: str | None = None
        self.last_error: str | None = None
        self.cancel_event = threading.Event()
        self.thread: threading.Thread | None = None

    def snapshot(self) -> DownloadProgress:
        return DownloadProgress(
            repo_id=self.repo_id,
            revision=self.revision,
            state=self.state,
            bytes_done=self.bytes_done,
            bytes_total=self.bytes_total,
            file_done=self.file_done,
            file_count=self.file_count,
            current_file=self.current_file,
            last_error=self.last_error,
        )


def _repo_dirname(repo_id: str) -> str:
    """`org/name` → `models--org--name`（HF hub 布局；单段 repo → `models--name`）。"""
    parts = repo_id.split("/")
    if len(parts) == 1:
        return f"models--{parts[0]}"
    return f"models--{parts[0]}--{'--'.join(parts[1:])}"


def _clip_error(error: Exception) -> str:
    text = f"{type(error).__name__}: {error}".splitlines()[0]
    return text[:MAX_LAST_ERROR]


# ---- 缺省执行器（stdlib 直下 / huggingface_hub 懒加载） ----


def _execute_stdlib(plan: DownloadPlan, progress: Callable[..., None], cancel_event: threading.Event) -> None:
    """stdlib 单文件直下：`resolve/<revision>/<file>` 直链（spec §1 退化路径）。

    逐 chunk 写 `<file>.part`，完成即原子 rename；chunk 级响应取消。
    """
    total_downloaded = 0
    for index, (relative, url) in enumerate(zip(plan.files, plan.source_urls)):
        if cancel_event.is_set():
            raise CancelledError("cancelled")
        destination = plan.target_root / _repo_dirname(plan.repo_id) / "snapshots" / plan.revision / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        part_path = destination.with_name(destination.name + ".part")
        progress(0, None, index, len(plan.files), relative)
        downloaded = 0
        with urllib.request.urlopen(url, timeout=_DOWNLOAD_TIMEOUT_SECONDS) as response, part_path.open("wb") as sink:  # noqa: S310 - URL 由 Rust 校验过的白名单目标派生
            while True:
                if cancel_event.is_set():
                    raise CancelledError("cancelled")
                chunk = response.read(_CHUNK_SIZE)
                if not chunk:
                    break
                sink.write(chunk)
                downloaded += len(chunk)
                total_downloaded += len(chunk)
                progress(total_downloaded, None, index, len(plan.files), relative)
        part_path.replace(destination)
        progress(total_downloaded, total_downloaded, index + 1, len(plan.files), relative)


def _execute_with_sdk(plan: DownloadPlan, progress: Callable[..., None], cancel_event: threading.Event) -> None:
    """huggingface_hub 路径：逐文件 `hf_hub_download`（取消在文件边界生效）。"""
    try:
        from huggingface_hub import hf_hub_download
    except Exception as error:  # noqa: BLE001 - 竞态下的 SDK 缺失如实失败
        raise RuntimeError(f"huggingface_hub 不可用：{error}") from error

    accumulated = 0
    for index, relative in enumerate(plan.files):
        if cancel_event.is_set():
            raise CancelledError("cancelled")
        local = hf_hub_download(
            repo_id=plan.repo_id,
            filename=relative,
            revision=plan.revision,
            cache_dir=str(plan.target_root),
        )
        accumulated += Path(local).stat().st_size
        progress(accumulated, None, index + 1, len(plan.files), relative)


def _make_default_executor() -> Callable[[DownloadPlan, Callable[..., None], threading.Event], None]:
    def executor(plan: DownloadPlan, progress: Callable[..., None], cancel_event: threading.Event) -> None:
        if plan.via == "huggingface_hub":
            _execute_with_sdk(plan, progress, cancel_event)
        else:
            _execute_stdlib(plan, progress, cancel_event)

    return executor


# ---- 状态机 ----


class DownloadManager:
    """显式下载状态机（线程安全）。

    Args:
        cache_root: 下载目标根（spec §2.2：HF 缓存根或 `HARNESS_HUB_MODELS_ROOT` 覆盖）。
        info_fetcher: `(repo_id, revision) -> 信息对象`，需要 `.size_bytes` 与
            `.files`（`.path` 属性序列）；缺省走 `models.info.fetch_repo_info`。
        sdk_probe: `() -> bool`；缺省 find_spec 探测 `huggingface_hub`（绝不 import）。
        executor: `(plan, progress, cancel_event) -> None`；缺省按 `via` 分派
            stdlib / SDK 实现（测试注入 fake）。
    """

    def __init__(
        self,
        cache_root: Path,
        info_fetcher: Callable[[str, str], Any] | None = None,
        sdk_probe: Callable[[], bool] | None = None,
        executor: Callable[[DownloadPlan, Callable[..., None], threading.Event], None] | None = None,
    ) -> None:
        self.cache_root = Path(cache_root)
        self._info_fetcher = info_fetcher
        self._sdk_probe = sdk_probe if sdk_probe is not None else self._default_sdk_probe
        self._executor = executor if executor is not None else _make_default_executor()
        self._tasks: OrderedDict[str, _Task] = OrderedDict()
        self._registry_lock = threading.Lock()
        self._state_lock = threading.Lock()

    @staticmethod
    def _default_sdk_probe() -> bool:
        import importlib.util

        try:
            return importlib.util.find_spec("huggingface_hub") is not None
        except Exception:  # noqa: BLE001 - 探测失败一律视为不可用
            return False

    def _fetch_info(self, repo_id: str, revision: str) -> Any:
        if self._info_fetcher is not None:
            return self._info_fetcher(repo_id, revision)
        from .info import fetch_repo_info

        return fetch_repo_info(repo_id, revision)

    # ---------- start ----------

    def start(
        self,
        repo_id: str,
        revision: str,
        files: list[str] | None = None,
        confirmed_disk_budget: bool = False,
    ) -> dict[str, Any]:
        """发起显式下载；预算门不满足时返回 needsConfirmation（不创建任务）。"""
        repo_id = validate_repo_id(repo_id)
        revision = validate_revision(revision)
        file_whitelist = validate_file_list(files)

        with self._registry_lock:
            for task in self._tasks.values():
                if (
                    task.repo_id == repo_id
                    and task.revision == revision
                    and task.state in _IN_FLIGHT_STATES
                ):
                    raise DuplicateDownloadError(f"{repo_id}@{revision} 已有进行中下载")

        # 预估与文件清单都来自一次元数据查询（网络，显式动作的一部分）。
        info = self._fetch_info(repo_id, revision)
        estimate = getattr(info, "size_bytes", None)
        if (
            isinstance(estimate, int)
            and estimate > DISK_BUDGET_BYTES
            and not confirmed_disk_budget
        ):
            return {"needsConfirmation": True, "estimatedBytes": estimate}

        if file_whitelist is not None:
            chosen = tuple(file_whitelist)
        else:
            chosen = tuple(
                file.path
                for file in getattr(info, "files", ())
                if getattr(file, "path", None)
            )
        via = "huggingface_hub" if self._sdk_probe() else "stdlib"
        source_urls = tuple(
            f"https://huggingface.co/{repo_id}/resolve/{revision}/{file}" for file in chosen
        )
        plan = DownloadPlan(
            repo_id=repo_id,
            revision=revision,
            files=chosen,
            via=via,
            target_root=self.cache_root,
            source_urls=source_urls,
        )

        task = _Task(uuid.uuid4().hex, repo_id, revision, plan)
        with self._registry_lock:
            self._prune_finished_locked()
            self._tasks[task.id] = task
        thread = threading.Thread(target=self._run, args=(task,), daemon=True)
        with self._state_lock:
            task.thread = thread
        thread.start()
        return {"downloadId": task.id, "state": "preparing"}

    def _prune_finished_locked(self) -> None:
        """超过保留上限时淘汰最老的终态任务（注册表不无限增长）。"""
        if len(self._tasks) < _MAX_KEPT_TASKS:
            return
        for task_id, task in list(self._tasks.items()):
            if task.state in _TERMINAL_STATES:
                del self._tasks[task_id]
            if len(self._tasks) < _MAX_KEPT_TASKS:
                return

    # ---------- worker ----------

    def _run(self, task: _Task) -> None:
        with self._state_lock:
            if task.state == "preparing":
                task.state = "downloading"

        def progress(
            bytes_done: int,
            bytes_total: int | None,
            file_done: int,
            file_count: int,
            current_file: str | None,
        ) -> None:
            with self._state_lock:
                task.bytes_done = int(bytes_done)
                task.bytes_total = None if bytes_total is None else int(bytes_total)
                task.file_done = int(file_done)
                task.file_count = int(file_count)
                task.current_file = current_file

        try:
            self._executor(task.plan, progress, task.cancel_event)
        except CancelledError:
            self._finish(task, "cancelled", None)
        except Exception as error:  # noqa: BLE001 - 状态机兜底：失败收敛为 failed
            self._finish(task, "failed", _clip_error(error))
        else:
            self._finish(task, "completed", None)

    def _finish(self, task: _Task, state: str, last_error: str | None) -> None:
        with self._state_lock:
            task.state = state
            task.last_error = last_error
        if state in ("cancelled", "failed"):
            self._remove_part_files(task.plan)

    @staticmethod
    def _remove_part_files(plan: DownloadPlan) -> None:
        """取消/失败后删除 `<file>.part`（绝不留半截文件冒充完整资产）。"""
        base = plan.target_root / _repo_dirname(plan.repo_id) / "snapshots" / plan.revision
        for relative in plan.files:
            part = base / relative
            try:
                part.with_name(part.name + ".part").unlink(missing_ok=True)
            except OSError:
                continue

    # ---------- progress / cancel / states ----------

    def progress(self, download_id: str) -> dict[str, Any]:
        with self._registry_lock:
            task = self._tasks.get(download_id)
        if task is None:
            raise ValueError("未知 downloadId")
        with self._state_lock:
            return task.snapshot().to_json()

    def cancel(self, download_id: str) -> dict[str, Any]:
        with self._registry_lock:
            task = self._tasks.get(download_id)
        if task is None:
            raise ValueError("未知 downloadId")
        task.cancel_event.set()
        thread = task.thread
        if thread is not None and thread.is_alive() and thread is not threading.current_thread():
            # 有界等待执行器收敛（stdlib chunk 级 / SDK 文件边界），让幂等取消
            # 的返回值如实反映终态；超时则如实返回当前状态。
            thread.join(timeout=2.0)
        return self.progress(download_id)

    def states(self) -> list[dict[str, Any]]:
        with self._registry_lock:
            tasks = list(self._tasks.values())
        states: list[dict[str, Any]] = []
        for task in tasks:
            with self._state_lock:
                snapshot = ModelDownloadState(
                    repo_id=task.repo_id,
                    revision=task.revision,
                    state=task.state,
                    via=task.plan.via,
                )
            states.append(snapshot.to_json())
        return states

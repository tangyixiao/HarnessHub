import { useEffect, useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import {
  isTauriRuntime,
  modelsCacheList,
  modelsDownloadCancel,
  modelsDownloadProgress,
  modelsDownloadStart,
  modelsInfo,
  modelsStatus,
  type ModelDownloadProgress,
  type ModelDownloadStartOutcome,
  type ModelsCacheListing,
  type ModelsStatus,
} from '@/lib/ipc';

const TERMINAL_DOWNLOAD_STATES = new Set(['completed', 'failed', 'cancelled']);

/** 字节数的可读展示（GiB / MiB 自动换档；未知如实显示「未知大小」）。 */
function formatBytes(bytes: number | null): string {
  const GIB = 1024 * 1024 * 1024;
  const MIB = 1024 * 1024;
  if (bytes === null) {
    return '未知大小';
  }
  if (bytes >= GIB) {
    return `${(bytes / GIB).toFixed(2)} GiB`;
  }
  if (bytes >= MIB) {
    return `${(bytes / MIB).toFixed(1)} MiB`;
  }
  return `${bytes} B`;
}

/**
 * 模型资产卡片（F23 spec §3.7）：HF 元数据查询、本地缓存清单、显式下载。
 *
 * 纪律（AGENTS.md / spec §2 / §3.7）：
 * - 渲染路径零 IPC：无 useEffect 自动拉取，只在用户点按钮时调用 @/lib/ipc 的 wrapper；
 * - 组件只 import @/lib/ipc，禁止直接 invoke；
 * - 下载是**显式网络动作**：先 confirm（文案明示「下载走网络、目标为 HF 缓存」）；
 *   预估 > 2 GiB（spec §2.3 磁盘预算门）时后端返回 needsConfirmation，二次 confirm
 *   文案**必须含字节数**，以 `confirmedDiskBudget: true` 重发；
 * - 进度轮询 1s 间隔，**仅在卡片展开且有进行中下载时**轮询（effect cleanup 停止）；
 * - 取消/失败不留半截文件（`.part` 纪律在 Python 侧，UI 只如实展示状态）；
 * - sidecar 未就绪（available=false）时显示 unavailableReason，禁用全部动作。
 */
export function ModelAssetsCard() {
  const connected = isTauriRuntime();
  const [status, setStatus] = useState<ModelsStatus | null>(null);
  const [cache, setCache] = useState<ModelsCacheListing | null>(null);
  const [repoId, setRepoId] = useState('');
  const [revision, setRevision] = useState('main');
  const [filesText, setFilesText] = useState('');
  const [formError, setFormError] = useState<string | null>(null);
  const [infoSummary, setInfoSummary] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<ModelDownloadStartOutcome | null>(null);
  const [progress, setProgress] = useState<ModelDownloadProgress | null>(null);
  const [activeDownloadId, setActiveDownloadId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const runtimeReady = status !== null && status.available;

  // 进度轮询（spec §3.7）：1s 间隔，仅在卡片展开且有进行中下载时轮询；
  // 终态 / 组件卸载 / 下载 id 变化即停止（cleanup 清 interval）。
  useEffect(() => {
    if (activeDownloadId === null) {
      return;
    }
    let cancelled = false;
    const interval = window.setInterval(() => {
      if (cancelled) {
        return;
      }
      void (async () => {
        const result = await modelsDownloadProgress(activeDownloadId);
        if (!result.ok || cancelled) {
          return;
        }
        const next = result.data;
        if (!next.available || next.progress === null) {
          return;
        }
        setProgress(next.progress);
        if (TERMINAL_DOWNLOAD_STATES.has(next.progress.state)) {
          setActiveDownloadId(null);
        }
      })();
    }, 1000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [activeDownloadId]);

  const refresh = async () => {
    setBusy(true);
    setError(null);
    const [statusResult, cacheResult] = await Promise.all([modelsStatus(), modelsCacheList()]);
    setBusy(false);
    if (!statusResult.ok) {
      setError(statusResult.error);
      return;
    }
    if (!cacheResult.ok) {
      setError(cacheResult.error);
      return;
    }
    setStatus(statusResult.data);
    setCache(cacheResult.data);
  };

  /** 表单解析：files 文本 → 白名单数组（空文本 = null 全量）；'invalid' 表示解析失败。 */
  const parseFiles = (): string[] | null | 'invalid' => {
    const text = filesText.trim();
    if (text.length === 0) {
      return null;
    }
    const files = text
      .split(',')
      .map((file) => file.trim())
      .filter((file) => file.length > 0);
    if (files.length === 0) {
      return 'invalid';
    }
    return files;
  };

  /** 发起一次 start；返回 outcome（IPC 层失败返回 null，error 已 setState）。 */
  const requestStart = async (
    confirmedDiskBudget: boolean,
  ): Promise<ModelDownloadStartOutcome | null> => {
    const files = parseFiles();
    if (files === 'invalid') {
      setFormError('文件白名单为空：留空表示全量，或用逗号分隔具体文件名');
      return null;
    }
    setFormError(null);
    setBusy(true);
    setError(null);
    const result = await modelsDownloadStart(
      repoId.trim(),
      revision.trim(),
      files,
      confirmedDiskBudget,
    );
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return null;
    }
    const next = result.data;
    setOutcome(next);
    if (next.state === 'preparing' && next.downloadId !== null) {
      setProgress(null);
      setActiveDownloadId(next.downloadId);
    }
    return next;
  };

  const beginDownload = async () => {
    if (repoId.trim().length === 0 || revision.trim().length === 0) {
      setFormError('repoId 与 revision 必填（revision 不假设 main）');
      return;
    }
    setFormError(null);
    // 显式动作第一道 confirm：明示网络下载与目标目录（spec §3.7）。
    const confirmed = window.confirm(
      `将发起网络下载：${repoId.trim()}@${revision.trim()}（走网络，目标为 HF 缓存目录，绝不写项目目录）。确认开始？`,
    );
    if (!confirmed) {
      return;
    }
    const first = await requestStart(false);
    // 预算门命中 → 第二道 confirm 必须含字节数（spec §2.3 / §3.7）。
    if (first === null || first.state !== 'needsConfirmation') {
      return;
    }
    const message = `预估下载体积 ${formatBytes(first.estimatedBytes)}，超过 2 GiB 磁盘预算。下载走网络，目标为 HF 缓存目录。确认继续？`;
    if (!window.confirm(message)) {
      return;
    }
    await requestStart(true);
  };

  const queryInfo = async () => {
    if (repoId.trim().length === 0 || revision.trim().length === 0) {
      setFormError('repoId 与 revision 必填（revision 不假设 main）');
      return;
    }
    setFormError(null);
    setBusy(true);
    setError(null);
    // 元数据查询是显式网络动作：confirm 明示将访问 huggingface.co（仅元数据）。
    const confirmed = window.confirm(
      `将访问 huggingface.co 查询 ${repoId.trim()}@${revision.trim()} 的元数据（仅元数据，不下载文件）。继续？`,
    );
    if (!confirmed) {
      setBusy(false);
      return;
    }
    const result = await modelsInfo(repoId.trim(), revision.trim());
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    if (!result.data.available || result.data.info === null) {
      setError(result.data.unavailableReason ?? '元数据查询失败');
      return;
    }
    const info = result.data.info;
    setInfoSummary(
      `${info.repoId}@${info.revision}：${info.fileCount} 个文件，预估 ${formatBytes(info.sizeBytes)}${
        info.gated ? '（gated 仓库，可能需要 HF_TOKEN）' : ''
      }${info.filesTruncated ? '（文件清单已截断）' : ''}`,
    );
  };

  const cancelDownload = async () => {
    if (activeDownloadId === null) {
      return;
    }
    setBusy(true);
    const result = await modelsDownloadCancel(activeDownloadId);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    if (result.data.available) {
      setActiveDownloadId(null);
    } else {
      setError(result.data.unavailableReason ?? '取消失败');
    }
  };

  const downloading = activeDownloadId !== null;

  return (
    <Card className="mb-6" data-testid="model-assets-card">
      <CardTitle>模型资产</CardTitle>
      <CardDescription>
        HF 模型资产（F23）：元数据查询、本地缓存发现、显式下载（revision pinning）。
        下载走网络、目标为 HF 缓存目录；transformers 推理执行不在本阶段（如实降级）。
      </CardDescription>

      <div className="mt-3 flex flex-wrap gap-2">
        <Button
          variant="secondary"
          size="sm"
          onClick={() => void refresh()}
          disabled={!connected || busy}
          data-testid="model-assets-refresh"
        >
          加载状态
        </Button>
      </div>

      <div className="mt-4" aria-live="polite">
        {busy ? <p className="text-xs text-content-muted">正在执行…</p> : null}

        {error !== null ? (
          <p className="text-xs text-signal-error" role="alert">
            {error}
          </p>
        ) : null}

        {status !== null && !status.available ? (
          <p className="text-xs text-signal-error" data-testid="model-assets-unavailable">
            {status.unavailableReason ?? '模型资产功能不可用'}
          </p>
        ) : null}

        {status !== null && runtimeReady ? (
          <div className="space-y-1 text-xs" data-testid="model-assets-status">
            <p className="text-content-muted">
              huggingface_hub SDK：
              <span data-testid="model-assets-sdk">
                {status.sdk.huggingfaceHub ? '可用' : '未安装（下载退化为 stdlib 直下）'}
              </span>
            </p>
            <p className="text-content-muted">
              缓存/下载根：<span className="font-mono">{status.cacheRoot ?? '未知'}</span>
            </p>
            {status.downloads.length > 0 ? (
              <div data-testid="model-assets-downloads">
                <p className="text-content-muted">本进程下载记录（跨重启不恢复，如实标注）：</p>
                {status.downloads.map((download) => (
                  <p
                    key={`${download.repoId}@${download.revision}`}
                    className="font-mono"
                    data-testid={`model-assets-download-row-${download.repoId}`}
                  >
                    {download.repoId}@{download.revision} — {download.state}
                    {download.via !== null ? `（${download.via}）` : ''}
                  </p>
                ))}
              </div>
            ) : null}
          </div>
        ) : null}

        {cache !== null && cache.available ? (
          <div className="mt-3 space-y-1 text-xs" data-testid="model-assets-cache">
            <p className="text-content-muted">
              本地缓存清单（只读扫描，扫描根{' '}
              <span className="font-mono">{cache.cacheRoot ?? '未知'}</span>）：
            </p>
            {cache.entries.length === 0 ? (
              <p className="text-content-muted" data-testid="model-assets-cache-empty">
                缓存为空（没有已下载的模型资产）。
              </p>
            ) : (
              cache.entries.map((entry) => (
                <div
                  key={entry.repoId}
                  className="font-mono"
                  data-testid={`model-assets-cache-${entry.repoId}`}
                >
                  {entry.repoId} — {formatBytes(entry.sizeBytes)} — revisions:{' '}
                  {entry.revisions.length > 0 ? entry.revisions.join(', ') : '（refs 缺失）'}
                  {entry.lastUsedAt !== null ? ` — 最近使用 ${entry.lastUsedAt}` : ''}
                </div>
              ))
            )}
          </div>
        ) : null}

        {cache !== null && !cache.available ? (
          <p className="text-xs text-signal-error" data-testid="model-assets-cache-unavailable">
            {cache.unavailableReason ?? '缓存清单不可用'}
          </p>
        ) : null}

        {runtimeReady ? (
          <div className="mt-4 space-y-1 text-xs" data-testid="model-assets-download-form">
            <label className="block text-content-muted" htmlFor="model-assets-repo-input">
              repoId（如 org/model-name）
            </label>
            <input
              id="model-assets-repo-input"
              className="w-80 rounded border border-white/20 bg-white/[0.05] px-2 py-1 font-mono"
              value={repoId}
              onChange={(event) => {
                setRepoId(event.target.value);
                setFormError(null);
              }}
              data-testid="model-assets-repo-input"
            />
            <label className="block text-content-muted" htmlFor="model-assets-revision-input">
              revision（不假设 main）
            </label>
            <input
              id="model-assets-revision-input"
              className="w-80 rounded border border-white/20 bg-white/[0.05] px-2 py-1 font-mono"
              value={revision}
              onChange={(event) => {
                setRevision(event.target.value);
                setFormError(null);
              }}
              data-testid="model-assets-revision-input"
            />
            <label className="block text-content-muted" htmlFor="model-assets-files-input">
              文件白名单（可选，逗号分隔；留空 = 全量）
            </label>
            <input
              id="model-assets-files-input"
              className="w-80 rounded border border-white/20 bg-white/[0.05] px-2 py-1 font-mono"
              value={filesText}
              onChange={(event) => {
                setFilesText(event.target.value);
                setFormError(null);
              }}
              placeholder="model.safetensors, tokenizer.json"
              data-testid="model-assets-files-input"
            />
            {formError !== null ? (
              <p className="text-signal-error" role="alert" data-testid="model-assets-form-error">
                {formError}
              </p>
            ) : null}

            <div className="mt-2 flex flex-wrap gap-2">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => void beginDownload()}
                disabled={!connected || busy || downloading || repoId.trim().length === 0}
                data-testid="model-assets-download"
              >
                下载
              </Button>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => void queryInfo()}
                disabled={!connected || busy || downloading || repoId.trim().length === 0}
                data-testid="model-assets-info"
              >
                查询元数据
              </Button>
              {downloading ? (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => void cancelDownload()}
                  disabled={busy}
                  data-testid="model-assets-cancel"
                >
                  取消下载
                </Button>
              ) : null}
            </div>
          </div>
        ) : null}

        {infoSummary !== null ? (
          <p className="mt-2 text-xs text-content-muted" data-testid="model-assets-info-summary">
            {infoSummary}
          </p>
        ) : null}

        {progress !== null ? (
          <div className="mt-2 space-y-1 text-xs" data-testid="model-assets-progress">
            <p className="font-mono">
              {progress.repoId}@{progress.revision} — {progress.state}
            </p>
            <p className="text-content-muted">
              {formatBytes(progress.bytesDone)}
              {progress.bytesTotal !== null
                ? ` / ${formatBytes(progress.bytesTotal)}`
                : '（总量未知）'}
              {progress.fileCount > 0 ? ` — 文件 ${progress.fileDone}/${progress.fileCount}` : ''}
              {progress.currentFile !== null ? ` — ${progress.currentFile}` : ''}
            </p>
            {progress.lastError !== null ? (
              <p
                className="text-signal-error"
                role="alert"
                data-testid="model-assets-progress-error"
              >
                {progress.lastError}
              </p>
            ) : null}
          </div>
        ) : null}

        {outcome !== null && outcome.state === 'needsConfirmation' ? (
          <p className="text-xs text-content-muted" data-testid="model-assets-needs-confirmation">
            预估 {formatBytes(outcome.estimatedBytes)}，超过 2 GiB 磁盘预算——等待确认。
          </p>
        ) : null}

        {outcome !== null && outcome.state === 'unavailable' ? (
          <p className="text-xs text-signal-error" data-testid="model-assets-start-unavailable">
            {outcome.reason ?? '下载不可用'}
          </p>
        ) : null}

        {outcome !== null && outcome.state === 'error' ? (
          <p className="text-xs text-signal-error" data-testid="model-assets-start-error">
            {outcome.reason ?? '下载发起失败'}
          </p>
        ) : null}
      </div>
    </Card>
  );
}

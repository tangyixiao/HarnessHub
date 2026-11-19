import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ModelAssetsCard } from '@/features/settings/ModelAssetsCard';

type Internals = { __TAURI_INTERNALS__?: unknown };

/**
 * 按命令名分派，便于验证「哪个命令在什么时候被调用」（与 McpRuntimeCard.test.tsx
 * 的 stubCommands 同款）。值可以是直接返回的数据，也可以是惰性求值的函数。
 */
function stubCommands(
  handlers: Record<string, unknown | (() => unknown)>,
): ReturnType<typeof vi.fn> {
  const invoke = vi.fn(async (command: string) => {
    if (!(command in handlers)) {
      throw new Error(`未预期的命令：${command}`);
    }
    const handler = handlers[command];
    return typeof handler === 'function' ? (handler as () => unknown)() : handler;
  });
  (window as unknown as Internals).__TAURI_INTERNALS__ = { invoke };
  return invoke;
}

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

/* ------------------------------------------------------------------ *
 * fixtures —— 与 Rust serde camelCase 契约同形（PR #76 逐键锁定）。
 * ------------------------------------------------------------------ */

/** `models_status`：sidecar 就绪、SDK 未安装（stdlib 退化路径如实展示）。 */
const MODELS_STATUS_PAYLOAD = {
  available: true,
  unavailableReason: null,
  sdk: { huggingfaceHub: false },
  cacheRoot: 'C:/Users/t/.cache/huggingface/hub',
  downloads: [{ repoId: 'org/small', revision: 'main', state: 'completed', via: 'stdlib' }],
  protocolVersion: '1.0',
};

/** sidecar 未就绪时的形状（spec §3.6：unavailable 默认值）。 */
const MODELS_STATUS_UNAVAILABLE_PAYLOAD = {
  available: false,
  unavailableReason: 'Python sidecar 未就绪，模型资产状态不可用',
  sdk: { huggingfaceHub: false },
  cacheRoot: null,
  downloads: [],
  protocolVersion: null,
};

/** `models_cache_list`：两个已缓存资产（含 refs 缺失的形态）。 */
const MODELS_CACHE_PAYLOAD = {
  available: true,
  unavailableReason: null,
  cacheRoot: 'C:/Users/t/.cache/huggingface/hub',
  entries: [
    {
      repoId: 'org/small',
      revisions: ['main'],
      sizeBytes: 1048576,
      lastUsedAt: '2026-11-19T00:00:00Z',
    },
    { repoId: 'org/bare', revisions: [], sizeBytes: 0, lastUsedAt: null },
  ],
};

/** `models_download_start`：直接进入 preparing（小文件，不触发预算门）。 */
const DOWNLOAD_STARTED_PAYLOAD = {
  state: 'preparing',
  downloadId: 'dl-42',
  estimatedBytes: null,
  reason: null,
};

/** `models_download_start`：预算门命中（> 2 GiB 且未确认，spec §2.3）。 */
const DOWNLOAD_NEEDS_CONFIRMATION_PAYLOAD = {
  state: 'needsConfirmation',
  downloadId: null,
  estimatedBytes: 2147483649,
  reason: null,
};

/** `models_download_start`：sidecar 中途掉线（spec §3.6）。 */
const DOWNLOAD_UNAVAILABLE_PAYLOAD = {
  state: 'unavailable',
  downloadId: null,
  estimatedBytes: null,
  reason: 'models.download.start 调用失败：sidecar 未启动',
};

/** `models_download_progress`：下载中（有总量）。 */
const DOWNLOAD_PROGRESS_PAYLOAD = {
  available: true,
  unavailableReason: null,
  progress: {
    repoId: 'org/small',
    revision: 'main',
    state: 'downloading',
    bytesDone: 512,
    bytesTotal: 1024,
    fileDone: 0,
    fileCount: 1,
    currentFile: 'model.safetensors',
    lastError: null,
  },
};

/** `models_download_progress`：到达终态（completed）。 */
const DOWNLOAD_COMPLETED_PAYLOAD = {
  available: true,
  unavailableReason: null,
  progress: {
    repoId: 'org/small',
    revision: 'main',
    state: 'completed',
    bytesDone: 1024,
    bytesTotal: 1024,
    fileDone: 1,
    fileCount: 1,
    currentFile: 'model.safetensors',
    lastError: null,
  },
};

/** `models_download_cancel`：幂等取消成功。 */
const DOWNLOAD_CANCELLED_PAYLOAD = {
  available: true,
  unavailableReason: null,
  downloadId: 'dl-42',
  state: 'cancelled',
};

function modelCard(): HTMLElement {
  return screen.getByTestId('model-assets-card');
}

describe('ModelAssetsCard（F23 §3.7）', () => {
  it('渲染路径零 IPC：挂载后、点击前不发起任何 invoke 调用', () => {
    const invoke = stubCommands({
      models_status: MODELS_STATUS_PAYLOAD,
      models_cache_list: MODELS_CACHE_PAYLOAD,
    });
    render(<ModelAssetsCard />);
    expect(modelCard()).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalled();
  });

  it('加载状态：渲染 SDK 可用性、缓存根、下载记录与缓存清单', async () => {
    const invoke = stubCommands({
      models_status: MODELS_STATUS_PAYLOAD,
      models_cache_list: MODELS_CACHE_PAYLOAD,
    });
    const user = userEvent.setup();
    render(<ModelAssetsCard />);

    await user.click(screen.getByTestId('model-assets-refresh'));

    await waitFor(() => {
      expect(screen.getByTestId('model-assets-status')).toBeInTheDocument();
    });
    expect(screen.getByTestId('model-assets-sdk')).toHaveTextContent(
      '未安装（下载退化为 stdlib 直下）',
    );
    expect(screen.getAllByText('C:/Users/t/.cache/huggingface/hub').length).toBe(2);
    expect(screen.getByTestId('model-assets-download-row-org/small')).toHaveTextContent(
      'org/small@main — completed（stdlib）',
    );
    expect(screen.getByTestId('model-assets-cache-org/small')).toHaveTextContent('1.0 MiB');
    expect(screen.getByTestId('model-assets-cache-org/small')).toHaveTextContent('revisions: main');
    expect(screen.getByTestId('model-assets-cache-org/bare')).toHaveTextContent('（refs 缺失）');
    // 恰好两条命令：models_status + models_cache_list。
    expect(invoke.mock.calls.map((call) => call[0])).toEqual([
      'models_status',
      'models_cache_list',
    ]);
  });

  it('sidecar 未就绪：显示 unavailableReason，禁用下载与元数据按钮', async () => {
    stubCommands({
      models_status: MODELS_STATUS_UNAVAILABLE_PAYLOAD,
      models_cache_list: {
        available: false,
        unavailableReason: 'Python sidecar 未就绪，模型缓存清单不可用',
        cacheRoot: null,
        entries: [],
      },
    });
    const user = userEvent.setup();
    render(<ModelAssetsCard />);

    await user.click(screen.getByTestId('model-assets-refresh'));

    await waitFor(() => {
      expect(screen.getByTestId('model-assets-unavailable')).toHaveTextContent(
        'Python sidecar 未就绪，模型资产状态不可用',
      );
    });
    expect(screen.getByTestId('model-assets-cache-unavailable')).toBeInTheDocument();
    // unavailable 时下载表单不渲染（全部动作不可达，spec §3.6 fail-closed）。
    expect(screen.queryByTestId('model-assets-download-form')).not.toBeInTheDocument();
    expect(screen.queryByTestId('model-assets-download')).not.toBeInTheDocument();
  });

  it('repoId 为空时下载按钮禁用；revision 必填（不假设 main）', async () => {
    stubCommands({
      models_status: MODELS_STATUS_PAYLOAD,
      models_cache_list: MODELS_CACHE_PAYLOAD,
    });
    const user = userEvent.setup();
    render(<ModelAssetsCard />);

    await user.click(screen.getByTestId('model-assets-refresh'));
    await waitFor(() => {
      expect(screen.getByTestId('model-assets-download-form')).toBeInTheDocument();
    });

    // repoId 为空 → 下载禁用。
    expect(screen.getByTestId('model-assets-download')).toBeDisabled();
    await user.type(screen.getByTestId('model-assets-repo-input'), 'org/small');
    expect(screen.getByTestId('model-assets-download')).toBeEnabled();

    // 清空 revision → 点击时提示必填，不发起任何下载命令。
    const revisionInput = screen.getByTestId('model-assets-revision-input');
    await user.clear(revisionInput);
    await user.click(screen.getByTestId('model-assets-download'));
    expect(screen.getByTestId('model-assets-form-error')).toHaveTextContent(
      'repoId 与 revision 必填',
    );
  });

  it('小文件下载：confirm 后 start → preparing → 1s 轮询 → completed 终态停止', async () => {
    let pollCount = 0;
    const invoke = stubCommands({
      models_status: MODELS_STATUS_PAYLOAD,
      models_cache_list: MODELS_CACHE_PAYLOAD,
      models_download_start: DOWNLOAD_STARTED_PAYLOAD,
      models_download_progress: () => {
        pollCount += 1;
        return DOWNLOAD_COMPLETED_PAYLOAD;
      },
      models_download_cancel: DOWNLOAD_CANCELLED_PAYLOAD,
    });
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const user = userEvent.setup();
    render(<ModelAssetsCard />);

    await user.click(screen.getByTestId('model-assets-refresh'));
    await waitFor(() => {
      expect(screen.getByTestId('model-assets-download-form')).toBeInTheDocument();
    });

    await user.type(screen.getByTestId('model-assets-repo-input'), 'org/small');
    await user.click(screen.getByTestId('model-assets-download'));

    // 显式动作必须先 confirm，且文案明示「走网络 + 目标为 HF 缓存」。
    await waitFor(() => {
      expect(confirmSpy).toHaveBeenCalledTimes(1);
    });
    expect(confirmSpy.mock.calls[0]?.[0]).toContain('走网络');
    expect(confirmSpy.mock.calls[0]?.[0]).toContain('HF 缓存');

    // start 命令：repoId / revision / files(null) / confirmedDiskBudget(false)。
    await waitFor(() => {
      const startCall = invoke.mock.calls.find((call) => call[0] === 'models_download_start');
      expect(startCall).toBeDefined();
    });
    const startCall = invoke.mock.calls.find((call) => call[0] === 'models_download_start');
    expect(startCall?.[1]).toEqual({
      repoId: 'org/small',
      revision: 'main',
      files: null,
      confirmedDiskBudget: false,
    });

    // 轮询直至终态（completed）后停止。
    await waitFor(
      () => {
        expect(pollCount).toBeGreaterThan(0);
      },
      { timeout: 3000 },
    );
    expect(screen.getByTestId('model-assets-progress')).toHaveTextContent('completed');
    // 终态后 1s 内不再轮询：记录当前次数，等待一个轮询周期验证不变。
    const counted = pollCount;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(pollCount).toBe(counted);
  });

  it('预算门：needsConfirmation → 二次 confirm 含字节数 → confirmedDiskBudget:true 重发', async () => {
    const invoke = stubCommands({
      models_status: MODELS_STATUS_PAYLOAD,
      models_cache_list: MODELS_CACHE_PAYLOAD,
      models_download_start: (function sequence() {
        const responses = [DOWNLOAD_NEEDS_CONFIRMATION_PAYLOAD, DOWNLOAD_STARTED_PAYLOAD];
        let index = 0;
        return () => responses[index++] ?? DOWNLOAD_STARTED_PAYLOAD;
      })(),
      models_download_progress: DOWNLOAD_COMPLETED_PAYLOAD,
    });
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const user = userEvent.setup();
    render(<ModelAssetsCard />);

    await user.click(screen.getByTestId('model-assets-refresh'));
    await waitFor(() => {
      expect(screen.getByTestId('model-assets-download-form')).toBeInTheDocument();
    });
    await user.type(screen.getByTestId('model-assets-repo-input'), 'org/big');
    await user.click(screen.getByTestId('model-assets-download'));

    // 第一次 confirm（明示网络）→ start(false) → needsConfirmation。
    await waitFor(() => {
      const calls = invoke.mock.calls.filter((call) => call[0] === 'models_download_start');
      expect(calls.length).toBeGreaterThan(0);
    });
    let startCalls = invoke.mock.calls.filter((call) => call[0] === 'models_download_start');
    expect(startCalls[0]?.[1]).toMatchObject({ repoId: 'org/big', confirmedDiskBudget: false });

    // 第二次 confirm 必须含字节数与磁盘预算提示（spec §3.7）。
    await waitFor(() => {
      expect(confirmSpy.mock.calls.length).toBeGreaterThanOrEqual(2);
    });
    expect(confirmSpy.mock.calls[1]?.[0]).toContain('2.00 GiB');
    expect(confirmSpy.mock.calls[1]?.[0]).toContain('2 GiB');

    // 重发必须带 confirmedDiskBudget: true。
    await waitFor(() => {
      startCalls = invoke.mock.calls.filter((call) => call[0] === 'models_download_start');
      expect(startCalls.length).toBe(2);
    });
    expect(startCalls[1]?.[1]).toMatchObject({ repoId: 'org/big', confirmedDiskBudget: true });
    // 第二次 start 后进入轮询（1s 间隔）：等进度行渲染出来（completed 终态）。
    await waitFor(
      () => {
        expect(screen.getByTestId('model-assets-progress')).toBeInTheDocument();
      },
      { timeout: 3000 },
    );
  });

  it('预算门被用户拒绝：不重发 confirmedDiskBudget:true', async () => {
    const invoke = stubCommands({
      models_status: MODELS_STATUS_PAYLOAD,
      models_cache_list: MODELS_CACHE_PAYLOAD,
      models_download_start: DOWNLOAD_NEEDS_CONFIRMATION_PAYLOAD,
    });
    vi.spyOn(window, 'confirm').mockReturnValueOnce(true).mockReturnValueOnce(false);
    const user = userEvent.setup();
    render(<ModelAssetsCard />);

    await user.click(screen.getByTestId('model-assets-refresh'));
    await waitFor(() => {
      expect(screen.getByTestId('model-assets-download-form')).toBeInTheDocument();
    });
    await user.type(screen.getByTestId('model-assets-repo-input'), 'org/big');
    await user.click(screen.getByTestId('model-assets-download'));

    await waitFor(() => {
      expect(screen.getByTestId('model-assets-needs-confirmation')).toBeInTheDocument();
    });
    // 只有第一次 start；拒绝后绝不重发。
    const startCalls = invoke.mock.calls.filter((call) => call[0] === 'models_download_start');
    expect(startCalls.length).toBe(1);
    expect(startCalls[0]?.[1]).toMatchObject({ confirmedDiskBudget: false });
  });

  it('下载中：显示进度与取消按钮；取消后停止轮询', async () => {
    const invoke = stubCommands({
      models_status: MODELS_STATUS_PAYLOAD,
      models_cache_list: MODELS_CACHE_PAYLOAD,
      models_download_start: DOWNLOAD_STARTED_PAYLOAD,
      models_download_progress: DOWNLOAD_PROGRESS_PAYLOAD,
      models_download_cancel: DOWNLOAD_CANCELLED_PAYLOAD,
    });
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const user = userEvent.setup();
    render(<ModelAssetsCard />);

    await user.click(screen.getByTestId('model-assets-refresh'));
    await waitFor(() => {
      expect(screen.getByTestId('model-assets-download-form')).toBeInTheDocument();
    });
    await user.type(screen.getByTestId('model-assets-repo-input'), 'org/small');
    await user.click(screen.getByTestId('model-assets-download'));

    // 进度行渲染：字节、文件、当前文件。
    await waitFor(() => {
      expect(screen.getByTestId('model-assets-progress')).toBeInTheDocument();
    });
    expect(screen.getByTestId('model-assets-progress')).toHaveTextContent('downloading');
    expect(screen.getByTestId('model-assets-progress')).toHaveTextContent('512 B / 1024 B');
    expect(screen.getByTestId('model-assets-progress')).toHaveTextContent('model.safetensors');

    // 取消按钮在场且可点；点击后调 cancel 命令并停止轮询。
    expect(screen.getByTestId('model-assets-cancel')).toBeEnabled();
    await user.click(screen.getByTestId('model-assets-cancel'));
    await waitFor(() => {
      expect(invoke.mock.calls.some((call) => call[0] === 'models_download_cancel')).toBeTruthy();
    });
    expect(invoke.mock.calls.find((call) => call[0] === 'models_download_cancel')?.[1]).toEqual({
      downloadId: 'dl-42',
    });
    // 取消后取消按钮消失（downloading=false）。
    await waitFor(() => {
      expect(screen.queryByTestId('model-assets-cancel')).not.toBeInTheDocument();
    });
  });

  it('start 返回 unavailable（sidecar 中途掉线）：如实展示原因', async () => {
    stubCommands({
      models_status: MODELS_STATUS_PAYLOAD,
      models_cache_list: MODELS_CACHE_PAYLOAD,
      models_download_start: DOWNLOAD_UNAVAILABLE_PAYLOAD,
    });
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const user = userEvent.setup();
    render(<ModelAssetsCard />);

    await user.click(screen.getByTestId('model-assets-refresh'));
    await waitFor(() => {
      expect(screen.getByTestId('model-assets-download-form')).toBeInTheDocument();
    });
    await user.type(screen.getByTestId('model-assets-repo-input'), 'org/small');
    await user.click(screen.getByTestId('model-assets-download'));

    await waitFor(() => {
      expect(screen.getByTestId('model-assets-start-unavailable')).toHaveTextContent(
        'models.download.start 调用失败：sidecar 未启动',
      );
    });
  });

  it('坏形状 payload fail-closed：状态/缓存收敛为不可用默认，不抛异常', async () => {
    stubCommands({
      models_status: { available: 'yes', downloads: 'nope' },
      models_cache_list: 'bad',
    });
    const user = userEvent.setup();
    render(<ModelAssetsCard />);

    await user.click(screen.getByTestId('model-assets-refresh'));
    await waitFor(() => {
      expect(screen.getByTestId('model-assets-unavailable')).toBeInTheDocument();
    });
    expect(screen.getByTestId('model-assets-cache-unavailable')).toBeInTheDocument();
    // 下载表单不渲染（available=false），动作全部禁用。
    expect(screen.queryByTestId('model-assets-download-form')).not.toBeInTheDocument();
  });

  it('文件白名单：逗号分隔解析后透传数组', async () => {
    const invoke = stubCommands({
      models_status: MODELS_STATUS_PAYLOAD,
      models_cache_list: MODELS_CACHE_PAYLOAD,
      models_download_start: DOWNLOAD_STARTED_PAYLOAD,
      models_download_progress: DOWNLOAD_COMPLETED_PAYLOAD,
    });
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const user = userEvent.setup();
    render(<ModelAssetsCard />);

    await user.click(screen.getByTestId('model-assets-refresh'));
    await waitFor(() => {
      expect(screen.getByTestId('model-assets-download-form')).toBeInTheDocument();
    });
    await user.type(screen.getByTestId('model-assets-repo-input'), 'org/small');
    await user.type(
      screen.getByTestId('model-assets-files-input'),
      'model.safetensors, tokenizer.json',
    );
    await user.click(screen.getByTestId('model-assets-download'));

    await waitFor(() => {
      const startCall = invoke.mock.calls.find((call) => call[0] === 'models_download_start');
      expect(startCall).toBeDefined();
    });
    const startCall = invoke.mock.calls.find((call) => call[0] === 'models_download_start');
    expect(startCall?.[1]).toEqual({
      repoId: 'org/small',
      revision: 'main',
      files: ['model.safetensors', 'tokenizer.json'],
      confirmedDiskBudget: false,
    });
  });
});

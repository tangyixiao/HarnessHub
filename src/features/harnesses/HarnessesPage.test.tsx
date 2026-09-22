import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { HarnessesPage } from '@/features/harnesses/HarnessesPage';

type Internals = { __TAURI_INTERNALS__?: unknown };

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

function stubHarnesses(payload: unknown): void {
  (window as unknown as Internals).__TAURI_INTERNALS__ = {
    invoke: vi.fn(async () => payload),
  };
}

/**
 * 按命令名分派，便于验证「哪个命令在什么时候被调用」。
 *
 * 值可以是直接返回的数据，也可以是**惰性求值的函数**：要模拟失败必须用函数形式，
 * 否则 `Promise.reject(...)` 在桩定义时就被创建，等到真正 await 之前已是孤儿拒绝，
 * vitest 会报 unhandled rejection。
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

/** 与 Rust `HarnessRegistry::summaries()` 序列化结果同形（camelCase）。 */
const INSTALLED_CODEX = {
  id: 'codex',
  displayName: 'Codex',
  installed: true,
  binaryPath: 'D:/npm-global/codex.cmd',
  version: '0.152.1',
  capabilities: {
    launch: true,
    terminal: true,
    resume: true,
    usage: false,
    replay: false,
    toolCalls: false,
    subagents: false,
    liveState: false,
    worktree: false,
  },
  dataPaths: ['C:/Users/dev/.codex'],
};

describe('HarnessesPage', () => {
  it('展示真实检测到的版本、binary 路径与数据目录', async () => {
    stubHarnesses([INSTALLED_CODEX]);

    render(<HarnessesPage />);

    expect(await screen.findByText('Codex')).toBeInTheDocument();
    expect(screen.getByText('0.152.1')).toBeInTheDocument();
    expect(screen.getByText('D:/npm-global/codex.cmd')).toBeInTheDocument();
    expect(screen.getByText('C:/Users/dev/.codex')).toBeInTheDocument();
    expect(screen.getByText('已安装')).toBeInTheDocument();
  });

  it('按适配器上报的能力渲染矩阵，未验证的能力显示 —', async () => {
    stubHarnesses([INSTALLED_CODEX]);

    render(<HarnessesPage />);
    await screen.findByText('Codex');

    expect(screen.getByTestId('capability-launch')).toHaveTextContent('✓');
    expect(screen.getByTestId('capability-terminal')).toHaveTextContent('✓');
    expect(screen.getByTestId('capability-usage')).toHaveTextContent('—');
    expect(screen.getByTestId('capability-liveState')).toHaveTextContent('—');
    expect(screen.getByTestId('capability-toolCalls')).toHaveTextContent('—');
  });

  it('未安装时显示不可用，而不是假装有数据', async () => {
    stubHarnesses([
      {
        ...INSTALLED_CODEX,
        installed: false,
        binaryPath: null,
        version: null,
        dataPaths: [],
      },
    ]);

    render(<HarnessesPage />);

    expect(await screen.findByText('不可用')).toBeInTheDocument();
    expect(screen.getByText('版本未知')).toBeInTheDocument();
    expect(screen.getByText('未检测到可执行文件')).toBeInTheDocument();
    expect(screen.getByText('未检测到数据目录')).toBeInTheDocument();
  });

  it('已安装但读不到版本时明确说明版本未知', async () => {
    stubHarnesses([{ ...INSTALLED_CODEX, version: null }]);

    render(<HarnessesPage />);

    expect(await screen.findByText('已安装')).toBeInTheDocument();
    expect(screen.getByText('版本未知')).toBeInTheDocument();
  });

  it('payload 字段残缺也不崩溃，并保守显示为不可用', async () => {
    stubHarnesses([{ id: 'codex' }]);

    render(<HarnessesPage />);

    // displayName 回退成 id，因此用状态文案定位卡片，避免文本重复歧义。
    const status = await screen.findByText('不可用');
    const section = status.closest('section');
    expect(section).not.toBeNull();
    expect(within(section as HTMLElement).getByText('版本未知')).toBeInTheDocument();
    expect(within(section as HTMLElement).getByTestId('capability-launch')).toHaveTextContent('—');
  });

  it('后端返回空列表时给出明确空态', async () => {
    stubHarnesses([]);

    render(<HarnessesPage />);

    expect(await screen.findByText(/未检测到任何 Harness/)).toBeInTheDocument();
  });

  it('浏览器模式下如实说明 IPC 不可用，且不显示任何 Harness 数据', async () => {
    render(<HarnessesPage />);

    expect(await screen.findByText(/请用 pnpm tauri dev 启动桌面应用/)).toBeInTheDocument();
    expect(screen.queryByText('Codex')).not.toBeInTheDocument();
  });

  it('IPC 报错时展示错误原因而不是白屏', async () => {
    (window as unknown as Internals).__TAURI_INTERNALS__ = {
      invoke: vi.fn(async () => {
        throw new Error('detect failed');
      }),
    };

    render(<HarnessesPage />);

    expect(await screen.findByText(/detect failed/)).toBeInTheDocument();
  });

  it('说明 — 的准确含义是「当前未支持」，而不是「待验证」', async () => {
    stubHarnesses([INSTALLED_CODEX]);

    render(<HarnessesPage />);
    await screen.findByText('Codex');

    expect(screen.getByText(/当前未支持/)).toBeInTheDocument();
    expect(screen.queryByText(/待验证/)).not.toBeInTheDocument();
  });

  it('初次加载只读：只调用 list_harnesses，不写库', async () => {
    const invoke = stubCommands({ list_harnesses: [INSTALLED_CODEX] });

    render(<HarnessesPage />);
    await screen.findByText('Codex');

    const commands = invoke.mock.calls.map((call) => call[0]);
    expect(commands).toEqual(['list_harnesses']);
    expect(commands).not.toContain('refresh_harnesses');
  });

  it('点「重新检测并同步」才触发写路径，并展示同步结果', async () => {
    const invoke = stubCommands({
      list_harnesses: [INSTALLED_CODEX],
      refresh_harnesses: { harnesses: 1, installations: 1 },
    });

    render(<HarnessesPage />);
    await screen.findByText('Codex');

    await userEvent.click(screen.getByRole('button', { name: '重新检测并同步' }));

    const commands = invoke.mock.calls.map((call) => call[0]);
    expect(commands).toEqual(['list_harnesses', 'refresh_harnesses', 'list_harnesses']);
    expect(await screen.findByText(/已同步：1 个 Harness 定义 \/ 1 个安装/)).toBeInTheDocument();
  });

  it('同步失败时展示错误，而不是静默忽略', async () => {
    stubCommands({
      list_harnesses: [INSTALLED_CODEX],
      refresh_harnesses: () => {
        throw new Error('inventory sync failed');
      },
    });

    render(<HarnessesPage />);
    await screen.findByText('Codex');

    await userEvent.click(screen.getByRole('button', { name: '重新检测并同步' }));

    // refreshHarnesses 捕获异常后返回 ok:false
    expect(await screen.findByText(/inventory sync failed/)).toBeInTheDocument();
  });

  it('浏览器模式下同步按钮不可点，避免发起注定失败的调用', async () => {
    render(<HarnessesPage />);
    await screen.findByText(/请用 pnpm tauri dev 启动桌面应用/);

    expect(screen.getByRole('button', { name: '重新检测并同步' })).toBeDisabled();
  });
});

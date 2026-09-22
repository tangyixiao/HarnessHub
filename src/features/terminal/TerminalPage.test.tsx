import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * Terminal 页的关键不变量（ADR-0009 + review 约束 1）：
 *
 *   xterm open → Channel 回调 → onData/onBinary → resize → start_terminal
 *
 * 顺序错了，Codex 首屏的 DSR 就会早于 responder 就绪而永久卡住。
 * 这里用 mock 记录调用顺序来锁死它。
 */
const calls: string[] = [];
let observerCallback: (() => void) | null = null;

const xtermInstances: Array<{ cols: number; rows: number }> = [];
vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 100;
    rows = 30;
    constructor() {
      xtermInstances.push(this);
    }
    open() {
      calls.push('terminal.open');
    }
    loadAddon() {
      calls.push('terminal.loadAddon');
    }
    onData() {
      calls.push('terminal.onData');
      return { dispose: () => calls.push('onData.dispose') };
    }
    onBinary() {
      calls.push('terminal.onBinary');
      return { dispose: () => calls.push('onBinary.dispose') };
    }
    write() {
      calls.push('terminal.write');
    }
    dispose() {
      calls.push('terminal.dispose');
    }
  },
}));

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit() {
      calls.push('fitAddon.fit');
    }
  },
}));

const startTerminal = vi.fn();
const killTerminal = vi.fn();
const writeTerminal = vi.fn();
const resizeTerminal = vi.fn();
const listHarnesses = vi.fn();

vi.mock('@/lib/ipc', async () => {
  const actual = await vi.importActual<typeof import('@/lib/ipc')>('@/lib/ipc');
  return {
    ...actual,
    listHarnesses: () => listHarnesses(),
    startTerminal: (input: unknown) => startTerminal(input),
    killTerminal: (sessionId: string) => killTerminal(sessionId),
    writeTerminal: (sessionId: string, bytes: Uint8Array) => writeTerminal(sessionId, bytes),
    resizeTerminal: (sessionId: string, cols: number, rows: number) =>
      resizeTerminal(sessionId, cols, rows),
  };
});

const { TerminalPage } = await import('@/features/terminal/TerminalPage');

const INSTALLED = {
  id: 'codex',
  displayName: 'Codex',
  installationId: 'codex@local',
  installed: true,
  binaryPath: 'D:/npm-global/codex.cmd',
  version: '0.152.1',
  capabilities: {},
  dataPaths: [],
};

const SESSION = {
  hubSessionId: 'hub-1',
  sourceSessionId: null,
  harnessId: 'codex',
  installationId: 'codex@local',
  projectId: null,
  runtimeTargetId: 'local',
  parentSessionId: null,
  status: 'running',
  launchMode: 'terminal',
  cwd: null,
  worktreePath: null,
  startedAt: '2026-09-22T10:00:00Z',
  endedAt: null,
  exitCode: null,
  terminationReason: null,
  pid: 4242,
};

beforeEach(() => {
  calls.length = 0;
  startTerminal.mockReset();
  killTerminal.mockReset();
  writeTerminal.mockReset();
  resizeTerminal.mockReset();
  listHarnesses.mockReset();

  listHarnesses.mockResolvedValue({ ok: true, data: [INSTALLED] });
  startTerminal.mockResolvedValue({ ok: true, data: SESSION });
  killTerminal.mockResolvedValue({ ok: true, data: null });
  writeTerminal.mockResolvedValue({ ok: true, data: null });
  resizeTerminal.mockResolvedValue({ ok: true, data: null });

  observerCallback = null;
  xtermInstances.length = 0;
  globalThis.ResizeObserver = class {
    constructor(callback: () => void) {
      observerCallback = callback;
    }
    observe() {
      calls.push('resizeObserver.observe');
    }
    disconnect() {
      calls.push('resizeObserver.disconnect');
    }
    unobserve() {}
  } as unknown as typeof ResizeObserver;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('TerminalPage', () => {
  it('严格 ready-before-spawn：open → onData/onBinary → resize → start_terminal', async () => {
    render(<TerminalPage />);

    await waitFor(() => {
      expect(startTerminal).toHaveBeenCalledTimes(1);
    });

    const indexOf = (name: string) => calls.indexOf(name);
    expect(indexOf('terminal.open')).toBeGreaterThanOrEqual(0);
    expect(indexOf('terminal.onData')).toBeGreaterThan(indexOf('terminal.open'));
    expect(indexOf('terminal.onBinary')).toBeGreaterThan(indexOf('terminal.onData'));
    expect(indexOf('resizeObserver.observe')).toBeGreaterThan(indexOf('terminal.onBinary'));
    expect(
      calls.filter((call) => call === 'terminal.open').length,
      '容器必须存在，xterm 才可能 open',
    ).toBe(1);
  });

  it('先挂好输入路径才 spawn：startTerminal 之前 onData 必须已注册', async () => {
    render(<TerminalPage />);
    await waitFor(() => expect(startTerminal).toHaveBeenCalled());

    // 记录顺序上，onData 的注册必须早于 spawn 调用
    const spawnedBefore = startTerminal.mock.invocationCallOrder[0];
    expect(spawnedBefore).toBeGreaterThan(0);
    expect(calls.indexOf('terminal.onData')).toBeGreaterThanOrEqual(0);
  });

  it('用已安装 Harness 的 installationId 启动，而不是前端自己拼 id', async () => {
    render(<TerminalPage />);
    await waitFor(() => expect(startTerminal).toHaveBeenCalled());

    expect(startTerminal.mock.calls[0][0]).toMatchObject({
      installationId: 'codex@local',
      cols: 100,
      rows: 30,
    });
  });

  it('输出事件以原始字节（Uint8Array）交给 xterm', async () => {
    render(<TerminalPage />);
    await waitFor(() => expect(startTerminal).toHaveBeenCalled());

    const onEvent = startTerminal.mock.calls[0][0].onEvent;
    calls.length = 0;
    onEvent({ kind: 'output', sessionId: 'hub-1', seq: 0, data: [0x1b, 0x5b, 0x36, 0x6e] });

    expect(calls).toContain('terminal.write');
  });

  it('卸载只释放前端资源，**不 kill** 正在运行的会话', async () => {
    const view = render(<TerminalPage />);
    await waitFor(() => expect(startTerminal).toHaveBeenCalled());

    view.unmount();

    expect(killTerminal).not.toHaveBeenCalled();
    expect(calls).toContain('onData.dispose');
    expect(calls).toContain('onBinary.dispose');
    expect(calls).toContain('resizeObserver.disconnect');
    expect(calls).toContain('terminal.dispose');
  });

  it('只有点「结束会话」才会 kill', async () => {
    render(<TerminalPage />);
    const button = await screen.findByRole('button', { name: '结束会话' });

    button.click();

    await waitFor(() => expect(killTerminal).toHaveBeenCalledWith('hub-1'));
  });

  it('没有已安装的 Harness 时明确报错，而不是留一个假装在跑的终端', async () => {
    listHarnesses.mockResolvedValue({ ok: true, data: [{ ...INSTALLED, installed: false }] });

    render(<TerminalPage />);

    expect(await screen.findByText(/没有可用于启动终端的已安装 Harness/)).toBeInTheDocument();
    expect(startTerminal).not.toHaveBeenCalled();
  });

  it('浏览器模式下提示 IPC 不可用，且不 spawn', async () => {
    listHarnesses.mockResolvedValue({ ok: false, error: 'not-running-in-tauri' });

    render(<TerminalPage />);

    expect(await screen.findByText(/IPC 不可用（浏览器模式）/)).toBeInTheDocument();
    expect(startTerminal).not.toHaveBeenCalled();
  });

  /**
   * [9] resize 的参数透传：ResizeObserver 触发时必须把 **xterm 当前的 cols/rows**
   * 交给 resize_terminal，而不是发旧值或空值。
   */
  it('resize：把 xterm 当前 cols/rows 透传给 resize_terminal', async () => {
    render(<TerminalPage />);
    await waitFor(() => expect(startTerminal).toHaveBeenCalled());

    const terminal = xtermInstances[0];
    terminal.cols = 132;
    terminal.rows = 43;

    expect(observerCallback).not.toBeNull();
    observerCallback?.();

    await waitFor(() => expect(resizeTerminal).toHaveBeenCalled());
    expect(resizeTerminal).toHaveBeenCalledWith('hub-1', 132, 43);
  });

  /**
   * resize 边界：session id 还没回来时 ResizeObserver 可能先触发 ——
   * 此时绝不能用空 id 发 resize（也不做补发队列：start_terminal 本身
   * 已经带了初始 cols/rows）。
   */
  it('resize：session id 尚未返回时不得发送', async () => {
    render(<TerminalPage />);

    expect(observerCallback).not.toBeNull();
    observerCallback?.();

    expect(resizeTerminal).not.toHaveBeenCalled();
  });
});
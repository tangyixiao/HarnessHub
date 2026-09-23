import { act, render, screen, waitFor } from '@testing-library/react';
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

    await clickLaunch();

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

    await clickLaunch();
    await waitFor(() => expect(startTerminal).toHaveBeenCalled());

    // 记录顺序上，onData 的注册必须早于 spawn 调用
    const spawnedBefore = startTerminal.mock.invocationCallOrder[0];
    expect(spawnedBefore).toBeGreaterThan(0);
    expect(calls.indexOf('terminal.onData')).toBeGreaterThanOrEqual(0);
  });

  it('用已安装 Harness 的 installationId 启动，而不是前端自己拼 id', async () => {
    render(<TerminalPage />);

    await clickLaunch();
    await waitFor(() => expect(startTerminal).toHaveBeenCalled());

    expect(startTerminal.mock.calls[0][0]).toMatchObject({
      installationId: 'codex@local',
      cols: 100,
      rows: 30,
    });
  });

  it('输出事件以原始字节（Uint8Array）交给 xterm', async () => {
    render(<TerminalPage />);

    await clickLaunch();
    await waitFor(() => expect(startTerminal).toHaveBeenCalled());

    const onEvent = startTerminal.mock.calls[0][0].onEvent;
    calls.length = 0;
    await act(async () => {
      onEvent({ kind: 'output', sessionId: 'hub-1', seq: 0, data: [0x1b, 0x5b, 0x36, 0x6e] });
    });

    expect(calls).toContain('terminal.write');
  });

  it('卸载只释放前端资源，**不 kill** 正在运行的会话', async () => {
    const view = render(<TerminalPage />);

    await clickLaunch();
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

    await clickLaunch();
    const button = await screen.findByRole('button', { name: '结束会话' });

    await click(button);

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

    await clickLaunch();
    await waitFor(() => expect(startTerminal).toHaveBeenCalled());

    const terminal = xtermInstances[0];
    terminal.cols = 132;
    terminal.rows = 43;

    expect(observerCallback).not.toBeNull();
    await act(async () => {
      observerCallback?.();
    });

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
    await act(async () => {
      observerCallback?.();
    });

    expect(resizeTerminal).not.toHaveBeenCalled();
  });
});

/** 显式启动：Terminal 页现在是 configure → explicit launch（不再 mount 自动 spawn）。 */
async function clickLaunch() {
  await settle();
  const button = screen.getByRole('button', { name: '启动' }) as HTMLButtonElement;
  await click(button);
  await settle();
}

/*
 * Multi-Harness 启动语义（Task 7B）。
 *
 * 锁死的是**竞态与选择语义**，不是 Harness 名字：组件里不得出现 codex / claude 分支。
 * 只用 render / screen + 原生 DOM 事件，避免为本组用例引入新的测试依赖。
 */
const CLAUDE_INSTALLED = {
  ...INSTALLED,
  id: 'claude',
  displayName: 'Claude Code',
  installationId: 'claude@local',
};

const CLAUDE_SESSION = {
  ...SESSION,
  hubSessionId: 'hub-claude',
  harnessId: 'claude',
  installationId: 'claude@local',
};

/**
 * 等一小段真实时间，让 effect 里的异步启动流程落地。
 *
 * **必须包在 `act` 里**：mount 的 `listHarnesses()` 与启动时的 `startTerminal()` 都是
 * promise 落定后 setState；不包会打出 "not wrapped in act(...)" 警告 —— 测试仍会通过，
 * 但警告会污染输出，也会掩盖真正的 act 违规。
 */
const settle = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 40));
  });

/** 点击同样会同步 setState，因此一律走这个 act 包装的 helper。 */
const click = (element: HTMLElement) =>
  act(async () => {
    element.click();
  });

/**
 * 受控 `<select>` 的切换。
 *
 * 不能直接 `select.value = x`：React 的 value tracker 会认为「没变」。走原型上的原生
 * value setter 再抛 change 事件（RTL 内部的做法），并包在 `act` 里。
 */
const pickHarness = (installationId: string) =>
  act(async () => {
    const select = screen.getByLabelText('启动的 Harness') as HTMLSelectElement;
    const nativeSetter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')?.set;
    nativeSetter?.call(select, installationId);
    select.dispatchEvent(new Event('change', { bubbles: true }));
  });

/** 受控 cwd `<input>`：与 `pickHarness` 同理，直接赋值会被 value tracker 忽略。 */
const setCwdInput = (value: string) =>
  act(async () => {
    const input = screen.getByLabelText('工作目录（可选）') as HTMLInputElement;
    const nativeSetter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
    nativeSetter?.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });

describe('TerminalPage multi-harness 启动语义', () => {
  it('0 个已安装 → 不调用 start_terminal，并如实报错', async () => {
    listHarnesses.mockResolvedValue({ ok: true, data: [] });

    render(<TerminalPage />);
    await settle();

    expect(startTerminal).not.toHaveBeenCalled();
    expect(screen.getByText(/没有可用于启动终端的已安装 Harness/)).toBeInTheDocument();
  });

  it('1 个已安装 → mount 不自动启动；点启动恰好一次，rerender 不重复 spawn', async () => {
    const view = render(<TerminalPage />);

    await clickLaunch();
    await settle();

    expect(startTerminal).toHaveBeenCalledTimes(1);
    expect(startTerminal.mock.calls[0]?.[0]).toMatchObject({ installationId: 'codex@local' });

    view.rerender(<TerminalPage />);
    await settle();

    expect(startTerminal).toHaveBeenCalledTimes(1);
    // selector 现在始终渲染（installed >= 1），不再是「唯一选择就藏起来」
    expect(screen.getByLabelText('启动的 Harness')).toBeInTheDocument();
  });

  it('2 个已安装 → mount 不 spawn，显示 selector 与启动按钮', async () => {
    listHarnesses.mockResolvedValue({ ok: true, data: [INSTALLED, CLAUDE_INSTALLED] });

    render(<TerminalPage />);
    await settle();

    expect(startTerminal).not.toHaveBeenCalled();
    expect(screen.getByLabelText('启动的 Harness')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '启动' })).toBeInTheDocument();
    // 默认选中列表里第一个（注册顺序，确定）
    expect((screen.getByLabelText('启动的 Harness') as HTMLSelectElement).value).toBe(
      'codex@local',
    );
  });

  it('用户选 claude@local → start_terminal 收到的正是 claude@local', async () => {
    listHarnesses.mockResolvedValue({ ok: true, data: [INSTALLED, CLAUDE_INSTALLED] });
    startTerminal.mockResolvedValue({ ok: true, data: CLAUDE_SESSION });

    render(<TerminalPage />);
    await settle();

    await pickHarness('claude@local');
    await settle();

    await click(screen.getByRole('button', { name: '启动' }) as HTMLButtonElement);
    await settle();

    expect(startTerminal).toHaveBeenCalledTimes(1);
    expect(startTerminal.mock.calls[0]?.[0]).toMatchObject({ installationId: 'claude@local' });
  });

  it('双击启动只产生一个 session', async () => {
    listHarnesses.mockResolvedValue({ ok: true, data: [INSTALLED, CLAUDE_INSTALLED] });

    render(<TerminalPage />);
    await settle();

    const start = screen.getByRole('button', { name: '启动' }) as HTMLButtonElement;
    await click(start);
    await click(start);
    await settle();

    expect(startTerminal).toHaveBeenCalledTimes(1);
  });

  it('running 时 selector 被禁用（切换只影响下一次启动）', async () => {
    listHarnesses.mockResolvedValue({ ok: true, data: [INSTALLED, CLAUDE_INSTALLED] });

    render(<TerminalPage />);
    await settle();

    await click(screen.getByRole('button', { name: '启动' }) as HTMLButtonElement);
    await settle();

    expect((screen.getByLabelText('启动的 Harness') as HTMLSelectElement).disabled).toBe(true);
    expect(screen.getByText(/session hub-1/)).toBeInTheDocument();
  });

  it('启动失败保留选择，并且可以重试', async () => {
    listHarnesses.mockResolvedValue({ ok: true, data: [INSTALLED, CLAUDE_INSTALLED] });
    startTerminal.mockResolvedValueOnce({ ok: false, error: '启动进程失败' });

    render(<TerminalPage />);
    await settle();

    await pickHarness('claude@local');
    await click(screen.getByRole('button', { name: '启动' }) as HTMLButtonElement);
    await settle();

    expect(screen.getByText('启动进程失败')).toBeInTheDocument();
    expect((screen.getByLabelText('启动的 Harness') as HTMLSelectElement).value).toBe(
      'claude@local',
    );

    startTerminal.mockResolvedValue({ ok: true, data: CLAUDE_SESSION });
    await click(screen.getByRole('button', { name: '启动' }) as HTMLButtonElement);
    await settle();

    expect(startTerminal).toHaveBeenCalledTimes(2);
    expect(screen.getByText(/session hub-claude/)).toBeInTheDocument();
  });

  it('ended 之后重新启动会创建新的 session', async () => {
    listHarnesses.mockResolvedValue({ ok: true, data: [INSTALLED, CLAUDE_INSTALLED] });
    startTerminal.mockResolvedValue({ ok: true, data: SESSION });

    render(<TerminalPage />);
    await settle();
    await click(screen.getByRole('button', { name: '启动' }) as HTMLButtonElement);
    await settle();

    // 让后端推一个 exited 事件（真实 reaper 的等价物）
    const onEvent = startTerminal.mock.calls[0]?.[0]?.onEvent as (event: unknown) => void;
    await act(async () => {
      onEvent({ kind: 'exited', reason: 'natural_exit', exitCode: 0 });
    });
    await settle();

    expect(screen.getByRole('button', { name: '重新启动' })).toBeInTheDocument();

    startTerminal.mockResolvedValue({ ok: true, data: { ...SESSION, hubSessionId: 'hub-2' } });
    await click(screen.getByRole('button', { name: '重新启动' }) as HTMLButtonElement);
    await settle();

    expect(startTerminal).toHaveBeenCalledTimes(2);
    expect(screen.getByText(/session hub-2/)).toBeInTheDocument();
  });
});

/*
 * Launch options：工作目录（通用能力，不是 Claude 特例）。
 * 语义：空白 → 不传 cwd（后端收到 None、sessions.cwd 记 NULL）；非空 → trim 后原样传。
 */
describe('TerminalPage launch options（cwd）', () => {
  it('1 个已安装时 mount 也不再自动启动', async () => {
    render(<TerminalPage />);
    await settle();

    expect(startTerminal).not.toHaveBeenCalled();

    await click(screen.getByRole('button', { name: '启动' }) as HTMLButtonElement);
    await settle();

    expect(startTerminal).toHaveBeenCalledTimes(1);
  });

  it('cwd 空白 → 不传 cwd（保持 None 语义）', async () => {
    render(<TerminalPage />);
    await settle();

    await setCwdInput('   ');
    await click(screen.getByRole('button', { name: '启动' }) as HTMLButtonElement);
    await settle();

    const input = startTerminal.mock.calls[0]?.[0] as { cwd?: string };
    expect(input.cwd).toBeUndefined();
  });
});

/*
 * 补上上一轮标注的 cwd 缺口。两个 describe 共用模块级的 `setCwdInput`
 * （原生 value setter，见文件上方），不再各自维护一份。
 */
describe('TerminalPage cwd 透传', () => {
  it('非空 cwd（含反斜杠）→ trim 后精确透传', async () => {
    render(<TerminalPage />);
    await settle();

    await setCwdInput('   D:\\HarnessHub-E2E\\claude-terminal   ');
    await click(screen.getByRole('button', { name: '启动' }) as HTMLButtonElement);
    await settle();

    const input = startTerminal.mock.calls[0]?.[0] as { cwd?: string };
    expect(input.cwd).toBe('D:\\HarnessHub-E2E\\claude-terminal');
  });

  it('running 禁用 cwd；ended 后改 cwd → 新 session 用新值', async () => {
    render(<TerminalPage />);
    await settle();

    await setCwdInput('D:\\first');
    await click(screen.getByRole('button', { name: '启动' }) as HTMLButtonElement);
    await settle();

    expect((screen.getByLabelText('工作目录（可选）') as HTMLInputElement).disabled).toBe(true);
    expect((startTerminal.mock.calls[0]?.[0] as { cwd?: string }).cwd).toBe('D:\\first');
    // 运行中展示的是**事实**（activeCwd），不是输入框里的选择
    expect(screen.getByText(/cwd D:\\first/)).toBeInTheDocument();

    const onEvent = startTerminal.mock.calls[0]?.[0]?.onEvent as (event: unknown) => void;
    await act(async () => {
      onEvent({ kind: 'exited', reason: 'natural_exit', exitCode: 0 });
    });
    await settle();

    expect((screen.getByLabelText('工作目录（可选）') as HTMLInputElement).disabled).toBe(false);
    await setCwdInput('D:\\second');
    startTerminal.mockResolvedValue({ ok: true, data: { ...SESSION, hubSessionId: 'hub-2' } });
    await click(screen.getByRole('button', { name: '重新启动' }) as HTMLButtonElement);
    await settle();

    expect((startTerminal.mock.calls[1]?.[0] as { cwd?: string }).cwd).toBe('D:\\second');
    expect(screen.getByText(/session hub-2/)).toBeInTheDocument();
  });
});

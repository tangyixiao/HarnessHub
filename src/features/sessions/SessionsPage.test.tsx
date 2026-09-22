import { render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { SessionsPage } from '@/features/sessions/SessionsPage';

type Internals = { __TAURI_INTERNALS__?: unknown };

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

function stubSessions(payload: unknown): void {
  (window as unknown as Internals).__TAURI_INTERNALS__ = {
    invoke: vi.fn(async () => payload),
  };
}

/** 与 Rust `SessionRecord`（serde camelCase）同形。 */
const CREATED_SESSION = {
  hubSessionId: '6f1c0f7e-0000-4000-8000-000000000001',
  sourceSessionId: null,
  harnessId: 'codex',
  installationId: 'codex@local',
  projectId: null,
  runtimeTargetId: 'local',
  parentSessionId: null,
  status: 'created',
  launchMode: 'terminal',
  cwd: 'D:/work/harness-hub',
  worktreePath: null,
  startedAt: '2026-09-22T10:00:00Z',
  endedAt: null,
  exitCode: null,
};

const RUNNING_SESSION = { ...CREATED_SESSION, status: 'running' };

const EXITED_SESSION = {
  ...CREATED_SESSION,
  hubSessionId: '6f1c0f7e-0000-4000-8000-000000000002',
  status: 'exited',
  endedAt: '2026-09-22T10:05:00Z',
  exitCode: 0,
};

describe('SessionsPage', () => {
  /**
   * 最重要的一条：**没有进程就不能显示成「仍在运行」**。
   * 见 docs/adr/0006-session-state-machine.md。
   */
  it('已创建但未启动的会话显示「已创建 · 尚未启动」，绝不能显示「仍在运行」', async () => {
    stubSessions([CREATED_SESSION]);

    render(<SessionsPage />);

    expect(await screen.findByText('已创建 · 尚未启动')).toBeInTheDocument();
    expect(screen.getByText(/尚未启动（没有进程）/)).toBeInTheDocument();
    expect(screen.queryByText('运行中')).not.toBeInTheDocument();
    expect(screen.queryByText(/仍在运行/)).not.toBeInTheDocument();
  });

  it('展示真实会话记录与状态', async () => {
    stubSessions([RUNNING_SESSION, EXITED_SESSION]);

    render(<SessionsPage />);

    expect(await screen.findByText('运行中')).toBeInTheDocument();
    expect(screen.getByText('已结束')).toBeInTheDocument();
    expect(screen.getAllByText('codex')).toHaveLength(2);
    expect(screen.getAllByText('D:/work/harness-hub')).toHaveLength(2);
  });

  it('展示 hub_session_id 以便与外部 source_session_id 区分', async () => {
    stubSessions([RUNNING_SESSION]);

    render(<SessionsPage />);
    await screen.findByText('运行中');

    expect(screen.getByText(RUNNING_SESSION.hubSessionId)).toBeInTheDocument();
  });

  it('展示安装 id，区分同一 Harness 的不同安装', async () => {
    stubSessions([CREATED_SESSION]);

    render(<SessionsPage />);
    await screen.findByText('已创建 · 尚未启动');

    expect(screen.getByText(/安装：codex@local/)).toBeInTheDocument();
  });

  it('结束时间与退出码在会话结束后才显示', async () => {
    stubSessions([EXITED_SESSION]);

    render(<SessionsPage />);
    await screen.findByText('已结束');

    expect(screen.getByText(/2026-09-22 10:05:00/)).toBeInTheDocument();
    expect(screen.getByText(/退出码 0/)).toBeInTheDocument();
  });

  it('运行中的会话不显示退出码数值，而是明确说仍在运行', async () => {
    stubSessions([RUNNING_SESSION]);

    render(<SessionsPage />);
    await screen.findByText('运行中');

    // 注意：文案是「仍在运行（尚未收到退出码）」，所以匹配必须以「退出码」开头，
    // 否则 /退出码/ 会命中它自己。
    expect(screen.queryByText(/^退出码/)).not.toBeInTheDocument();
    expect(screen.getByText(/仍在运行/)).toBeInTheDocument();
  });

  it('失败与未知状态如实渲染', async () => {
    stubSessions([
      { ...RUNNING_SESSION, hubSessionId: 'h-failed', status: 'failed', exitCode: 130 },
      { ...RUNNING_SESSION, hubSessionId: 'h-unknown', status: 'nonsense' },
    ]);

    render(<SessionsPage />);

    expect(await screen.findByText('失败')).toBeInTheDocument();
    expect(screen.getByText('未知')).toBeInTheDocument();
  });

  it('没有会话时给出明确空态，并说明为什么还不能新建', async () => {
    stubSessions([]);

    render(<SessionsPage />);

    expect(await screen.findByText(/还没有任何会话记录/)).toBeInTheDocument();
    expect(screen.getByText(/PTY 尚未接入/)).toBeInTheDocument();
  });

  it('payload 字段残缺也不崩溃', async () => {
    stubSessions([{ hubSessionId: 'h-only-id' }]);

    render(<SessionsPage />);

    const status = await screen.findByText('未知');
    const section = status.closest('section');
    expect(section).not.toBeNull();
    expect(within(section as HTMLElement).getByText('h-only-id')).toBeInTheDocument();
  });

  it('浏览器模式下如实说明 IPC 不可用，且不显示任何会话', async () => {
    render(<SessionsPage />);

    expect(await screen.findByText(/请用 pnpm tauri dev 启动桌面应用/)).toBeInTheDocument();
    expect(screen.queryByText('运行中')).not.toBeInTheDocument();
  });

  it('IPC 报错时展示原因而不是白屏', async () => {
    (window as unknown as Internals).__TAURI_INTERNALS__ = {
      invoke: vi.fn(async () => {
        throw new Error('database is locked');
      }),
    };

    render(<SessionsPage />);

    expect(await screen.findByText(/database is locked/)).toBeInTheDocument();
  });
});

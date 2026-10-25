import userEvent from '@testing-library/user-event';
import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ReplayPage } from '@/features/replay/ReplayPage';

type Internals = { __TAURI_INTERNALS__?: unknown };

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

function stubTauri(
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>,
): void {
  (window as unknown as Internals).__TAURI_INTERNALS__ = { invoke };
}

/**
 * 与 Rust `HarnessRegistry::summaries()`（serde camelCase）同形：
 * claude 的 capability replay = true（B6 已实现），codex 为 false。
 */
const CLAUDE_HARNESS = {
  id: 'claude',
  displayName: 'Claude Code',
  installationId: 'claude@local',
  installed: true,
  binaryPath: 'C:/Users/dev/.local/bin/claude.exe',
  version: '2.0.76',
  capabilities: {
    launch: true,
    terminal: true,
    resume: true,
    usage: true,
    replay: true,
    toolCalls: false,
    subagents: false,
    liveState: false,
    worktree: false,
  },
  dataPaths: ['C:/Users/dev/.claude'],
};

const CODEX_HARNESS = {
  id: 'codex',
  displayName: 'Codex',
  installationId: 'codex@local',
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

/**
 * 与 Rust `ReplaySessionSummary` / `ReplaySession`（serde camelCase）同形，
 * 形状手工复刻自 Rust 契约测试 `replay_dtos_serialize_with_camel_case_keys`。
 */
const SESSION_SUMMARY = {
  harnessId: 'claude',
  sessionId: 'b3a1b3a1-0000-4000-8000-000000000003',
  projectDir: 'D--work-demo',
  sizeBytes: 1024,
  modifiedMs: 1_761_000_000_000,
  firstUserPreview: '帮我看看这个报错',
  startedAt: '2026-10-25T10:00:00.000Z',
  lastAt: '2026-10-25T10:01:00.000Z',
};

const REPLAY_SESSION = {
  harnessId: 'claude',
  sessionId: 'b3a1b3a1-0000-4000-8000-000000000003',
  sourcePath: 'C:/Users/dev/.claude/projects/D--work-demo/b3a1.jsonl',
  messages: [
    {
      seq: 0,
      role: 'user',
      sidechain: false,
      timestamp: '2026-10-25T10:00:00.000Z',
      textParts: ['帮我看看这个报错'],
      thinkingParts: [],
      model: null,
      unsupportedContentKinds: {},
    },
    {
      seq: 1,
      role: 'assistant',
      sidechain: true,
      timestamp: '2026-10-25T10:00:05.000Z',
      textParts: ['回答正文'],
      thinkingParts: ['先分析一下报错原因'],
      model: 'claude-sonnet-4-6',
      unsupportedContentKinds: { tool_use: 2 },
    },
  ],
  // BTreeMap 序列化按键排序：file-history-snapshot 在 summary 之前。
  skippedKindCounts: { 'file-history-snapshot': 1, summary: 1 },
  malformedLines: 2,
};

describe('ReplayPage', () => {
  it('有 replay 能力时渲染会话列表，点击后渲染只读时间线（消息正文、思考、子代理徽标、skipped 脚注）', async () => {
    const user = userEvent.setup();
    const invokeSpy = vi.fn(async (cmd: string, _args?: Record<string, unknown>) => {
      if (cmd === 'list_harnesses') return [CODEX_HARNESS, CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return REPLAY_SESSION;
      return null;
    });
    stubTauri(invokeSpy);

    render(<ReplayPage />);

    // 左列：按后端顺序渲染会话摘要。
    expect(await screen.findByText('D--work-demo')).toBeInTheDocument();
    expect(screen.getByText('帮我看看这个报错')).toBeInTheDocument();
    expect(screen.getByText(/2026-10-25 10:00:00 ~ 2026-10-25 10:01:00/)).toBeInTheDocument();
    expect(screen.getByText(/1\.0 KiB/)).toBeInTheDocument();

    // 点击会话 → 右侧时间线。
    await user.click(screen.getByRole('button', { name: /D--work-demo/ }));

    expect(await screen.findByText('回答正文')).toBeInTheDocument();
    // user 消息正文同时出现在列表预览与时间线里，共两处。
    expect(await screen.findAllByText('帮我看看这个报错')).toHaveLength(2);
    // thinkingParts 渲染为「思考」标注区，不与正文混排。
    expect(screen.getByText('思考')).toBeInTheDocument();
    expect(screen.getByText('先分析一下报错原因')).toBeInTheDocument();
    // sidechain 消息带「子代理」徽标。
    expect(screen.getByText('子代理')).toBeInTheDocument();
    // unsupportedContentKinds 非空时在该消息卡脚注计数。
    expect(screen.getByText(/未展开内容：tool_use ×2/)).toBeInTheDocument();
    // 底部脚注如实渲染 skippedKindCounts 与 malformedLines。
    expect(
      screen.getByText(/已跳过 2 行非消息记录（file-history-snapshot ×1、summary ×1）/),
    ).toBeInTheDocument();
    expect(screen.getByText(/无法解析 2 行/)).toBeInTheDocument();

    // 契约：按 session_id 读取，前端传不了文件系统路径。
    const readCall = invokeSpy.mock.calls.find(([cmd]) => cmd === 'replay_read_session');
    expect(readCall?.[1]).toEqual({
      harnessId: 'claude',
      sessionId: 'b3a1b3a1-0000-4000-8000-000000000003',
    });
  });

  it('会话列表保持后端返回顺序（mtime 倒序），前端不重排', async () => {
    const newer = {
      ...SESSION_SUMMARY,
      sessionId: 'newer-1',
      projectDir: 'D--work-beta',
      modifiedMs: 1_761_000_000_999,
    };
    const older = {
      ...SESSION_SUMMARY,
      sessionId: 'older-2',
      projectDir: 'D--work-alpha',
      modifiedMs: 1_761_000_000_000,
    };
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [newer, older];
      return null;
    });

    render(<ReplayPage />);

    const items = await screen.findAllByRole('button', { name: /D--work-(alpha|beta)/ });
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent('D--work-beta');
    expect(items[1]).toHaveTextContent('D--work-alpha');
  });

  it('没有任何 replay 能力的 Harness 时渲染说明卡，且不发起会话列表查询', async () => {
    const invokeSpy = vi.fn(async (cmd: string) =>
      cmd === 'list_harnesses' ? [CODEX_HARNESS] : [],
    );
    stubTauri(invokeSpy);

    render(<ReplayPage />);

    expect(await screen.findByText(/还没有支持回放的 Harness/)).toBeInTheDocument();
    // 说明卡要讲清楚门控依据：capability 矩阵里 replay = false 就不渲染入口。
    expect(screen.getByText(/capabilities\.replay = true/)).toBeInTheDocument();
    expect(invokeSpy.mock.calls.some(([cmd]) => cmd === 'replay_list_sessions')).toBe(false);
  });

  it('读取失败时渲染错误卡，而不是白屏', async () => {
    const user = userEvent.setup();
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      throw new Error('转录超过 16 MiB 上限');
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    expect(await screen.findByText(/转录超过 16 MiB 上限/)).toBeInTheDocument();
  });

  it('session 未命中发现结果（后端返回 null）时如实说明，不渲染空时间线', async () => {
    const user = userEvent.setup();
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      return null;
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    expect(await screen.findByText(/已不在发现列表中/)).toBeInTheDocument();
  });

  it('messages 为空时渲染空态说明', async () => {
    const user = userEvent.setup();
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return { ...REPLAY_SESSION, messages: [] };
      return null;
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    expect(await screen.findByText(/没有可回放的消息/)).toBeInTheDocument();
  });

  it('发现列表为空时给出说明，不报错', async () => {
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      return [];
    });

    render(<ReplayPage />);

    expect(await screen.findByText(/没有发现可回放的 Claude 会话/)).toBeInTheDocument();
  });

  it('浏览器模式下如实说明 IPC 不可用', async () => {
    render(<ReplayPage />);

    expect(await screen.findByText(/请用 pnpm tauri dev 启动桌面应用/)).toBeInTheDocument();
  });
});

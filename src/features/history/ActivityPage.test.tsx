import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ActivityPage } from '@/features/history/ActivityPage';

type Internals = { __TAURI_INTERNALS__?: unknown };

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

/**
 * 按命令名分派，**遇到未预期的命令就抛错**。
 *
 * 这就是 Activity 页只读契约的测试手段（ADR-0012）：只要渲染路径偷偷调用了
 * `refresh_usage` 或别的命令，测试就会失败 —— 不需要真的去观察有没有起进程。
 */
function stubCommands(
  handlers: Record<string, unknown | (() => unknown)>,
): ReturnType<typeof vi.fn> {
  const invoke = vi.fn(async (command: string) => {
    if (!(command in handlers)) {
      throw new Error(`Activity 不得调用 ${command}`);
    }
    const handler = handlers[command];
    return typeof handler === 'function' ? (handler as () => unknown)() : handler;
  });
  (window as unknown as Internals).__TAURI_INTERNALS__ = { invoke };
  return invoke;
}

/** 与 Rust `activity_timeline_serializes_with_camel_case_keys` 同形。 */
function activityTimeline(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    range: {
      kind: 'today',
      startUtc: '2026-09-22T16:00:00Z',
      endUtc: '2026-09-23T16:00:00Z',
      timezoneOffsetMinutes: 480,
      nowUtc: '2026-09-22T23:30:00Z',
    },
    events: [
      {
        kind: 'usage',
        occurredAt: '2026-09-22T23:30:00Z',
        day: '2026-09-23',
        hour: 7,
        harnessId: 'codex',
        projectId: null,
        sourceSessionId: 's-1',
        model: 'gpt-5.6-sol',
        totalTokens: 32,
        costMicrounits: 12_340_000,
        costIsLowerBound: false,
        status: null,
        launchMode: null,
        durationMs: null,
        cwd: null,
        hubSessionId: null,
      },
    ],
    days: [
      {
        day: '2026-09-23',
        usageEventCount: 1,
        managedSessionCount: 0,
        totalTokens: 32,
        costMicrounits: 12_340_000,
        costIsLowerBound: false,
      },
    ],
    hours: Array.from({ length: 24 }, (_, hour) => ({
      hour,
      usageEventCount: hour === 7 ? 1 : 0,
      managedSessionCount: 0,
      totalTokens: hour === 7 ? 32 : 0,
    })),
    usageEventCount: 1,
    managedSessionCount: 0,
    activeDays: 1,
    totalTokens: 32,
    maxDayTokens: 32,
    maxHourTokens: 32,
    hasMore: false,
    ...overrides,
  };
}

function stubActivity(timeline: unknown): ReturnType<typeof vi.fn> {
  return stubCommands({ activity_timeline: timeline });
}

describe('ActivityPage', () => {
  it('mount 只调用 activity_timeline，并带上范围与时区偏移', async () => {
    const invoke = stubActivity(activityTimeline());

    render(<ActivityPage />);

    await waitFor(() => {
      expect(screen.getByTestId('kpi-active-days')).toHaveTextContent('1');
    });

    expect(invoke).toHaveBeenCalled();
    expect(invokesOnlyActivity(invoke));
    const [command, args] = invoke.mock.calls.at(-1) as [string, Record<string, unknown>];
    expect(command).toBe('activity_timeline');
    expect(args?.range).toBe('30d');
    expect(typeof args?.timezoneOffsetMinutes).toBe('number');
    expect(args?.limit).toBe(60);
  });

  it('切换范围会重新查询，而不是复用上一次的结果', async () => {
    const invoke = stubActivity(activityTimeline());

    render(<ActivityPage />);
    await waitFor(() => {
      expect(screen.getByTestId('kpi-tokens')).toHaveTextContent('32');
    });

    await userEvent.click(screen.getByRole('button', { name: '7 天' }));

    await waitFor(() => {
      expect(invoke.mock.calls.at(-1)?.[1]?.range).toBe('7d');
    });
    expect(invokesOnlyActivity(invoke));
  });

  it('空数据库显示空态，而不是白屏', async () => {
    stubActivity(
      activityTimeline({
        events: [],
        days: [],
        usageEventCount: 0,
        managedSessionCount: 0,
        activeDays: 0,
        totalTokens: 0,
      }),
    );

    render(<ActivityPage />);

    expect(await screen.findByText('还没有活动数据')).toBeInTheDocument();
  });

  it('缺失字段退化成安全默认值，不会抛错', async () => {
    stubActivity(activityTimeline({ days: undefined, hours: undefined, events: undefined }));

    render(<ActivityPage />);

    expect(await screen.findByTestId('activity-hours')).toBeInTheDocument();
    expect(screen.getByTestId('kpi-tokens')).toHaveTextContent('32');
    expect(screen.getByText('这个范围内没有按天分布的数据。')).toBeInTheDocument();
  });

  it('点日历里的一天只显示那一天的事件', async () => {
    stubActivity(
      activityTimeline({
        days: [
          {
            day: '2026-09-22',
            usageEventCount: 1,
            managedSessionCount: 0,
            totalTokens: 10,
            costMicrounits: null,
            costIsLowerBound: true,
          },
          {
            day: '2026-09-23',
            usageEventCount: 1,
            managedSessionCount: 0,
            totalTokens: 22,
            costMicrounits: 12_340_000,
            costIsLowerBound: false,
          },
        ],
        events: [
          {
            kind: 'usage',
            occurredAt: '2026-09-22T10:00:00Z',
            day: '2026-09-22',
            hour: 18,
            harnessId: 'codex',
            projectId: null,
            sourceSessionId: 's-0',
            model: 'gpt-5.6-sol',
            totalTokens: 10,
            costMicrounits: null,
            costIsLowerBound: true,
            status: null,
            launchMode: null,
            durationMs: null,
            cwd: null,
            hubSessionId: null,
          },
          {
            kind: 'usage',
            occurredAt: '2026-09-22T23:30:00Z',
            day: '2026-09-23',
            hour: 7,
            harnessId: 'codex',
            projectId: null,
            sourceSessionId: 's-1',
            model: 'gpt-5.6-sol',
            totalTokens: 22,
            costMicrounits: 12_340_000,
            costIsLowerBound: false,
            status: null,
            launchMode: null,
            durationMs: null,
            cwd: null,
            hubSessionId: null,
          },
        ],
      }),
    );

    render(<ActivityPage />);

    const events = await screen.findByTestId('activity-events');
    expect(withinList(events)).toHaveLength(2);

    await userEvent.click(screen.getByTitle('2026-09-22 周二 · 10 tokens · 1 条用量 · 0 个会话'));

    expect(screen.getByText('2026-09-22 的事件。')).toBeInTheDocument();
    expect(withinList(screen.getByTestId('activity-events'))).toHaveLength(1);
    // 成本缺失由后端判定为「≥」，前端只负责把符号显示出来（ADR-0012 决策四）。
    // 事件行与按天表格里都会出现这个值，因此用 `getAllByText`。
    expect(screen.getAllByText('≥ 未知').length).toBeGreaterThan(0);
    expect(screen.getByText('成本不完整')).toBeInTheDocument();
    expect(screen.getByTitle(/2026-09-22 周二 · 10 tokens/)).toBeInTheDocument();
  });
  it('hasMore 提示还有更多事件，聚合不受截断', async () => {
    stubActivity(activityTimeline({ hasMore: true }));

    render(<ActivityPage />);

    expect(await screen.findByText(/事件流最多显示 60 条/)).toBeInTheDocument();
  });

  it('查询失败保留上一次的数字并报错', async () => {
    let calls = 0;
    stubCommands({
      activity_timeline: () => {
        calls += 1;
        if (calls === 1) return activityTimeline();
        throw new Error('boom');
      },
    });

    render(<ActivityPage />);

    expect(await screen.findByTestId('kpi-tokens')).toHaveTextContent('32');

    await userEvent.click(screen.getByRole('button', { name: '今天' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
    expect(screen.getByTestId('kpi-tokens')).toHaveTextContent('32');
  });

  it('浏览器模式下提示用 pnpm tauri dev 启动', async () => {
    stubCommands({
      activity_timeline: () => {
        throw new Error('not-running-in-tauri');
      },
    });

    render(<ActivityPage />);

    expect(await screen.findByText(/请用 pnpm tauri dev 启动桌面应用/)).toBeInTheDocument();
  });

  it('会话事件显示状态与时长', async () => {
    stubActivity(
      activityTimeline({
        events: [
          {
            kind: 'session',
            occurredAt: '2026-09-22T10:00:00Z',
            day: '2026-09-23',
            hour: 18,
            harnessId: 'codex',
            projectId: null,
            sourceSessionId: null,
            model: null,
            totalTokens: 0,
            costMicrounits: null,
            costIsLowerBound: false,
            status: 'exited',
            launchMode: 'terminal',
            durationMs: 95_000,
            cwd: 'D:/HarnessHub',
            hubSessionId: 'hub-1',
          },
        ],
        managedSessionCount: 1,
      }),
    );

    render(<ActivityPage />);

    const events = await screen.findByTestId('activity-events');
    expect(withinList(events)).toHaveLength(1);
    expect(screen.getByText('1 分 35 秒')).toBeInTheDocument();
    expect(screen.getByText('exited')).toBeInTheDocument();
  });
});

function withinList(list: HTMLElement): HTMLElement[] {
  return Array.from(list.querySelectorAll('li'));
}

/** 渲染路径只允许 `activity_timeline`（ADR-0012 只读契约）。 */
function invokesOnlyActivity(invoke: ReturnType<typeof vi.fn>): boolean {
  return invoke.mock.calls.every((call) => call[0] === 'activity_timeline');
}

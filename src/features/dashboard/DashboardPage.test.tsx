import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { DashboardPage } from '@/features/dashboard/DashboardPage';

type Internals = { __TAURI_INTERNALS__?: unknown };

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

/**
 * 按命令名分派，**遇到未预期的命令就抛错**。
 *
 * 这正是 Dashboard 隐私/性能契约的测试手段（ADR-0012）：只要渲染路径偷偷调用了
 * `refresh_usage` 或别的命令，测试就会失败 —— 不需要真的去观察有没有起进程。
 */
function stubCommands(
  handlers: Record<string, unknown | (() => unknown)>,
): ReturnType<typeof vi.fn> {
  const invoke = vi.fn(async (command: string) => {
    if (!(command in handlers)) {
      throw new Error(`Dashboard 不得调用 ${command}`);
    }
    const handler = handlers[command];
    return typeof handler === 'function' ? (handler as () => unknown)() : handler;
  });
  (window as unknown as Internals).__TAURI_INTERNALS__ = { invoke };
  return invoke;
}

const APP_INFO = { version: '0.0.0', tauriVersion: '2.0.0' };
const DB_HEALTH = {
  schemaVersion: 7,
  tableCount: 18,
  foreignKeysEnabled: true,
  journalMode: 'wal',
  appliedMigrations: [],
};

/** 与 Rust `usage_summary_serializes_with_camel_case_keys` 同形。 */
function usageSummary(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    range: {
      kind: '30d',
      startUtc: '2026-08-24T16:00:00Z',
      endUtc: '2026-09-23T16:00:00Z',
      timezoneOffsetMinutes: 480,
      nowUtc: '2026-09-22T23:30:00Z',
    },
    totals: {
      inputTokens: 150,
      outputTokens: 50,
      cachedInputTokens: 0,
      cacheCreationTokens: 0,
      reasoningTokens: 0,
      totalTokens: 200,
    },
    costMicrounits: 12_340_000,
    currency: 'USD',
    costIsLowerBound: false,
    missingPricingRecords: 0,
    timestamplessRecords: 0,
    excludedTimestampless: 0,
    eventCount: 2,
    usageSessions: 2,
    managedSessions: 1,
    byHarness: [
      {
        key: 'codex',
        totalTokens: 200,
        costMicrounits: 12_340_000,
        costIsLowerBound: false,
        eventCount: 2,
      },
    ],
    byModel: [
      {
        key: 'gpt-5.6-sol',
        totalTokens: 200,
        costMicrounits: 12_340_000,
        costIsLowerBound: false,
        eventCount: 2,
      },
    ],
    byProject: [],
    timeline: [
      {
        day: '2026-09-23',
        totalTokens: 200,
        costMicrounits: 12_340_000,
        costIsLowerBound: false,
        eventCount: 2,
      },
    ],
    ...overrides,
  };
}

function stubDashboard(summary: unknown): ReturnType<typeof vi.fn> {
  return stubCommands({
    app_info: APP_INFO,
    db_health: DB_HEALTH,
    usage_summary: summary,
  });
}

describe('DashboardPage', () => {
  it('空库时显示 0 与空状态，而不是报错或留白', async () => {
    stubDashboard(
      usageSummary({
        totals: {
          inputTokens: 0,
          outputTokens: 0,
          cachedInputTokens: 0,
          cacheCreationTokens: 0,
          reasoningTokens: 0,
          totalTokens: 0,
        },
        costMicrounits: null,
        currency: null,
        eventCount: 0,
        usageSessions: 0,
        managedSessions: 0,
        byHarness: [],
        byModel: [],
        timeline: [],
      }),
    );

    render(<DashboardPage />);

    expect(await screen.findByText('还没有使用量数据')).toBeInTheDocument();
    expect(screen.getByTestId('kpi-tokens')).toHaveTextContent('0');
    expect(screen.getByTestId('kpi-usage-sessions')).toHaveTextContent('0');
    expect(screen.getByTestId('kpi-managed-sessions')).toHaveTextContent('0');
    expect(screen.getByTestId('kpi-cost')).toHaveTextContent('—');
  });

  it('有数据时显示 Token、金额与两个不同的会话数', async () => {
    stubDashboard(usageSummary());

    render(<DashboardPage />);

    await waitFor(() => {
      expect(screen.getByTestId('kpi-tokens')).toHaveTextContent('200');
    });
    expect(screen.getByTestId('kpi-cost')).toHaveTextContent('$12.34');
    // Usage Sessions 与 Managed Sessions 必须分开显示（2 vs 1）。
    expect(screen.getByText('Usage Sessions')).toBeInTheDocument();
    expect(screen.getByText('Managed Sessions')).toBeInTheDocument();
    expect(screen.getByTestId('kpi-usage-sessions')).toHaveTextContent('2');
    expect(screen.getByTestId('kpi-managed-sessions')).toHaveTextContent('1');
    expect(screen.getByText('gpt-5.6-sol')).toBeInTheDocument();
  });

  it('缺价格时显示 ≥ 与提示，而不是精确金额', async () => {
    stubDashboard(
      usageSummary({
        costMicrounits: 0,
        costIsLowerBound: true,
        missingPricingRecords: 1,
      }),
    );

    render(<DashboardPage />);

    expect(await screen.findByText('≥ $0.00')).toBeInTheDocument();
    expect(screen.getByText(/部分记录缺少价格（1 条），实际成本可能更高/)).toBeInTheDocument();
  });

  it('mount 与切换范围只调用 usage_summary，绝不自动刷新', async () => {
    const invoke = stubDashboard(usageSummary());
    const user = userEvent.setup();

    render(<DashboardPage />);
    await screen.findByTestId('kpi-tokens');

    await user.click(screen.getByRole('button', { name: '今天' }));

    await waitFor(() => {
      const commands = invoke.mock.calls.map((call) => call[0]);
      expect(commands).toContain('usage_summary');
      expect(commands).not.toContain('refresh_usage');
      expect(
        commands.every((name) => ['app_info', 'db_health', 'usage_summary'].includes(String(name))),
      ).toBe(true);
    });

    const summaryCalls = invoke.mock.calls.filter((call) => call[0] === 'usage_summary');
    expect(summaryCalls.length).toBeGreaterThanOrEqual(2);
    expect(summaryCalls.at(-1)?.[1]).toMatchObject({ range: 'today' });
  });

  it('切换范围失败时保留上一次的数字并显示错误', async () => {
    const invoke = vi.fn(async (command: string, args?: unknown) => {
      if (command === 'app_info') return APP_INFO;
      if (command === 'db_health') return DB_HEALTH;
      if (command === 'usage_summary') {
        const range = (args as { range: string }).range;
        if (range === 'today') throw new Error('boom');
        return usageSummary();
      }
      throw new Error(`Dashboard 不得调用 ${command}`);
    });
    (window as unknown as Internals).__TAURI_INTERNALS__ = { invoke };
    const user = userEvent.setup();

    render(<DashboardPage />);
    await screen.findByTestId('kpi-tokens');

    await user.click(screen.getByRole('button', { name: '今天' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('boom');
    expect(screen.getByTestId('kpi-tokens')).toHaveTextContent('200');
  });

  it('只有点击「刷新用量」才会调用 refresh_usage，并在成功后重新查询', async () => {
    const invoke = stubCommands({
      app_info: APP_INFO,
      db_health: DB_HEALTH,
      usage_summary: usageSummary(),
      refresh_usage: {
        import: {
          id: 'import-1',
          source: 'ccusage',
          status: 'succeeded',
          recordsSeen: 7,
          recordsInserted: 7,
          recordsUpdated: 0,
          recordsSkipped: 0,
        },
        reconciliation: { eventsTotalTokens: 200 },
      },
    });
    const user = userEvent.setup();

    render(<DashboardPage />);
    await screen.findByTestId('kpi-tokens');

    await user.click(screen.getByRole('button', { name: '刷新用量' }));

    await waitFor(() => {
      const commands = invoke.mock.calls.map((call) => call[0]);
      expect(commands).toContain('refresh_usage');
      const refreshIndex = commands.lastIndexOf('refresh_usage');
      expect(commands.slice(refreshIndex)).toContain('usage_summary');
    });
    expect(await screen.findByText(/已读取 7 条记录/)).toBeInTheDocument();
  });

  it('刷新失败时显示错误且不清零已有数字', async () => {
    stubCommands({
      app_info: APP_INFO,
      db_health: DB_HEALTH,
      usage_summary: usageSummary(),
      refresh_usage: () => {
        throw new Error('外部命令失败（退出码 2）');
      },
    });
    const user = userEvent.setup();

    render(<DashboardPage />);
    await screen.findByTestId('kpi-tokens');

    await user.click(screen.getByRole('button', { name: '刷新用量' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('退出码 2');
    expect(screen.getByTestId('kpi-tokens')).toHaveTextContent('200');
  });

  it('无时间戳记录被排除时给出解释', async () => {
    stubDashboard(usageSummary({ excludedTimestampless: 3 }));

    render(<DashboardPage />);

    expect(await screen.findByText(/另有 3 条记录没有可推导的发生时间/)).toBeInTheDocument();
  });
});

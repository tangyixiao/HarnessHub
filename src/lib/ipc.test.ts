import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  NOT_IN_TAURI,
  createSession,
  finishSession,
  getAppInfo,
  getDbHealth,
  getUsageSources,
  invokeCommand,
  isTauriRuntime,
  listHarnesses,
  listSessions,
  normalizeHarnessSummary,
  normalizeSessionRecord,
  normalizeUsageCapabilities,
  normalizeUsageImport,
  normalizeUsageReconciliation,
  normalizeUsageSource,
  refreshHarnesses,
  refreshUsage,
} from '@/lib/ipc';

type Internals = { __TAURI_INTERNALS__?: unknown };

function stubTauri(invoke: (cmd: string, args?: unknown) => Promise<unknown>): void {
  (window as unknown as Internals).__TAURI_INTERNALS__ = { invoke };
}

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

describe('isTauriRuntime', () => {
  it('在没有 __TAURI_INTERNALS__ 时为 false', () => {
    expect(isTauriRuntime()).toBe(false);
  });

  it('在存在 __TAURI_INTERNALS__.invoke 时为 true', () => {
    stubTauri(async () => null);
    expect(isTauriRuntime()).toBe(true);
  });

  it('在 __TAURI_INTERNALS__ 存在但没有 invoke 时为 false', () => {
    (window as unknown as Internals).__TAURI_INTERNALS__ = {};
    expect(isTauriRuntime()).toBe(false);
  });
});

describe('invokeCommand', () => {
  it('不在 Tauri 运行时返回 not-running-in-tauri，而不是抛异常', async () => {
    await expect(invokeCommand('app_info')).resolves.toEqual({
      ok: false,
      error: NOT_IN_TAURI,
    });
  });

  it('把命令结果包装成 ok:true', async () => {
    stubTauri(async (cmd, args) => ({ cmd, args }));
    await expect(invokeCommand('ping', { a: 1 })).resolves.toEqual({
      ok: true,
      data: { cmd: 'ping', args: { a: 1 } },
    });
  });

  it('把命令异常包装成 ok:false 且保留消息', async () => {
    stubTauri(async () => {
      throw new Error('migration failed');
    });
    await expect(invokeCommand('db_health')).resolves.toEqual({
      ok: false,
      error: 'migration failed',
    });
  });

  it('把非 Error 抛出物也转成字符串', async () => {
    stubTauri(async () => {
      throw 'boom';
    });
    await expect(invokeCommand('db_health')).resolves.toEqual({ ok: false, error: 'boom' });
  });
});

describe('typed command wrappers', () => {
  it('getAppInfo 使用 app_info 命令名', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => ({
      name: 'Harness Hub',
      version: '0.0.0',
      tauriVersion: '2',
    }));
    stubTauri(invokeSpy);

    const result = await getAppInfo();

    // 只锁命令名：@tauri-apps/api 会自行补齐默认参数与 options。
    expect(invokeSpy).toHaveBeenCalledTimes(1);
    expect(invokeSpy.mock.calls[0]?.[0]).toBe('app_info');
    expect(result).toEqual({
      ok: true,
      data: { name: 'Harness Hub', version: '0.0.0', tauriVersion: '2' },
    });
  });

  it('getDbHealth 使用 db_health 命令名', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => ({
      schemaVersion: 1,
      appliedMigrations: ['0001_init'],
      tableCount: 14,
      foreignKeysEnabled: true,
      journalMode: 'wal',
    }));
    stubTauri(invokeSpy);

    const result = await getDbHealth();

    expect(invokeSpy).toHaveBeenCalledTimes(1);
    expect(invokeSpy.mock.calls[0]?.[0]).toBe('db_health');
    expect(result.ok).toBe(true);
  });
});

/**
 * Rust 侧 `HarnessSummary` 是 IPC 契约类型：serde camelCase，
 * capabilities 的 `tool_calls` / `live_state` 序列化为 `toolCalls` / `liveState`。
 * 这里用一份手工复刻的 payload 锁住前端侧的解析行为。
 */
const CODEX_PAYLOAD = {
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

describe('normalizeHarnessSummary', () => {
  it('保留合法 payload 的全部字段', () => {
    expect(normalizeHarnessSummary(CODEX_PAYLOAD)).toEqual(CODEX_PAYLOAD);
  });

  it('缺 id 的条目返回 null，由调用方过滤掉', () => {
    expect(normalizeHarnessSummary({ displayName: 'Codex' })).toBeNull();
    expect(normalizeHarnessSummary(null)).toBeNull();
    expect(normalizeHarnessSummary('codex')).toBeNull();
    expect(normalizeHarnessSummary({ id: '' })).toBeNull();
  });

  it('字段缺失时补安全默认值，而不是产生 undefined', () => {
    const summary = normalizeHarnessSummary({ id: 'codex' });

    expect(summary).not.toBeNull();
    expect(summary?.displayName).toBe('codex');
    expect(summary?.installed).toBe(false);
    expect(summary?.binaryPath).toBeNull();
    expect(summary?.version).toBeNull();
    expect(summary?.dataPaths).toEqual([]);
    expect(summary?.capabilities).toEqual({
      launch: false,
      terminal: false,
      resume: false,
      usage: false,
      replay: false,
      toolCalls: false,
      subagents: false,
      liveState: false,
      worktree: false,
    });
  });

  it('capabilities 不是对象时不崩溃', () => {
    const summary = normalizeHarnessSummary({ id: 'codex', capabilities: 'nonsense' });

    expect(summary?.capabilities.launch).toBe(false);
  });

  it('capabilities 里的非布尔值一律视为 false，不渲染假支持', () => {
    const summary = normalizeHarnessSummary({
      id: 'codex',
      capabilities: { launch: 'yes', usage: 1, terminal: true },
    });

    expect(summary?.capabilities.terminal).toBe(true);
    expect(summary?.capabilities.launch).toBe(false);
    expect(summary?.capabilities.usage).toBe(false);
  });

  it('dataPaths 里的非字符串元素被过滤掉', () => {
    const summary = normalizeHarnessSummary({ id: 'codex', dataPaths: ['/a', 42, null] });

    expect(summary?.dataPaths).toEqual(['/a']);
  });
});

describe('listHarnesses', () => {
  it('使用 list_harnesses 命令名', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => [CODEX_PAYLOAD]);
    stubTauri(invokeSpy);

    const result = await listHarnesses();

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('list_harnesses');
    expect(result).toEqual({ ok: true, data: [CODEX_PAYLOAD] });
  });

  it('不在 Tauri 运行时返回 not-running-in-tauri', async () => {
    await expect(listHarnesses()).resolves.toEqual({ ok: false, error: NOT_IN_TAURI });
  });

  it('后端返回非数组时不崩溃，按空列表处理', async () => {
    stubTauri(async () => ({ unexpected: true }));

    await expect(listHarnesses()).resolves.toEqual({ ok: true, data: [] });
  });

  it('丢弃无法解析的条目而不是让整页失败', async () => {
    stubTauri(async () => [CODEX_PAYLOAD, { nope: 1 }, null]);

    const result = await listHarnesses();

    expect(result.ok).toBe(true);
    expect(result.ok && result.data).toHaveLength(1);
  });

  it('命令抛错时返回 ok:false 而不是崩溃', async () => {
    stubTauri(async () => {
      throw new Error('detect failed');
    });

    await expect(listHarnesses()).resolves.toEqual({ ok: false, error: 'detect failed' });
  });
});

/** 与 Rust `SessionRecord`（serde camelCase）同形。 */
const SESSION_PAYLOAD = {
  hubSessionId: '6f1c0f7e-0000-4000-8000-000000000001',
  sourceSessionId: null,
  harnessId: 'codex',
  installationId: 'codex@local',
  projectId: null,
  runtimeTargetId: 'local',
  parentSessionId: null,
  status: 'created',
  launchMode: 'terminal',
  cwd: 'D:/work',
  worktreePath: null,
  startedAt: '2026-09-22T10:00:00Z',
  endedAt: null,
  exitCode: null,
  terminationReason: null,
  pid: null,
};

describe('normalizeSessionRecord', () => {
  it('保留合法 payload 的全部字段', () => {
    expect(normalizeSessionRecord(SESSION_PAYLOAD)).toEqual(SESSION_PAYLOAD);
  });

  it('缺 hubSessionId 的条目返回 null', () => {
    expect(normalizeSessionRecord({ status: 'running' })).toBeNull();
    expect(normalizeSessionRecord(null)).toBeNull();
    expect(normalizeSessionRecord({ hubSessionId: '   ' })).toBeNull();
  });

  it('created 是合法状态，不得被降级为 unknown', () => {
    expect(normalizeSessionRecord({ ...SESSION_PAYLOAD, status: 'created' })?.status).toBe(
      'created',
    );
  });

  it('未知 status 归一化为 unknown，不得当成 running', () => {
    const record = normalizeSessionRecord({ ...SESSION_PAYLOAD, status: 'weird' });

    expect(record?.status).toBe('unknown');
  });

  it('未知 launchMode 归一化为 terminal（与 Rust 侧解码一致）', () => {
    const record = normalizeSessionRecord({ ...SESSION_PAYLOAD, launchMode: 'telepathy' });

    expect(record?.launchMode).toBe('terminal');
  });

  it('字段缺失时补安全默认值', () => {
    const record = normalizeSessionRecord({ hubSessionId: 'h1' });

    expect(record).not.toBeNull();
    expect(record?.harnessId).toBe('');
    expect(record?.installationId).toBeNull();
    expect(record?.runtimeTargetId).toBe('');
    expect(record?.status).toBe('unknown');
    expect(record?.endedAt).toBeNull();
    expect(record?.exitCode).toBeNull();
  });

  it('exitCode 非数字时视为 null，不把字符串渲染出来', () => {
    const record = normalizeSessionRecord({ ...SESSION_PAYLOAD, exitCode: '130' });

    expect(record?.exitCode).toBeNull();
  });

  it('terminationReason 保留已知值', () => {
    for (const reason of [
      'natural_exit',
      'user_killed',
      'launch_failed',
      'runtime_error',
      'host_shutdown',
      'lost',
    ]) {
      expect(
        normalizeSessionRecord({ ...SESSION_PAYLOAD, terminationReason: reason })
          ?.terminationReason,
      ).toBe(reason);
    }
  });

  it('未知 terminationReason 视为未记录，绝不当作正常退出', () => {
    const record = normalizeSessionRecord({
      ...SESSION_PAYLOAD,
      terminationReason: 'because-i-said-so',
    });

    expect(record?.terminationReason).toBeNull();
  });
});

describe('session commands', () => {
  it('listSessions 使用 list_sessions 并按需传 limit', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => [SESSION_PAYLOAD]);
    stubTauri(invokeSpy);

    const result = await listSessions(10);

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('list_sessions');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({ limit: 10 });
    expect(result).toEqual({ ok: true, data: [SESSION_PAYLOAD] });
  });

  it('listSessions 不传 limit 时不带上 limit 参数', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => []);
    stubTauri(invokeSpy);

    await listSessions();

    // @tauri-apps/api 会把缺失的 args 补成 {}，所以断言「没有 limit 键」而不是 undefined。
    const args = invokeSpy.mock.calls[0]?.[1] as Record<string, unknown> | undefined;
    expect(args?.limit).toBeUndefined();
  });

  it('listSessions 后端返回非数组时按空列表处理', async () => {
    stubTauri(async () => null);

    await expect(listSessions()).resolves.toEqual({ ok: true, data: [] });
  });

  it('createSession 传 camelCase 参数（Rust 侧 harness_id / project_id / cwd）', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => SESSION_PAYLOAD);
    stubTauri(invokeSpy);

    const result = await createSession({ harnessId: 'codex', cwd: 'D:/work' });

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('create_session');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({
      harnessId: 'codex',
      projectId: null,
      cwd: 'D:/work',
    });
    expect(result.ok).toBe(true);
  });

  it('finishSession 传 hubSessionId 与 exitCode', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => true);
    stubTauri(invokeSpy);

    const result = await finishSession('h1', 'user_killed', 0);

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('finish_session');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({
      hubSessionId: 'h1',
      exitCode: 0,
      reason: 'user_killed',
    });
    expect(result).toEqual({ ok: true, data: true });
  });

  it('不在 Tauri 运行时一律返回 not-running-in-tauri', async () => {
    await expect(listSessions()).resolves.toEqual({ ok: false, error: NOT_IN_TAURI });
    await expect(createSession({ harnessId: 'codex' })).resolves.toEqual({
      ok: false,
      error: NOT_IN_TAURI,
    });
    await expect(finishSession('h1', 'natural_exit')).resolves.toEqual({
      ok: false,
      error: NOT_IN_TAURI,
    });
  });
});

describe('refreshHarnesses', () => {
  it('使用 refresh_harnesses 命令名并返回同步计数', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => ({
      harnesses: 1,
      installations: 1,
    }));
    stubTauri(invokeSpy);

    const result = await refreshHarnesses();

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('refresh_harnesses');
    expect(result).toEqual({ ok: true, data: { harnesses: 1, installations: 1 } });
  });

  it('后端返回异常形状时收敛为 0，而不是抛错', async () => {
    stubTauri(async () => ({ unexpected: true }));

    await expect(refreshHarnesses()).resolves.toEqual({
      ok: true,
      data: { harnesses: 0, installations: 0 },
    });
  });

  it('不在 Tauri 运行时返回 not-running-in-tauri', async () => {
    await expect(refreshHarnesses()).resolves.toEqual({ ok: false, error: NOT_IN_TAURI });
  });
});

/*
 * 这些 payload 的形状与 Rust 侧的序列化契约测试**一一对应**
 * （`usage::tests::usage_source_serializes_with_camel_case_keys`、
 * `commands::tests::usage_refresh_report_serializes_with_camel_case_keys`）。
 * 两边用同一形状，任何一侧悄悄改名都会被其中一侧抓住。
 */
describe('normalizeUsageSource', () => {
  const available = {
    id: 'ccusage',
    displayName: 'ccusage',
    version: 'ccusage 20.0.24',
    status: 'available',
    capabilities: { detect: true, import: true, watch: false, reconcile: true },
    runner: 'managed-npx',
    reason: null,
  };

  it('解析 Rust 侧的真实形状', () => {
    expect(normalizeUsageSource(available)).toEqual(available);
  });

  it('缺少 id 时返回 null，由调用方丢弃该条目', () => {
    expect(normalizeUsageSource({ status: 'available' })).toBeNull();
    expect(normalizeUsageSource(null)).toBeNull();
  });

  it('未知 status / runner 收敛为安全默认值', () => {
    const normalized = normalizeUsageSource({
      id: 'ccusage',
      status: 'something-new',
      runner: 'latest',
    });

    expect(normalized?.status).toBe('unavailable');
    expect(normalized?.runner).toBeNull();
  });

  it('缺失能力一律为 false，不声称没实现的能力', () => {
    expect(normalizeUsageCapabilities({ detect: true })).toEqual({
      detect: true,
      import: false,
      watch: false,
      reconcile: false,
    });
    expect(normalizeUsageCapabilities(undefined)).toEqual({
      detect: false,
      import: false,
      watch: false,
      reconcile: false,
    });
  });
});

describe('normalizeUsageImport', () => {
  const succeeded = {
    id: 'import-1',
    source: 'ccusage',
    sourceVersion: 'ccusage 20.0.24',
    runner: 'managed-npx',
    reportKind: 'session',
    status: 'succeeded',
    startedAt: '2026-09-22T10:00:00Z',
    completedAt: '2026-09-22T10:00:01Z',
    recordsSeen: 7,
    recordsInserted: 7,
    recordsUpdated: 0,
    recordsSkipped: 0,
    recordsTimestampless: 1,
    error: null,
  };

  it('解析 Rust 侧的真实形状', () => {
    expect(normalizeUsageImport(succeeded)).toEqual(succeeded);
  });

  it('字段缺失时不抛异常，也不编造计数', () => {
    const normalized = normalizeUsageImport({ id: 'import-2' });

    expect(normalized).not.toBeNull();
    expect(normalized?.recordsInserted).toBe(0);
    expect(normalized?.status).toBe('failed');
    expect(normalized?.sourceVersion).toBeNull();
  });

  it('保留失败原因（失败必须能被解释）', () => {
    const normalized = normalizeUsageImport({
      id: 'import-3',
      status: 'failed',
      error: '外部命令失败（退出码 2）：Unknown session option',
    });

    expect(normalized?.error).toContain('退出码 2');
  });
});

describe('normalizeUsageReconciliation', () => {
  it('保留三态：true / false / null 含义不同', () => {
    const identity = normalizeUsageReconciliation({
      eventsTotalTokens: 600,
      reportTotalTokens: 1510,
      rowTotalResidual: 910,
      costMicrounitsDelta: 4,
      unpricedEvents: 1,
      timestampless: 0,
      tokensIdentityHolds: true,
    });
    expect(identity.tokensIdentityHolds).toBe(true);
    expect(identity.rowTotalResidual).toBe(910);
    expect(identity.costMicrounitsDelta).toBe(4);

    expect(
      normalizeUsageReconciliation({ reportTotalTokens: null, tokensIdentityHolds: null })
        .tokensIdentityHolds,
    ).toBeNull();
  });

  it('形状异常时全部收敛为安全的空值，而不是抛错', () => {
    expect(normalizeUsageReconciliation(undefined)).toEqual({
      eventsTotalTokens: 0,
      reportTotalTokens: null,
      rowTotalResidual: 0,
      costMicrounitsDelta: null,
      unpricedEvents: 0,
      timestampless: 0,
      tokensIdentityHolds: null,
    });
  });
});

describe('getUsageSources', () => {
  it('使用 get_usage_sources 命令名并过滤掉无法解析的条目', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => [
      { id: 'ccusage', status: 'available', capabilities: { detect: true } },
      { nonsense: true },
    ]);
    stubTauri(invokeSpy);

    const result = await getUsageSources();

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('get_usage_sources');
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.data).toHaveLength(1);
      expect(result.data[0]?.id).toBe('ccusage');
    }
  });

  it('不在 Tauri 运行时返回 not-running-in-tauri', async () => {
    await expect(getUsageSources()).resolves.toEqual({ ok: false, error: NOT_IN_TAURI });
  });
});

describe('refreshUsage', () => {
  it('使用 refresh_usage 命令名并返回审计行 + 对账', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => ({
      import: {
        id: 'import-1',
        source: 'ccusage',
        status: 'succeeded',
        recordsSeen: 7,
        recordsInserted: 7,
      },
      reconciliation: { eventsTotalTokens: 600, reportTotalTokens: 1510, rowTotalResidual: 910 },
    }));
    stubTauri(invokeSpy);

    const result = await refreshUsage();

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('refresh_usage');
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.data.import.recordsInserted).toBe(7);
      expect(result.data.reconciliation.rowTotalResidual).toBe(910);
    }
  });

  it('缺少 import 时返回明确错误，而不是伪造一次成功', async () => {
    stubTauri(async () => ({ reconciliation: {} }));

    await expect(refreshUsage()).resolves.toEqual({
      ok: false,
      error: 'invalid-usage-import-payload',
    });
  });

  it('把后端的失败原样传给调用方', async () => {
    stubTauri(async () => {
      throw new Error('外部命令失败（退出码 2）');
    });

    const result = await refreshUsage();

    expect(result.ok).toBe(false);
  });

  it('不在 Tauri 运行时返回 not-running-in-tauri', async () => {
    await expect(refreshUsage()).resolves.toEqual({ ok: false, error: NOT_IN_TAURI });
  });
});

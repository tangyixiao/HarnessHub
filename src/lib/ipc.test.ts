import { afterEach, describe, expect, it, vi } from 'vitest';
import codebuddySummary from '../../fixtures/codebuddy/summary.json';
import geminiSummary from '../../fixtures/gemini/summary.json';
import opencodeSummary from '../../fixtures/opencode/summary.json';

import {
  NOT_IN_TAURI,
  createSession,
  finishSession,
  getActivityTimeline,
  getAppInfo,
  getDbHealth,
  getProjectSessions,
  getSessionGitSnapshots,
  getUsageSources,
  getUsageSummary,
  invokeCommand,
  isTauriRuntime,
  listHarnesses,
  listProjects,
  listSessions,
  listPlugins,
  normalizeActivityTimeline,
  normalizeHarnessSummary,
  normalizePlugin,
  normalizePluginRefreshReport,
  normalizeProjectRecord,
  normalizeSessionGitSnapshot,
  normalizeSessionGitSummary,
  normalizeSessionRecord,
  normalizeUsageCapabilities,
  normalizeUsageImport,
  normalizeUsageReconciliation,
  normalizeUsageSource,
  normalizeUsageSummary,
  pluginRefresh,
  refreshHarnesses,
  refreshUsage,
  registerProject,
  removeProject,
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
  it('Gemini fixture 与 Rust serialization contract 共用同一份，字段逐一保留', () => {
    expect(normalizeHarnessSummary(geminiSummary)).toEqual(geminiSummary);
  });
  it('CodeBuddy fixture 与 Rust serialization contract 共用同一份，字段逐一保留', () => {
    expect(normalizeHarnessSummary(codebuddySummary)).toEqual(codebuddySummary);
  });
  it('OpenCode fixture 与 Rust serialization contract 共用同一份，字段逐一保留', () => {
    expect(normalizeHarnessSummary(opencodeSummary)).toEqual(opencodeSummary);
  });
  it('字段残缺的 payload 补全 false 安全默认值，不渲染假支持', () => {
    const incomplete = normalizeHarnessSummary({ id: 'gemini', installed: true });
    expect(incomplete?.capabilities).toEqual({
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
    expect(incomplete?.installationId).toBeNull();
  });
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

describe('Project registry IPC（§20 第 14 步）', () => {
  const PROJECT_PAYLOAD = {
    id: 'proj-1',
    name: 'demo',
    repoRoot: 'D:/work/demo',
    createdAt: '2026-10-12T00:00:00Z',
    updatedAt: '2026-10-12T00:00:01Z',
    lastOpenedAt: '2026-10-12T00:00:01Z',
  };

  it('normalizeProjectRecord 完整保留字段', () => {
    expect(normalizeProjectRecord(PROJECT_PAYLOAD)).toEqual(PROJECT_PAYLOAD);
  });

  it('normalizeProjectRecord 拿不到 id 返回 null，缺字段补安全默认值', () => {
    expect(normalizeProjectRecord({ name: '无 id' })).toBeNull();
    expect(normalizeProjectRecord('not-an-object')).toBeNull();
    expect(normalizeProjectRecord({ id: 'proj-1' })).toEqual({
      id: 'proj-1',
      name: '',
      repoRoot: '',
      createdAt: '',
      updatedAt: '',
      lastOpenedAt: null,
    });
  });

  it('registerProject 传 path 与 name（Rust 侧 path / name）', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => PROJECT_PAYLOAD);
    stubTauri(invokeSpy);

    const result = await registerProject('D:/work/demo', '工作仓库');

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('register_project');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({ path: 'D:/work/demo', name: '工作仓库' });
    expect(result).toEqual({ ok: true, data: PROJECT_PAYLOAD });
  });

  it('registerProject 不传 name 时传 null', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => PROJECT_PAYLOAD);
    stubTauri(invokeSpy);

    await registerProject('D:/work/demo');

    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({ path: 'D:/work/demo', name: null });
  });

  it('listProjects 使用 list_projects 并按需传 limit', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => [PROJECT_PAYLOAD]);
    stubTauri(invokeSpy);

    const result = await listProjects(10);
    expect(invokeSpy.mock.calls[0]?.[0]).toBe('list_projects');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({ limit: 10 });
    expect(result).toEqual({ ok: true, data: [PROJECT_PAYLOAD] });

    await listProjects();
    // @tauri-apps/api 会把缺失的 args 补成 {}（同 listSessions 测试），断言「没有 limit 键」。
    expect((invokeSpy.mock.calls[1]?.[1] as Record<string, unknown>)?.limit).toBeUndefined();
  });

  it('listProjects 后端返回非数组时按空列表处理', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => 'unexpected');
    stubTauri(invokeSpy);

    await expect(listProjects()).resolves.toEqual({ ok: true, data: [] });
  });

  it('removeProject 传 projectId（Rust 侧 project_id）', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => true);
    stubTauri(invokeSpy);

    const result = await removeProject('proj-1');

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('remove_project');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({ projectId: 'proj-1' });
    expect(result).toEqual({ ok: true, data: true });
  });

  it('getProjectSessions 传 projectId 并归一化 session 记录', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => [
      {
        hubSessionId: 's1',
        harnessId: 'codex',
        runtimeTargetId: 'local',
        status: 'exited',
        launchMode: 'terminal',
        startedAt: '2026-10-12T00:00:00Z',
      },
    ]);
    stubTauri(invokeSpy);

    const result = await getProjectSessions('proj-1', 10);

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('get_project_sessions');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({ projectId: 'proj-1', limit: 10 });
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.data).toHaveLength(1);
      expect(result.data[0]?.hubSessionId).toBe('s1');
      expect(result.data[0]?.projectId).toBeNull();
    }
  });

  it('项目命令不在 Tauri 运行时一律返回 not-running-in-tauri', async () => {
    await expect(registerProject('D:/work/demo')).resolves.toEqual({
      ok: false,
      error: NOT_IN_TAURI,
    });
    await expect(listProjects()).resolves.toEqual({ ok: false, error: NOT_IN_TAURI });
    await expect(removeProject('proj-1')).resolves.toEqual({ ok: false, error: NOT_IN_TAURI });
    await expect(getProjectSessions('proj-1')).resolves.toEqual({
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

describe('normalizeUsageSummary', () => {
  /** 与 Rust `usage_summary_serializes_with_camel_case_keys` 同形。 */
  const payload = {
    range: {
      kind: 'all',
      startUtc: null,
      endUtc: null,
      timezoneOffsetMinutes: 480,
      nowUtc: '2026-09-22T23:30:00Z',
    },
    totals: {
      inputTokens: 150,
      outputTokens: 0,
      cachedInputTokens: 0,
      cacheCreationTokens: 0,
      reasoningTokens: 0,
      totalTokens: 150,
    },
    costMicrounits: 1000,
    currency: 'USD',
    costIsLowerBound: true,
    missingPricingRecords: 1,
    timestamplessRecords: 0,
    excludedTimestampless: 0,
    eventCount: 2,
    usageSessions: 2,
    managedSessions: 0,
    byHarness: [
      {
        key: 'codex',
        totalTokens: 100,
        costMicrounits: 1000,
        costIsLowerBound: false,
        eventCount: 1,
      },
    ],
    byModel: [],
    byProject: [],
    timeline: [
      {
        day: '2026-09-23',
        totalTokens: 150,
        costMicrounits: 1000,
        costIsLowerBound: true,
        eventCount: 2,
      },
    ],
  };

  it('解析 Rust 侧的真实形状', () => {
    const summary = normalizeUsageSummary(payload);

    expect(summary?.totals.totalTokens).toBe(150);
    expect(summary?.costMicrounits).toBe(1000);
    expect(summary?.costIsLowerBound).toBe(true);
    expect(summary?.usageSessions).toBe(2);
    expect(summary?.managedSessions).toBe(0);
    expect(summary?.timeline[0]?.day).toBe('2026-09-23');
  });

  it('完全不是对象时才返回 null（其余形状都收敛成安全默认值）', () => {
    expect(normalizeUsageSummary(null)).toBeNull();
    expect(normalizeUsageSummary('nope')).toBeNull();

    const partial = normalizeUsageSummary({ totals: { totalTokens: 5 } });
    expect(partial?.totals.totalTokens).toBe(5);
    expect(partial?.costMicrounits).toBeNull();
    expect(partial?.costIsLowerBound).toBe(false);
    expect(partial?.byHarness).toEqual([]);
    expect(partial?.range.kind).toBe('all');
  });

  it('丢掉没有 key 的分组，而不是渲染一个空名字的行', () => {
    const summary = normalizeUsageSummary({
      byModel: [{ totalTokens: 1 }, { key: 'gpt-5', totalTokens: 2 }],
    });

    expect(summary?.byModel).toEqual([
      {
        key: 'gpt-5',
        totalTokens: 2,
        costMicrounits: null,
        costIsLowerBound: false,
        eventCount: 0,
      },
    ]);
  });

  it('未知 range 收敛为 all，不带任何时间边界', () => {
    const summary = normalizeUsageSummary({ range: { kind: 'last-year', startUtc: 'x' } });

    expect(summary?.range.kind).toBe('all');
    expect(summary?.range.startUtc).toBe('x');
  });
});

describe('getUsageSummary', () => {
  it('把 range 与本地时区偏移一起传给 usage_summary', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => ({ totals: { totalTokens: 7 } }));
    stubTauri(invokeSpy);

    const result = await getUsageSummary({ range: '7d', timezoneOffsetMinutes: 480 });

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('usage_summary');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({
      range: '7d',
      timezoneOffsetMinutes: 480,
    });
    expect(result.ok).toBe(true);
  });

  it('缺省时区偏移时用本机时区（不是 0）', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => ({}));
    stubTauri(invokeSpy);

    await getUsageSummary({ range: 'today' });

    const args = invokeSpy.mock.calls[0]?.[1] as { timezoneOffsetMinutes: number };
    expect(args.timezoneOffsetMinutes).toBe(-new Date().getTimezoneOffset());
  });

  it('形状完全不对时返回明确错误，而不是伪造一份空汇总', async () => {
    stubTauri(async () => 42);

    await expect(getUsageSummary({ range: 'all' })).resolves.toEqual({
      ok: false,
      error: 'invalid-usage-summary-payload',
    });
  });

  it('不在 Tauri 运行时返回 not-running-in-tauri', async () => {
    await expect(getUsageSummary({ range: 'all' })).resolves.toEqual({
      ok: false,
      error: NOT_IN_TAURI,
    });
  });
});

describe('Activity Timeline IPC（§20 第 15 步）', () => {
  /** 与 Rust `activity_timeline_serializes_with_camel_case_keys` 同形。 */
  const payload = {
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
      {
        kind: 'session',
        occurredAt: '2026-09-22T10:00:00Z',
        day: '2026-09-22',
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
    days: [
      {
        day: '2026-09-22',
        usageEventCount: 0,
        managedSessionCount: 1,
        totalTokens: 0,
        costMicrounits: null,
        costIsLowerBound: false,
      },
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
    managedSessionCount: 1,
    activeDays: 2,
    totalTokens: 32,
    maxDayTokens: 32,
    maxHourTokens: 32,
    hasMore: false,
  };

  it('解析 Rust 侧的真实形状', () => {
    const timeline = normalizeActivityTimeline(payload);

    expect(timeline?.usageEventCount).toBe(1);
    expect(timeline?.managedSessionCount).toBe(1);
    expect(timeline?.activeDays).toBe(2);
    expect(timeline?.totalTokens).toBe(32);
    expect(timeline?.hasMore).toBe(false);
    expect(timeline?.range.kind).toBe('today');
    expect(timeline?.range.timezoneOffsetMinutes).toBe(480);
    expect(timeline?.events[0]?.kind).toBe('usage');
    expect(timeline?.events[0]?.day).toBe('2026-09-23');
    expect(timeline?.events[0]?.hour).toBe(7);
    expect(timeline?.events[1]?.durationMs).toBe(95_000);
    expect(timeline?.days).toHaveLength(2);
    expect(timeline?.hours).toHaveLength(24);
    expect(timeline?.hours[7]?.totalTokens).toBe(32);
  });

  it('完全不是对象时才返回 null（其余形状都收敛成安全默认值）', () => {
    expect(normalizeActivityTimeline(null)).toBeNull();
    expect(normalizeActivityTimeline('nope')).toBeNull();

    const partial = normalizeActivityTimeline({ events: 'nope', days: null, hours: 3 });
    expect(partial?.events).toEqual([]);
    expect(partial?.days).toEqual([]);
    expect(partial?.hours).toEqual([]);
    expect(partial?.usageEventCount).toBe(0);
    expect(partial?.hasMore).toBe(false);
    expect(partial?.range.kind).toBe('all');
  });

  it('丢掉没有发生时间的事件，而不是渲染一行没有时间的记录', () => {
    const timeline = normalizeActivityTimeline({
      events: [{ totalTokens: 1 }, { occurredAt: '2026-09-22T23:30:00Z', totalTokens: 2 }],
    });

    expect(timeline?.events).toHaveLength(1);
    expect(timeline?.events[0]?.occurredAt).toBe('2026-09-22T23:30:00Z');
    expect(timeline?.events[0]?.totalTokens).toBe(2);
  });

  it('未知 kind 收敛为 usage，越界 hour 收敛为 0', () => {
    const timeline = normalizeActivityTimeline({
      events: [
        {
          kind: 'git-diff',
          occurredAt: '2026-09-22T23:30:00Z',
          hour: 99,
          totalTokens: 5,
        },
      ],
      hours: [{ hour: -1, totalTokens: 1 }],
    });

    expect(timeline?.events[0]?.kind).toBe('usage');
    expect(timeline?.events[0]?.hour).toBe(0);
    expect(timeline?.hours[0]?.hour).toBe(0);
  });

  it('getActivityTimeline 把范围、时区偏移与 limit 一起传给后端', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => ({}));
    stubTauri(invokeSpy);

    const result = await getActivityTimeline({
      range: '7d',
      timezoneOffsetMinutes: 480,
      limit: 60,
    });

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('activity_timeline');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({
      range: '7d',
      timezoneOffsetMinutes: 480,
      limit: 60,
    });
    expect(result.ok).toBe(true);
  });

  it('省略 limit 时不带上这个参数（后端用自己的默认值）', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => ({}));
    stubTauri(invokeSpy);

    await getActivityTimeline({ range: 'all' });

    const args = invokeSpy.mock.calls[0]?.[1] as Record<string, unknown>;
    expect('limit' in args).toBe(false);
    expect(typeof args.timezoneOffsetMinutes).toBe('number');
  });

  it('形状完全不对时返回明确错误，而不是伪造一份空时间线', async () => {
    stubTauri(async () => 42);

    await expect(getActivityTimeline({ range: 'all' })).resolves.toEqual({
      ok: false,
      error: 'invalid-activity-timeline-payload',
    });
  });
});

/**
 * 插件列表的 payload 是手工复刻的 Rust `PluginSummary` 形状（camelCase）。
 * 与 `CODEX_PAYLOAD` 同一策略：锁住前端解析行为，而不是去调真后端。
 */
const USAGE_CONNECT_PAYLOAD = {
  id: 'usage-connect',
  name: 'Usage Connect',
  version: '0.1.0',
  apiVersion: '1',
  path: 'D:/HarnessHub/plugins/examples/usage-connect',
  status: 'ready',
  capabilities: {
    detect: false,
    launch: false,
    resume: false,
    usage: true,
    trace: false,
    profiles: false,
    mcp: false,
    skills: false,
  },
  skills: [],
  registeredAt: '2026-10-17T12:00:00Z',
};

describe('normalizePlugin', () => {
  it('读出全部字段', () => {
    const plugin = normalizePlugin(USAGE_CONNECT_PAYLOAD);

    expect(plugin).toEqual({
      id: 'usage-connect',
      name: 'Usage Connect',
      version: '0.1.0',
      apiVersion: '1',
      path: 'D:/HarnessHub/plugins/examples/usage-connect',
      status: 'ready',
      capabilities: {
        detect: false,
        launch: false,
        resume: false,
        usage: true,
        trace: false,
        profiles: false,
        mcp: false,
        skills: false,
      },
      skills: [],
      registeredAt: '2026-10-17T12:00:00Z',
    });
  });

  it('缺 capabilities 时八个布尔全退化成 false（而不是 undefined）', () => {
    const plugin = normalizePlugin({ id: 'bare', name: 'Bare' });

    expect(plugin?.capabilities).toEqual({
      detect: false,
      launch: false,
      resume: false,
      usage: false,
      trace: false,
      profiles: false,
      mcp: false,
      skills: false,
    });
  });

  it('没有 id 的行直接丢弃，不生成无名插件', () => {
    expect(normalizePlugin({ name: 'No Id' })).toBeNull();
    expect(normalizePlugin({ id: '' })).toBeNull();
    expect(normalizePlugin(null)).toBeNull();
    expect(normalizePlugin(42)).toBeNull();
  });

  it('skills 里有坏项时逐项丢弃，保留其余', () => {
    const plugin = normalizePlugin({
      id: 'hello-skill',
      skills: [
        { skillId: 'demo', name: 'Demo', description: '演示', body: '---\n' },
        { name: '没有 skillId' },
        'not-an-object',
      ],
    });

    expect(plugin?.skills).toEqual([
      { skillId: 'demo', name: 'Demo', description: '演示', body: '---\n' },
    ]);
  });

  it('skills 字段缺失或不是数组时退化成空数组', () => {
    expect(normalizePlugin({ id: 'a' })?.skills).toEqual([]);
    expect(normalizePlugin({ id: 'a', skills: 'oops' })?.skills).toEqual([]);
  });
});

describe('normalizePluginRefreshReport', () => {
  it('读出 registered 与 errors', () => {
    expect(
      normalizePluginRefreshReport({
        registered: ['usage-connect', 'hello-skill'],
        errors: ['broken: manifest 缺少 [adapter] command'],
      }),
    ).toEqual({
      registered: ['usage-connect', 'hello-skill'],
      errors: ['broken: manifest 缺少 [adapter] command'],
    });
  });

  it('字段缺失时退化成空数组而不是 undefined', () => {
    expect(normalizePluginRefreshReport({})).toEqual({ registered: [], errors: [] });
    expect(normalizePluginRefreshReport(null)).toEqual({ registered: [], errors: [] });
  });

  it('数组里混入非字符串时只保留字符串项', () => {
    expect(normalizePluginRefreshReport({ registered: ['a', 1, null, 'b'] })).toEqual({
      registered: ['a', 'b'],
      errors: [],
    });
  });
});

describe('plugin IPC wrappers', () => {
  it('listPlugins 使用 plugin_list 命令名并逐个 normalize', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => [
      USAGE_CONNECT_PAYLOAD,
      { name: '坏行没有 id' },
    ]);
    stubTauri(invokeSpy);

    const result = await listPlugins();

    expect(invokeSpy).toHaveBeenCalledTimes(1);
    expect(invokeSpy.mock.calls[0]?.[0]).toBe('plugin_list');
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.data).toHaveLength(1);
      expect(result.data[0]?.id).toBe('usage-connect');
    }
  });

  it('listPlugins 在顶层不是数组时返回空列表（而不是报错）', async () => {
    stubTauri(async () => ({ nope: true }));

    await expect(listPlugins()).resolves.toEqual({ ok: true, data: [] });
  });

  it('listPlugins 把后端错误原样传出', async () => {
    stubTauri(async () => {
      throw new Error('数据库锁中毒');
    });

    await expect(listPlugins()).resolves.toEqual({
      ok: false,
      error: '数据库锁中毒',
    });
  });

  it('pluginRefresh 使用 plugin_refresh 命令名', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => ({ registered: ['a'], errors: [] }));
    stubTauri(invokeSpy);

    const result = await pluginRefresh();

    expect(invokeSpy).toHaveBeenCalledTimes(1);
    expect(invokeSpy.mock.calls[0]?.[0]).toBe('plugin_refresh');
    expect(result).toEqual({ ok: true, data: { registered: ['a'], errors: [] } });
  });
});

/**
 * IPC 契约 `session_git_snapshots` 的返回项（serde camelCase），
 * 形状手工复刻自冻结的契约：head 为 40 位 hex 或 null，四类计数非负整数。
 */
const SNAPSHOT_PAYLOAD = {
  hubSessionId: '6f1c0f7e-0000-4000-8000-000000000001',
  phase: 'start',
  capturedAt: '2026-10-24T10:00:00Z',
  worktreeRoot: 'D:/work/harness-hub',
  head: '1234567890abcdef1234567890abcdef12345678',
  stagedFiles: 2,
  unstagedFiles: 1,
  untrackedFiles: 3,
  conflictedFiles: 0,
};

describe('normalizeSessionGitSnapshot', () => {
  it('解析 Rust 侧的真实 camelCase 形状，字段逐一保留', () => {
    expect(normalizeSessionGitSnapshot(SNAPSHOT_PAYLOAD)).toEqual(SNAPSHOT_PAYLOAD);
  });

  it('缺 hubSessionId 的条目返回 null，由调用方丢弃', () => {
    expect(normalizeSessionGitSnapshot({ phase: 'start' })).toBeNull();
    expect(normalizeSessionGitSnapshot({ hubSessionId: '   ', phase: 'start' })).toBeNull();
    expect(normalizeSessionGitSnapshot(null)).toBeNull();
    expect(normalizeSessionGitSnapshot('start')).toBeNull();
  });

  it('非法 phase 丢弃该行，绝不猜成 start 或 end', () => {
    expect(normalizeSessionGitSnapshot({ ...SNAPSHOT_PAYLOAD, phase: 'middle' })).toBeNull();
    expect(normalizeSessionGitSnapshot({ ...SNAPSHOT_PAYLOAD, phase: undefined })).toBeNull();
  });

  it('计数缺省 0，head / 字符串字段缺省安全空值', () => {
    expect(normalizeSessionGitSnapshot({ hubSessionId: 'h1', phase: 'end' })).toEqual({
      hubSessionId: 'h1',
      phase: 'end',
      capturedAt: '',
      worktreeRoot: '',
      head: null,
      stagedFiles: 0,
      unstagedFiles: 0,
      untrackedFiles: 0,
      conflictedFiles: 0,
    });
  });

  it('head 只认字符串或 null，unborn 仓库不会被当成空字符串', () => {
    expect(normalizeSessionGitSnapshot({ ...SNAPSHOT_PAYLOAD, head: null })?.head).toBeNull();
    expect(normalizeSessionGitSnapshot({ ...SNAPSHOT_PAYLOAD, head: 42 })?.head).toBeNull();
    expect(normalizeSessionGitSnapshot({ ...SNAPSHOT_PAYLOAD, head: 'abc' })?.head).toBe('abc');
  });

  it('计数里的非数字收敛为 0，不把字符串渲染出来', () => {
    const snapshot = normalizeSessionGitSnapshot({ ...SNAPSHOT_PAYLOAD, stagedFiles: '2' });

    expect(snapshot?.stagedFiles).toBe(0);
  });
});

describe('normalizeSessionGitSummary', () => {
  it('解析 Rust 侧的真实 camelCase 形状，字段逐一保留', () => {
    expect(normalizeSessionGitSummary({ commits: 2, insertions: 41, deletions: 7 })).toEqual({
      commits: 2,
      insertions: 41,
      deletions: 7,
    });
  });

  it('null（没有窗口）与 0 commits（窗口内无提交）是两个不同的答案', () => {
    expect(normalizeSessionGitSummary(null)).toBeNull();
    expect(normalizeSessionGitSummary('x')).toBeNull();
    // 后端 Some(0,0,0)：同 HEAD 的真实摘要，必须保留为 0 而不是折叠成 null。
    expect(normalizeSessionGitSummary({ commits: 0, insertions: 0, deletions: 0 })).toEqual({
      commits: 0,
      insertions: 0,
      deletions: 0,
    });
  });

  it('计数缺省 / 非数字收敛为 0，不把字符串渲染出来', () => {
    expect(normalizeSessionGitSummary({})).toEqual({
      commits: 0,
      insertions: 0,
      deletions: 0,
    });
    expect(normalizeSessionGitSummary({ commits: '2' })?.commits).toBe(0);
  });
});

describe('getSessionGitSnapshots', () => {
  it('使用 session_git_snapshots 命令名并传 hubSessionId', async () => {
    const invokeSpy = vi.fn(async (..._args: unknown[]) => [SNAPSHOT_PAYLOAD]);
    stubTauri(invokeSpy);

    const result = await getSessionGitSnapshots('h1');

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('session_git_snapshots');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({ hubSessionId: 'h1' });
    expect(result).toEqual({ ok: true, data: [SNAPSHOT_PAYLOAD] });
  });

  it('丢弃无法解析的条目而不是让整页失败', async () => {
    stubTauri(async () => [SNAPSHOT_PAYLOAD, { phase: 'start' }, null]);

    const result = await getSessionGitSnapshots('h1');

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.data).toHaveLength(1);
      expect(result.data[0]?.hubSessionId).toBe(SNAPSHOT_PAYLOAD.hubSessionId);
    }
  });

  it('后端返回非数组时按空列表处理', async () => {
    stubTauri(async () => null);

    await expect(getSessionGitSnapshots('h1')).resolves.toEqual({ ok: true, data: [] });
  });

  it('命令抛错时把错误原样传出', async () => {
    stubTauri(async () => {
      throw new Error('snapshot read failed');
    });

    await expect(getSessionGitSnapshots('h1')).resolves.toEqual({
      ok: false,
      error: 'snapshot read failed',
    });
  });

  it('不在 Tauri 运行时返回 not-running-in-tauri', async () => {
    await expect(getSessionGitSnapshots('h1')).resolves.toEqual({
      ok: false,
      error: NOT_IN_TAURI,
    });
  });
});

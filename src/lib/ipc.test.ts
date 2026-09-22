import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  NOT_IN_TAURI,
  createSession,
  finishSession,
  getAppInfo,
  getDbHealth,
  invokeCommand,
  isTauriRuntime,
  listHarnesses,
  listSessions,
  normalizeHarnessSummary,
  normalizeSessionRecord,
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
  projectId: null,
  runtimeTargetId: 'local',
  parentSessionId: null,
  status: 'running',
  launchMode: 'terminal',
  cwd: 'D:/work',
  worktreePath: null,
  startedAt: '2026-09-22T10:00:00Z',
  endedAt: null,
  exitCode: null,
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
    expect(record?.runtimeTargetId).toBe('');
    expect(record?.status).toBe('unknown');
    expect(record?.endedAt).toBeNull();
    expect(record?.exitCode).toBeNull();
  });

  it('exitCode 非数字时视为 null，不把字符串渲染出来', () => {
    const record = normalizeSessionRecord({ ...SESSION_PAYLOAD, exitCode: '130' });

    expect(record?.exitCode).toBeNull();
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

    const result = await finishSession('h1', 0);

    expect(invokeSpy.mock.calls[0]?.[0]).toBe('finish_session');
    expect(invokeSpy.mock.calls[0]?.[1]).toEqual({ hubSessionId: 'h1', exitCode: 0 });
    expect(result).toEqual({ ok: true, data: true });
  });

  it('不在 Tauri 运行时一律返回 not-running-in-tauri', async () => {
    await expect(listSessions()).resolves.toEqual({ ok: false, error: NOT_IN_TAURI });
    await expect(createSession({ harnessId: 'codex' })).resolves.toEqual({
      ok: false,
      error: NOT_IN_TAURI,
    });
    await expect(finishSession('h1')).resolves.toEqual({ ok: false, error: NOT_IN_TAURI });
  });
});

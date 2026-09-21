import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  NOT_IN_TAURI,
  getAppInfo,
  getDbHealth,
  invokeCommand,
  isTauriRuntime,
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

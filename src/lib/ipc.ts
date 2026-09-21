import { invoke } from '@tauri-apps/api/core';

/**
 * 前端唯一的 IPC 入口。
 *
 * 约定（见 AGENTS.md）：`src/features/*` 里的组件禁止直接 `invoke()`。
 * 这里统一做三件事：
 *   1. 检测是否运行在 Tauri 运行时（浏览器 `pnpm dev` 下要优雅降级）。
 *   2. 把异常转成可渲染的 `IpcResult`，避免每个页面各写一遍 try/catch。
 *   3. 为每个命令提供带类型的包装函数。
 */

export const NOT_IN_TAURI = 'not-running-in-tauri';

export type IpcResult<T> = { ok: true; data: T } | { ok: false; error: string };

export type AppInfo = {
  name: string;
  version: string;
  tauriVersion: string;
};

export type DbHealth = {
  /** 已应用的最新 schema 版本；空库为 0。 */
  schemaVersion: number;
  appliedMigrations: string[];
  tableCount: number;
  foreignKeysEnabled: boolean;
  journalMode: string;
};

type TauriInternals = {
  invoke?: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
};

function readTauriInternals(): TauriInternals | null {
  if (typeof window === 'undefined') return null;
  const internals = (window as unknown as { __TAURI_INTERNALS__?: TauriInternals })
    .__TAURI_INTERNALS__;
  if (!internals || typeof internals.invoke !== 'function') return null;
  return internals;
}

/** 是否运行在 Tauri 窗口内。浏览器直开时为 false。 */
export function isTauriRuntime(): boolean {
  return readTauriInternals() !== null;
}

/** 调用一个 Rust 命令，永不抛异常。 */
export async function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<IpcResult<T>> {
  if (!isTauriRuntime()) {
    return { ok: false, error: NOT_IN_TAURI };
  }

  try {
    const data = (await invoke(command, args)) as T;
    return { ok: true, data };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : String(error) };
  }
}

export function getAppInfo(): Promise<IpcResult<AppInfo>> {
  return invokeCommand<AppInfo>('app_info');
}

export function getDbHealth(): Promise<IpcResult<DbHealth>> {
  return invokeCommand<DbHealth>('db_health');
}

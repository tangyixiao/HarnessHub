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

/* ------------------------------------------------------------------ *
 * Harness 检测
 * ------------------------------------------------------------------ */

/**
 * 能力矩阵。字段名与 Rust `HarnessCapabilities` 的 serde camelCase 一致：
 * Rust 的 `tool_calls` / `live_state` 序列化为 `toolCalls` / `liveState`。
 */
export type HarnessCapabilities = {
  launch: boolean;
  terminal: boolean;
  resume: boolean;
  usage: boolean;
  replay: boolean;
  toolCalls: boolean;
  subagents: boolean;
  liveState: boolean;
  worktree: boolean;
};

/** 与 Rust `HarnessRegistry::summaries()` 的序列化结果同形。 */
export type HarnessSummary = {
  id: string;
  displayName: string;
  installed: boolean;
  binaryPath: string | null;
  version: string | null;
  capabilities: HarnessCapabilities;
  dataPaths: string[];
};

const CAPABILITY_KEYS = [
  'launch',
  'terminal',
  'resume',
  'usage',
  'replay',
  'toolCalls',
  'subagents',
  'liveState',
  'worktree',
] as const satisfies ReadonlyArray<keyof HarnessCapabilities>;

function unsupportedCapabilities(): HarnessCapabilities {
  return {
    launch: false,
    terminal: false,
    resume: false,
    usage: false,
    replay: false,
    toolCalls: false,
    subagents: false,
    liveState: false,
    worktree: false,
  };
}

function normalizeCapabilities(raw: unknown): HarnessCapabilities {
  if (typeof raw !== 'object' || raw === null) {
    return unsupportedCapabilities();
  }

  const source = raw as Record<string, unknown>;
  const capabilities = unsupportedCapabilities();

  for (const key of CAPABILITY_KEYS) {
    // 只有明确的 true 才算支持。缺字段、字符串、数字一律视为不支持 ——
    // 「不确定」绝不能渲染成「支持」。
    capabilities[key] = source[key] === true;
  }

  return capabilities;
}

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `HarnessSummary`。
 *
 * 后端 payload 变更（字段改名、字段缺失、类型变化）不应让整页白屏，
 * 因此这里做运行时校验：拿不到 `id` 才丢弃该条目，其余一律补默认值。
 */
export function normalizeHarnessSummary(raw: unknown): HarnessSummary | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const id = typeof source.id === 'string' ? source.id.trim() : '';
  if (id.length === 0) {
    return null;
  }

  return {
    id,
    displayName: typeof source.displayName === 'string' && source.displayName.length > 0
      ? source.displayName
      : id,
    installed: source.installed === true,
    binaryPath: typeof source.binaryPath === 'string' ? source.binaryPath : null,
    version: typeof source.version === 'string' ? source.version : null,
    capabilities: normalizeCapabilities(source.capabilities),
    dataPaths: Array.isArray(source.dataPaths)
      ? source.dataPaths.filter((path): path is string => typeof path === 'string')
      : [],
  };
}

/**
 * 已注册 Harness 的**真实**检测结果（installed / version / binary path / 能力矩阵）。
 *
 * UI 不自己做任何二进制探测：本机扫描只发生在 Rust Control Plane。
 */
export async function listHarnesses(): Promise<IpcResult<HarnessSummary[]>> {
  const result = await invokeCommand<unknown>('list_harnesses');
  if (!result.ok) {
    return result;
  }

  const summaries = Array.isArray(result.data)
    ? result.data
        .map(normalizeHarnessSummary)
        .filter((summary): summary is HarnessSummary => summary !== null)
    : [];

  return { ok: true, data: summaries };
}

/* ------------------------------------------------------------------ *
 * Session
 * ------------------------------------------------------------------ */

export type SessionStatus = 'running' | 'exited' | 'failed' | 'unknown';
export type LaunchMode = 'terminal' | 'resume' | 'imported';

/**
 * 与 Rust `SessionRecord`（serde camelCase）同形。
 *
 * `hubSessionId` 是 Harness Hub 自己的标识；`sourceSessionId` 是外部 Harness 的原始
 * id（PTY 会话没有，导入的历史会话才有）。两者不可混用。
 */
export type SessionRecord = {
  hubSessionId: string;
  sourceSessionId: string | null;
  harnessId: string;
  projectId: string | null;
  runtimeTargetId: string;
  parentSessionId: string | null;
  status: SessionStatus;
  launchMode: LaunchMode;
  cwd: string | null;
  worktreePath: string | null;
  startedAt: string;
  endedAt: string | null;
  exitCode: number | null;
};

const SESSION_STATUSES = ['running', 'exited', 'failed', 'unknown'] as const;
const LAUNCH_MODES = ['terminal', 'resume', 'imported'] as const;

function asStringOrNull(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `SessionRecord`。
 *
 * 未知 `status` 一律降级为 `unknown` —— 绝不能把没见过的状态当成 `running`
 * （那会让 UI 声称一个可能已经死掉的会话仍在运行）。
 */
export function normalizeSessionRecord(raw: unknown): SessionRecord | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const hubSessionId = typeof source.hubSessionId === 'string' ? source.hubSessionId.trim() : '';
  if (hubSessionId.length === 0) {
    return null;
  }

  const status = SESSION_STATUSES.find((candidate) => candidate === source.status) ?? 'unknown';
  const launchMode =
    LAUNCH_MODES.find((candidate) => candidate === source.launchMode) ?? 'terminal';

  return {
    hubSessionId,
    sourceSessionId: asStringOrNull(source.sourceSessionId),
    harnessId: typeof source.harnessId === 'string' ? source.harnessId : '',
    projectId: asStringOrNull(source.projectId),
    runtimeTargetId: typeof source.runtimeTargetId === 'string' ? source.runtimeTargetId : '',
    parentSessionId: asStringOrNull(source.parentSessionId),
    status,
    launchMode,
    cwd: asStringOrNull(source.cwd),
    worktreePath: asStringOrNull(source.worktreePath),
    startedAt: typeof source.startedAt === 'string' ? source.startedAt : '',
    endedAt: asStringOrNull(source.endedAt),
    exitCode: typeof source.exitCode === 'number' ? source.exitCode : null,
  };
}

/** 最近会话，按开始时间倒序。 */
export async function listSessions(limit?: number): Promise<IpcResult<SessionRecord[]>> {
  const result = await invokeCommand<unknown>(
    'list_sessions',
    limit === undefined ? undefined : { limit },
  );
  if (!result.ok) {
    return result;
  }

  const sessions = Array.isArray(result.data)
    ? result.data
        .map(normalizeSessionRecord)
        .filter((session): session is SessionRecord => session !== null)
    : [];

  return { ok: true, data: sessions };
}

/**
 * 新建一条 Session 记录（`status = running`）。
 *
 * **不启动任何进程**：PTY 启动属于后续 Task。此命令只负责统一标识 + 运行目标绑定 + 落库。
 */
export function createSession(input: {
  harnessId: string;
  projectId?: string | null;
  cwd?: string | null;
}): Promise<IpcResult<SessionRecord>> {
  return invokeCommand<SessionRecord>('create_session', {
    harnessId: input.harnessId,
    projectId: input.projectId ?? null,
    cwd: input.cwd ?? null,
  });
}

/** 结束一条仍处于 running 的会话；返回是否真的更新了行（幂等）。 */
export function finishSession(
  hubSessionId: string,
  exitCode?: number | null,
): Promise<IpcResult<boolean>> {
  return invokeCommand<boolean>('finish_session', {
    hubSessionId,
    exitCode: exitCode ?? null,
  });
}

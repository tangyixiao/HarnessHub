import { Channel, invoke } from '@tauri-apps/api/core';

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
 * Terminal（Task 4）
 *
 * Session ID 是唯一的公共句柄：前端不持有 Rust PTY handle，PID 只是诊断字段。
 * ------------------------------------------------------------------ */

/** 与 Rust `PtyEvent` 同形（serde tag = "kind", camelCase）。 */
export type PtyEvent =
  | { kind: 'started'; sessionId: string; pid: number | null }
  | { kind: 'output'; sessionId: string; seq: number; data: number[] }
  | { kind: 'exited'; sessionId: string; exitCode: number | null; reason: TerminationReason }
  | { kind: 'error'; sessionId: string; message: string };

/**
 * 启动终端会话，返回 hubSessionId。
 *
 * 输出走 Tauri Channel（有序、原始 bytes）。**调用前必须先就绪**：
 * xterm 已 open、Channel 回调已挂、onData/onBinary 已挂、resize 已挂 ——
 * 否则 Codex 首屏的 DSR 会早于 responder 就绪而卡住（见 ADR-0009）。
 */
export async function startTerminal(input: {
  installationId: string;
  cwd?: string | null;
  cols: number;
  rows: number;
  onEvent: (event: PtyEvent) => void;
}): Promise<IpcResult<SessionRecord>> {
  if (!isTauriRuntime()) {
    return { ok: false, error: NOT_IN_TAURI };
  }

  try {
    const output = new Channel<PtyEvent>();
    // 先挂回调再 invoke：Channel 必须在 start_terminal 之前 ready。
    output.onmessage = input.onEvent;

    const raw = await invoke('start_terminal', {
      installationId: input.installationId,
      cwd: input.cwd ?? null,
      cols: input.cols,
      rows: input.rows,
      output,
    });

    const record = normalizeSessionRecord(raw);
    return record ? { ok: true, data: record } : { ok: false, error: 'invalid-session-payload' };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : String(error) };
  }
}

/**
 * 把**原始字节**写进 PTY。
 *
 * 不在 Rust 侧猜编码：onData 侧用 TextEncoder 编码，onBinary 侧按 code unit
 * 低 8 位还原。DSR/CPR 的回应也走这条路径。
 */
export function writeTerminal(sessionId: string, bytes: Uint8Array): Promise<IpcResult<null>> {
  return invokeCommand<null>('write_terminal', { sessionId, data: Array.from(bytes) });
}

export function resizeTerminal(
  sessionId: string,
  cols: number,
  rows: number,
): Promise<IpcResult<null>> {
  return invokeCommand<null>('resize_terminal', { sessionId, cols, rows });
}

/** 用户主动结束。终态由 Rust 侧的 reaper 依真实退出写入。 */
export function killTerminal(sessionId: string): Promise<IpcResult<null>> {
  return invokeCommand<null>('kill_terminal', { sessionId });
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
  /** 该 Harness 在当前运行目标上的安装 id（形如 codex@local），供终端 IPC 使用。 */
  installationId: string | null;
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
    displayName:
      typeof source.displayName === 'string' && source.displayName.length > 0
        ? source.displayName
        : id,
    installationId: asStringOrNull(source.installationId),
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

export type SessionStatus = 'created' | 'running' | 'exited' | 'failed' | 'unknown';
export type LaunchMode = 'terminal' | 'resume' | 'imported';

/**
 * **为什么**会话结束了。与 `exitCode` 正交：非零退出码不等于同一种失败
 * （用户强杀 / CLI 参数错误 / Agent 工作失败 / Harness Hub 自身故障）。
 */
export type TerminationReason =
  'natural_exit' | 'user_killed' | 'launch_failed' | 'runtime_error' | 'host_shutdown' | 'lost';

/**
 * 与 Rust `SessionRecord`（serde camelCase）同形。
 *
 * `hubSessionId` 是 Harness Hub 自己的标识；`sourceSessionId` 是外部 Harness 的原始
 * id（PTY 会话没有，导入的历史会话才有）。两者不可混用。
 *
 * `installationId` 指向 `harness_installations.id`（形如 `codex@local`），
 * 因此同一个 Harness 在不同 runtime target 上的安装可以被区分。
 */
export type SessionRecord = {
  hubSessionId: string;
  sourceSessionId: string | null;
  harnessId: string;
  installationId: string | null;
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
  /** 终止原因；created / running 阶段以及迁移前的存量终态行为 null（原因未记录）。 */
  terminationReason: TerminationReason | null;
  /** 进程 PID。**只用于诊断**，不是 IPC 句柄（PID 会复用）。 */
  pid: number | null;
};

const SESSION_STATUSES = ['created', 'running', 'exited', 'failed', 'unknown'] as const;
const LAUNCH_MODES = ['terminal', 'resume', 'imported'] as const;
const TERMINATION_REASONS = [
  'natural_exit',
  'user_killed',
  'launch_failed',
  'runtime_error',
  'host_shutdown',
  'lost',
] as const;

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
    installationId: asStringOrNull(source.installationId),
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
    // 未知的终止原因视为「未记录」，绝不当成正常退出。
    terminationReason:
      TERMINATION_REASONS.find((candidate) => candidate === source.terminationReason) ?? null,
    pid: typeof source.pid === 'number' ? source.pid : null,
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

/**
 * 结束一条仍处于 running 的会话；返回是否真的更新了行（幂等）。
 *
 * `reason` 必填：**非零退出码不等于同一种失败**，调用方必须说明是用户主动结束、
 * 运行故障还是启动失败。终态由 Rust 侧用 reason + exitCode 共同推导。
 */
export function finishSession(
  hubSessionId: string,
  reason: TerminationReason,
  exitCode?: number | null,
): Promise<IpcResult<boolean>> {
  return invokeCommand<boolean>('finish_session', {
    hubSessionId,
    exitCode: exitCode ?? null,
    reason,
  });
}

/* ------------------------------------------------------------------ *
 * Project registry（§20 第 14 步）
 * ------------------------------------------------------------------ */

/** 一条已注册项目（与 Rust `ProjectRecord` 同形）。 */
export type ProjectRecord = {
  id: string;
  name: string;
  repoRoot: string;
  createdAt: string;
  updatedAt: string;
  lastOpenedAt: string | null;
};

/**
 * 安全解析 `ProjectRecord`：拿不到主键 `id` 一律返回 null ——
 * 宁可丢掉一条残缺记录，也不渲染一个无法删除的卡片。
 */
export function normalizeProjectRecord(raw: unknown): ProjectRecord | null {
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
    name: typeof source.name === 'string' ? source.name : '',
    repoRoot: typeof source.repoRoot === 'string' ? source.repoRoot : '',
    createdAt: typeof source.createdAt === 'string' ? source.createdAt : '',
    updatedAt: typeof source.updatedAt === 'string' ? source.updatedAt : '',
    lastOpenedAt: asStringOrNull(source.lastOpenedAt),
  };
}

/**
 * 登记一个本地目录为项目：Rust 侧自动识别 repo root，按 root 幂等 upsert。
 * 返回登记后的记录（重复登记返回既有行，并刷新 lastOpenedAt）。
 */
export function registerProject(path: string, name?: string): Promise<IpcResult<ProjectRecord>> {
  return invokeCommand<ProjectRecord>('register_project', {
    path,
    name: name ?? null,
  });
}

/** 最近项目，按 lastOpenedAt / createdAt 倒序。 */
export async function listProjects(limit?: number): Promise<IpcResult<ProjectRecord[]>> {
  const result = await invokeCommand<unknown>(
    'list_projects',
    limit === undefined ? undefined : { limit },
  );
  if (!result.ok) {
    return result;
  }

  const projects = Array.isArray(result.data)
    ? result.data
        .map(normalizeProjectRecord)
        .filter((project): project is ProjectRecord => project !== null)
    : [];

  return { ok: true, data: projects };
}

/** 删除项目（幂等）。Session 保留但失去归属；项目级设置级联删除。 */
export function removeProject(projectId: string): Promise<IpcResult<boolean>> {
  return invokeCommand<boolean>('remove_project', { projectId });
}

/** 某个项目下的最近 Session 历史。 */
export async function getProjectSessions(
  projectId: string,
  limit?: number,
): Promise<IpcResult<SessionRecord[]>> {
  const raw = await invokeCommand<unknown>('get_project_sessions', {
    projectId,
    ...(limit === undefined ? {} : { limit }),
  });
  if (!raw.ok) {
    return raw;
  }

  const sessions = Array.isArray(raw.data)
    ? raw.data
        .map(normalizeSessionRecord)
        .filter((session): session is SessionRecord => session !== null)
    : [];

  return { ok: true, data: sessions };
}

/* ------------------------------------------------------------------ *
 * Harness 清单同步
 * ------------------------------------------------------------------ */

/** 一次同步的结果（与 Rust `ReconcileReport` 同形）。 */
export type ReconcileReport = {
  harnesses: number;
  installations: number;
};

/**
 * 显式同步 Harness 清单：`detect` → `reconcile` → SQLite。
 *
 * 语义上是**写操作**（会更新 `harnesses` / `harness_installations`），
 * 因此只在用户点 Refresh 或应用启动时调用；`listHarnesses()` 保持纯读。
 */
export async function refreshHarnesses(): Promise<IpcResult<ReconcileReport>> {
  const result = await invokeCommand<unknown>('refresh_harnesses');
  if (!result.ok) {
    return result;
  }

  const source = (result.data ?? {}) as Record<string, unknown>;
  return {
    ok: true,
    data: {
      harnesses: typeof source.harnesses === 'number' ? source.harnesses : 0,
      installations: typeof source.installations === 'number' ? source.installations : 0,
    },
  };
}

/* ------------------------------------------------------------------ *
 * Usage（ccusage）
 * ------------------------------------------------------------------ */

/** 与 Rust `SourceStatus` 同形。 */
export type UsageSourceStatus = 'available' | 'unavailable';

/**
 * 数据实际是怎么拿到的。**落库值**，不是展示文案：
 * `managed-npx` 表示用的是托管 runner（固定 `ccusage@20.0.24`，永远不是 `latest`）。
 */
export type UsageRunnerKind = 'path' | 'configured' | 'managed-npx';

/** 与 Rust `UsageCapabilities` 同形：能力 = 代码实现了，未实现必须是 false。 */
export type UsageCapabilities = {
  detect: boolean;
  import: boolean;
  watch: boolean;
  reconcile: boolean;
};

/**
 * 与 Rust `UsageSource` 同形。
 *
 * `version` 是**上一次导入实际看到的版本**（只读路径不起进程去探测），
 * 因此它可能为 `null`（从未成功导入过）；`reason` 在 `unavailable` 时给出人话原因。
 */
export type UsageSource = {
  id: string;
  displayName: string;
  version: string | null;
  status: UsageSourceStatus;
  capabilities: UsageCapabilities;
  runner: UsageRunnerKind | null;
  reason: string | null;
};

/** 与 Rust `ImportStatus` 同形。 */
export type UsageImportStatus = 'running' | 'succeeded' | 'failed';

/** 与 Rust `UsageImport` 同形。 */
export type UsageImport = {
  id: string;
  source: string;
  sourceVersion: string | null;
  runner: UsageRunnerKind | null;
  reportKind: string | null;
  status: UsageImportStatus;
  startedAt: string;
  completedAt: string | null;
  recordsSeen: number;
  recordsInserted: number;
  recordsUpdated: number;
  recordsSkipped: number;
  recordsTimestampless: number;
  error: string | null;
};

/**
 * 与 Rust `Reconciliation` 同形（同快照对账）。
 *
 * `tokensIdentityHolds` 为 `null` 表示**无法对账**（来源没给 totals），
 * 不是「对上了」—— UI 必须区分这三态：true / false / null。
 * `costMicrounitsDelta` 是每个事件独立舍入带来的残差，必须如实显示而不是抹掉。
 */
export type UsageReconciliation = {
  eventsTotalTokens: number;
  reportTotalTokens: number | null;
  rowTotalResidual: number;
  costMicrounitsDelta: number | null;
  unpricedEvents: number;
  timestampless: number;
  tokensIdentityHolds: boolean | null;
};

/** 与 Rust `UsageRefreshReport` 同形。 */
export type UsageRefreshReport = {
  import: UsageImport;
  reconciliation: UsageReconciliation;
};

const USAGE_SOURCE_STATUSES: readonly UsageSourceStatus[] = ['available', 'unavailable'];
const USAGE_RUNNERS: readonly UsageRunnerKind[] = ['path', 'configured', 'managed-npx'];
const USAGE_IMPORT_STATUSES: readonly UsageImportStatus[] = ['running', 'succeeded', 'failed'];

function asNumber(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

function asBooleanOrNull(value: unknown): boolean | null {
  return typeof value === 'boolean' ? value : null;
}

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `UsageCapabilities`。
 *
 * 未知/缺失一律 `false`：能力矩阵宁可低估，也不能声称一个没实现的能力（ADR-0005）。
 */
export function normalizeUsageCapabilities(raw: unknown): UsageCapabilities {
  const source = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  return {
    detect: source.detect === true,
    import: source.import === true,
    watch: source.watch === true,
    reconcile: source.reconcile === true,
  };
}

export function normalizeUsageSource(raw: unknown): UsageSource | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const id = typeof source.id === 'string' ? source.id.trim() : '';
  if (id.length === 0) {
    return null;
  }

  const status = USAGE_SOURCE_STATUSES.includes(source.status as UsageSourceStatus)
    ? (source.status as UsageSourceStatus)
    : 'unavailable';
  const runner = USAGE_RUNNERS.includes(source.runner as UsageRunnerKind)
    ? (source.runner as UsageRunnerKind)
    : null;

  return {
    id,
    displayName:
      typeof source.displayName === 'string' && source.displayName.length > 0
        ? source.displayName
        : id,
    version: asStringOrNull(source.version),
    status,
    capabilities: normalizeUsageCapabilities(source.capabilities),
    runner,
    reason: asStringOrNull(source.reason),
  };
}

export function normalizeUsageImport(raw: unknown): UsageImport | null {
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
    source: typeof source.source === 'string' ? source.source : '',
    sourceVersion: asStringOrNull(source.sourceVersion),
    runner: USAGE_RUNNERS.includes(source.runner as UsageRunnerKind)
      ? (source.runner as UsageRunnerKind)
      : null,
    reportKind: asStringOrNull(source.reportKind),
    status: USAGE_IMPORT_STATUSES.includes(source.status as UsageImportStatus)
      ? (source.status as UsageImportStatus)
      : 'failed',
    startedAt: typeof source.startedAt === 'string' ? source.startedAt : '',
    completedAt: asStringOrNull(source.completedAt),
    recordsSeen: asNumber(source.recordsSeen),
    recordsInserted: asNumber(source.recordsInserted),
    recordsUpdated: asNumber(source.recordsUpdated),
    recordsSkipped: asNumber(source.recordsSkipped),
    recordsTimestampless: asNumber(source.recordsTimestampless),
    error: asStringOrNull(source.error),
  };
}

export function normalizeUsageReconciliation(raw: unknown): UsageReconciliation {
  const source = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  return {
    eventsTotalTokens: asNumber(source.eventsTotalTokens),
    reportTotalTokens:
      typeof source.reportTotalTokens === 'number' ? source.reportTotalTokens : null,
    rowTotalResidual: asNumber(source.rowTotalResidual),
    costMicrounitsDelta:
      typeof source.costMicrounitsDelta === 'number' ? source.costMicrounitsDelta : null,
    unpricedEvents: asNumber(source.unpricedEvents),
    timestampless: asNumber(source.timestampless),
    tokensIdentityHolds: asBooleanOrNull(source.tokensIdentityHolds),
  };
}

/**
 * Usage 数据源的**只读**视图。
 *
 * 与 `listHarnesses()` 同一条纪律：纯读、不起外部进程。
 * 需要真正刷新时请显式调用 `refreshUsage()`。
 */
export async function getUsageSources(): Promise<IpcResult<UsageSource[]>> {
  const result = await invokeCommand<unknown>('get_usage_sources');
  if (!result.ok) {
    return result;
  }

  const sources = Array.isArray(result.data)
    ? result.data
        .map(normalizeUsageSource)
        .filter((source): source is UsageSource => source !== null)
    : [];

  return { ok: true, data: sources };
}

/**
 * 显式刷新 Usage：`ccusage` → 归一化 → 幂等落库。
 *
 * 语义上是**写操作**（会起外部进程并写 `usage_imports` / `usage_events`），
 * 因此只允许由用户动作触发，绝不在渲染时调用。
 */
export async function refreshUsage(): Promise<IpcResult<UsageRefreshReport>> {
  const result = await invokeCommand<unknown>('refresh_usage');
  if (!result.ok) {
    return result;
  }

  const source = (result.data ?? {}) as Record<string, unknown>;
  const importRecord = normalizeUsageImport(source.import);
  if (importRecord === null) {
    return { ok: false, error: 'invalid-usage-import-payload' };
  }

  return {
    ok: true,
    data: {
      import: importRecord,
      reconciliation: normalizeUsageReconciliation(source.reconciliation),
    },
  };
}

/* ------------------------------------------------------------------ *
 * Usage 汇总（Dashboard，ADR-0012）
 * ------------------------------------------------------------------ */

export type UsageRangeKind = 'today' | '7d' | '30d' | 'all';

/** 与 Rust `UsageRangeWindow` 同形。[startUtc, endUtc) 左闭右开。 */
export type UsageRangeWindow = {
  kind: UsageRangeKind;
  startUtc: string | null;
  endUtc: string | null;
  timezoneOffsetMinutes: number;
  nowUtc: string;
};

export type UsageTotals = {
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number;
  cacheCreationTokens: number;
  /** 只在单模型会话行上有值，因此这是**下界**（ADR-0011 决策十二）。 */
  reasoningTokens: number;
  totalTokens: number;
};

export type UsageBucket = {
  key: string;
  totalTokens: number;
  costMicrounits: number | null;
  costIsLowerBound: boolean;
  eventCount: number;
};

export type UsageDayBucket = {
  /** **本地**日（由后端按 timezoneOffsetMinutes 换算）。 */
  day: string;
  totalTokens: number;
  costMicrounits: number | null;
  costIsLowerBound: boolean;
  eventCount: number;
};

/**
 * 与 Rust `UsageSummary` 同形。**判定逻辑全在后端**：
 *
 * - `costIsLowerBound` 为 true 时前端只能显示 `≥ 金额`，并提示「实际成本可能更高」；
 * - `costMicrounits` 为 `null` 表示「一条有价格的记录都没有」（不是 0 元）；
 * - `usageSessions`（外部历史里的会话）与 `managedSessions`（Harness Hub 管理的会话）
 *   是两个不同的数字，UI 不得合并。
 */
export type UsageSummary = {
  range: UsageRangeWindow;
  totals: UsageTotals;
  costMicrounits: number | null;
  currency: string | null;
  costIsLowerBound: boolean;
  missingPricingRecords: number;
  timestamplessRecords: number;
  excludedTimestampless: number;
  eventCount: number;
  usageSessions: number;
  managedSessions: number;
  byHarness: UsageBucket[];
  byModel: UsageBucket[];
  byProject: UsageBucket[];
  timeline: UsageDayBucket[];
};

const USAGE_RANGES: readonly UsageRangeKind[] = ['today', '7d', '30d', 'all'];

function normalizeBucketArray(raw: unknown): UsageBucket[] {
  if (!Array.isArray(raw)) {
    return [];
  }

  return raw.flatMap((entry): UsageBucket[] => {
    if (typeof entry !== 'object' || entry === null) {
      return [];
    }
    const source = entry as Record<string, unknown>;
    const key = typeof source.key === 'string' ? source.key : '';
    if (key.length === 0) {
      return [];
    }
    return [
      {
        key,
        totalTokens: asNumber(source.totalTokens),
        costMicrounits: typeof source.costMicrounits === 'number' ? source.costMicrounits : null,
        costIsLowerBound: source.costIsLowerBound === true,
        eventCount: asNumber(source.eventCount),
      },
    ];
  });
}

function normalizeDayBucketArray(raw: unknown): UsageDayBucket[] {
  if (!Array.isArray(raw)) {
    return [];
  }

  return raw.flatMap((entry): UsageDayBucket[] => {
    if (typeof entry !== 'object' || entry === null) {
      return [];
    }
    const source = entry as Record<string, unknown>;
    const day = typeof source.day === 'string' ? source.day : '';
    if (day.length === 0) {
      return [];
    }
    return [
      {
        day,
        totalTokens: asNumber(source.totalTokens),
        costMicrounits: typeof source.costMicrounits === 'number' ? source.costMicrounits : null,
        costIsLowerBound: source.costIsLowerBound === true,
        eventCount: asNumber(source.eventCount),
      },
    ];
  });
}

function normalizeRangeWindow(raw: unknown, fallbackOffset: number): UsageRangeWindow {
  const source = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  return {
    kind: USAGE_RANGES.includes(source.kind as UsageRangeKind)
      ? (source.kind as UsageRangeKind)
      : 'all',
    startUtc: asStringOrNull(source.startUtc),
    endUtc: asStringOrNull(source.endUtc),
    timezoneOffsetMinutes:
      typeof source.timezoneOffsetMinutes === 'number'
        ? source.timezoneOffsetMinutes
        : fallbackOffset,
    nowUtc: typeof source.nowUtc === 'string' ? source.nowUtc : '',
  };
}

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `UsageSummary`。
 *
 * 只有「完全不是对象」才返回 `null`：Dashboard 宁可显示 0 与空列表，
 * 也不要因为后端多/少一个字段就白屏。
 */
export function normalizeUsageSummary(
  raw: unknown,
  fallbackOffsetMinutes = 0,
): UsageSummary | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const totals = (
    typeof source.totals === 'object' && source.totals !== null ? source.totals : {}
  ) as Record<string, unknown>;

  return {
    range: normalizeRangeWindow(source.range, fallbackOffsetMinutes),
    totals: {
      inputTokens: asNumber(totals.inputTokens),
      outputTokens: asNumber(totals.outputTokens),
      cachedInputTokens: asNumber(totals.cachedInputTokens),
      cacheCreationTokens: asNumber(totals.cacheCreationTokens),
      reasoningTokens: asNumber(totals.reasoningTokens),
      totalTokens: asNumber(totals.totalTokens),
    },
    costMicrounits: typeof source.costMicrounits === 'number' ? source.costMicrounits : null,
    currency: asStringOrNull(source.currency),
    costIsLowerBound: source.costIsLowerBound === true,
    missingPricingRecords: asNumber(source.missingPricingRecords),
    timestamplessRecords: asNumber(source.timestamplessRecords),
    excludedTimestampless: asNumber(source.excludedTimestampless),
    eventCount: asNumber(source.eventCount),
    usageSessions: asNumber(source.usageSessions),
    managedSessions: asNumber(source.managedSessions),
    byHarness: normalizeBucketArray(source.byHarness),
    byModel: normalizeBucketArray(source.byModel),
    byProject: normalizeBucketArray(source.byProject),
    timeline: normalizeDayBucketArray(source.timeline),
  };
}

/** 本机时区偏移：加到 UTC 上得到本地时间的分钟数（UTC+8 → 480）。 */
export function localTimezoneOffsetMinutes(now: Date = new Date()): number {
  return -now.getTimezoneOffset();
}

/**
 * Dashboard 的**只读**聚合查询。
 *
 * 这是 Dashboard 唯一的数据来源：不起任何外部进程、不写数据库。
 * 需要更新数据时先调用 `refreshUsage()`（用户显式动作），成功后再调用本函数。
 */
export async function getUsageSummary(options: {
  range: UsageRangeKind;
  timezoneOffsetMinutes?: number;
}): Promise<IpcResult<UsageSummary>> {
  const timezoneOffsetMinutes = options.timezoneOffsetMinutes ?? localTimezoneOffsetMinutes();
  const result = await invokeCommand<unknown>('usage_summary', {
    range: options.range,
    timezoneOffsetMinutes,
  });
  if (!result.ok) {
    return result;
  }

  const summary = normalizeUsageSummary(result.data, timezoneOffsetMinutes);
  if (summary === null) {
    return { ok: false, error: 'invalid-usage-summary-payload' };
  }

  return { ok: true, data: summary };
}

/* ------------------------------------------------------------------ *
 * Activity Timeline（只读投影，ADR-0012）
 * ------------------------------------------------------------------ */

export type ActivityEventKind = 'usage' | 'session';

const ACTIVITY_EVENT_KINDS: ActivityEventKind[] = ['usage', 'session'];

/** 活动流里的一条事件。`day` / `hour` 已由后端按 `timezoneOffsetMinutes` 本地化。 */
export interface ActivityEvent {
  kind: ActivityEventKind;
  occurredAt: string;
  day: string;
  hour: number;
  harnessId: string;
  projectId: string | null;
  sourceSessionId: string | null;
  model: string | null;
  totalTokens: number;
  costMicrounits: number | null;
  costIsLowerBound: boolean;
  status: string | null;
  launchMode: string | null;
  durationMs: number | null;
  cwd: string | null;
  hubSessionId: string | null;
}

export interface ActivityDayBucket {
  day: string;
  usageEventCount: number;
  managedSessionCount: number;
  totalTokens: number;
  costMicrounits: number | null;
  costIsLowerBound: boolean;
}

export interface ActivityHourBucket {
  hour: number;
  usageEventCount: number;
  managedSessionCount: number;
  totalTokens: number;
}

export interface ActivityTimeline {
  range: UsageRangeWindow;
  events: ActivityEvent[];
  days: ActivityDayBucket[];
  hours: ActivityHourBucket[];
  usageEventCount: number;
  managedSessionCount: number;
  activeDays: number;
  totalTokens: number;
  maxDayTokens: number;
  maxHourTokens: number;
  hasMore: boolean;
}

function normalizeActivityEventArray(raw: unknown): ActivityEvent[] {
  if (!Array.isArray(raw)) {
    return [];
  }

  return raw.flatMap((entry): ActivityEvent[] => {
    if (typeof entry !== 'object' || entry === null) {
      return [];
    }
    const source = entry as Record<string, unknown>;
    const occurredAt = typeof source.occurredAt === 'string' ? source.occurredAt : '';
    if (occurredAt.length === 0) {
      return [];
    }
    const hour = asNumber(source.hour);
    return [
      {
        kind: ACTIVITY_EVENT_KINDS.includes(source.kind as ActivityEventKind)
          ? (source.kind as ActivityEventKind)
          : 'usage',
        occurredAt,
        day: typeof source.day === 'string' ? source.day : '',
        hour: hour >= 0 && hour <= 23 ? hour : 0,
        harnessId: typeof source.harnessId === 'string' ? source.harnessId : '',
        projectId: asStringOrNull(source.projectId),
        sourceSessionId: asStringOrNull(source.sourceSessionId),
        model: asStringOrNull(source.model),
        totalTokens: asNumber(source.totalTokens),
        costMicrounits: typeof source.costMicrounits === 'number' ? source.costMicrounits : null,
        costIsLowerBound: source.costIsLowerBound === true,
        status: asStringOrNull(source.status),
        launchMode: asStringOrNull(source.launchMode),
        durationMs: typeof source.durationMs === 'number' ? source.durationMs : null,
        cwd: asStringOrNull(source.cwd),
        hubSessionId: asStringOrNull(source.hubSessionId),
      },
    ];
  });
}

function normalizeActivityDayArray(raw: unknown): ActivityDayBucket[] {
  if (!Array.isArray(raw)) {
    return [];
  }

  return raw.flatMap((entry): ActivityDayBucket[] => {
    if (typeof entry !== 'object' || entry === null) {
      return [];
    }
    const source = entry as Record<string, unknown>;
    const day = typeof source.day === 'string' ? source.day : '';
    if (day.length === 0) {
      return [];
    }
    return [
      {
        day,
        usageEventCount: asNumber(source.usageEventCount),
        managedSessionCount: asNumber(source.managedSessionCount),
        totalTokens: asNumber(source.totalTokens),
        costMicrounits: typeof source.costMicrounits === 'number' ? source.costMicrounits : null,
        costIsLowerBound: source.costIsLowerBound === true,
      },
    ];
  });
}

function normalizeActivityHourArray(raw: unknown): ActivityHourBucket[] {
  if (!Array.isArray(raw)) {
    return [];
  }

  return raw.flatMap((entry): ActivityHourBucket[] => {
    if (typeof entry !== 'object' || entry === null) {
      return [];
    }
    const source = entry as Record<string, unknown>;
    const hour = asNumber(source.hour);
    return [
      {
        hour: hour >= 0 && hour <= 23 ? hour : 0,
        usageEventCount: asNumber(source.usageEventCount),
        managedSessionCount: asNumber(source.managedSessionCount),
        totalTokens: asNumber(source.totalTokens),
      },
    ];
  });
}

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `ActivityTimeline`。
 *
 * 与 `normalizeUsageSummary` 同一策略：只有「完全不是对象」才返回 `null`；
 * 缺字段一律退化成 0 / null，页面宁可显示空日历也不要白屏。
 */
export function normalizeActivityTimeline(
  raw: unknown,
  fallbackOffsetMinutes = 0,
): ActivityTimeline | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  return {
    range: normalizeRangeWindow(source.range, fallbackOffsetMinutes),
    events: normalizeActivityEventArray(source.events),
    days: normalizeActivityDayArray(source.days),
    hours: normalizeActivityHourArray(source.hours),
    usageEventCount: asNumber(source.usageEventCount),
    managedSessionCount: asNumber(source.managedSessionCount),
    activeDays: asNumber(source.activeDays),
    totalTokens: asNumber(source.totalTokens),
    maxDayTokens: asNumber(source.maxDayTokens),
    maxHourTokens: asNumber(source.maxHourTokens),
    hasMore: source.hasMore === true,
  };
}

/**
 * Activity 页的**只读**活动流查询（ADR-0012）。
 *
 * 不起任何外部进程、不写数据库；`limit` 只截断事件流，后端用 `hasMore`
 * 告知前端「还有更多」，聚合数字不受截断影响。
 */
export async function getActivityTimeline(options: {
  range: UsageRangeKind;
  timezoneOffsetMinutes?: number;
  limit?: number;
}): Promise<IpcResult<ActivityTimeline>> {
  const timezoneOffsetMinutes = options.timezoneOffsetMinutes ?? localTimezoneOffsetMinutes();
  const result = await invokeCommand<unknown>('activity_timeline', {
    range: options.range,
    timezoneOffsetMinutes,
    ...(options.limit === undefined ? {} : { limit: options.limit }),
  });
  if (!result.ok) {
    return result;
  }

  const timeline = normalizeActivityTimeline(result.data, timezoneOffsetMinutes);
  if (timeline === null) {
    return { ok: false, error: 'invalid-activity-timeline-payload' };
  }

  return { ok: true, data: timeline };
}

/* ------------------------------------------------------------------ *
 * 插件宿主（Phase 9.3）
 * ------------------------------------------------------------------ */

/**
 * 与 Rust `PluginCapabilities` 同形，八个布尔与 manifest `[capabilities]` 一一对应。
 *
 * Rust 侧保证八个键**永远全在**（契约测试盯住），所以这里不做缺键降级——
 * 缺了就说明后端违约，而不是前端该猜。
 */
export type PluginCapabilities = {
  detect: boolean;
  launch: boolean;
  resume: boolean;
  usage: boolean;
  trace: boolean;
  profiles: boolean;
  mcp: boolean;
  skills: boolean;
};

/** 一个已缓存的 skill。`body` 是 `SKILL.md` 原文，宿主不解析、前端也不解析。 */
export type PluginSkill = {
  skillId: string;
  name: string;
  description: string;
  body: string;
};

/**
 * 一个已登记的插件。
 *
 * `status` 保持成自由字符串而不是联合类型：后端新增状态（比如将来的 `disabled`）
 * 不该让前端把整行丢掉；UI 只对认识的 `ready` / `degraded` 分支着色。
 */
export type Plugin = {
  id: string;
  name: string;
  version: string;
  apiVersion: string;
  path: string;
  status: string;
  capabilities: PluginCapabilities;
  skills: PluginSkill[];
  registeredAt: string;
};

/** 一次 `plugin_refresh` 的结果。单个插件失败落在 `errors` 里，不是整体失败（R14）。 */
export type PluginRefreshReport = {
  registered: string[];
  errors: string[];
};

const EMPTY_CAPABILITIES: PluginCapabilities = {
  detect: false,
  launch: false,
  resume: false,
  usage: false,
  trace: false,
  profiles: false,
  mcp: false,
  skills: false,
};

function normalizePluginCapabilities(raw: unknown): PluginCapabilities {
  if (typeof raw !== 'object' || raw === null) {
    return { ...EMPTY_CAPABILITIES };
  }

  const source = raw as Record<string, unknown>;
  const flag = (key: keyof PluginCapabilities): boolean => source[key] === true;

  return {
    detect: flag('detect'),
    launch: flag('launch'),
    resume: flag('resume'),
    usage: flag('usage'),
    trace: flag('trace'),
    profiles: flag('profiles'),
    mcp: flag('mcp'),
    skills: flag('skills'),
  };
}

function normalizePluginSkill(raw: unknown): PluginSkill | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const skillId = typeof source.skillId === 'string' ? source.skillId.trim() : '';
  if (skillId.length === 0) {
    return null;
  }

  return {
    skillId,
    name: typeof source.name === 'string' ? source.name : skillId,
    description: typeof source.description === 'string' ? source.description : '',
    body: typeof source.body === 'string' ? source.body : '',
  };
}

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `Plugin`。
 *
 * 与 `normalizeSessionRecord` 同一策略：没有 `id` 的行直接丢弃——无名插件
 * 在 UI 上既不能定位也不能刷新，留着只会变成一行打不开的鬼影。
 */
export function normalizePlugin(raw: unknown): Plugin | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const id = typeof source.id === 'string' ? source.id.trim() : '';
  if (id.length === 0) {
    return null;
  }

  const skills = Array.isArray(source.skills)
    ? source.skills
        .map(normalizePluginSkill)
        .filter((skill): skill is PluginSkill => skill !== null)
    : [];

  return {
    id,
    name: typeof source.name === 'string' ? source.name : id,
    version: typeof source.version === 'string' ? source.version : '',
    apiVersion: typeof source.apiVersion === 'string' ? source.apiVersion : '',
    path: typeof source.path === 'string' ? source.path : '',
    status: typeof source.status === 'string' ? source.status : 'unknown',
    capabilities: normalizePluginCapabilities(source.capabilities),
    skills,
    registeredAt: typeof source.registeredAt === 'string' ? source.registeredAt : '',
  };
}

function asStringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === 'string')
    : [];
}

/**
 * 收敛 `plugin_refresh` 的返回。
 *
 * 形状错了也**不报错**：刷新是写路径，此时插件表可能已经更新了，
 * 让 UI 因为解析失败而显示「刷新失败」会掩盖真实结果。宁可显示空报告。
 */
export function normalizePluginRefreshReport(raw: unknown): PluginRefreshReport {
  if (typeof raw !== 'object' || raw === null) {
    return { registered: [], errors: [] };
  }

  const source = raw as Record<string, unknown>;
  return {
    registered: asStringArray(source.registered),
    errors: asStringArray(source.errors),
  };
}

/**
 * 已登记插件列表。**纯读**：不扫目录、不起子进程（ADR-0012）。
 *
 * 想发现新插件或让 degraded 插件复活，调用 [`pluginRefresh`]。
 */
export async function listPlugins(): Promise<IpcResult<Plugin[]>> {
  const result = await invokeCommand<unknown>('plugin_list');
  if (!result.ok) {
    return result;
  }

  const plugins = Array.isArray(result.data)
    ? result.data.map(normalizePlugin).filter((plugin): plugin is Plugin => plugin !== null)
    : [];

  return { ok: true, data: plugins };
}

/**
 * 显式刷新插件：扫描目录 → 登记 → 握手 → 拉取 skill 缓存。
 *
 * 语义上是**写操作**（起子进程、写 `plugins` 表），只允许用户动作触发，
 * 与 [`refreshUsage`] 同一条纪律。
 */
export async function pluginRefresh(): Promise<IpcResult<PluginRefreshReport>> {
  const result = await invokeCommand<unknown>('plugin_refresh');
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizePluginRefreshReport(result.data) };
}

/* ------------------------------------------------------------------ *
 * Git Session Snapshot（ROADMAP 阶段 A1，best-effort 观测）
 * ------------------------------------------------------------------ */

export type SnapshotPhase = 'start' | 'end';

/**
 * 与 IPC 契约 `session_git_snapshots` 的返回项同形（serde camelCase）。
 *
 * 这是**仓库状态观测**，不是产出归因：四类计数是快照时刻的工作区状态，
 * UI 只能说「会话期间的变更」，绝不能声称这些变更是谁写的（规划 R5）。
 */
export type SessionGitSnapshot = {
  hubSessionId: string;
  phase: SnapshotPhase;
  capturedAt: string;
  worktreeRoot: string;
  /** 40 位 hex；仓库还没有任何提交（unborn HEAD）时为 null。 */
  head: string | null;
  stagedFiles: number;
  unstagedFiles: number;
  untrackedFiles: number;
  conflictedFiles: number;
};

const SNAPSHOT_PHASES: readonly SnapshotPhase[] = ['start', 'end'];

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `SessionGitSnapshot`。
 *
 * 拿不到 `hubSessionId` 或 `phase` 非法 → 丢弃该行：不知道属于哪个会话、
 * 不知道是开始还是结束的快照没有观测意义，宁可少显示也不猜。
 * 四类计数缺省 0，`head` 只认字符串或 null。
 */
export function normalizeSessionGitSnapshot(raw: unknown): SessionGitSnapshot | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const hubSessionId = typeof source.hubSessionId === 'string' ? source.hubSessionId.trim() : '';
  if (hubSessionId.length === 0) {
    return null;
  }

  const phase = SNAPSHOT_PHASES.find((candidate) => candidate === source.phase);
  if (phase === undefined) {
    return null;
  }

  return {
    hubSessionId,
    phase,
    capturedAt: typeof source.capturedAt === 'string' ? source.capturedAt : '',
    worktreeRoot: typeof source.worktreeRoot === 'string' ? source.worktreeRoot : '',
    head: asStringOrNull(source.head),
    stagedFiles: asNumber(source.stagedFiles),
    unstagedFiles: asNumber(source.unstagedFiles),
    untrackedFiles: asNumber(source.untrackedFiles),
    conflictedFiles: asNumber(source.conflictedFiles),
  };
}

/** 某个会话的 Git 快照（start / end 各至多一条）。**纯读**：不碰外部仓库、不写库。 */
export async function getSessionGitSnapshots(
  hubSessionId: string,
): Promise<IpcResult<SessionGitSnapshot[]>> {
  const result = await invokeCommand<unknown>('session_git_snapshots', { hubSessionId });
  if (!result.ok) {
    return result;
  }

  const snapshots = Array.isArray(result.data)
    ? result.data
        .map(normalizeSessionGitSnapshot)
        .filter((snapshot): snapshot is SessionGitSnapshot => snapshot !== null)
    : [];

  return { ok: true, data: snapshots };
}

/** 会话窗口 Git 摘要（R5：窗口内仓库发生了什么，不是 AI 产出归因）。 */
export type SessionGitSummary = {
  commits: number;
  insertions: number;
  deletions: number;
};

/**
 * 收敛 `session_git_summary` 的返回。
 *
 * `null`（没有窗口：缺快照 / unborn HEAD / 仓库不可达）与「0 commits」是两回事，
 * 必须保留三态：null（没有摘要）/ 0（窗口内没有 commit）/ N（有变化）。
 * 计数字段缺省 0；完全不是对象也返回 null。
 */
export function normalizeSessionGitSummary(raw: unknown): SessionGitSummary | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  return {
    commits: asNumber(source.commits),
    insertions: asNumber(source.insertions),
    deletions: asNumber(source.deletions),
  };
}

/**
 * 会话窗口摘要（ROADMAP A3）。**纯读**：按需现算（后端至多跑两条有界 git 命令），
 * 不落库、不缓存。Rust 侧在锁外跑 git，本调用可能耗时至几秒。
 */
export async function getSessionGitSummary(
  hubSessionId: string,
): Promise<IpcResult<SessionGitSummary | null>> {
  const result = await invokeCommand<unknown>('session_git_summary', { hubSessionId });
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeSessionGitSummary(result.data) };
}

/* ------------------------------------------------------------------ *
 * 会话回放（ROADMAP B6，只读）
 * ------------------------------------------------------------------ */

/** 回放消息的角色。与 Rust `ReplayRole`（serde snake_case）一致。 */
export type ReplayRole = 'user' | 'assistant';

/**
 * 一次工具调用（与 Rust `ReplayToolCall`，serde camelCase 同形）。
 *
 * **只读、只摘要**：`inputJson` 是原样 JSON 字符串（后端截断到 2 KiB），
 * 前端不解析工具语义；`resultJson` 只在转录里有对应 tool_result 时非 null。
 */
export type ReplayToolCall = {
  /** 转录里的 tool_use id（与 tool_result 的关联键）。 */
  id: string;
  name: string;
  inputJson: string;
  /** 转录中没有对应 tool_result 时为 null（工具可能还在跑就断了）。 */
  resultJson: string | null;
  /** 结果被后端截断过（UI 必须标注「已截断」，不能静默丢内容）。 */
  resultTruncated: boolean;
  /** 输入被后端截断过。 */
  inputTruncated: boolean;
};

/** 一条子代理线程（与 Rust `SubagentThread`，serde camelCase 同形）。 */
export type SubagentThread = {
  /** 线程内消息的 seq（指向 messages 里的 seq 字段）。 */
  messageSeqs: number[];
  startedAt: string | null;
  lastAt: string | null;
};

/** 一条可回放的消息（与 Rust `ReplayMessage`，serde camelCase 同形）。 */
export type ReplayMessage = {
  seq: number;
  role: ReplayRole;
  /** sidechain（子代理链）里的消息；B8 用它构建子代理线程。 */
  sidechain: boolean;
  timestamp: string | null;
  /** 正文 text 部分，按出现顺序。 */
  textParts: string[];
  /** thinking 部分（UI 渲染为「思考」标注区，不与正文混排）。 */
  thinkingParts: string[];
  model: string | null;
  /** 工具调用（assistant 的 tool_use + user 行回填的 tool_result）。 */
  toolCalls: ReplayToolCall[];
  /** 认识但 B8 仍不展开的内容种类（如 image）→ 出现次数。 */
  unsupportedContentKinds: Record<string, number>;
};

/** 一次完整的只读回放（与 Rust `ReplaySession`，serde camelCase 同形）。 */
export type ReplaySession = {
  harnessId: string;
  sessionId: string;
  /** 转录文件的真实路径；只用于展示出处，UI 上不可点击、不可传回。 */
  sourcePath: string;
  messages: ReplayMessage[];
  /** 子代理线程（sidechain 按 parentUuid 链分段，按出现顺序）。 */
  subagentThreads: SubagentThread[];
  /** 被跳过的行类型（summary / file-history-snapshot / …）→ 次数。 */
  skippedKindCounts: Record<string, number>;
  /** 无法解析为 JSON 的行数。 */
  malformedLines: number;
};

/** 列表项摘要（与 Rust `ReplaySessionSummary`，serde camelCase 同形）。 */
export type ReplaySessionSummary = {
  harnessId: string;
  sessionId: string;
  /** 转录所在的项目目录名（Claude 用编码后的路径名，如 `D--work-demo`）。 */
  projectDir: string;
  sizeBytes: number;
  /** 修改时间（Unix 毫秒）。列表排序键：后端已按 mtime 倒序，前端不重排。 */
  modifiedMs: number;
  /** 首条 user 消息预览（≤ 120 字符）；无则 null。 */
  firstUserPreview: string | null;
  /** 预读窗口内的首/末时间戳；文件超过预读上限时 last 只是下界。 */
  startedAt: string | null;
  lastAt: string | null;
};

const REPLAY_ROLES: readonly ReplayRole[] = ['user', 'assistant'];

/**
 * 收敛「字符串 → 次数」的计数对象（skippedKindCounts / unsupportedContentKinds）。
 *
 * 完全不是对象 → `{}`；个别键的值不是有限数字 → 丢弃该键。
 * 宁可少显示一个计数，也不把字符串 / undefined 当成 0 渲染出来。
 */
function asCountMap(raw: unknown): Record<string, number> {
  if (typeof raw !== 'object' || raw === null) {
    return {};
  }

  const counts: Record<string, number> = {};
  for (const [key, value] of Object.entries(raw)) {
    if (typeof value === 'number' && Number.isFinite(value)) {
      counts[key] = value;
    }
  }
  return counts;
}

/**
 * 收敛 IPC 边界上的一条工具调用。
 *
 * 缺 `id` 或 `name` 的条目直接丢弃：id 是与 tool_result 关联的键、name 是
 * 工具卡的存在理由，残缺的工具卡既不能定位也不能展示。`inputJson` 非字符串
 * 收敛为 "{}"；`resultJson` 只认字符串或 null；两个 truncated 只认 boolean，
 * 缺省 false —— 截断标注绝不能凭一个字符串渲染出来。
 */
function normalizeReplayToolCall(raw: unknown): ReplayToolCall | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const id = typeof source.id === 'string' ? source.id.trim() : '';
  const name = typeof source.name === 'string' ? source.name : '';
  if (id.length === 0 || name.length === 0) {
    return null;
  }

  return {
    id,
    name,
    inputJson: typeof source.inputJson === 'string' ? source.inputJson : '{}',
    resultJson: typeof source.resultJson === 'string' ? source.resultJson : null,
    resultTruncated: source.resultTruncated === true,
    inputTruncated: source.inputTruncated === true,
  };
}

/**
 * 收敛 IPC 边界上的一条子代理线程。
 *
 * 缺 `messageSeqs`、或 seq 里有任何一个非正整数（0 / 负数 / 小数 / 非数字）
 * 的条目**整条丢弃**：线程的价值就在于点击后能按 seq 链定位消息，链不干净
 * 就宁可少显示一条线程，也不渲染一条点不全的鬼影行。`startedAt` / `lastAt`
 * 只认字符串或 null。
 */
function normalizeSubagentThread(raw: unknown): SubagentThread | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  if (!Array.isArray(source.messageSeqs) || source.messageSeqs.length === 0) {
    return null;
  }

  const messageSeqs: number[] = [];
  for (const seq of source.messageSeqs) {
    if (typeof seq !== 'number' || !Number.isInteger(seq) || seq <= 0) {
      return null;
    }
    messageSeqs.push(seq);
  }

  return {
    messageSeqs,
    startedAt: asStringOrNull(source.startedAt),
    lastAt: asStringOrNull(source.lastAt),
  };
}

/**
 * 收敛 IPC 边界上的一条回放消息。
 *
 * `role` 只认 user / assistant：解析层认识的行形状就这两种，其它（未来的
 * system / tool 行）整条丢弃，绝不猜成 user 或 assistant 渲染出来。
 * `sidechain` 只认 `true` —— 「不确定」绝不能渲染成「子代理」。
 */
function normalizeReplayMessage(raw: unknown): ReplayMessage | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const role = REPLAY_ROLES.find((candidate) => candidate === source.role);
  if (role === undefined) {
    return null;
  }

  return {
    seq: asNumber(source.seq),
    role,
    sidechain: source.sidechain === true,
    timestamp: asStringOrNull(source.timestamp),
    textParts: asStringArray(source.textParts),
    thinkingParts: asStringArray(source.thinkingParts),
    model: asStringOrNull(source.model),
    toolCalls: Array.isArray(source.toolCalls)
      ? source.toolCalls
          .map(normalizeReplayToolCall)
          .filter((call): call is ReplayToolCall => call !== null)
      : [],
    unsupportedContentKinds: asCountMap(source.unsupportedContentKinds),
  };
}

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `ReplaySession`。
 *
 * 拿不到 `sessionId` 丢弃（没有 id 的回放在 UI 上无法定位）；`messages` 非数组
 * 或条目残缺时逐条过滤；计数对象非对象 → `{}`。只读命令，payload 形状漂移
 * 不该让整页白屏。
 */
export function normalizeReplaySession(raw: unknown): ReplaySession | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const sessionId = typeof source.sessionId === 'string' ? source.sessionId.trim() : '';
  if (sessionId.length === 0) {
    return null;
  }

  const messages = Array.isArray(source.messages)
    ? source.messages
        .map(normalizeReplayMessage)
        .filter((message): message is ReplayMessage => message !== null)
    : [];

  const subagentThreads = Array.isArray(source.subagentThreads)
    ? source.subagentThreads
        .map(normalizeSubagentThread)
        .filter((thread): thread is SubagentThread => thread !== null)
    : [];

  return {
    harnessId: typeof source.harnessId === 'string' ? source.harnessId : '',
    sessionId,
    sourcePath: typeof source.sourcePath === 'string' ? source.sourcePath : '',
    messages,
    subagentThreads,
    skippedKindCounts: asCountMap(source.skippedKindCounts),
    malformedLines: asNumber(source.malformedLines),
  };
}

/**
 * 收敛发现列表里的一条摘要。拿不到 `sessionId` 丢弃 —— 点击后要用它去
 * `replay_read_session` 换正文，没有 id 的行既打不开也修不好。
 */
export function normalizeReplaySessionSummary(raw: unknown): ReplaySessionSummary | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const sessionId = typeof source.sessionId === 'string' ? source.sessionId.trim() : '';
  if (sessionId.length === 0) {
    return null;
  }

  return {
    harnessId: typeof source.harnessId === 'string' ? source.harnessId : '',
    sessionId,
    projectDir: typeof source.projectDir === 'string' ? source.projectDir : '',
    sizeBytes: asNumber(source.sizeBytes),
    modifiedMs: asNumber(source.modifiedMs),
    firstUserPreview: asStringOrNull(source.firstUserPreview),
    startedAt: asStringOrNull(source.startedAt),
    lastAt: asStringOrNull(source.lastAt),
  };
}

/**
 * 某个 Harness 的可回放会话列表（只读发现 + 有界预读，不落库）。
 *
 * 非 claude 的 harness 按契约返回空数组；列表保持后端顺序（mtime 倒序），
 * 前端不重排。
 */
export async function listReplaySessions(
  harnessId: string,
): Promise<IpcResult<ReplaySessionSummary[]>> {
  const result = await invokeCommand<unknown>('replay_list_sessions', { harnessId });
  if (!result.ok) {
    return result;
  }

  const sessions = Array.isArray(result.data)
    ? result.data
        .map(normalizeReplaySessionSummary)
        .filter((summary): summary is ReplaySessionSummary => summary !== null)
    : [];

  return { ok: true, data: sessions };
}

/**
 * 读取一条回放正文（按 session_id；路径由后端白名单解析，前端传不了路径）。
 *
 * 三态：`data: null` = 未命中发现结果（转录可能已被移动 / 清理），不是错误；
 * `ok: false` = 真失败（如转录超过大小上限）；其余为完整会话。
 */
export async function readReplaySession(
  harnessId: string,
  sessionId: string,
): Promise<IpcResult<ReplaySession | null>> {
  const result = await invokeCommand<unknown>('replay_read_session', { harnessId, sessionId });
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeReplaySession(result.data) };
}

/* ------------------------------------------------------------------ *
 * 回放搜索（ROADMAP B7，migration 0011）
 *
 * 设置 / 搜索是纯 SQLite 操作；reindex 是显式用户动作（读外部转录、写索引
 * 表），后端按 ADR-0012 不得在渲染路径自动触发，前端也不代替用户点它。
 * 索引内容只落本地 SQLite —— 关闭开关时后端清空全部索引行。
 * ------------------------------------------------------------------ */

/** 回放搜索设置（与 Rust `SearchSettings`，serde camelCase 同形）。 */
export type SearchSettings = {
  enabled: boolean;
  /** 排除的项目目录名（如 `D--work-beta`）；收紧名单时后端立即清掉对应索引行。 */
  excludedDirs: string[];
};

/** 一次重建索引的审计报告（与 Rust `ReindexReport`，serde camelCase 同形）。 */
export type ReindexReport = {
  /** 发现的转录数（含排除的）。 */
  scanned: number;
  /** 新建或更新的索引条目。 */
  indexed: number;
  /** 指纹未变而跳过的。 */
  unchanged: number;
  /** 命中排除名单而完全不读的。 */
  excluded: number;
  /** 源文件已消失而被清理的旧索引。 */
  removed: number;
};

/** 一条搜索命中（与 Rust `SearchHit`，serde camelCase 同形）。 */
export type ReplaySearchHit = {
  harnessId: string;
  sessionId: string;
  seq: number;
  role: ReplayRole;
  /** FTS5 snippet，命中词用 «» 包裹；UI 原样渲染，不解释标记。 */
  snippet: string;
  /** 会话首条 user 预览；无则 null。 */
  firstUserPreview: string | null;
};

/**
 * 收敛回放搜索设置。
 *
 * `enabled` 只认 `true`：默认关闭是隐私契约（migration 0011 的默认行就是关），
 * 形状漂移时宁可显示「关」，绝不能把未知值渲染成「已开启」。
 */
export function normalizeReplaySearchSettings(raw: unknown): SearchSettings {
  if (typeof raw !== 'object' || raw === null) {
    return { enabled: false, excludedDirs: [] };
  }

  const source = raw as Record<string, unknown>;
  return {
    enabled: source.enabled === true,
    excludedDirs: asStringArray(source.excludedDirs),
  };
}

/**
 * 收敛重建索引的审计报告。计数缺省 0：报告是展示用的审计数字，
 * 形状漂移不能让 UI 显示 NaN / undefined。
 */
export function normalizeReindexReport(raw: unknown): ReindexReport {
  if (typeof raw !== 'object' || raw === null) {
    return { scanned: 0, indexed: 0, unchanged: 0, excluded: 0, removed: 0 };
  }

  const source = raw as Record<string, unknown>;
  return {
    scanned: asNumber(source.scanned),
    indexed: asNumber(source.indexed),
    unchanged: asNumber(source.unchanged),
    excluded: asNumber(source.excluded),
    removed: asNumber(source.removed),
  };
}

/**
 * 收敛 IPC 边界上的一条搜索命中。
 *
 * 拿不到 `sessionId` 丢弃（点击后要用它定位会话，没有 id 的命中无法落地）；
 * `snippet` 非字符串整条丢弃 —— 命中片段是这条结果的存在理由，宁可少显示
 * 也不渲染 `undefined`。`seq` 缺省 0；`role` 只认 user / assistant，
 * 未知值收窄为 user，绝不猜成 assistant 渲染。
 */
export function normalizeReplaySearchHit(raw: unknown): ReplaySearchHit | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const sessionId = typeof source.sessionId === 'string' ? source.sessionId.trim() : '';
  if (sessionId.length === 0 || typeof source.snippet !== 'string') {
    return null;
  }

  return {
    harnessId: typeof source.harnessId === 'string' ? source.harnessId : '',
    sessionId,
    seq: asNumber(source.seq),
    role: source.role === 'assistant' ? 'assistant' : 'user',
    snippet: source.snippet,
    firstUserPreview: asStringOrNull(source.firstUserPreview),
  };
}

/** 读取回放搜索设置（默认关闭）。 */
export async function getReplaySearchSettings(): Promise<IpcResult<SearchSettings>> {
  const result = await invokeCommand<unknown>('replay_search_settings_get');
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeReplaySearchSettings(result.data) };
}

/** 更新回放搜索设置（关闭时后端清空全部索引行，隐私纪律：关掉就真的没了）。 */
export async function setReplaySearchSettings(settings: SearchSettings): Promise<IpcResult<null>> {
  return invokeCommand<null>('replay_search_settings_set', { settings });
}

/** 显式重建 / 增量更新索引（用户动作；未启用时后端报明确错误）。 */
export async function reindexReplaySearch(): Promise<IpcResult<ReindexReport>> {
  const result = await invokeCommand<unknown>('replay_search_reindex');
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeReindexReport(result.data) };
}

/**
 * 搜索已索引的回放内容（纯读 SQLite）。
 *
 * 「索引未启用」与「查询 <3 字符」时后端 reject（错误串含「未启用」/「3 个字符」），
 * 错误原样传出，由 UI 分类成行内提示而不是「搜索失败」错误卡。
 */
export async function searchReplay(query: string): Promise<IpcResult<ReplaySearchHit[]>> {
  const result = await invokeCommand<unknown>('replay_search', { query });
  if (!result.ok) {
    return result;
  }

  const hits = Array.isArray(result.data)
    ? result.data
        .map(normalizeReplaySearchHit)
        .filter((hit): hit is ReplaySearchHit => hit !== null)
    : [];

  return { ok: true, data: hits };
}

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

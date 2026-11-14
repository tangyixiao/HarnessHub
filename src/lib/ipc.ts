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
 * 恢复一条外部会话（ROADMAP B9）：后端以 `--resume <source_session_id>` 启动 PTY，
 * 返回的 `SessionRecord` 带 `launchMode = "resume"` 与传入的 `sourceSessionId`。
 *
 * 与 `startTerminal` 同构：Channel 先挂回调再 invoke（ready-before-spawn，ADR-0009），
 * 返回值经 `normalizeSessionRecord` 校验。
 *
 * 错误语义（后端契约已冻结）：
 * - capability `resume` = false 的 harness（如 codex）→ reject，错误串含「尚未支持恢复」；
 * - 同一 `sourceSessionId` 重复恢复 → 唯一索引 reject（错误串含 UNIQUE 或 source_session_id）。
 */
export async function resumeTerminal(input: {
  installationId: string;
  sourceSessionId: string;
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
    // 先挂回调再 invoke：Channel 必须在 resume_terminal 之前 ready。
    output.onmessage = input.onEvent;

    const raw = await invoke('resume_terminal', {
      installationId: input.installationId,
      sourceSessionId: input.sourceSessionId,
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
  /** 配置投影（E15）；当前只有 claude 为 true。 */
  profiles: boolean;
  /** 记忆文件投影（E19）；claude/codex/gemini 为 true。 */
  prompts: boolean;
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
  'profiles',
  'prompts',
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
    profiles: false,
    prompts: false,
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
 * Worktree（ROADMAP C10）
 *
 * list 纯读；add 是**显式用户动作**（写磁盘 + 检出代码），调用方必须先
 * `window.confirm` 二次确认。repoRoot 必须是已登记项目的 repo root，
 * 否则后端 reject（错误串含「未登记的项目根」），原样传出交由 UI 展示。
 * ------------------------------------------------------------------ */

/**
 * 一条 worktree（主树或链接树），与 Rust `WorktreeEntry`（serde camelCase）同形。
 *
 * `head` 是 40 位 hex（unborn HEAD 为 null）；`branch` 是短分支名
 * （`refs/heads/main` → `main`），detached 时为 null。
 */
export type WorktreeEntry = {
  /** 绝对路径（`git worktree list --porcelain` 原样）。 */
  path: string;
  head: string | null;
  branch: string | null;
  /** 是否主工作树（porcelain 首个条目）。 */
  main: boolean;
  detached: boolean;
  bare: boolean;
};

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `WorktreeEntry`。
 *
 * 拿不到 `path` 丢弃 —— 路径是 worktree 的唯一句柄（高亮 / 去重都靠它）；
 * `head` / `branch` 只认字符串或 null；三个布尔只认 `true`，缺省 false，
 * 「不确定」绝不能渲染成真。
 */
export function normalizeWorktreeEntry(raw: unknown): WorktreeEntry | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const path = typeof source.path === 'string' ? source.path.trim() : '';
  if (path.length === 0) {
    return null;
  }

  return {
    path,
    head: asStringOrNull(source.head),
    branch: asStringOrNull(source.branch),
    main: source.main === true,
    detached: source.detached === true,
    bare: source.bare === true,
  };
}

/** 某个已登记项目的全部 worktree。**纯读**：不改仓库、不写库。 */
export async function listWorktrees(repoRoot: string): Promise<IpcResult<WorktreeEntry[]>> {
  const result = await invokeCommand<unknown>('worktree_list', { repoRoot });
  if (!result.ok) {
    return result;
  }

  const entries = Array.isArray(result.data)
    ? result.data
        .map(normalizeWorktreeEntry)
        .filter((entry): entry is WorktreeEntry => entry !== null)
    : [];

  return { ok: true, data: entries };
}

/**
 * 创建 worktree（`git worktree add`，检出可能耗时几秒）。
 *
 * 错误语义（后端契约冻结，原样传出）：repoRoot 未登记、目标路径嵌套在仓库
 * 内部（「不允许位于仓库内部」）、分支冲突、路径已占用，都 reject。
 */
export async function addWorktree(
  repoRoot: string,
  path: string,
  branch: string,
  createBranch: boolean,
): Promise<IpcResult<WorktreeEntry>> {
  const result = await invokeCommand<unknown>('worktree_add', {
    repoRoot,
    path,
    branch,
    createBranch,
  });
  if (!result.ok) {
    return result;
  }

  const entry = normalizeWorktreeEntry(result.data);
  return entry ? { ok: true, data: entry } : { ok: false, error: 'invalid-worktree-payload' };
}

/* ------------------------------------------------------------------ *
 * Worktree 状态 / 合并 / 删除（ROADMAP C11）
 *
 * status 纯读（后端有界跑 `git status --porcelain`）；remove / merge 是
 * **显式用户动作**：前端必须 window.confirm（文案写明「删除目录」/「快进合并」），
 * 失败原样传出交由 UI 在行内展示。repoRoot 校验同 C10。
 * ------------------------------------------------------------------ */

/**
 * 一条 worktree 的未提交修改计数（与 Rust `WorktreeStatusCounts`，serde camelCase 同形）。
 *
 * 干净树四类计数全部为 0；徽标的「修改 N 项」取四数之和。
 */
export type WorktreeStatusCounts = {
  stagedFiles: number;
  unstagedFiles: number;
  untrackedFiles: number;
  conflictedFiles: number;
};

/**
 * 收敛 `worktree_status` 的返回。
 *
 * 计数缺省 0（与 `normalizeSessionGitSnapshot` 同一纪律）：形状漂移不能让
 * 徽标渲染出 NaN / undefined。
 */
export function normalizeWorktreeStatusCounts(raw: unknown): WorktreeStatusCounts {
  const source = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  return {
    stagedFiles: asNumber(source.stagedFiles),
    unstagedFiles: asNumber(source.unstagedFiles),
    untrackedFiles: asNumber(source.untrackedFiles),
    conflictedFiles: asNumber(source.conflictedFiles),
  };
}

/** 某条 worktree 的未提交修改计数。**纯读**：不改仓库、不写库。 */
export async function getWorktreeStatus(
  repoRoot: string,
  worktreePath: string,
): Promise<IpcResult<WorktreeStatusCounts>> {
  const result = await invokeCommand<unknown>('worktree_status', { repoRoot, worktreePath });
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeWorktreeStatusCounts(result.data) };
}

/** 快进合并结果（与 Rust `MergeOutcome`，serde camelCase 同形）。 */
export type MergeOutcome = {
  /** 合并前主树 HEAD（回溯用）；主树 unborn HEAD 时为 null。 */
  previousHead: string | null;
  /** 合并后主树 HEAD。 */
  newHead: string | null;
};

/**
 * 收敛 `worktree_merge` 的返回。
 *
 * head 只认字符串或 null（与 `normalizeWorktreeEntry` 的 head 同一纪律）：
 * unborn HEAD 的 null 绝不能被折算成空字符串或编造的 hash。
 */
export function normalizeMergeOutcome(raw: unknown): MergeOutcome {
  const source = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  return {
    previousHead: asStringOrNull(source.previousHead),
    newHead: asStringOrNull(source.newHead),
  };
}

/**
 * 删除一条**干净**的链接 worktree（`git worktree remove`）。
 *
 * 错误语义（后端契约冻结，原样传出）：脏树（错误含「未提交修改」并列出计数）、
 * 主树 / bare（「主工作树」）、不是本仓库的 worktree，都 reject。
 */
export function removeWorktree(repoRoot: string, worktreePath: string): Promise<IpcResult<null>> {
  return invokeCommand<null>('worktree_remove', { repoRoot, worktreePath });
}

/**
 * 把 worktree 的检出分支**快进**合并到主工作树（`git merge --ff-only`）。
 *
 * 错误语义（后端契约冻结，原样传出）：主树脏（「主工作树有未提交修改」）、
 * 非 fast-forward（错误含「merge」）、分支不是任何 worktree 的检出分支，都 reject；
 * 绝不做 rebase / force。
 */
export async function mergeWorktree(
  repoRoot: string,
  branch: string,
): Promise<IpcResult<MergeOutcome>> {
  const result = await invokeCommand<unknown>('worktree_merge', { repoRoot, branch });
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeMergeOutcome(result.data) };
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

/* ------------------------------------------------------------------ *
 * Provider 配置现状（ROADMAP E14，只读）
 *
 * 纯读：解析各 Harness 现有配置文件的选定键，只用于展示。不写任何外部
 * 文件——投影 / 切换是 E15 的独立验收，本区 UI 不提供修改入口。
 * secret 值已在 Rust 层脱敏（`<redacted>` / `set`/`unset`），前端原样展示。
 * ------------------------------------------------------------------ */

/**
 * 一个 Harness 的当前配置形态（与 Rust `ProfileSummary`，serde camelCase 同形）。
 *
 * `fields` 的键用「点路径」表示嵌套（如 `env.ANTHROPIC_MODEL`）；
 * secret 值已被 Rust 层替换为 `<redacted>`（gemini 的 key 只有 `set`/`unset`）。
 */
export type ProfileSummary = {
  harnessId: string;
  /** 配置文件绝对路径（展示用；文件不存在时为候选路径）。 */
  sourcePath: string;
  /** 已脱敏的键值对。只含字符串值，别的类型在解析时丢弃。 */
  fields: Record<string, string>;
  parsedAt: string;
};

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `ProfileSummary`。
 *
 * 拿不到 `harnessId` 丢弃 —— 不知道属于哪个 Harness 的配置现状无法在 UI 定位；
 * `fields` 非对象 → `{}`；值只认字符串，非字符串丢该键（与 `asCountMap` 同一
 * 纪律：宁可少显示一行，也不把数字 / 布尔 / undefined 当成配置值渲染出来）。
 */
export function normalizeProfileSummary(raw: unknown): ProfileSummary | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const harnessId = typeof source.harnessId === 'string' ? source.harnessId.trim() : '';
  if (harnessId.length === 0) {
    return null;
  }

  const fields: Record<string, string> = {};
  if (typeof source.fields === 'object' && source.fields !== null) {
    for (const [key, value] of Object.entries(source.fields)) {
      if (typeof value === 'string') {
        fields[key] = value;
      }
    }
  }

  return {
    harnessId,
    sourcePath: typeof source.sourcePath === 'string' ? source.sourcePath : '',
    fields,
    parsedAt: typeof source.parsedAt === 'string' ? source.parsedAt : '',
  };
}

/**
 * 读取一个 Harness 的 Provider 配置现状（只读发现）。
 *
 * 三态：`data: null` = 配置文件不存在（未初始化），**不是错误**；
 * `ok: false` = 真失败。未实现 harness（如 copilot）按后端契约 reject
 * （错误串含「尚未实现」），原样传出交由 UI 行内展示。
 */
export async function discoverProfile(
  harnessId: string,
): Promise<IpcResult<ProfileSummary | null>> {
  const result = await invokeCommand<unknown>('profile_discover', { harnessId });
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeProfileSummary(result.data) };
}

/* ------------------------------------------------------------------ *
 * Profile Plane 接线（E15 投影 / E16 凭据 / E17 体检）
 *
 * 与 Rust DTO 一一对应（契约测试锁定键名）；secret 值只在
 * credential_register / profile_project 的**请求方向**出现，
 * 任何响应方向都不携带 secret。
 * ------------------------------------------------------------------ */

/** 与 Rust `profile::projection::ProjectionReport` 同形。 */
export type ProjectionReport = {
  targetPath: string;
  backupId: string;
  changedKeys: number;
};

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `ProjectionReport`。
 *
 * 拿不到 `targetPath` 或 `backupId` 丢弃 —— 投影报告没有这两个字段就无法
 * 向用户交代「改了哪个文件、备份在哪」；`changedKeys` 负数按 0 处理。
 */
export function normalizeProjectionReport(raw: unknown): ProjectionReport | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const targetPath = typeof source.targetPath === 'string' ? source.targetPath : '';
  const backupId = typeof source.backupId === 'string' ? source.backupId : '';
  if (targetPath.length === 0 || backupId.length === 0) {
    return null;
  }

  return {
    targetPath,
    backupId,
    changedKeys: asNumber(source.changedKeys),
  };
}

/** 与 Rust `profile::projection::BackupInfo` 同形。 */
export type BackupInfo = {
  id: string;
  harnessId: string;
  createdAt: string;
  path: string;
  targetPath: string;
};

/** 收敛未知 JSON 为 `BackupInfo`；缺 `id` 或 `harnessId` 丢弃（无法定位的备份不渲染）。 */
export function normalizeBackupInfo(raw: unknown): BackupInfo | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const id = typeof source.id === 'string' ? source.id : '';
  const harnessId = typeof source.harnessId === 'string' ? source.harnessId : '';
  if (id.length === 0 || harnessId.length === 0) {
    return null;
  }

  return {
    id,
    harnessId,
    createdAt: typeof source.createdAt === 'string' ? source.createdAt : '',
    path: typeof source.path === 'string' ? source.path : '',
    targetPath: typeof source.targetPath === 'string' ? source.targetPath : '',
  };
}

/** 与 Rust `credentials::CredentialRefRecord` 同形 —— **永不含值**。 */
export type CredentialRef = {
  id: string;
  label: string;
  harnessId: string;
  storeKind: string;
  createdAt: string;
};

/** 收敛未知 JSON 为 `CredentialRef`；缺 `id` 或 `label` 丢弃（没有标签的句柄无法操作）。 */
export function normalizeCredentialRef(raw: unknown): CredentialRef | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const id = typeof source.id === 'string' ? source.id : '';
  const label = typeof source.label === 'string' ? source.label : '';
  if (id.length === 0 || label.length === 0) {
    return null;
  }

  return {
    id,
    label,
    harnessId: typeof source.harnessId === 'string' ? source.harnessId : '',
    storeKind: typeof source.storeKind === 'string' ? source.storeKind : '',
    createdAt: typeof source.createdAt === 'string' ? source.createdAt : '',
  };
}

/** 与 Rust `profile::doctor::CheckItem` 同形。 */
export type DoctorCheckItem = {
  check: string;
  status: 'ok' | 'warn' | 'error';
  message: string;
  suggestion: string | null;
};

/** 收敛未知 JSON 为 `DoctorCheckItem`；`status` 不是三态之一按 `error` 处理（不可信输入宁严重勿漏）。 */
export function normalizeCheckItem(raw: unknown): DoctorCheckItem | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const check = typeof source.check === 'string' ? source.check : '';
  const message = typeof source.message === 'string' ? source.message : '';
  if (check.length === 0 || message.length === 0) {
    return null;
  }

  const status = source.status;
  return {
    check,
    status: status === 'ok' || status === 'warn' || status === 'error' ? status : 'error',
    message,
    suggestion: typeof source.suggestion === 'string' ? source.suggestion : null,
  };
}

/**
 * 把字段投影进 harness 原生配置（E15，仅 claude）。**显式写动作**——
 * 调用方必须先向用户 confirm（含目标文件路径），绝不在渲染路径调用。
 */
export async function projectProfile(
  harnessId: string,
  fields: Record<string, string>,
): Promise<IpcResult<ProjectionReport>> {
  const result = await invokeCommand<unknown>('profile_project', { harnessId, fields });
  if (!result.ok) {
    return result;
  }

  const report = normalizeProjectionReport(result.data);
  return report ? { ok: true, data: report } : { ok: false, error: 'invalid-projection-report' };
}

/** 某个 harness 的投影备份列表（纯读，时间倒序）。 */
export async function listProfileBackups(harnessId: string): Promise<IpcResult<BackupInfo[]>> {
  const result = await invokeCommand<unknown[]>('profile_backups', { harnessId });
  if (!result.ok) {
    return result;
  }

  const items = (Array.isArray(result.data) ? result.data : [])
    .map(normalizeBackupInfo)
    .filter((item): item is BackupInfo => item !== null);
  return { ok: true, data: items };
}

/** 恢复到某个备份。显式写动作；目标路径由后端自己的备份记录决定。 */
export function restoreProfileBackup(
  harnessId: string,
  backupId: string,
): Promise<IpcResult<null>> {
  return invokeCommand<null>('profile_restore', { harnessId, backupId });
}

/**
 * 登记凭据（E16）：值进 OS Secret Store（FileSecretStore 兜底），SQLite 只存句柄。
 * **值只在这一次请求里跨 IPC**；调用方必须用密码型输入框，不做任何回显。
 */
export function registerCredential(
  harnessId: string,
  label: string,
  value: string,
): Promise<IpcResult<CredentialRef>> {
  return invokeCommand<CredentialRef>('credential_register', { harnessId, label, value });
}

/** 凭据句柄列表（纯读，无值）。 */
export async function listCredentials(harnessId: string): Promise<IpcResult<CredentialRef[]>> {
  const result = await invokeCommand<unknown[]>('credential_list', { harnessId });
  if (!result.ok) {
    return result;
  }

  const items = (Array.isArray(result.data) ? result.data : [])
    .map(normalizeCredentialRef)
    .filter((item): item is CredentialRef => item !== null);
  return { ok: true, data: items };
}

/** 删除凭据：先删 store 值再删句柄行。显式动作，前端 confirm。 */
export function removeCredential(id: string): Promise<IpcResult<boolean>> {
  return invokeCommand<boolean>('credential_remove', { id });
}

/** 只读配置体检（E17）。 */
export async function runConfigDoctor(harnessId: string): Promise<IpcResult<DoctorCheckItem[]>> {
  const result = await invokeCommand<unknown[]>('config_doctor', { harnessId });
  if (!result.ok) {
    return result;
  }

  const items = (Array.isArray(result.data) ? result.data : [])
    .map(normalizeCheckItem)
    .filter((item): item is DoctorCheckItem => item !== null);
  return { ok: true, data: items };
}

/* ------------------------------------------------------------------ *
 * MCP Binding Registry（ROADMAP E18）
 *
 * Hub 的 SQLite 是 MCP 配置的 source-of-truth；projection / sync 复用 E15
 * 的 backup / temp / verify / rename 管线写 harness 原生配置（见
 * src-tauri/src/mcp/mod.rs）。安全红线：
 * - envKeys 只是环境变量**名**元数据，任何变量值都不进 Hub；
 * - sync 只覆盖 Hub 管理的键，条目内用户手写的 env / headers 一律保留。
 * preview 纯读；apply / import 是**显式动作**，调用方必须 confirm 后才调用。
 * ------------------------------------------------------------------ */

/** MCP Server 的传输方式（与 Rust `McpTransport`，serde snake_case 同形）。 */
export type McpTransport = 'stdio' | 'http';

/** 一个登记在 Hub 的 MCP Server（与 Rust `McpServer`，serde camelCase 同形）。 */
export type McpServer = {
  id: string;
  /** 写进 harness 配置时使用的键（限 `[A-Za-z0-9._-]`）。 */
  name: string;
  transport: McpTransport;
  /** stdio：可执行文件。 */
  command: string | null;
  /** stdio：参数。 */
  args: string[];
  /** http：endpoint。 */
  url: string | null;
  /** 需要的环境变量**名**（展示 / Doctor 提示用；值绝不进 Hub）。 */
  envKeys: string[];
  enabled: boolean;
  /** `hub`（用户登记）或 `imported:<harness>`（从 harness 配置导入）。 */
  source: string;
  createdAt: string;
  updatedAt: string;
};

/** 新建 / 更新 MCP Server 的入参（与 Rust 命令 `mcp_server_upsert` 的 input 同形）。 */
export type McpServerInput = {
  /** null = 新建；否则按 id 覆盖。 */
  id: string | null;
  name: string;
  transport: McpTransport;
  command: string | null;
  args: string[];
  url: string | null;
  envKeys: string[];
  enabled: boolean;
};

/** 一条绑定：某 server 在哪个 scope 生效（与 Rust `McpBinding`，serde camelCase 同形）。 */
export type McpBinding = {
  id: string;
  serverId: string;
  /** 冗余展示名（JOIN 取出，前端不必再查一遍）。 */
  serverName: string;
  /** `global` | `harness`。 */
  scope: string;
  /** harness 作用域时是目标 harness 的 id；global 为 null。 */
  harnessId: string | null;
  createdAt: string;
};

/** 一条同步差异（与 Rust `McpDiffEntry`，serde camelCase 同形）。 */
export type McpDiffEntry = {
  /** harness 配置里的 server 键名。 */
  name: string;
  /** 给 UI 的一句人话摘要（如「命令从 npx 改为 bunx」）。 */
  summary: string;
};

/** 一次同步的预览（与 Rust `McpSyncPreview`，serde camelCase 同形）。 */
export type McpSyncPreview = {
  harnessId: string;
  /** 目标 harness 配置文件的绝对路径（confirm 文案里要展示给用户看）。 */
  targetPath: string;
  /** 将新增的 server 键名。 */
  additions: string[];
  /** 将覆盖的条目（两侧都有但内容有差异）。 */
  updates: McpDiffEntry[];
  /** 将移除的条目（harness 配置里有、Hub 侧没有）。 */
  removals: McpDiffEntry[];
};

/** 一次同步的结果（与 Rust `McpSyncReport`，serde camelCase 同形）。 */
export type McpSyncReport = {
  targetPath: string;
  /** 写入前备份的 id（回溯用，对应 E15 的备份管线）。 */
  backupId: string;
  added: number;
  updated: number;
  removed: number;
};

/** 一次导入的结果（与 Rust `McpImportReport`，serde camelCase 同形）。 */
export type McpImportReport = {
  harnessId: string;
  /** 导入成功的 server 键名（Registry 里 source = imported:<harness>）。 */
  imported: string[];
  /** 被跳过的键名（如重名冲突 / 键名不合法）。 */
  skipped: string[];
};

/**
 * 收敛 IPC 边界上的未知 JSON 成安全的 `McpServer`。
 *
 * 拿不到 `id` 丢弃 —— Registry 条目的改删 / 绑定都靠 id 定位；`transport`
 * 只认 `stdio` / `http`，未知值整条丢弃（绝不能把未知传输的 server 当成
 * stdio 渲染或投影）；`enabled` 只认 `true`，缺省 false ——「不确定」绝不
 * 能渲染成已启用。
 */
export function normalizeMcpServer(raw: unknown): McpServer | null {
  if (typeof raw !== 'object' || raw === null) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  const id = typeof source.id === 'string' ? source.id.trim() : '';
  if (id.length === 0) {
    return null;
  }

  const transport = source.transport;
  if (transport !== 'stdio' && transport !== 'http') {
    return null;
  }

  return {
    id,
    name: typeof source.name === 'string' ? source.name : '',
    transport,
    command: asStringOrNull(source.command),
    args: asStringArray(source.args),
    url: asStringOrNull(source.url),
    envKeys: asStringArray(source.envKeys),
    enabled: source.enabled === true,
    source: typeof source.source === 'string' ? source.source : '',
    createdAt: typeof source.createdAt === 'string' ? source.createdAt : '',
    updatedAt: typeof source.updatedAt === 'string' ? source.updatedAt : '',
  };
}

/**
 * 收敛一条绑定。拿不到 `id` 丢弃 —— 删除 / 展示都要靠它定位；
 * `harnessId` 只认非空字符串或 null，global 作用域的 null 不折算成 ''。
 */
export function normalizeMcpBinding(raw: unknown): McpBinding | null {
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
    serverId: typeof source.serverId === 'string' ? source.serverId : '',
    serverName: typeof source.serverName === 'string' ? source.serverName : '',
    scope: typeof source.scope === 'string' ? source.scope : '',
    harnessId: asStringOrNull(source.harnessId),
    createdAt: typeof source.createdAt === 'string' ? source.createdAt : '',
  };
}

/**
 * 收敛 diff 条目列表。拿不到 `name` 的条目丢弃 —— 差异没有键名无法落地；
 * `summary` 缺省空串（UI 显示「无说明」即可，不该因此丢整条差异）。
 */
function asMcpDiffEntries(value: unknown): McpDiffEntry[] {
  if (!Array.isArray(value)) {
    return [];
  }

  const entries: McpDiffEntry[] = [];
  for (const item of value) {
    if (typeof item !== 'object' || item === null) {
      continue;
    }
    const source = item as Record<string, unknown>;
    const name = typeof source.name === 'string' ? source.name.trim() : '';
    if (name.length === 0) {
      continue;
    }
    entries.push({ name, summary: typeof source.summary === 'string' ? source.summary : '' });
  }
  return entries;
}

/**
 * 收敛同步预览。
 *
 * preview 是**纯读**，形状漂移时收敛为空预览而不是报错：让 UI 因为解析失败
 * 弹「预览加载失败」没有意义，空预览里没有 targetPath / 差异，调用方自然
 * 不会走到下一步的 confirm。
 */
export function normalizeMcpSyncPreview(raw: unknown): McpSyncPreview {
  if (typeof raw !== 'object' || raw === null) {
    return { harnessId: '', targetPath: '', additions: [], updates: [], removals: [] };
  }

  const source = raw as Record<string, unknown>;
  return {
    harnessId: typeof source.harnessId === 'string' ? source.harnessId : '',
    targetPath: typeof source.targetPath === 'string' ? source.targetPath : '',
    additions: asStringArray(source.additions),
    updates: asMcpDiffEntries(source.updates),
    removals: asMcpDiffEntries(source.removals),
  };
}

/**
 * 收敛同步报告。apply 是写路径，响应形状漂移时**不报错**（写已经发生了，
 * 误报「失败」会诱导用户重试造成二次写入）；计数缺省 0，路径缺省空串，
 * 绝不渲染 NaN / undefined。
 */
export function normalizeMcpSyncReport(raw: unknown): McpSyncReport {
  if (typeof raw !== 'object' || raw === null) {
    return { targetPath: '', backupId: '', added: 0, updated: 0, removed: 0 };
  }

  const source = raw as Record<string, unknown>;
  return {
    targetPath: typeof source.targetPath === 'string' ? source.targetPath : '',
    backupId: typeof source.backupId === 'string' ? source.backupId : '',
    added: asNumber(source.added),
    updated: asNumber(source.updated),
    removed: asNumber(source.removed),
  };
}

/**
 * 收敛导入报告。import 也是显式写动作，报告形状漂移同样不报错（导入已经
 * 发生）；名单缺省 []，宁可显示空报告也不渲染 undefined。
 */
export function normalizeMcpImportReport(raw: unknown): McpImportReport {
  if (typeof raw !== 'object' || raw === null) {
    return { harnessId: '', imported: [], skipped: [] };
  }

  const source = raw as Record<string, unknown>;
  return {
    harnessId: typeof source.harnessId === 'string' ? source.harnessId : '',
    imported: asStringArray(source.imported),
    skipped: asStringArray(source.skipped),
  };
}

/** 列出 Registry 里登记的全部 MCP Server。**纯读**：不改库、不碰配置文件。 */
export async function listMcpServers(): Promise<IpcResult<McpServer[]>> {
  const result = await invokeCommand<unknown>('mcp_server_list');
  if (!result.ok) {
    return result;
  }

  const servers = Array.isArray(result.data)
    ? result.data.map(normalizeMcpServer).filter((server): server is McpServer => server !== null)
    : [];

  return { ok: true, data: servers };
}

/**
 * 新建（id 为 null）或更新一个 MCP Server 的登记信息。
 *
 * 只写 Registry：本命令不触碰任何 harness 配置文件；真正写配置是
 * preview → apply 的显式两步（见 `applyMcpSync`）。
 */
export async function upsertMcpServer(input: McpServerInput): Promise<IpcResult<McpServer>> {
  const result = await invokeCommand<unknown>('mcp_server_upsert', { input });
  if (!result.ok) {
    return result;
  }

  const server = normalizeMcpServer(result.data);
  return server ? { ok: true, data: server } : { ok: false, error: 'invalid-mcp-server-payload' };
}

/** 从 Registry 删除一个 MCP Server（只删登记行，不动 harness 配置）。 */
export async function removeMcpServer(id: string): Promise<IpcResult<boolean>> {
  return invokeCommand<boolean>('mcp_server_remove', { id });
}

/** 列出全部绑定（server × scope）。**纯读**：不改库、不碰配置文件。 */
export async function listMcpBindings(): Promise<IpcResult<McpBinding[]>> {
  const result = await invokeCommand<unknown>('mcp_binding_list');
  if (!result.ok) {
    return result;
  }

  const bindings = Array.isArray(result.data)
    ? result.data
        .map(normalizeMcpBinding)
        .filter((binding): binding is McpBinding => binding !== null)
    : [];

  return { ok: true, data: bindings };
}

/**
 * 新增一条绑定：某 server 在 `global` 或某个 harness 作用域生效。
 *
 * 绑定只是 Registry 里的意图记录，不触碰 harness 配置文件。
 */
export async function addMcpBinding(
  serverId: string,
  scope: string,
  harnessId: string | null,
): Promise<IpcResult<McpBinding>> {
  const result = await invokeCommand<unknown>('mcp_binding_add', { serverId, scope, harnessId });
  if (!result.ok) {
    return result;
  }

  const binding = normalizeMcpBinding(result.data);
  return binding
    ? { ok: true, data: binding }
    : { ok: false, error: 'invalid-mcp-binding-payload' };
}

/** 删除一条绑定（只删意图记录，不动 harness 配置）。 */
export async function removeMcpBinding(id: string): Promise<IpcResult<boolean>> {
  return invokeCommand<boolean>('mcp_binding_remove', { id });
}

/**
 * 预演「把 Registry 内容同步进某 harness 配置」的差异。**纯读**：不写库、
 * 不写文件，UI 可以反复调用；确认要执行时再走 `applyMcpSync`。
 */
export async function previewMcpSync(harnessId: string): Promise<IpcResult<McpSyncPreview>> {
  const result = await invokeCommand<unknown>('mcp_sync_preview', { harnessId });
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeMcpSyncPreview(result.data) };
}

/**
 * 真正执行同步：把 Registry 内容写入 harness 原生配置（写前自动备份）。
 *
 * **显式动作，调用方必须 confirm**：UI 必须先展示 `previewMcpSync` 的差异
 * （含 targetPath 与增 / 改 / 删清单）并让用户确认后才可调用；绝不可以
 * 在渲染路径自动触发。
 */
export async function applyMcpSync(harnessId: string): Promise<IpcResult<McpSyncReport>> {
  const result = await invokeCommand<unknown>('mcp_sync_apply', { harnessId });
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeMcpSyncReport(result.data) };
}

/**
 * 从 harness 现有配置反向导入 MCP Server 到 Registry（source = imported:<harness>）。
 *
 * **显式动作，调用方必须 confirm**：导入会新增 Registry 行，UI 必须
 * `window.confirm` 后才可调用；本命令不修改 harness 配置文件本身。
 */
export async function importMcpConfig(harnessId: string): Promise<IpcResult<McpImportReport>> {
  const result = await invokeCommand<unknown>('mcp_import', { harnessId });
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeMcpImportReport(result.data) };
}

/* ------------------------------------------------------------------ *
 * Prompts Registry + Skills 只读发现（ROADMAP E19）
 *
 * 与 Rust DTO 一一对应（serde camelCase，契约测试已锁定键名）：
 * - prompt 是 canonical markdown 的统一登记（Hub SQLite 为 source-of-truth），
 *   投影进 harness 记忆文件（CLAUDE.md / AGENTS.md / GEMINI.md）走 E15 文本管线；
 * - prompt_list / prompt_render_preview / skills_list 是**纯读**，可反复调用；
 * - prompt_upsert / prompt_remove / prompt_render_apply 是**显式动作**，
 *   调用方必须 confirm 后才调用（apply 的 confirm 文案必须含 preview 的 targetPath）。
 * ------------------------------------------------------------------ */

/** 一条登记的 prompt（与 Rust `PromptArtifact`，serde camelCase 同形）。 */
export type PromptArtifact = {
  id: string;
  name: string;
  /** canonical markdown 原文。 */
  content: string;
  /** 绑定项目；null = 全局（进每个项目的渲染）。 */
  projectId: string | null;
  enabled: boolean;
  createdAt: string;
  updatedAt: string;
};

/** 新建 / 更新 prompt 的入参（与 Rust `PromptInput`，serde camelCase 同形）。 */
export type PromptInput = {
  /** null = 新建；否则按 id 覆盖。 */
  id: string | null;
  name: string;
  content: string;
  /** null = 全局。 */
  projectId: string | null;
  enabled: boolean;
};

/** 记忆文件投影预览（与 Rust `PromptRenderPreview`，serde camelCase 同形）。 */
export type PromptRenderPreview = {
  harnessId: string;
  /** 投影目标绝对路径（`<repo>/<CLAUDE.md|AGENTS.md|GEMINI.md>`），confirm 文案要展示。 */
  targetPath: string;
  /** 将写盘的全文（含标记块与块外用户内容）。 */
  content: string;
  /** true = 目标文件尚不存在，apply 会新建（此时报告的 backupId 是合法的空串）。 */
  createsFile: boolean;
};

/** 一个发现的 skill（只读快照；与 Rust `SkillSummary`，serde camelCase 同形）。 */
export type SkillSummary = {
  /** 目录名（skills 目录下的 `<name>/`，也是 claude 的加载 id）。 */
  name: string;
  /** frontmatter 的 description（缺失为空串）。 */
  description: string;
  /** SKILL.md 绝对路径。 */
  path: string;
};

/** 一次 skills 扫描的报告（与 Rust `SkillsReport`，serde camelCase 同形）。 */
export type SkillsReport = {
  skills: SkillSummary[];
  /** 无法解析的条目（目录名；不崩纪律的观察面）。 */
  skipped: string[];
};

/**
 * 收敛 IPC 边界上的未知 JSON 成安全的 `PromptArtifact`。
 *
 * 拿不到 `id` 丢弃 —— prompt 的更新 / 删除 / 渲染都靠 id 定位；`enabled`
 * 只认 `true`，缺省 false（「不确定」绝不能渲染成已启用）；`projectId` 的
 * null 是全局语义，不能折算成 ''。
 */
export function normalizePromptArtifact(raw: unknown): PromptArtifact | null {
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
    content: typeof source.content === 'string' ? source.content : '',
    projectId: asStringOrNull(source.projectId),
    enabled: source.enabled === true,
    createdAt: typeof source.createdAt === 'string' ? source.createdAt : '',
    updatedAt: typeof source.updatedAt === 'string' ? source.updatedAt : '',
  };
}

/**
 * 收敛渲染预览。preview 是**纯读**，形状漂移时收敛为空预览而不是报错：
 * 让 UI 因为解析失败弹「预览加载失败」没有意义，空预览没有 targetPath /
 * content，调用方自然不会走到下一步的 confirm。
 */
export function normalizePromptRenderPreview(raw: unknown): PromptRenderPreview {
  if (typeof raw !== 'object' || raw === null) {
    return { harnessId: '', targetPath: '', content: '', createsFile: false };
  }

  const source = raw as Record<string, unknown>;
  return {
    harnessId: typeof source.harnessId === 'string' ? source.harnessId : '',
    targetPath: typeof source.targetPath === 'string' ? source.targetPath : '',
    content: typeof source.content === 'string' ? source.content : '',
    createsFile: source.createsFile === true,
  };
}

/**
 * 收敛 `prompt_render_apply` 的报告（复用 E15 `ProjectionReport` 形状）。
 *
 * **与 `normalizeProjectionReport` 的差异**：那条把 `backupId` 空串当成
 * 「报告残缺」丢弃，因为 JSON 投影（settings.json）总是先有文件后有备份；
 * 而记忆文件投影是「创建或更新」——目标文件不存在时 apply 会新建文件，
 * Rust 侧报告的 `backupId` 就是**合法的空串**（没有备份可记，见
 * src-tauri/src/prompts/render.rs 的 render_apply）。沿用旧的「空串丢弃」
 * 会把一次成功的新建误报成失败，诱导用户重试造成二次写入，所以这里空串
 * 照单全收；apply 是写路径，响应形状漂移同样收敛为空报告而不误报失败。
 */
function normalizePromptProjectionReport(raw: unknown): ProjectionReport {
  if (typeof raw !== 'object' || raw === null) {
    return { targetPath: '', backupId: '', changedKeys: 0 };
  }

  const source = raw as Record<string, unknown>;
  return {
    targetPath: typeof source.targetPath === 'string' ? source.targetPath : '',
    backupId: typeof source.backupId === 'string' ? source.backupId : '',
    changedKeys: asNumber(source.changedKeys),
  };
}

/**
 * 收敛 skills 扫描报告。
 *
 * `skills` 缺失 / 非数组收敛为 []（skills 目录不存在是正常状态，不是错误）；
 * 拿不到 `name` 的条目丢弃 —— 目录名就是 claude 的加载 id，没有它条目无法
 * 展示或定位；`skipped` 照单保留（哪些目录坏了必须如实告诉用户）。
 */
export function normalizeSkillsReport(raw: unknown): SkillsReport {
  if (typeof raw !== 'object' || raw === null) {
    return { skills: [], skipped: [] };
  }

  const source = raw as Record<string, unknown>;
  const skills: SkillSummary[] = [];
  if (Array.isArray(source.skills)) {
    for (const item of source.skills) {
      if (typeof item !== 'object' || item === null) {
        continue;
      }
      const entry = item as Record<string, unknown>;
      const name = typeof entry.name === 'string' ? entry.name.trim() : '';
      if (name.length === 0) {
        continue;
      }
      skills.push({
        name,
        description: typeof entry.description === 'string' ? entry.description : '',
        path: typeof entry.path === 'string' ? entry.path : '',
      });
    }
  }

  return { skills, skipped: asStringArray(source.skipped) };
}

/**
 * prompt 列表。**纯读**：不改库、不碰文件。
 *
 * `projectId` 传 null = 全部；传项目 id = 该项目绑定 + 全局。键名 `projectId`
 * 恒在（null 也传，对应 Rust `Option<String>` 的 None），不要省略键。
 */
export async function listPrompts(projectId: string | null): Promise<IpcResult<PromptArtifact[]>> {
  const result = await invokeCommand<unknown>('prompt_list', { projectId });
  if (!result.ok) {
    return result;
  }

  const prompts = Array.isArray(result.data)
    ? result.data
        .map(normalizePromptArtifact)
        .filter((prompt): prompt is PromptArtifact => prompt !== null)
    : [];
  return { ok: true, data: prompts };
}

/**
 * 新建（input.id 为 null）或更新一条 prompt。**显式动作**：写 Hub 的 SQLite
 * （只写 Registry，不碰任何 harness 记忆文件），调用方 confirm 后才调用。
 */
export async function upsertPrompt(input: PromptInput): Promise<IpcResult<PromptArtifact>> {
  const result = await invokeCommand<unknown>('prompt_upsert', { input });
  if (!result.ok) {
    return result;
  }

  const prompt = normalizePromptArtifact(result.data);
  return prompt ? { ok: true, data: prompt } : { ok: false, error: 'invalid-prompt-payload' };
}

/** 删除一条 prompt（只删 Registry 行，不动 harness 记忆文件）。**显式动作，前端 confirm**。 */
export async function removePrompt(id: string): Promise<IpcResult<boolean>> {
  return invokeCommand<boolean>('prompt_remove', { id });
}

/**
 * 记忆文件投影预览。**纯读**：不写库、不写文件，UI 可以反复调用；
 * 确认要执行时再走 `applyPromptRender`。
 */
export async function previewPromptRender(
  projectId: string,
  harnessId: string,
): Promise<IpcResult<PromptRenderPreview>> {
  const result = await invokeCommand<unknown>('prompt_render_preview', { projectId, harnessId });
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizePromptRenderPreview(result.data) };
}

/**
 * 真正执行记忆文件投影：把该 scope（projectId + 全局）的 enabled prompts 写进
 * harness 记忆文件（E15 文本管线，写前自动备份；目标文件不存在时新建，
 * 此时报告的 backupId 为空串）。
 *
 * **显式动作，调用方必须 confirm**：UI 必须先展示 `previewPromptRender` 的
 * 预览（confirm 文案必须含 targetPath 与将写入的全文）并让用户确认后才可
 * 调用；绝不在渲染路径自动触发。
 */
export async function applyPromptRender(
  projectId: string,
  harnessId: string,
): Promise<IpcResult<ProjectionReport>> {
  const result = await invokeCommand<unknown>('prompt_render_apply', { projectId, harnessId });
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizePromptProjectionReport(result.data) };
}

/**
 * 只读发现 `~/.claude/skills/<name>/SKILL.md`。**纯读**：claude 自己是 skills
 * 的加载方，Hub 只发现展示，不复制投影、不改任何 skill 文件。
 */
export async function listSkills(): Promise<IpcResult<SkillsReport>> {
  const result = await invokeCommand<unknown>('skills_list');
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeSkillsReport(result.data) };
}

/* ------------------------------------------------------------------ *
 * Python Sidecar 生命周期状态（F20，spec §5）
 *
 * 与 Rust `SidecarStatus`（serde rename_all = "camelCase"）一一对应。
 * 契约纪律：Rust 侧 contract test 锁实际键名，这里用同形状 fixture 锁解析行为；
 * 不靠「看起来应该是 camelCase」猜。
 *
 * 失败一律收敛成状态、不向上传播为 IPC 错误（验收 #5）：Python 缺失只影响
 * 这一张卡片，PTY / Git / Session 等主功能零感知。
 * ------------------------------------------------------------------ */

export type SidecarState = 'stopped' | 'ready' | 'degraded';

/** 与 Rust `RuntimeInfo`（serde camelCase）同形。 */
export interface SidecarRuntimeInfo {
  name: string;
  version: string;
  transport: string;
  python: string;
  protocolVersion: string;
  providers: string[];
  capabilities: string[];
  methods: string[];
}

/** 与 Rust `SidecarStatus`（serde camelCase）同形。 */
export interface SidecarStatus {
  state: SidecarState;
  pid: number | null;
  runtime: SidecarRuntimeInfo | null;
  negotiatedCapabilities: string[];
  protocolVersion: string | null;
  lastError: string | null;
  restartCount: number;
}

function isSidecarState(value: unknown): value is SidecarState {
  return value === 'stopped' || value === 'ready' || value === 'degraded';
}

/** 每次返回全新对象，避免多处共用同一个数组引用。 */
function degradedSidecarStatus(): SidecarStatus {
  return {
    state: 'degraded',
    pid: null,
    runtime: null,
    negotiatedCapabilities: [],
    protocolVersion: null,
    lastError: null,
    restartCount: 0,
  };
}

function normalizeSidecarRuntime(raw: unknown): SidecarRuntimeInfo | null {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return null;
  }

  const source = raw as Record<string, unknown>;
  return {
    name: typeof source.name === 'string' ? source.name : '',
    version: typeof source.version === 'string' ? source.version : '',
    transport: typeof source.transport === 'string' ? source.transport : '',
    python: typeof source.python === 'string' ? source.python : '',
    protocolVersion: typeof source.protocolVersion === 'string' ? source.protocolVersion : '',
    providers: asStringArray(source.providers),
    capabilities: asStringArray(source.capabilities),
    methods: asStringArray(source.methods),
  };
}

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `SidecarStatus`。
 *
 * `state` 是判别字段：**认不出来就整体 fail-closed**，连同一 payload 里的
 * `pid` / `runtime` 也不采信 —— 一个连状态都说不清的后端，没资格让 UI 显示
 * 「ready」。其余字段逐项安全默认（null / [] / 0 / ''），绝不抛异常。
 */
export function normalizeSidecarStatus(raw: unknown): SidecarStatus {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return degradedSidecarStatus();
  }

  const source = raw as Record<string, unknown>;
  if (!isSidecarState(source.state)) {
    return degradedSidecarStatus();
  }

  return {
    state: source.state,
    pid: typeof source.pid === 'number' && Number.isFinite(source.pid) ? source.pid : null,
    runtime: normalizeSidecarRuntime(source.runtime),
    negotiatedCapabilities: asStringArray(source.negotiatedCapabilities),
    protocolVersion: typeof source.protocolVersion === 'string' ? source.protocolVersion : null,
    lastError: typeof source.lastError === 'string' ? source.lastError : null,
    restartCount: asNumber(source.restartCount),
  };
}

async function invokeSidecar(command: string): Promise<IpcResult<SidecarStatus>> {
  const result = await invokeCommand<unknown>(command);
  if (!result.ok) {
    return result;
  }

  return { ok: true, data: normalizeSidecarStatus(result.data) };
}

/** 纯读：含崩溃检出（status 会把已退出的进程翻成 degraded）。 */
export function sidecarStatus(): Promise<IpcResult<SidecarStatus>> {
  return invokeSidecar('sidecar_status');
}

/** 幂等启动；失败由 Rust 侧降级为状态，不返回 Err。 */
export function sidecarStart(): Promise<IpcResult<SidecarStatus>> {
  return invokeSidecar('sidecar_start');
}

/** 幂等停止。 */
export function sidecarStop(): Promise<IpcResult<SidecarStatus>> {
  return invokeSidecar('sidecar_stop');
}

/** 显式恢复入口：stop + start，restartCount += 1。 */
export function sidecarRestart(): Promise<IpcResult<SidecarStatus>> {
  return invokeSidecar('sidecar_restart');
}

// --- F21 Provider Gateway：跨 IPC decoder（spec §6.1 / §7.1）---

export type GatewayState = 'ready' | 'degraded' | 'unavailable';
export type GatewaySidecarState = 'stopped' | 'ready' | 'degraded';

export interface GatewayProvider {
  id: string;
  mode: string;
  importable: boolean;
  detail: string;
}

export interface GatewayTokenization {
  precedence: string[];
  levels: string[];
  revisionBinding: boolean;
}

export interface GatewayStatus {
  state: GatewayState;
  sidecarState: GatewaySidecarState;
  mode: string;
  providers: GatewayProvider[];
  tokenization: GatewayTokenization;
  capabilities: string[];
  protocolVersion: string | null;
  lastError: string | null;
}

/** spec §3.2 冻结的 10 项 gateway 能力；fail-closed 默认复用，绝不猜测后端能力。 */
const F21_CAPABILITIES: string[] = [
  'ping',
  'runtime.info',
  'shutdown',
  'gateway.status',
  'gateway.routing',
  'gateway.streaming',
  'gateway.errors',
  'tokenization.precedence',
  'tokenization.estimate',
  'tokenization.revision',
];

/** spec §5.1 冻结的 tokenization 默认值。 */
function defaultGatewayTokenization(): GatewayTokenization {
  return {
    precedence: ['provider_reported', 'provider_count_api', 'local_exact', 'estimated'],
    levels: ['local_exact', 'estimated'],
    revisionBinding: true,
  };
}

/**
 * 网关状态不可用时的 fail-closed 安全默认。
 * 每次返回全新对象/数组，避免多处共用同一个数组引用。
 */
export function unavailableGatewayStatus(): GatewayStatus {
  return {
    state: 'unavailable',
    sidecarState: 'stopped',
    mode: 'unified',
    providers: [],
    tokenization: defaultGatewayTokenization(),
    capabilities: [...F21_CAPABILITIES],
    protocolVersion: null,
    lastError: '网关状态不可用',
  };
}

function isGatewayState(value: unknown): value is GatewayState {
  return value === 'ready' || value === 'degraded' || value === 'unavailable';
}

function isGatewaySidecarState(value: unknown): value is GatewaySidecarState {
  return value === 'stopped' || value === 'ready' || value === 'degraded';
}

function asGatewayString(value: unknown): string {
  return typeof value === 'string' ? value : '';
}

function asGatewayStringOrNull(value: unknown): string | null {
  return typeof value === 'string' ? value : null;
}

function normalizeGatewayProvider(raw: unknown): GatewayProvider {
  const source = raw as Record<string, unknown>;
  return {
    id: asGatewayString(source.id),
    mode: asGatewayString(source.mode),
    importable: typeof source.importable === 'boolean' ? source.importable : false,
    detail: asGatewayString(source.detail),
  };
}

function normalizeGatewayTokenization(raw: unknown): GatewayTokenization {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return defaultGatewayTokenization();
  }
  const source = raw as Record<string, unknown>;
  return {
    precedence: asStringArray(source.precedence),
    levels: asStringArray(source.levels),
    revisionBinding: typeof source.revisionBinding === 'boolean' ? source.revisionBinding : false,
  };
}

/**
 * 把 Rust gateway_status DTO（spec §6.1）归一化成 GatewayStatus。
 * 非对象 / 数组 / state 非三枚举一律 fail-closed；逐字段 typeof 守卫，坏数据不泄漏。
 */
export function normalizeGatewayStatus(raw: unknown): GatewayStatus {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return unavailableGatewayStatus();
  }
  const source = raw as Record<string, unknown>;
  if (!isGatewayState(source.state)) {
    return unavailableGatewayStatus();
  }

  const providers = Array.isArray(source.providers)
    ? source.providers
        .filter(
          (item): item is Record<string, unknown> =>
            typeof item === 'object' && item !== null && !Array.isArray(item),
        )
        .map(normalizeGatewayProvider)
    : [];

  return {
    state: source.state,
    sidecarState: isGatewaySidecarState(source.sidecarState) ? source.sidecarState : 'stopped',
    mode: asGatewayString(source.mode),
    providers,
    tokenization: normalizeGatewayTokenization(source.tokenization),
    capabilities: asStringArray(source.capabilities),
    protocolVersion: asGatewayStringOrNull(source.protocolVersion),
    lastError: asGatewayStringOrNull(source.lastError),
  };
}

/** 与 sidecarStatus() 同构：只走 invokeCommand，命令名固定 gateway_status。 */
async function invokeGateway(command: string): Promise<IpcResult<GatewayStatus>> {
  const result = await invokeCommand<unknown>(command);
  if (!result.ok) {
    return result;
  }
  return { ok: true, data: normalizeGatewayStatus(result.data) };
}

export function gatewayStatus(): Promise<IpcResult<GatewayStatus>> {
  return invokeGateway('gateway_status');
}

// --- F22 MCP Runtime：client 运行时状态与只读工具调用（spec §3.2-§3.6 / §3.8 / §5）---

/** spec §3.2 冻结的四个 server 运行时状态；degraded = 连接后进程死亡 / 握手失败（保留 lastError）。 */
export type McpServerRuntimeState = 'disconnected' | 'connecting' | 'ready' | 'degraded';

/**
 * 一台 MCP server 的运行时状态（与 Rust `McpServerState`，serde camelCase 同形）。
 *
 * - `serverName` / `serverVersion` 是 spec §3.2 `serverInfo { name, version }` 的扁平化
 *   投影；归一化时同时接受嵌套 `serverInfo` 形状（契约两侧任何一侧先落地都能解析）。
 * - `name` / `transport` 用原样字符串收敛 —— 未知值绝不猜成已知枚举（对齐
 *   `normalizeMcpServer`「未知传输不能当 stdio 渲染」的红线）。
 */
export interface McpServerState {
  /** Registry 键名（`^[A-Za-z0-9._-]+$`）；connect/disconnect 的行动定位最终靠 serverId。 */
  name: string;
  transport: string;
  state: McpServerRuntimeState;
  protocolVersion: string | null;
  serverName: string;
  serverVersion: string;
  toolCount: number;
  lastError: string | null;
  /** 需要的环境变量**名**（值绝不进 Hub；子进程继承宿主环境）。 */
  envKeys: string[];
  /** 提示用户宿主环境需有这些变量（信息性，不阻塞 UI）。 */
  envMissingKeys: string[];
}

/** `mcp_runtime_status` 的返回（与 Rust `McpRuntimeStatus`，serde camelCase 同形）。 */
export interface McpRuntimeStatus {
  /** Python sidecar 是否就绪；false 时前端禁用全部运行时操作。 */
  available: boolean;
  unavailableReason: string | null;
  servers: McpServerState[];
}

/** 一个已发现的工具（与 Rust `McpToolDescriptor`，serde camelCase 同形；spec §3.5）。 */
export interface McpToolDescriptor {
  name: string;
  description: string;
  /** 只回顶层属性键名（≤32 个），不回显完整 schema（防超大 payload）。 */
  inputSchemaKeys: string[];
}

/** 一次工具调用的结果（与 Rust `McpToolCallOutcome`，serde camelCase 同形；spec §3.6 / §3.8）。 */
export interface McpToolCallOutcome {
  /**
   * Rust 策略（v1 只读规则）放行与否；false 时不执行、`reason` 说明原因。
   * 这是 HTTP 200 语义的结构化返回，不是 IPC 错误。
   */
  allowed: boolean;
  reason: string;
  isError: boolean;
  contentCount: number;
  contentTypes: string[];
  /** 只拼 `type == "text"` 的 blocks；无 text block 时为 null（spec §3.6）。 */
  textPreview: string | null;
  truncated: boolean;
}

function isMcpServerRuntimeState(value: unknown): value is McpServerRuntimeState {
  return (
    value === 'disconnected' || value === 'connecting' || value === 'ready' || value === 'degraded'
  );
}

/**
 * 认不出状态就整体 degraded：绝不能把说不清的 server 显示成 ready
 * （对齐 normalizeSidecarStatus 的 fail-closed 纪律）。每次返回全新对象 / 数组。
 */
function degradedMcpServerState(): McpServerState {
  return {
    name: '',
    transport: '',
    state: 'degraded',
    protocolVersion: null,
    serverName: '',
    serverVersion: '',
    toolCount: 0,
    lastError: null,
    envKeys: [],
    envMissingKeys: [],
  };
}

/**
 * serverInfo 字段收敛：优先扁平键（Rust DTO 投影），回退嵌套 `serverInfo`
 * （spec §3.2 原始形状）；两处都拿不到合法字符串时空串。
 */
function asMcpServerInfoField(flat: unknown, info: unknown, key: 'name' | 'version'): string {
  if (typeof flat === 'string' && flat.length > 0) {
    return flat;
  }
  if (typeof info === 'object' && info !== null && !Array.isArray(info)) {
    const value = (info as Record<string, unknown>)[key];
    if (typeof value === 'string' && value.length > 0) {
      return value;
    }
  }
  return '';
}

/**
 * 把 IPC 边界上的未知 JSON 收敛成安全的 `McpServerState`。
 *
 * `state` 认不出来收敛 `degraded`（绝不假装 ready）；其余字段逐项安全默认
 * （null / [] / 0 / ''），绝不抛异常。
 */
export function normalizeMcpServerState(raw: unknown): McpServerState {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return degradedMcpServerState();
  }

  const source = raw as Record<string, unknown>;
  return {
    name: typeof source.name === 'string' ? source.name : '',
    transport: typeof source.transport === 'string' ? source.transport : '',
    state: isMcpServerRuntimeState(source.state) ? source.state : 'degraded',
    protocolVersion: typeof source.protocolVersion === 'string' ? source.protocolVersion : null,
    serverName: asMcpServerInfoField(source.serverName, source.serverInfo, 'name'),
    serverVersion: asMcpServerInfoField(source.serverVersion, source.serverInfo, 'version'),
    toolCount: asNumber(source.toolCount),
    lastError: typeof source.lastError === 'string' ? source.lastError : null,
    envKeys: asStringArray(source.envKeys),
    envMissingKeys: asStringArray(source.envMissingKeys),
  };
}

/**
 * 把 `mcp_runtime_status` 的返回收敛成安全的 `McpRuntimeStatus`。
 *
 * `available` 只认 `true` —— 「不确定」绝不渲染成可用（运行时操作全部禁用）；
 * servers 里非对象 / 拿不到 `name` 的条目丢弃（没有键名无法展示或定位）。
 */
export function normalizeMcpRuntimeStatus(raw: unknown): McpRuntimeStatus {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return { available: false, unavailableReason: null, servers: [] };
  }

  const source = raw as Record<string, unknown>;
  const servers: McpServerState[] = [];
  if (Array.isArray(source.servers)) {
    for (const item of source.servers) {
      if (typeof item !== 'object' || item === null || Array.isArray(item)) {
        continue;
      }
      const state = normalizeMcpServerState(item);
      if (state.name.length === 0) {
        continue;
      }
      servers.push(state);
    }
  }

  return {
    available: source.available === true,
    unavailableReason:
      typeof source.unavailableReason === 'string' ? source.unavailableReason : null,
    servers,
  };
}

/**
 * 把 `mcp_runtime_tools_list` 的返回收敛成安全的 `McpToolDescriptor[]`。
 *
 * 拿不到 `name` 的条目丢弃 —— 工具名是调用的定位键；非数组整体收敛 []。
 */
export function normalizeMcpToolList(raw: unknown): McpToolDescriptor[] {
  if (!Array.isArray(raw)) {
    return [];
  }

  const tools: McpToolDescriptor[] = [];
  for (const item of raw) {
    if (typeof item !== 'object' || item === null || Array.isArray(item)) {
      continue;
    }
    const source = item as Record<string, unknown>;
    const name = typeof source.name === 'string' ? source.name.trim() : '';
    if (name.length === 0) {
      continue;
    }
    tools.push({
      name,
      description: typeof source.description === 'string' ? source.description : '',
      inputSchemaKeys: asStringArray(source.inputSchemaKeys),
    });
  }
  return tools;
}

/**
 * 把 `mcp_runtime_tool_call` 的返回收敛成安全的 `McpToolCallOutcome`。
 *
 * `allowed` 只认 `true` —— 坏形状绝不假装放行；`isError` / `truncated` 同理只认
 * `true`（不确定不是错误，也不虚报截断）。
 */
export function normalizeMcpToolCallOutcome(raw: unknown): McpToolCallOutcome {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return {
      allowed: false,
      reason: '',
      isError: false,
      contentCount: 0,
      contentTypes: [],
      textPreview: null,
      truncated: false,
    };
  }

  const source = raw as Record<string, unknown>;
  return {
    allowed: source.allowed === true,
    reason: typeof source.reason === 'string' ? source.reason : '',
    isError: source.isError === true,
    contentCount: asNumber(source.contentCount),
    contentTypes: asStringArray(source.contentTypes),
    textPreview: typeof source.textPreview === 'string' ? source.textPreview : null,
    truncated: source.truncated === true,
  };
}

/**
 * 把 `mcp_runtime_connect` 的返回收敛成安全的 `McpServerState`。
 *
 * connect 成功返回该 server 的 mcp.status 单元素形状（toolCount=0，spec §3.3）；
 * 任何失败收敛 `state: "degraded"` + `lastError`，**不抛 RPC 错**（F20 状态收敛
 * 纪律），所以这里同样只做 fail-closed 归一化。
 */
export function normalizeMcpConnectOutcome(raw: unknown): McpServerState {
  return normalizeMcpServerState(raw);
}

async function invokeMcpRuntimeStatus(command: string): Promise<IpcResult<McpRuntimeStatus>> {
  const result = await invokeCommand<unknown>(command);
  if (!result.ok) {
    return result;
  }
  return { ok: true, data: normalizeMcpRuntimeStatus(result.data) };
}

async function invokeMcpServerState(
  command: string,
  args: Record<string, unknown>,
): Promise<IpcResult<McpServerState>> {
  const result = await invokeCommand<unknown>(command, args);
  if (!result.ok) {
    return result;
  }
  return { ok: true, data: normalizeMcpServerState(result.data) };
}

/**
 * MCP Runtime 状态快照。**纯读**：含连接存活状况（state 由 Rust 侧收敛，不抛错）。
 */
export function mcpRuntimeStatus(): Promise<IpcResult<McpRuntimeStatus>> {
  return invokeMcpRuntimeStatus('mcp_runtime_status');
}

/**
 * 连接一台 server（Rust 从 Registry 读定义、拼 connect 参数、env 由子进程继承
 * 宿主环境）。连接本身只读（不执行任何工具），但会**启动外部 MCP server 进程
 * 或发起网络连接** —— UI 必须先明示这一行为，之后才可调用。
 */
export function mcpRuntimeConnect(serverId: string): Promise<IpcResult<McpServerState>> {
  return invokeMcpServerState('mcp_runtime_connect', { serverId });
}

/** 断开一台 server。幂等：未连接的 server 也返回 disconnected（spec §3.4）。 */
export function mcpRuntimeDisconnect(serverId: string): Promise<IpcResult<McpServerState>> {
  return invokeMcpServerState('mcp_runtime_disconnect', { serverId });
}

/** 只读工具发现（tools/list 分页循尽后归一，上限 512 个）。**纯读**。 */
export async function mcpRuntimeToolsList(
  serverId: string,
): Promise<IpcResult<McpToolDescriptor[]>> {
  const result = await invokeCommand<unknown>('mcp_runtime_tools_list', { serverId });
  if (!result.ok) {
    return result;
  }
  return { ok: true, data: normalizeMcpToolList(result.data) };
}

/**
 * 只读工具调用。**显式动作，调用方必须 confirm**（文案含工具名，并明示将连接
 * 外部 MCP server）；Rust 策略先审（v1 只允许 get/list/search/… 只读前缀），
 * deny 时返回 `allowed: false` + `reason`，不转发 Python。
 */
export async function mcpRuntimeToolCall(
  serverId: string,
  tool: string,
  args: Record<string, unknown>,
): Promise<IpcResult<McpToolCallOutcome>> {
  const result = await invokeCommand<unknown>('mcp_runtime_tool_call', {
    serverId,
    tool,
    arguments: args,
  });
  if (!result.ok) {
    return result;
  }
  return { ok: true, data: normalizeMcpToolCallOutcome(result.data) };
}

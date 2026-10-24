import { useEffect, useState } from 'react';

import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import { harnessIdentity } from '@/lib/harness-colors';
import { cn } from '@/lib/utils';
import {
  NOT_IN_TAURI,
  getSessionGitSnapshots,
  listSessions,
  type SessionGitSnapshot,
  type SessionRecord,
  type SessionStatus,
  type TerminationReason,
} from '@/lib/ipc';

const STATUS_LABEL: Record<SessionStatus, string> = {
  created: '已创建 · 尚未启动',
  running: '运行中',
  exited: '已结束',
  failed: '失败',
  unknown: '未知',
};

const STATUS_STYLE: Record<SessionStatus, string> = {
  created: 'border-signal-warn/30 bg-signal-warn/12 text-signal-warn',
  running: 'border-accent/30 bg-accent/12 text-accent',
  exited: 'border-signal-ok/30 bg-signal-ok/12 text-signal-ok',
  failed: 'border-signal-error/30 bg-signal-error/12 text-signal-error',
  unknown: 'border-border-subtle bg-white/[0.03] text-content-muted',
};

/** 终态着色的语义点：绿色=正常结束、红=失败、灰=未知，运行中用 accent。 */
const STATUS_DOT: Record<SessionStatus, string> = {
  created: 'bg-signal-warn',
  running: 'bg-accent',
  exited: 'bg-signal-ok',
  failed: 'bg-signal-error',
  unknown: 'bg-content-muted',
};

const TERMINATION_LABEL: Record<TerminationReason, string> = {
  natural_exit: '正常退出',
  user_killed: '用户主动结束',
  launch_failed: '启动失败',
  runtime_error: '运行故障',
  host_shutdown: 'Harness Hub 关闭',
  lost: '失去联系',
};

/**
 * 状态说明。**`created` 绝不能显示成「仍在运行」** —— 它登记了但还没有任何进程
 * （见 docs/adr/0006-session-state-machine.md）。
 *
 * 终态同时展示**终止原因**与**退出码**：非零退出不等于同一种失败，
 * 旧数据没有原因时如实说「原因未记录」，不猜。
 */
function statusDetail(session: SessionRecord): string {
  switch (session.status) {
    case 'created':
      return '尚未启动（没有进程）';
    case 'running':
      return '仍在运行（尚未收到退出码）';
    default: {
      const reason =
        session.terminationReason === null
          ? '原因未记录'
          : TERMINATION_LABEL[session.terminationReason];
      const code = session.exitCode === null ? '' : ` · 退出码 ${session.exitCode}`;
      return `${reason}${code}`;
    }
  }
}

type PageState =
  | { kind: 'loading' }
  | { kind: 'ipc-unavailable' }
  | { kind: 'error'; message: string }
  | { kind: 'ready'; sessions: SessionRecord[] };

/** 只展示到秒，避免把带毫秒的原始串整条塞进 UI。 */
function formatTimestamp(value: string): string {
  if (value.length === 0) return '—';
  return value.replace('T', ' ').replace(/\.\d+/, '').replace('Z', '');
}

/**
 * Sessions 页面。
 *
 * 只读展示已记录的会话。**本页没有「新建会话」按钮**：真正启动会话需要 PTY
 * （后续 Task），现在放一个按钮只会造出「running 但没有任何进程」的假会话。
 */
export function SessionsPage() {
  const [state, setState] = useState<PageState>({ kind: 'loading' });

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const result = await listSessions();
      if (cancelled) return;

      if (result.ok) {
        setState({ kind: 'ready', sessions: result.data });
      } else if (result.error === NOT_IN_TAURI) {
        setState({ kind: 'ipc-unavailable' });
      } else {
        setState({ kind: 'error', message: result.error });
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div>
      <PageHeader
        title="Sessions"
        description="Harness Hub 统一会话索引：hub_session_id 与外部 source_session_id 分离管理。"
      />

      {state.kind === 'loading' ? (
        <Card>
          <CardTitle>正在读取会话索引…</CardTitle>
        </Card>
      ) : null}

      {state.kind === 'ipc-unavailable' ? (
        <Card>
          <CardTitle>IPC 不可用（浏览器模式）</CardTitle>
          <CardDescription>
            请用 pnpm tauri dev 启动桌面应用。会话索引只存在于 Rust Control Plane 的 SQLite 中，
            浏览器模式读不到任何数据。
          </CardDescription>
        </Card>
      ) : null}

      {state.kind === 'error' ? (
        <Card className="border-signal-error/40">
          <CardTitle>读取失败</CardTitle>
          <CardDescription>IPC 错误：{state.message}</CardDescription>
        </Card>
      ) : null}

      {state.kind === 'ready' && state.sessions.length === 0 ? (
        <Card>
          <CardTitle>还没有任何会话记录</CardTitle>
          <CardDescription>
            启动真实会话需要 PTY 支持，PTY 尚未接入（见 docs/plans 的 Task 4）。
            在它落地之前，本页不会有数据，也不会显示任何占位会话。
          </CardDescription>
        </Card>
      ) : null}

      {state.kind === 'ready' && state.sessions.length > 0 ? (
        <div className="space-y-3">
          {state.sessions.map((session) => (
            <SessionCard key={session.hubSessionId} session={session} />
          ))}
          <p className="text-[11px] text-content-muted">
            「已创建 · 尚未启动」表示会话已登记但**没有任何进程**：它是 Task 4 接入 PTY 之前
            唯一诚实的状态（见 ADR-0006）。
          </p>
        </div>
      ) : null}
    </div>
  );
}

function SessionCard({ session }: { session: SessionRecord }) {
  const identity = harnessIdentity(session.harnessId ?? '');
  const [gitSnapshots, setGitSnapshots] = useState<SessionGitSnapshot[] | null>(null);

  // Git 快照是 best-effort 观测：读取失败或没有快照（cwd 缺失 / 非 Git 仓库）时
  // 完全不渲染区块 —— 不报错也不转圈，绝不影响卡片其余内容。
  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const result = await getSessionGitSnapshots(session.hubSessionId);
      if (cancelled) return;

      setGitSnapshots(result.ok && result.data.length > 0 ? result.data : null);
    })();

    return () => {
      cancelled = true;
    };
  }, [session.hubSessionId]);

  return (
    <section aria-label={session.hubSessionId}>
      <Card>
        <div className="flex items-start justify-between gap-4">
          <div className="flex min-w-0 items-center gap-2.5">
            <span
              aria-hidden
              className="flex size-8 shrink-0 items-center justify-center rounded-md border text-sm font-semibold uppercase"
              style={{
                color: identity.fg,
                backgroundColor: identity.bg,
                borderColor: identity.ring,
              }}
            >
              {(session.harnessId || '?').slice(0, 1)}
            </span>
            <div className="min-w-0">
              <CardTitle>{session.harnessId || '（未知 Harness）'}</CardTitle>
              <CardDescription className="truncate font-mono text-[11px]">
                {session.hubSessionId}
              </CardDescription>
            </div>
          </div>
          <span
            className={cn(
              'flex shrink-0 items-center gap-1.5 rounded-md border px-2 py-1 text-[11px] font-medium',
              STATUS_STYLE[session.status],
            )}
          >
            <span aria-hidden className={cn('size-1.5 rounded-full', STATUS_DOT[session.status])} />
            {STATUS_LABEL[session.status]}
          </span>
        </div>

        <dl className="mt-4 grid gap-3 text-xs sm:grid-cols-3">
          <Field label="开始时间（UTC）" value={formatTimestamp(session.startedAt)} mono />
          <Field
            label="结束时间（UTC）"
            value={session.endedAt === null ? '—' : formatTimestamp(session.endedAt)}
            mono
          />
          <Field label="工作目录" value={session.cwd ?? '—'} mono />
        </dl>

        <p className="mt-3 text-[11px] text-content-muted">
          {statusDetail(session)}
          {session.installationId === null ? '' : ` · 安装：${session.installationId}`}
          {session.sourceSessionId === null
            ? ' · 无外部 source session id（由 Harness Hub 创建）'
            : ` · 外部 session：${session.sourceSessionId}`}
        </p>

        {gitSnapshots !== null ? <GitSnapshotBlock snapshots={gitSnapshots} /> : null}
      </Card>
    </section>
  );
}

/** head 短 sha（前 7 位）。仓库还没有任何提交（unborn HEAD）时如实显示，不编造 sha。 */
function shortHead(head: string | null): string {
  return head === null ? 'unborn（无提交）' : head.slice(0, 7);
}

/**
 * 「Git · 会话期间的变更」区块。
 *
 * 用词纪律（规划 R5）：这里展示的是**仓库状态**（head 迁移 + 四类文件计数），
 * 不是产出归因 —— 绝不能写「AI 写了 N 行」这类声称变更是谁做的文案。
 */
function GitSnapshotBlock({ snapshots }: { snapshots: SessionGitSnapshot[] }) {
  const start = snapshots.find((snapshot) => snapshot.phase === 'start') ?? null;
  const end = snapshots.find((snapshot) => snapshot.phase === 'end') ?? null;
  // 四类计数是快照时刻的工作区状态：优先取会话结束时刻；只有 start 时如实展示 start 的。
  const summary = end ?? start;

  if (summary === null) {
    return null;
  }

  // 两端都有快照时展示 head 的迁移；只有一端时只展示那一端，不编造另一端。
  const head =
    start !== null && end !== null
      ? `${shortHead(start.head)} → ${shortHead(end.head)}`
      : shortHead(summary.head);

  return (
    <div className="mt-3 border-t border-border-subtle pt-3">
      <p className="text-[11px] font-medium text-content-muted">Git · 会话期间的变更</p>
      <dl className="mt-2 grid gap-3 text-xs sm:grid-cols-5">
        <Field label="HEAD" value={head} mono />
        <Field label="已暂存文件" value={String(summary.stagedFiles)} />
        <Field label="未暂存文件" value={String(summary.unstagedFiles)} />
        <Field label="未跟踪文件" value={String(summary.untrackedFiles)} />
        <Field label="冲突文件" value={String(summary.conflictedFiles)} />
      </dl>
    </div>
  );
}

function Field({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div>
      <dt className="text-content-muted">{label}</dt>
      <dd className={cn('mt-0.5 break-all text-[11px] text-content-primary', mono && 'font-mono')}>
        {value}
      </dd>
    </div>
  );
}

import { useEffect, useState } from 'react';

import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import { cn } from '@/lib/utils';
import {
  NOT_IN_TAURI,
  listSessions,
  type SessionRecord,
  type SessionStatus,
} from '@/lib/ipc';

const STATUS_LABEL: Record<SessionStatus, string> = {
  running: '运行中',
  exited: '已结束',
  failed: '失败',
  unknown: '未知',
};

const STATUS_STYLE: Record<SessionStatus, string> = {
  running: 'border-accent/40 bg-accent/10 text-accent',
  exited: 'border-signal-ok/40 bg-signal-ok/10 text-signal-ok',
  failed: 'border-signal-error/40 bg-signal-error/10 text-signal-error',
  unknown: 'border-border-subtle bg-surface-overlay/40 text-content-muted',
};

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
        </div>
      ) : null}
    </div>
  );
}

function SessionCard({ session }: { session: SessionRecord }) {
  const running = session.status === 'running';

  return (
    <section aria-label={session.hubSessionId}>
      <Card>
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <CardTitle>{session.harnessId || '（未知 Harness）'}</CardTitle>
            <CardDescription className="break-all font-mono">
              {session.hubSessionId}
            </CardDescription>
          </div>
          <span
            className={cn(
              'shrink-0 rounded-md border px-2 py-1 text-[11px] font-medium',
              STATUS_STYLE[session.status],
            )}
          >
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
          {running
            ? '仍在运行（尚未收到退出码）'
            : session.exitCode === null
              ? '已结束，退出码未记录'
              : `退出码 ${session.exitCode}`}
          {session.sourceSessionId === null
            ? ' · 无外部 source session id（由 Harness Hub 创建）'
            : ` · 外部 session：${session.sourceSessionId}`}
        </p>
      </Card>
    </section>
  );
}

function Field({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div>
      <dt className="text-content-muted">{label}</dt>
      <dd
        className={cn('mt-0.5 break-all text-[11px] text-content-primary', mono && 'font-mono')}
      >
        {value}
      </dd>
    </div>
  );
}

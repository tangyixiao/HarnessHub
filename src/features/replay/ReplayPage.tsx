import { useEffect, useRef, useState } from 'react';

import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import { cn } from '@/lib/utils';
import {
  NOT_IN_TAURI,
  listHarnesses,
  listReplaySessions,
  readReplaySession,
  type ReplayMessage,
  type ReplaySession,
  type ReplaySessionSummary,
} from '@/lib/ipc';

/**
 * B6 只实现了 Claude Code 的回放解析（`src-tauri/src/replay/claude.rs`）；
 * capability 门控在前，这里固定用 claude 拉取发现列表。
 */
const REPLAY_HARNESS_ID = 'claude';

/**
 * 能力门控状态：capability 矩阵说了算。没有任何 harness 实现 replay 时
 * 渲染说明卡 —— 这是「能力未实现」，不是错误，绝不能报错或白屏。
 */
type HarnessState =
  | { kind: 'loading' }
  | { kind: 'ipc-unavailable' }
  | { kind: 'error'; message: string }
  | { kind: 'no-replay-capability' }
  | { kind: 'ready' };

type ListState =
  | { kind: 'loading' }
  | { kind: 'error'; message: string }
  | { kind: 'ready'; sessions: ReplaySessionSummary[] };

/**
 * 右侧时间线的状态。`missing` 是 `replay_read_session` 返回 null 的情形：
 * session 未命中后端发现结果（转录可能已被移动 / 清理），不是错误也不是空会话。
 */
type DetailState =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'error'; message: string }
  | { kind: 'missing' }
  | { kind: 'ready'; session: ReplaySession };

/** 只展示到秒，避免把带毫秒的原始串整条塞进 UI（与 SessionsPage 同一格式）。 */
function formatTimestamp(value: string): string {
  if (value.length === 0) return '—';
  return value.replace('T', ' ').replace(/\.\d+/, '').replace('Z', '');
}

/** 字节数人类可读：1024 进制，与终端世界里 KiB/MiB 的习惯一致。 */
function formatBytes(sizeBytes: number): string {
  if (!Number.isFinite(sizeBytes) || sizeBytes < 0) return '—';
  if (sizeBytes < 1024) return `${sizeBytes} B`;

  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  let value = sizeBytes;
  let unit = 'B';
  for (const next of units) {
    if (value < 1024) break;
    value /= 1024;
    unit = next;
  }
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${unit}`;
}

/**
 * 底部脚注：如实渲染 skippedKindCounts 与 malformedLines。
 * 计数是「这一版解析器没渲染的行」，宁可全说出来也不吞掉 —— 上游格式漂移时
 * 用户应当能看到「还有多少内容没显示」。
 */
function skippedFooterText(session: ReplaySession): string {
  const entries = Object.entries(session.skippedKindCounts);
  const total = entries.reduce((sum, [, count]) => sum + count, 0);
  const breakdown =
    entries.length > 0
      ? `（${entries.map(([kind, count]) => `${kind} ×${count}`).join('、')}）`
      : '';
  return `已跳过 ${total} 行非消息记录${breakdown} · 无法解析 ${session.malformedLines} 行`;
}

/**
 * 会话回放页（ROADMAP B6）。
 *
 * **全部只读**：发现列表由后端扫描转录文件（有界预读），正文按 session_id
 * 按需现读、不落库；本页没有任何写操作按钮，也不把 `sourcePath` 传回后端
 * （路径白名单在后端，前端只持有 session id）。
 */
export function ReplayPage() {
  const [harnessState, setHarnessState] = useState<HarnessState>({ kind: 'loading' });
  const [listState, setListState] = useState<ListState>({ kind: 'loading' });
  const [detailState, setDetailState] = useState<DetailState>({ kind: 'idle' });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // 点击竞态：慢请求返回时若已有更新的点击，直接丢弃（最后一次点击胜出）。
  const requestSeq = useRef(0);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const result = await listHarnesses();
      if (cancelled) return;

      if (!result.ok) {
        setHarnessState(
          result.error === NOT_IN_TAURI
            ? { kind: 'ipc-unavailable' }
            : { kind: 'error', message: result.error },
        );
        return;
      }
      setHarnessState(
        result.data.some((harness) => harness.capabilities.replay)
          ? { kind: 'ready' }
          : { kind: 'no-replay-capability' },
      );
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (harnessState.kind !== 'ready') return;
    let cancelled = false;

    void (async () => {
      const result = await listReplaySessions(REPLAY_HARNESS_ID);
      if (cancelled) return;

      setListState(
        result.ok
          ? { kind: 'ready', sessions: result.data }
          : { kind: 'error', message: result.error },
      );
    })();

    return () => {
      cancelled = true;
    };
  }, [harnessState]);

  function selectSession(summary: ReplaySessionSummary): void {
    const seq = ++requestSeq.current;
    setSelectedId(summary.sessionId);
    setDetailState({ kind: 'loading' });

    void (async () => {
      const result = await readReplaySession(REPLAY_HARNESS_ID, summary.sessionId);
      if (seq !== requestSeq.current) return;

      if (!result.ok) {
        setDetailState({ kind: 'error', message: result.error });
      } else if (result.data === null) {
        setDetailState({ kind: 'missing' });
      } else {
        setDetailState({ kind: 'ready', session: result.data });
      }
    })();
  }

  return (
    <div>
      <PageHeader
        title="Replay"
        description="只读回放外部 Harness 的会话转录：左侧为发现列表（不落库），点开查看消息流。本页没有任何写操作。"
      />

      {harnessState.kind === 'loading' ? (
        <Card>
          <CardTitle>正在读取 Harness 能力矩阵…</CardTitle>
        </Card>
      ) : null}

      {harnessState.kind === 'ipc-unavailable' ? (
        <Card>
          <CardTitle>IPC 不可用（浏览器模式）</CardTitle>
          <CardDescription>
            请用 pnpm tauri dev 启动桌面应用。会话转录只存在于本机 Harness 数据目录，
            浏览器模式读不到任何数据。
          </CardDescription>
        </Card>
      ) : null}

      {harnessState.kind === 'error' ? (
        <Card className="border-signal-error/40">
          <CardTitle>读取 Harness 能力矩阵失败</CardTitle>
          <CardDescription>IPC 错误：{harnessState.message}</CardDescription>
        </Card>
      ) : null}

      {harnessState.kind === 'no-replay-capability' ? (
        <Card>
          <CardTitle>还没有支持回放的 Harness</CardTitle>
          <CardDescription>
            回放按能力矩阵门控：只有 capabilities.replay = true 的 Harness 才会出现会话列表。
            当前已注册的 Harness 都未实现回放解析（B6 仅 Claude Code
            实现，其余按同一契约逐步接入）。
          </CardDescription>
        </Card>
      ) : null}

      {harnessState.kind === 'ready' ? (
        <div className="grid items-start gap-4 lg:grid-cols-[minmax(280px,340px)_minmax(0,1fr)]">
          <nav aria-label="可回放会话列表" className="space-y-2">
            <div className="flex items-baseline justify-between">
              <p className="text-[13px] font-medium text-content-primary">Claude Code · 会话列表</p>
              {listState.kind === 'ready' ? (
                <p className="text-[11px] text-content-muted">{listState.sessions.length} 条</p>
              ) : null}
            </div>

            {listState.kind === 'loading' ? (
              <Card>
                <CardDescription>正在扫描会话转录…</CardDescription>
              </Card>
            ) : null}

            {listState.kind === 'error' ? (
              <Card className="border-signal-error/40">
                <CardTitle>读取会话列表失败</CardTitle>
                <CardDescription>IPC 错误：{listState.message}</CardDescription>
              </Card>
            ) : null}

            {listState.kind === 'ready' && listState.sessions.length === 0 ? (
              <Card>
                <CardDescription>
                  本机没有发现可回放的 Claude 会话（数据目录下没有会话转录文件）。
                </CardDescription>
              </Card>
            ) : null}

            {listState.kind === 'ready' && listState.sessions.length > 0 ? (
              <ul className="space-y-2">
                {listState.sessions.map((summary) => (
                  <li key={summary.sessionId}>
                    <SessionListItem
                      summary={summary}
                      selected={selectedId === summary.sessionId}
                      onSelect={() => selectSession(summary)}
                    />
                  </li>
                ))}
              </ul>
            ) : null}
          </nav>

          <section aria-label="回放时间线" aria-live="polite">
            {detailState.kind === 'idle' ? (
              <Card>
                <CardTitle>从左侧选择一个会话开始回放</CardTitle>
                <CardDescription>
                  时间线只读渲染 user / assistant 消息；thinking 渲染为「思考」标注区，
                  子代理（sidechain）消息带徽标，未展开的内容种类在消息卡脚注计数。
                </CardDescription>
              </Card>
            ) : null}

            {detailState.kind === 'loading' ? (
              <Card>
                <CardTitle>正在读取会话转录…</CardTitle>
              </Card>
            ) : null}

            {detailState.kind === 'error' ? (
              <Card className="border-signal-error/40">
                <CardTitle>读取失败</CardTitle>
                <CardDescription>IPC 错误：{detailState.message}</CardDescription>
              </Card>
            ) : null}

            {detailState.kind === 'missing' ? (
              <Card>
                <CardTitle>会话已不在发现列表中</CardTitle>
                <CardDescription>
                  后端没有命中这条 session id：转录文件可能已被移动或清理。刷新页面重新发现即可。
                </CardDescription>
              </Card>
            ) : null}

            {detailState.kind === 'ready' ? (
              <SessionTimeline session={detailState.session} />
            ) : null}
          </section>
        </div>
      ) : null}
    </div>
  );
}

function SessionListItem({
  summary,
  selected,
  onSelect,
}: {
  summary: ReplaySessionSummary;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={selected}
      className={cn(
        'w-full rounded-lg border px-3 py-2.5 text-left transition-colors duration-150',
        selected
          ? 'border-accent/50 bg-accent/10'
          : 'border-border-subtle bg-surface-raised/30 hover:bg-white/[0.05]',
      )}
    >
      <p className="truncate font-mono text-xs font-medium text-content-primary">
        {summary.projectDir}
      </p>
      <p className="mt-1 line-clamp-2 text-[11px] text-content-secondary">
        {summary.firstUserPreview ?? '（无首条消息预览）'}
      </p>
      <p className="mt-1.5 text-[10px] text-content-muted">
        {formatTimestamp(summary.startedAt ?? '')} ~ {formatTimestamp(summary.lastAt ?? '')} ·{' '}
        {formatBytes(summary.sizeBytes)}
      </p>
    </button>
  );
}

function SessionTimeline({ session }: { session: ReplaySession }) {
  return (
    <Card>
      <CardTitle className="break-all font-mono text-xs">{session.sessionId}</CardTitle>
      <CardDescription className="break-all font-mono">{session.sourcePath}</CardDescription>
      <p className="mt-1 text-[11px] text-content-muted">
        {session.messages.length} 条消息 · 只读回放，不写任何数据
      </p>

      {session.messages.length === 0 ? (
        <div className="mt-4 rounded-md border border-border-subtle bg-white/[0.03] px-3 py-4 text-center">
          <p className="text-xs text-content-secondary">这条会话没有可回放的消息</p>
          <p className="mt-1 text-[11px] text-content-muted">
            转录里没有解析出任何 user / assistant 行；其余行类型只计入下方脚注。
          </p>
        </div>
      ) : (
        <div className="mt-4 flex flex-col gap-3">
          {session.messages.map((message) => (
            <MessageCard key={message.seq} message={message} />
          ))}
        </div>
      )}

      <p className="mt-4 border-t border-border-subtle pt-3 text-[11px] text-content-muted">
        {skippedFooterText(session)}
      </p>
    </Card>
  );
}

function MessageCard({ message }: { message: ReplayMessage }) {
  const isUser = message.role === 'user';
  const unsupportedEntries = Object.entries(message.unsupportedContentKinds);

  return (
    <article
      aria-label={isUser ? '用户消息' : '助手消息'}
      className={cn(
        'max-w-[85%] rounded-lg border bg-surface-raised/45 p-3',
        // user 靠左、assistant 靠右，边框色再作一层区分。
        isUser ? 'mr-auto border-accent/30' : 'ml-auto border-signal-ok/25',
      )}
    >
      <header className="flex flex-wrap items-center gap-2 text-[11px] text-content-muted">
        <span className={cn('font-medium', isUser ? 'text-accent' : 'text-content-secondary')}>
          {isUser ? '用户' : '助手'}
        </span>
        <span>#{message.seq}</span>
        {message.sidechain ? (
          <span className="rounded border border-signal-warn/40 bg-signal-warn/10 px-1.5 py-0.5 font-medium text-signal-warn">
            子代理
          </span>
        ) : null}
        {message.model !== null ? <span>{message.model}</span> : null}
        <span>{formatTimestamp(message.timestamp ?? '')}</span>
      </header>

      {message.thinkingParts.length > 0 ? (
        <div className="mt-2 rounded-md border border-border-subtle bg-white/[0.03] px-2.5 py-2">
          <p className="text-[10px] font-medium uppercase tracking-wide text-content-muted">思考</p>
          {message.thinkingParts.map((part, index) => (
            <p
              key={index}
              className="mt-1 whitespace-pre-wrap text-[11px] italic leading-relaxed text-content-muted"
            >
              {part}
            </p>
          ))}
        </div>
      ) : null}

      {message.textParts.map((part, index) => (
        <p
          key={index}
          className="mt-2 whitespace-pre-wrap text-xs leading-relaxed text-content-primary"
        >
          {part}
        </p>
      ))}

      {unsupportedEntries.length > 0 ? (
        <p className="mt-2 border-t border-border-subtle pt-2 text-[10px] text-content-muted">
          {`未展开内容：${unsupportedEntries
            .map(([kind, count]) => `${kind} ×${count}`)
            .join('、')}`}
        </p>
      ) : null}
    </article>
  );
}

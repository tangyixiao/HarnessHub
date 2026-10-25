import { useEffect, useRef, useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import { cn } from '@/lib/utils';
import {
  NOT_IN_TAURI,
  getReplaySearchSettings,
  listHarnesses,
  listReplaySessions,
  readReplaySession,
  reindexReplaySearch,
  searchReplay,
  setReplaySearchSettings,
  type ReindexReport,
  type ReplayMessage,
  type ReplaySearchHit,
  type ReplaySession,
  type ReplaySessionSummary,
  type ReplayToolCall,
  type SearchSettings,
  type SubagentThread,
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
 * 会话回放页（ROADMAP B6 回放 + B7 搜索）。
 *
 * 会话转录本身**全部只读**：发现列表由后端扫描转录文件（有界预读），正文按
 * session_id 按需现读、不落库；时间线没有任何写操作按钮，也不把 `sourcePath`
 * 传回后端（路径白名单在后端，前端只持有 session id）。
 * 唯一的写路径是下方「搜索与索引」区：索引开关 / 排除目录写本地 SQLite
 * 设置行，重建索引写本地索引表 —— 都是显式用户动作，只碰本地数据库。
 */
export function ReplayPage() {
  const [harnessState, setHarnessState] = useState<HarnessState>({ kind: 'loading' });
  const [listState, setListState] = useState<ListState>({ kind: 'loading' });
  const [detailState, setDetailState] = useState<DetailState>({ kind: 'idle' });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // 点击竞态：慢请求返回时若已有更新的点击，直接丢弃（最后一次点击胜出）。
  const requestSeq = useRef(0);
  // 搜索命中跳转用：sessionId → 列表项节点，命中时滚动定位。
  const listItemRefs = useRef(new Map<string, HTMLElement>());

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

  /**
   * 搜索命中跳转：命中的会话已渲染在左侧列表时选中并滚动过去；
   * 不在列表里（列表还没加载完 / 该会话未被发现）就只停留在结果展示，
   * 不假装跳转 —— 时间线需要 transcript 正文，命中本身给不了。
   */
  function selectSessionFromHit(sessionId: string): void {
    if (listState.kind !== 'ready') return;
    const summary = listState.sessions.find((session) => session.sessionId === sessionId);
    if (!summary) return;

    selectSession(summary);
    listItemRefs.current.get(sessionId)?.scrollIntoView?.({ block: 'nearest' });
  }

  return (
    <div>
      <PageHeader
        title="Replay"
        description="只读回放外部 Harness 的会话转录：左侧为发现列表（不落库），点开查看消息流。搜索索引的开关与重建会写本地数据库（见「搜索与索引」）。"
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
        <>
          {/* B7：搜索与索引区，与会话列表同一能力门控（replay 就绪才渲染）。 */}
          <div className="mb-4">
            <ReplaySearchPanel onSelectSession={selectSessionFromHit} />
          </div>

          <div className="grid items-start gap-4 lg:grid-cols-[minmax(280px,340px)_minmax(0,1fr)]">
            <nav aria-label="可回放会话列表" className="space-y-2">
              <div className="flex items-baseline justify-between">
                <p className="text-[13px] font-medium text-content-primary">
                  Claude Code · 会话列表
                </p>
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
                    <li
                      key={summary.sessionId}
                      ref={(node) => {
                        if (node) {
                          listItemRefs.current.set(summary.sessionId, node);
                        } else {
                          listItemRefs.current.delete(summary.sessionId);
                        }
                      }}
                    >
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
                    工具调用展开为等宽工具卡，子代理线程列在时间线下方，
                    仍未展开的内容种类在消息卡脚注计数。
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
                // key = sessionId：切会话强制重挂载，消息引用映射与线程高亮
                // 随组件一起重建，不会把上一会话的状态带进新会话。
                <SessionTimeline
                  key={detailState.session.sessionId}
                  session={detailState.session}
                />
              ) : null}
            </section>
          </div>
        </>
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
  // 线程跳转用：seq → 消息卡外层节点。ref 回调随消息卡挂载 / 卸载自动增删；
  // 组件以 sessionId 为 key 重挂载（见调用处），映射不会残留上一个会话的节点。
  const messageRefs = useRef(new Map<number, HTMLElement>());
  // 点击线程后高亮的消息 seq。
  const [highlightedSeqs, setHighlightedSeqs] = useState<ReadonlySet<number>>(new Set());

  /**
   * 线程跳转（尽力而为）：按 seq 找已渲染的消息卡，滚动到第一条并高亮全部
   * 找得到的；一条都找不到（消息被 normalize 丢弃等）就原地不动，绝不报错。
   */
  function focusThread(thread: SubagentThread): void {
    const found = new Set<number>();
    let firstNode: HTMLElement | null = null;
    for (const seq of thread.messageSeqs) {
      const node = messageRefs.current.get(seq);
      if (node) {
        found.add(seq);
        firstNode ??= node;
      }
    }
    firstNode?.scrollIntoView?.({ block: 'nearest' });
    setHighlightedSeqs(found);
  }

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
            <div
              key={message.seq}
              ref={(node) => {
                if (node) {
                  messageRefs.current.set(message.seq, node);
                } else {
                  messageRefs.current.delete(message.seq);
                }
              }}
            >
              <MessageCard message={message} highlighted={highlightedSeqs.has(message.seq)} />
            </div>
          ))}
        </div>
      )}

      {session.subagentThreads.length > 0 ? (
        <section aria-label="子代理线程" className="mt-4 border-t border-border-subtle pt-3">
          <p className="text-[11px] font-medium text-content-secondary">
            子代理线程（{session.subagentThreads.length}）
          </p>
          <ul className="mt-2 space-y-1.5">
            {session.subagentThreads.map((thread, index) => {
              const seqList = thread.messageSeqs.map((seq) => `#${seq}`).join('、');
              const times = `${formatTimestamp(thread.startedAt ?? '')} ~ ${formatTimestamp(
                thread.lastAt ?? '',
              )}`;
              return (
                <li key={`${index}-${thread.messageSeqs.join('-')}`}>
                  <button
                    type="button"
                    onClick={() => focusThread(thread)}
                    className="w-full rounded-md border border-border-subtle bg-surface-raised/30 px-2.5 py-2 text-left text-[11px] text-content-secondary transition-colors duration-150 hover:bg-white/[0.05]"
                  >
                    <span className="font-medium text-content-primary">线程 {index + 1}</span>
                    {` · ${seqList} · ${times}`}
                  </button>
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}

      <p className="mt-4 border-t border-border-subtle pt-3 text-[11px] text-content-muted">
        {skippedFooterText(session)}
      </p>
    </Card>
  );
}

function MessageCard({
  message,
  highlighted = false,
}: {
  message: ReplayMessage;
  /** 被子代理线程点击命中：边框与外圈同时提亮，一眼可见（尽力而为高亮）。 */
  highlighted?: boolean;
}) {
  const isUser = message.role === 'user';
  const unsupportedEntries = Object.entries(message.unsupportedContentKinds);

  return (
    <article
      aria-label={isUser ? '用户消息' : '助手消息'}
      className={cn(
        'max-w-[85%] rounded-lg border bg-surface-raised/45 p-3',
        // user 靠左、assistant 靠右，边框色再作一层区分。
        isUser ? 'mr-auto border-accent/30' : 'ml-auto border-signal-ok/25',
        highlighted && 'border-accent/60 ring-1 ring-accent/40',
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

      {message.toolCalls.length > 0 ? (
        <ul aria-label="工具调用" className="mt-2 space-y-1.5">
          {message.toolCalls.map((call) => (
            <ToolCallCard key={call.id} call={call} />
          ))}
        </ul>
      ) : null}

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

/**
 * 一条工具调用卡：等宽字体渲染工具名 + 原样 inputJson；转录里有对应
 * tool_result 时缩进展示「→ 结果」。截断必须显式标注 —— 后端截断到 2 KiB
 * 是有界渲染的纪律，把残缺 JSON 当完整内容展示就是在说谎。
 */
function ToolCallCard({ call }: { call: ReplayToolCall }) {
  return (
    <li className="rounded-md border border-border-subtle bg-white/[0.03] px-2.5 py-2">
      <p className="font-mono text-[11px] leading-relaxed text-content-primary">
        <span className="font-medium">{call.name}</span>
        {call.inputTruncated ? (
          <span className="ml-2 text-[10px] text-signal-warn">输入已截断</span>
        ) : null}
      </p>
      <pre className="mt-1 whitespace-pre-wrap break-all font-mono text-[11px] leading-relaxed text-content-secondary">
        {call.inputJson}
      </pre>
      {call.resultJson !== null ? (
        <div className="mt-1.5 border-l-2 border-border-subtle pl-2.5">
          <p className="text-[10px] text-content-muted">
            {'→ 结果'}
            {call.resultTruncated ? (
              <span className="ml-2 text-signal-warn">结果已截断</span>
            ) : null}
          </p>
          <pre className="mt-1 whitespace-pre-wrap break-all font-mono text-[11px] leading-relaxed text-content-secondary">
            {call.resultJson}
          </pre>
        </div>
      ) : null}
    </li>
  );
}

/* ------------------------------------------------------------------ *
 * 搜索与索引（ROADMAP B7）
 * ------------------------------------------------------------------ */

/**
 * 后端（src-tauri/src/replay/index.rs::search）对两类输入直接 reject：
 * 错误串带「未启用」（索引被关闭）或「3 个字符」（trigram 分词器的下限）。
 * 它们是使用方式问题，不是故障 —— 渲染成行内提示，绝不套「搜索失败」错误卡。
 */
const SEARCH_HINT_PATTERNS: readonly string[] = ['未启用', '3 个字符'];

function isSearchUsageHint(error: string): boolean {
  return SEARCH_HINT_PATTERNS.some((pattern) => error.includes(pattern));
}

/** reindex 报告的一行审计文本，字段顺序与 Rust `ReindexReport` 一致。 */
function reindexReportText(report: ReindexReport): string {
  return `扫描 ${report.scanned} · 索引 ${report.indexed} · 未变 ${report.unchanged} · 排除 ${report.excluded} · 清理 ${report.removed}`;
}

type SettingsSaveState =
  { kind: 'idle' } | { kind: 'saving' } | { kind: 'failed'; message: string };

type ReindexState =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'done'; report: ReindexReport }
  | { kind: 'failed'; message: string };

type SearchState =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'hint'; message: string }
  | { kind: 'failed'; message: string }
  | { kind: 'done'; hits: ReplaySearchHit[] };

/**
 * 「搜索与索引」区（ROADMAP B7）。
 *
 * 隐私文案纪律：开关旁必须注明索引只存本地数据库、可随时关闭清除；
 * 绝不出现「上传 / 云端」类表述。开关与排除目录写本地设置行
 * （replay_search_settings_set）；重建索引是显式用户动作，不自动触发。
 */
function ReplaySearchPanel({ onSelectSession }: { onSelectSession: (sessionId: string) => void }) {
  const [settings, setSettings] = useState<SearchSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // 排除目录输入框是草稿：失焦 / 点击保存才提交；切换开关沿用已提交的名单。
  const [excludedDraft, setExcludedDraft] = useState('');
  const [saveState, setSaveState] = useState<SettingsSaveState>({ kind: 'idle' });
  const [reindexState, setReindexState] = useState<ReindexState>({ kind: 'idle' });
  const [query, setQuery] = useState('');
  const [searchState, setSearchState] = useState<SearchState>({ kind: 'idle' });

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const result = await getReplaySearchSettings();
      if (cancelled) return;

      if (!result.ok) {
        setLoadError(result.error);
        return;
      }
      setSettings(result.data);
      setExcludedDraft(result.data.excludedDirs.join(', '));
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  const enabled = settings?.enabled ?? false;
  const saving = saveState.kind === 'saving';

  /** 写设置行。失败时保留旧状态与错误提示，不静默吞掉。 */
  async function saveSettings(next: SearchSettings): Promise<boolean> {
    setSaveState({ kind: 'saving' });
    const result = await setReplaySearchSettings(next);
    if (!result.ok) {
      setSaveState({ kind: 'failed', message: result.error });
      return false;
    }
    setSaveState({ kind: 'idle' });
    setSettings(next);
    return true;
  }

  function toggleEnabled(next: boolean): void {
    if (!settings || saving) return;
    void saveSettings({ ...settings, enabled: next });
  }

  function commitExcludedDirs(): void {
    if (!settings || saving) return;
    const excludedDirs = excludedDraft
      .split(',')
      .map((item) => item.trim())
      .filter((item) => item.length > 0);

    void saveSettings({ ...settings, excludedDirs }).then((saved) => {
      if (saved) setExcludedDraft(excludedDirs.join(', '));
    });
  }

  function runReindex(): void {
    if (!enabled || reindexState.kind === 'running') return;
    setReindexState({ kind: 'running' });

    void (async () => {
      const result = await reindexReplaySearch();
      setReindexState(
        result.ok
          ? { kind: 'done', report: result.data }
          : { kind: 'failed', message: result.error },
      );
    })();
  }

  function runSearch(): void {
    const trimmed = query.trim();
    if (trimmed.length === 0 || searchState.kind === 'running') return;
    setSearchState({ kind: 'running' });

    void (async () => {
      const result = await searchReplay(trimmed);
      if (result.ok) {
        setSearchState({ kind: 'done', hits: result.data });
        return;
      }
      setSearchState(
        isSearchUsageHint(result.error)
          ? { kind: 'hint', message: result.error }
          : { kind: 'failed', message: result.error },
      );
    })();
  }

  const searchLocked = !enabled || searchState.kind === 'running';

  return (
    <Card>
      <CardTitle>搜索与索引</CardTitle>
      <CardDescription>
        在已索引的会话转录里全文搜索（当前仅支持 Claude Code）。命中词在片段里用 «» 标出。
      </CardDescription>

      {loadError !== null ? (
        <p role="alert" className="mt-3 text-xs text-red-400">
          读取索引设置失败：{loadError}
        </p>
      ) : null}

      {loadError === null && settings === null ? (
        <p className="mt-3 text-xs text-content-muted">正在读取索引设置…</p>
      ) : null}

      {settings !== null ? (
        <div className="mt-4 space-y-5">
          <div>
            <label className="flex items-start gap-2.5">
              <input
                type="checkbox"
                checked={enabled}
                disabled={saving}
                onChange={(event) => toggleEnabled(event.target.checked)}
                className="mt-0.5 size-4 shrink-0 accent-accent"
              />
              <span>
                <span className="block text-xs font-medium text-content-primary">启用回放索引</span>
                {/* 隐私文案：本地数据库、可关闭清除。不出现「上传 / 云端」。 */}
                <span className="mt-0.5 block text-[11px] text-content-muted">
                  索引内容保存在本地数据库，可随时关闭清除；关闭会立即清空已索引内容。
                </span>
              </span>
            </label>

            {!enabled ? (
              <p className="mt-2 text-[11px] text-content-secondary">
                索引已关闭：打开并重建索引后才能搜索
              </p>
            ) : null}

            {saveState.kind === 'failed' ? (
              <p role="alert" className="mt-2 text-xs text-red-400">
                保存设置失败：{saveState.message}
              </p>
            ) : null}
          </div>

          <div>
            <label
              htmlFor="replay-search-excluded-dirs"
              className="block text-xs font-medium text-content-primary"
            >
              排除目录
            </label>
            <div className="mt-1.5 flex gap-3">
              <input
                id="replay-search-excluded-dirs"
                type="text"
                value={excludedDraft}
                onChange={(event) => setExcludedDraft(event.target.value)}
                onBlur={() => commitExcludedDirs()}
                placeholder="项目目录名，逗号分隔，例如 D--work-beta"
                disabled={saving}
                className="min-w-0 flex-1 rounded-md border border-white/10 bg-white/[0.04] px-3 py-2 font-mono text-xs text-content-primary outline-none placeholder:text-content-muted focus:border-accent/60 disabled:opacity-50"
              />
              <Button variant="secondary" onClick={() => commitExcludedDirs()} disabled={saving}>
                保存排除目录
              </Button>
            </div>
            <p className="mt-1.5 text-[11px] text-content-muted">
              失焦或点击保存后生效；收紧名单时后端立即清除对应会话的索引行。
            </p>
          </div>

          <div>
            <Button
              variant="secondary"
              onClick={() => runReindex()}
              disabled={!enabled || reindexState.kind === 'running'}
            >
              {reindexState.kind === 'running' ? '正在重建索引…' : '重建索引'}
            </Button>
            {reindexState.kind === 'done' ? (
              <p aria-live="polite" className="mt-2 text-[11px] text-content-secondary">
                {reindexReportText(reindexState.report)}
              </p>
            ) : null}
            {reindexState.kind === 'failed' ? (
              <p role="alert" className="mt-2 text-xs text-red-400">
                重建索引失败：{reindexState.message}
              </p>
            ) : null}
          </div>

          <div>
            <form
              onSubmit={(event) => {
                event.preventDefault();
                runSearch();
              }}
              className="flex gap-3"
            >
              <input
                type="search"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="搜索会话内容（至少 3 个字符）"
                aria-label="搜索已索引的会话内容"
                disabled={searchLocked}
                className="min-w-0 flex-1 rounded-md border border-white/10 bg-white/[0.04] px-3 py-2 text-xs text-content-primary outline-none placeholder:text-content-muted focus:border-accent/60 disabled:opacity-50"
              />
              <Button type="submit" disabled={searchLocked}>
                搜索
              </Button>
            </form>

            {searchState.kind === 'hint' ? (
              <p className="mt-2 text-[11px] text-content-secondary">{searchState.message}</p>
            ) : null}
            {searchState.kind === 'failed' ? (
              <p role="alert" className="mt-2 text-xs text-red-400">
                搜索失败：{searchState.message}
              </p>
            ) : null}
            {searchState.kind === 'done' ? (
              searchState.hits.length === 0 ? (
                <p className="mt-2 text-xs text-content-secondary">没有匹配的索引内容</p>
              ) : (
                <ul aria-label="搜索结果" className="mt-3 space-y-2">
                  {searchState.hits.map((hit) => (
                    <li key={`${hit.sessionId}-${hit.seq}`}>
                      <SearchHitItem hit={hit} onSelect={() => onSelectSession(hit.sessionId)} />
                    </li>
                  ))}
                </ul>
              )
            ) : null}
          </div>
        </div>
      ) : null}
    </Card>
  );
}

/**
 * 一条搜索命中：snippet 原样渲染（«» 标记不解释、不转义），副标题给足定位
 * 信息（角色 · harness · 会话 id · seq）。整条可点击：由父级决定能否跳到
 * 左侧列表（会话已渲染才滚动选中，否则仅展示）。
 */
function SearchHitItem({ hit, onSelect }: { hit: ReplaySearchHit; onSelect: () => void }) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className="w-full rounded-lg border border-border-subtle bg-surface-raised/30 px-3 py-2.5 text-left transition-colors duration-150 hover:bg-white/[0.05]"
    >
      <p className="whitespace-pre-wrap text-xs leading-relaxed text-content-primary">
        {hit.snippet}
      </p>
      <p className="mt-1.5 text-[10px] text-content-muted">
        {hit.role === 'assistant' ? '助手' : '用户'} · {hit.harnessId} · 会话 {hit.sessionId} · #
        {hit.seq}
      </p>
    </button>
  );
}

import { CalendarDays, Clock, History, Zap } from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';

import { harnessIdentity } from '@/lib/harness-colors';
import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import {
  NOT_IN_TAURI,
  getActivityTimeline,
  type ActivityDayBucket,
  type ActivityEvent,
  type ActivityTimeline,
  type UsageRangeKind,
} from '@/lib/ipc';

const RANGES: { kind: UsageRangeKind; label: string }[] = [
  { kind: 'today', label: '今天' },
  { kind: '7d', label: '7 天' },
  { kind: '30d', label: '30 天' },
  { kind: 'all', label: '全部' },
];

const WEEKDAYS = ['周一', '周二', '周三', '周四', '周五', '周六', '周日'];

/** 展示的会话事件上限（后端 `hasMore` 会告知是否还有更多）。 */
const EVENT_LIMIT = 60;

/**
 * Activity = **SQLite 的只读投影**（ADR-0012）。
 *
 * - mount / 切换范围只调用 `activity_timeline`（纯读，不起任何外部进程）；
 * - 「≥」的判定来自后端，前端只负责格式化；
 * - 这里**不展示任何 git diff / commit**：v0.1 还没有 Git 观察器，
 *   而且按 CONTEXT.md 核心约束 7，时间窗口内的代码变化不能归因为 AI 产出。
 */
export function ActivityPage() {
  const [range, setRange] = useState<UsageRangeKind>('30d');
  const [timeline, setTimeline] = useState<ActivityTimeline | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedDay, setSelectedDay] = useState<string | null>(null);

  const loadTimeline = useCallback(async (next: UsageRangeKind) => {
    const result = await getActivityTimeline({
      range: next,
      limit: EVENT_LIMIT,
    });
    if (result.ok) {
      setTimeline(result.data);
      setError(null);
    } else {
      // 失败时**保留**上一次的数字：清零会让人以为「活动消失了」。
      setError(result.error);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadTimeline(range));
  }, [range, loadTimeline]);

  const days = useMemo(() => buildCalendar(timeline?.days ?? []), [timeline]);

  // 选中某一天后，如果范围切换导致那一天从聚合里消失，选择自动失效
  //（不做成 effect：在渲染期间归一化，避免 setState-in-effect）。
  const effectiveSelectedDay =
    selectedDay !== null && days.some((cell) => cell.day === selectedDay) ? selectedDay : null;

  const dayEvents = useMemo(() => {
    const events = timeline?.events ?? [];
    if (effectiveSelectedDay === null) return events;
    return events.filter((event) => event.day === effectiveSelectedDay);
  }, [effectiveSelectedDay, timeline]);

  const isEmpty =
    timeline !== null && timeline.usageEventCount === 0 && timeline.managedSessionCount === 0;

  return (
    <div>
      <PageHeader
        title="Activity"
        description="Coding Activity Timeline：按本地日汇总用量与会话，点一天展开当天的事件流。"
      />

      <div className="mb-4 flex flex-wrap items-center gap-2">
        <div className="inline-flex items-center gap-0.5 rounded-md border border-border-subtle bg-white/[0.03] p-0.5">
          {RANGES.map((item) => (
            <Button
              key={item.kind}
              size="sm"
              variant={item.kind === range ? 'secondary' : 'ghost'}
              aria-pressed={item.kind === range}
              className={item.kind === range ? 'bg-white/[0.08] text-content-primary' : ''}
              onClick={() => setRange(item.kind)}
            >
              {item.label}
            </Button>
          ))}
        </div>
      </div>

      {error ? (
        <p className="mb-4 text-xs text-danger" role="alert">
          {error === NOT_IN_TAURI
            ? 'IPC 不可用（浏览器模式）。请用 pnpm tauri dev 启动桌面应用。'
            : `错误：${error}`}
        </p>
      ) : null}

      <div className="grid gap-4 lg:grid-cols-4">
        <KpiCard
          testId="kpi-active-days"
          title="Active Days"
          icon={CalendarDays}
          value={timeline ? String(timeline.activeDays) : '—'}
          hint="这个范围内有活动记录的本地日"
        />
        <KpiCard
          testId="kpi-tokens"
          title="Tokens"
          icon={Zap}
          value={timeline ? formatTokens(timeline.totalTokens) : '—'}
          hint="usage 事件的 token 合计"
        />
        <KpiCard
          testId="kpi-usage-sessions"
          title="Usage Events"
          icon={History}
          value={timeline ? formatTokens(timeline.usageEventCount) : '—'}
          hint="来自外部用量历史的记录（不代表 Harness Hub 启动过）"
        />
        <KpiCard
          testId="kpi-managed-sessions"
          title="Managed Sessions"
          icon={Clock}
          value={timeline ? String(timeline.managedSessionCount) : '—'}
          hint="Harness Hub 自己启动并管理过的会话"
        />
      </div>

      {isEmpty ? (
        <Card className="mt-4">
          <CardTitle>还没有活动数据</CardTitle>
          <CardDescription>
            这个时间范围内没有任何已导入的用量记录，也没有 Harness Hub
            管理过的会话。数据不会自动拉取：需要时到 Dashboard 点「刷新用量」， 它才会调用 ccusage
            并把结果写入本机数据库（本页渲染永远只读数据库）。
          </CardDescription>
        </Card>
      ) : null}

      <div className="mt-4 grid gap-4 lg:grid-cols-2">
        <Card>
          <CardTitle>Activity Calendar</CardTitle>
          <CardDescription>
            按<span className="font-medium">本地日</span>（时区{' '}
            {timeline?.range.timezoneOffsetMinutes ?? 0} 分钟）汇总，颜色深浅表示 token
            规模；点一天展开事件。
          </CardDescription>
          {days.length > 0 ? (
            <CalendarGrid
              days={days}
              selectedDay={effectiveSelectedDay}
              onSelect={setSelectedDay}
            />
          ) : (
            <p className="mt-4 text-xs text-content-muted">这个范围内没有按天分布的数据。</p>
          )}
        </Card>

        <Card>
          <CardTitle>按小时分布</CardTitle>
          <CardDescription>
            本地小时 0..23 全覆盖（UTC {timeline?.range.timezoneOffsetMinutes ?? 0} 分钟偏移）。
          </CardDescription>
          <HourChart hours={timeline?.hours ?? []} max={timeline?.maxHourTokens ?? 0} />
        </Card>
      </div>

      <Card className="mt-4">
        <CardTitle>Session Timeline</CardTitle>
        <CardDescription>
          {effectiveSelectedDay === null
            ? '倒序事件流（usage 记录 + Harness Hub 管理的会话）。点日历里的一天只看那天。'
            : `${effectiveSelectedDay} 的事件。`}
        </CardDescription>
        {timeline && timeline.hasMore ? (
          <p className="mt-2 text-[11px] text-content-muted">
            事件流最多显示 {EVENT_LIMIT} 条（还有更多；按天 / 按小时的聚合不受此限制）。
          </p>
        ) : null}
        {dayEvents.length > 0 ? (
          <ul className="mt-4 space-y-2 text-xs" data-testid="activity-events">
            {dayEvents.map((event) => (
              <EventRow key={eventKey(event)} event={event} />
            ))}
          </ul>
        ) : (
          <p className="mt-4 text-xs text-content-muted">
            {effectiveSelectedDay === null ? '还没有事件。' : '这一天没有事件流记录。'}
          </p>
        )}
        {timeline?.days.length ? <DayTable days={timeline.days} /> : null}
      </Card>

      <Card className="mt-4">
        <CardTitle>范围与边界</CardTitle>
        <CardDescription>
          本页只读本机 SQLite，不调用任何外部进程、不展示 git diff ——时间窗口内的代码变化不能归因为
          AI 产出（CONTEXT.md 核心约束 7）。
        </CardDescription>
        <dl className="mt-4 grid grid-cols-2 gap-3 lg:grid-cols-4">
          <Metric label="时间范围" value={rangeLabel(range)} />
          <Metric label="窗口起点 (UTC)" value={timeline?.range.startUtc ?? '不限'} />
          <Metric label="窗口终点 (UTC)" value={timeline?.range.endUtc ?? '不限'} />
          <Metric label="查询时刻 (UTC)" value={timeline?.range.nowUtc ?? '—'} />
        </dl>
      </Card>
    </div>
  );
}

interface CalendarCell {
  day: string;
  bucket: ActivityDayBucket | null;
  weekday: string;
}

function buildCalendar(buckets: ActivityDayBucket[]): CalendarCell[] {
  return buckets.map((bucket) => {
    // `day` 是后端已经本地化过的 `YYYY-MM-DD`，这里只算星期。
    const date = new Date(`${bucket.day}T00:00:00Z`);
    const weekday = Number.isNaN(date.getTime())
      ? ''
      : (WEEKDAYS[(date.getUTCDay() + 6) % 7] ?? '');
    return { day: bucket.day, bucket, weekday };
  });
}

function CalendarGrid({
  days,
  selectedDay,
  onSelect,
}: {
  days: CalendarCell[];
  selectedDay: string | null;
  onSelect: (day: string | null) => void;
}) {
  return (
    <div className="mt-4">
      <div className="grid grid-cols-[repeat(auto-fill,minmax(3.25rem,1fr))] gap-1.5">
        {days.map((cell) => (
          <button
            key={cell.day}
            type="button"
            aria-pressed={cell.day === selectedDay}
            onClick={() => onSelect(cell.day === selectedDay ? null : cell.day)}
            title={`${cell.day} ${cell.weekday} · ${formatTokens(cell.bucket?.totalTokens ?? 0)} tokens · ${cell.bucket?.usageEventCount ?? 0} 条用量 · ${cell.bucket?.managedSessionCount ?? 0} 个会话`}
            className={[
              'flex h-14 flex-col justify-between rounded-md border p-1.5 text-left transition-colors',
              cell.day === selectedDay
                ? 'border-accent bg-accent/15'
                : 'border-border-subtle bg-white/[0.03] hover:bg-white/[0.06]',
            ].join(' ')}
            style={{
              backgroundColor:
                cell.day === selectedDay ? undefined : heatColor(cell.bucket?.totalTokens ?? 0),
            }}
          >
            <span className="font-mono text-[10px] text-content-muted">{cell.day.slice(5)}</span>
            <span className="text-[10px] tabular-nums text-content-secondary">
              {cell.bucket?.managedSessionCount ? `${cell.bucket.managedSessionCount} 会话` : ''}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

function HourChart({
  hours,
  max,
}: {
  hours: {
    hour: number;
    usageEventCount: number;
    managedSessionCount: number;
    totalTokens: number;
  }[];
  max: number;
}) {
  return (
    <div className="mt-4 flex h-32 items-end gap-1" data-testid="activity-hours">
      {hours.map((bucket) => (
        <span
          key={bucket.hour}
          className="flex-1 rounded-t-sm bg-accent/70"
          style={{ height: `${barWidth(bucket.totalTokens, max)}%`, minHeight: '2px' }}
          title={`${String(bucket.hour).padStart(2, '0')}:00 · ${formatTokens(bucket.totalTokens)} tokens · ${bucket.usageEventCount} 条用量 · ${bucket.managedSessionCount} 个会话`}
        />
      ))}
    </div>
  );
}

function DayTable({ days }: { days: ActivityDayBucket[] }) {
  const recent = [...days].reverse().slice(0, 14);
  return (
    <div className="mt-5 overflow-x-auto">
      <table className="w-full text-left text-xs">
        <thead className="text-content-muted">
          <tr>
            <th className="py-1 pr-3 font-medium">本地日</th>
            <th className="py-1 pr-3 font-medium">用量条数</th>
            <th className="py-1 pr-3 font-medium">会话数</th>
            <th className="py-1 pr-3 text-right font-medium">Tokens</th>
            <th className="py-1 text-right font-medium">成本</th>
          </tr>
        </thead>
        <tbody>
          {recent.map((bucket) => (
            <tr key={bucket.day} className="border-t border-border-subtle">
              <td className="py-1.5 pr-3 font-mono text-[11px]">{bucket.day}</td>
              <td className="py-1.5 pr-3 tabular-nums">{bucket.usageEventCount}</td>
              <td className="py-1.5 pr-3 tabular-nums">{bucket.managedSessionCount}</td>
              <td className="py-1.5 pr-3 text-right tabular-nums">
                {formatTokens(bucket.totalTokens)}
              </td>
              <td className="py-1.5 text-right tabular-nums">
                {formatCost(bucket.costMicrounits, bucket.costIsLowerBound)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function EventRow({ event }: { event: ActivityEvent }) {
  const identity = harnessIdentity(event.harnessId);
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md border border-border-subtle bg-white/[0.02] px-2.5 py-2">
      <span className="font-mono text-[11px] text-content-muted">
        {formatClock(event.occurredAt)}
      </span>
      <span
        className="rounded px-1.5 py-0.5 text-[10px]"
        style={{ color: identity.fg, backgroundColor: identity.bg }}
      >
        {event.harnessId || 'unknown'}
      </span>
      <span className="rounded bg-white/[0.05] px-1.5 py-0.5 text-[10px] text-content-secondary">
        {event.kind === 'usage' ? '用量' : '会话'}
      </span>
      {event.kind === 'usage' ? (
        <>
          <span className="text-content-secondary">{event.model ?? '未知模型'}</span>
          <span className="tabular-nums">{formatTokens(event.totalTokens)} tokens</span>
          <span className="tabular-nums text-content-muted">
            {formatCost(event.costMicrounits, event.costIsLowerBound)}
          </span>
          <span
            className="text-content-muted"
            title={`${event.totalTokens} tokens · ${event.costIsLowerBound ? '价格不完整，实际成本可能更高' : '价格完整'}`}
          >
            {event.costIsLowerBound ? '成本不完整' : ''}
          </span>
          {event.projectId ? (
            <span className="text-content-muted" title={event.projectId}>
              已关联项目
            </span>
          ) : null}
        </>
      ) : (
        <>
          <span className="text-content-secondary">
            {event.status === 'running' ? '运行中' : (event.status ?? '未知状态')}
          </span>
          <span className="text-content-muted">
            {event.launchMode === 'terminal' ? '终端启动' : (event.launchMode ?? '')}
          </span>
          {event.durationMs !== null ? (
            <span className="tabular-nums text-content-muted">
              {formatDuration(event.durationMs)}
            </span>
          ) : (
            <span className="text-content-muted">尚未结束</span>
          )}
          {event.cwd ? (
            <span className="truncate text-content-muted" title={event.cwd}>
              {event.cwd}
            </span>
          ) : null}
        </>
      )}
    </li>
  );
}

function KpiCard({
  testId,
  title,
  icon: Icon,
  value,
  hint,
}: {
  testId: string;
  title: string;
  icon: typeof Zap;
  value: string;
  hint?: string;
}) {
  return (
    <Card data-testid={testId}>
      <div className="flex items-center gap-2 text-content-muted">
        <Icon aria-hidden className="size-3.5" />
        <span className="text-xs">{title}</span>
      </div>
      <p className="mt-2 text-2xl font-semibold tabular-nums text-content-primary">{value}</p>
      {hint ? <p className="mt-1 text-[11px] text-content-muted">{hint}</p> : null}
    </Card>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt className="text-[11px] text-content-muted">{label}</dt>
      <dd className="mt-0.5 font-mono text-xs text-content-secondary">{value}</dd>
    </div>
  );
}

function eventKey(event: ActivityEvent): string {
  return `${event.kind}:${event.occurredAt}:${event.hubSessionId ?? event.sourceSessionId ?? ''}`;
}

function rangeLabel(range: UsageRangeKind): string {
  return RANGES.find((item) => item.kind === range)?.label ?? range;
}

/** 把 UTC RFC3339 显示成 `MM-DD HH:mm`（本地化由后端的 day/hour 负责）。 */
function formatClock(utc: string): string {
  const date = new Date(utc);
  if (Number.isNaN(date.getTime())) return utc;
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${pad(date.getUTCMonth() + 1)}-${pad(date.getUTCDate())} ${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())}`;
}

function formatDuration(ms: number): string {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  if (minutes < 60) return `${minutes} 分 ${rest} 秒`;
  const hours = Math.floor(minutes / 60);
  return `${hours} 时 ${minutes % 60} 分`;
}

function formatTokens(value: number): string {
  return value.toLocaleString('en-US');
}

function formatCost(microunits: number | null, lowerBound: boolean): string {
  if (microunits === null) return lowerBound ? '≥ 未知' : '—';
  const prefix = lowerBound ? '≥ ' : '';
  return `${prefix}$${(microunits / 1_000_000).toFixed(2)}`;
}

function barWidth(value: number, max: number): number {
  if (max <= 0) return 0;
  return Math.max(2, Math.round((value / max) * 100));
}

/** 热力图底色：没有记录是透明，有记录按 token 规模分四档。 */
function heatColor(tokens: number): string | undefined {
  if (tokens <= 0) return undefined;
  if (tokens < 1_000) return 'rgba(56, 189, 248, 0.10)';
  if (tokens < 10_000) return 'rgba(56, 189, 248, 0.20)';
  if (tokens < 100_000) return 'rgba(56, 189, 248, 0.32)';
  return 'rgba(56, 189, 248, 0.48)';
}

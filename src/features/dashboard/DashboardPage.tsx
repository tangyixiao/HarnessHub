import { useCallback, useEffect, useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import {
  NOT_IN_TAURI,
  getAppInfo,
  getDbHealth,
  getUsageSummary,
  refreshUsage,
  type AppInfo,
  type DbHealth,
  type UsageBucket,
  type UsageRangeKind,
  type UsageSummary,
} from '@/lib/ipc';

const RANGES: { kind: UsageRangeKind; label: string }[] = [
  { kind: 'today', label: '今天' },
  { kind: '7d', label: '7 天' },
  { kind: '30d', label: '30 天' },
  { kind: 'all', label: '全部' },
];

/**
 * Dashboard = **SQLite 的只读投影**（ADR-0012）。
 *
 * - mount / 切换范围只调用 `usage_summary`（纯读，不起任何外部进程）；
 * - 唯一的写入口是用户点「刷新用量」→ `refresh_usage` → 成功后重新查询；
 * - 「≥」的判定来自后端，前端只负责格式化；
 * - `usageSessions`（外部历史里的会话）与 `managedSessions`（Harness Hub 管理的会话）
 *   是两个不同的数字，绝不合并显示。
 */
export function DashboardPage() {
  const [range, setRange] = useState<UsageRangeKind>('30d');
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [app, setApp] = useState<AppInfo | null>(null);
  const [db, setDb] = useState<DbHealth | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshNote, setRefreshNote] = useState<string | null>(null);

  const loadSummary = useCallback(async (next: UsageRangeKind) => {
    const result = await getUsageSummary({ range: next });
    if (result.ok) {
      setSummary(result.data);
      setError(null);
    } else {
      // 失败时**保留**上一次的数字：清零会让人以为「用量没了」。
      setError(result.error);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const [appResult, dbResult] = await Promise.all([getAppInfo(), getDbHealth()]);
      if (cancelled) return;
      if (appResult.ok) setApp(appResult.data);
      if (dbResult.ok) setDb(dbResult.data);
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    // 放进微任务：effect 体内同步 setState 会触发级联渲染
    //（`react-hooks/set-state-in-effect`）。语义不变 —— 挂载后立刻查一次，
    // 切换范围时也立刻重查。
    void Promise.resolve().then(() => loadSummary(range));
  }, [range, loadSummary]);

  async function onRefreshUsage() {
    setRefreshing(true);
    setRefreshNote(null);
    const result = await refreshUsage();
    if (!result.ok) {
      setError(result.error);
    } else {
      const { import: record } = result.data;
      setRefreshNote(
        `已读取 ${record.recordsSeen} 条记录（新增 ${record.recordsInserted}，更新 ${record.recordsUpdated}，未变 ${record.recordsSkipped}）`,
      );
      await loadSummary(range);
    }
    setRefreshing(false);
  }

  const totals = summary?.totals;
  const isEmpty = summary !== null && summary.eventCount === 0;

  return (
    <div>
      <PageHeader
        title="Dashboard"
        description="Token / 成本 / 会话数全部来自本机 SQLite；渲染时不调用 ccusage。"
      />

      <div className="mb-4 flex flex-wrap items-center gap-2">
        {RANGES.map((item) => (
          <Button
            key={item.kind}
            variant={item.kind === range ? 'primary' : 'ghost'}
            aria-pressed={item.kind === range}
            onClick={() => setRange(item.kind)}
          >
            {item.label}
          </Button>
        ))}
        <span className="flex-1" />
        <Button variant="secondary" onClick={() => void onRefreshUsage()} disabled={refreshing}>
          {refreshing ? '正在读取用量…' : '刷新用量'}
        </Button>
      </div>

      {error ? (
        <p className="mb-4 text-xs text-danger" role="alert">
          {error === NOT_IN_TAURI
            ? 'IPC 不可用（浏览器模式）。请用 pnpm tauri dev 启动桌面应用。'
            : `错误：${error}`}
        </p>
      ) : null}

      {refreshNote ? (
        <p className="mb-4 text-xs text-content-muted" role="status">
          {refreshNote}
        </p>
      ) : null}

      <div className="grid gap-4 lg:grid-cols-4">
        <KpiCard
          testId="kpi-tokens"
          title="Tokens"
          value={totals ? formatTokens(totals.totalTokens) : '—'}
          hint={
            totals
              ? `输入 ${formatTokens(totals.inputTokens)} · 输出 ${formatTokens(totals.outputTokens)} · 缓存读 ${formatTokens(totals.cachedInputTokens)}`
              : undefined
          }
        />
        <KpiCard
          testId="kpi-cost"
          title="Known Cost"
          value={summary ? formatCost(summary.costMicrounits, summary.costIsLowerBound) : '—'}
          hint={
            summary?.costIsLowerBound
              ? `部分记录缺少价格（${summary.missingPricingRecords} 条），实际成本可能更高。`
              : undefined
          }
        />
        <KpiCard
          testId="kpi-usage-sessions"
          title="Usage Sessions"
          value={summary ? String(summary.usageSessions) : '—'}
          hint="来自外部用量历史的会话（不代表 Harness Hub 启动过）"
        />
        <KpiCard
          testId="kpi-managed-sessions"
          title="Managed Sessions"
          value={summary ? String(summary.managedSessions) : '—'}
          hint="Harness Hub 自己启动并管理过的会话"
        />
      </div>

      {isEmpty ? (
        <Card className="mt-4">
          <CardTitle>还没有使用量数据</CardTitle>
          <CardDescription>
            这个时间范围内没有任何已导入的记录。数据不会自动拉取：需要时点右上角「刷新用量」，
            它才会调用 ccusage 并把结果写入本机数据库（渲染本身永远只读数据库）。
          </CardDescription>
        </Card>
      ) : null}

      {summary && summary.excludedTimestampless > 0 ? (
        <p className="mt-3 text-[11px] text-content-muted">
          另有 {summary.excludedTimestampless} 条记录没有可推导的发生时间，因此不计入当前时间范围
          （选择「全部」可以看到它们）。
        </p>
      ) : null}

      <div className="mt-4 grid gap-4 lg:grid-cols-2">
        <Card>
          <CardTitle>时间线</CardTitle>
          <CardDescription>
            按**本地日**（时区 {summary?.range.timezoneOffsetMinutes ?? 0} 分钟）聚合。
          </CardDescription>
          {summary && summary.timeline.length > 0 ? (
            <ul className="mt-4 space-y-2 text-xs">
              {summary.timeline.slice(-14).map((bucket) => (
                <li key={bucket.day} className="flex items-center gap-3">
                  <span className="w-24 shrink-0 text-content-muted">{bucket.day}</span>
                  <span className="flex-1">
                    <span
                      className="block h-2 rounded bg-accent"
                      style={{
                        width: `${barWidth(bucket.totalTokens, summary.totals.totalTokens)}%`,
                      }}
                    />
                  </span>
                  <span className="w-28 shrink-0 text-right">
                    {formatTokens(bucket.totalTokens)}
                  </span>
                  <span className="w-24 shrink-0 text-right">
                    {formatCost(bucket.costMicrounits, bucket.costIsLowerBound)}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="mt-4 text-xs text-content-muted">没有按时段分布的数据。</p>
          )}
        </Card>

        <BreakdownCard
          title="Harness 分布"
          description="按上报的 agent 分组（分母是全部事件）。"
          buckets={summary?.byHarness ?? []}
          emptyHint="没有 Harness 维度的数据。"
        />

        <BreakdownCard
          title="Model 分布"
          description="一个会话可用多个模型，因此这里按模型明细计数。"
          buckets={summary?.byModel ?? []}
          emptyHint="没有模型维度的数据。"
        />

        <BreakdownCard
          title="Project 分布"
          description="v0.1 不从用量推断项目归属，因此这里通常为空。"
          buckets={summary?.byProject ?? []}
          emptyHint="尚未把用量关联到项目（不推断、不伪造归属）。"
        />
      </div>

      <Card className="mt-4">
        <CardTitle>控制平面</CardTitle>
        <CardDescription>
          数据全部来自本机 SQLite；上面的用量数字不依赖任何外部进程。
        </CardDescription>
        <dl className="mt-4 grid grid-cols-2 gap-3 text-xs lg:grid-cols-4">
          <Metric label="应用版本" value={app?.version ?? '—'} />
          <Metric label="Schema 版本" value={db ? String(db.schemaVersion) : '—'} />
          <Metric label="数据表数量" value={db ? String(db.tableCount) : '—'} />
          <Metric label="事件数" value={summary ? formatTokens(summary.eventCount) : '—'} />
        </dl>
      </Card>
    </div>
  );
}

function BreakdownCard({
  title,
  description,
  buckets,
  emptyHint,
}: {
  title: string;
  description: string;
  buckets: UsageBucket[];
  emptyHint: string;
}) {
  return (
    <Card>
      <CardTitle>{title}</CardTitle>
      <CardDescription>{description}</CardDescription>
      {buckets.length > 0 ? (
        <table className="mt-4 w-full text-xs">
          <thead>
            <tr className="text-left text-content-muted">
              <th className="font-normal">名称</th>
              <th className="text-right font-normal">Tokens</th>
              <th className="text-right font-normal">成本</th>
            </tr>
          </thead>
          <tbody>
            {buckets.slice(0, 8).map((bucket) => (
              <tr key={bucket.key}>
                <td className="truncate pr-2" title={bucket.key}>
                  {bucket.key}
                </td>
                <td className="text-right">{formatTokens(bucket.totalTokens)}</td>
                <td className="text-right">
                  {formatCost(bucket.costMicrounits, bucket.costIsLowerBound)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p className="mt-4 text-xs text-content-muted">{emptyHint}</p>
      )}
    </Card>
  );
}

function KpiCard({
  title,
  value,
  hint,
  testId,
}: {
  title: string;
  value: string;
  hint?: string;
  testId: string;
}) {
  return (
    <Card data-testid={testId}>
      <CardTitle>{title}</CardTitle>
      <p className="mt-2 text-2xl font-semibold text-content-primary">{value}</p>
      {hint ? <p className="mt-1 text-[11px] text-content-muted">{hint}</p> : null}
    </Card>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt className="text-content-muted">{label}</dt>
      <dd className="mt-0.5 font-medium text-content-primary">{value}</dd>
    </div>
  );
}

/** 微单位 → 显示文本。`lowerBound` 为 true 时必须带 `≥`（判定来自后端）。 */
export function formatCost(microunits: number | null, lowerBound: boolean): string {
  if (microunits === null) {
    return '—';
  }
  const amount = (microunits / 1_000_000).toFixed(2);
  return lowerBound ? `≥ $${amount}` : `$${amount}`;
}

export function formatTokens(value: number): string {
  return value.toLocaleString('en-US');
}

function barWidth(value: number, max: number): number {
  if (max <= 0) return 0;
  return Math.max(2, Math.round((value / max) * 100));
}

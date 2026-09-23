import { useEffect, useState } from 'react';

import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import { NOT_IN_TAURI, getAppInfo, getDbHealth, type AppInfo, type DbHealth } from '@/lib/ipc';

/**
 * Dashboard 是 Walking Skeleton 的验收页面：它必须能证明
 * 「React UI → Tauri IPC → Rust Control Plane → SQLite」这条链路真的通了。
 * 在此之前，所有数字都显示为未就绪，而不是填假数据。
 */
export function DashboardPage() {
  const [app, setApp] = useState<AppInfo | null>(null);
  const [db, setDb] = useState<DbHealth | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const [appResult, dbResult] = await Promise.all([getAppInfo(), getDbHealth()]);
      if (cancelled) return;

      if (appResult.ok) setApp(appResult.data);
      if (dbResult.ok) {
        setDb(dbResult.data);
        setError(null);
      } else {
        setError(dbResult.error);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div>
      <PageHeader
        title="Dashboard"
        description="控制平面状态与统一 Usage / Activity 视图。数据全部来自本机。"
      />

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardTitle>控制平面</CardTitle>
          <CardDescription>
            {error === NOT_IN_TAURI
              ? 'IPC 不可用（浏览器模式）。请用 pnpm tauri dev 启动桌面应用。'
              : error
                ? `IPC 错误：${error}`
                : '已连接 Rust Control Plane。'}
          </CardDescription>
          <dl className="mt-4 grid grid-cols-2 gap-3 text-xs">
            <Metric label="应用版本" value={app?.version ?? '—'} />
            <Metric label="Tauri" value={app?.tauriVersion ?? '—'} />
            <Metric label="Schema 版本" value={db ? String(db.schemaVersion) : '—'} />
            <Metric label="数据表数量" value={db ? String(db.tableCount) : '—'} />
            <Metric label="外键约束" value={db ? (db.foreignKeysEnabled ? 'ON' : 'OFF') : '—'} />
            <Metric label="Journal 模式" value={db?.journalMode ?? '—'} />
          </dl>
          {db && db.appliedMigrations.length > 0 ? (
            <p className="mt-3 text-[11px] text-content-muted">
              已应用迁移：{db.appliedMigrations.join(' → ')}
            </p>
          ) : null}
        </Card>

        <Card>
          <CardTitle>Usage（Phase 1 待接入）</CardTitle>
          <CardDescription>
            通过 ccusage JSON 归一化 Input / Output / Cache Token 与估算成本，来源标记 estimated。
          </CardDescription>
          <dl className="mt-4 grid grid-cols-2 gap-3 text-xs">
            <Metric label="Total Token" value="—" />
            <Metric label="Estimated Cost" value="—" />
            <Metric label="Model 数" value="—" />
            <Metric label="Harness 数" value="—" />
          </dl>
        </Card>

        <Card className="lg:col-span-2">
          <CardTitle>Walking Skeleton 剩余链路</CardTitle>
          <CardDescription>
            框架已就位。以下按 docs/plans 中的实施计划逐个 Task 完成，每一步保持项目可运行。
          </CardDescription>
          <ol className="mt-4 space-y-1.5 text-xs text-content-muted">
            {[
              'Codex 检测（binary / version / capabilities）',
              'PTY 启动 Codex 并持久化 Session',
              'ccusage JSON 导入与幂等去重',
              'Dashboard 展示真实 Token 数字',
              '退出重启后 Session 历史仍存在',
            ].map((step, index) => (
              <li key={step} className="flex gap-2">
                <span className="text-content-muted/70">{index + 1}.</span>
                <span>{step}</span>
              </li>
            ))}
          </ol>
        </Card>
      </div>
    </div>
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

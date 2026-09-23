import { useEffect, useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import { cn } from '@/lib/utils';
import {
  NOT_IN_TAURI,
  isTauriRuntime,
  listHarnesses,
  refreshHarnesses,
  type HarnessCapabilities,
  type HarnessSummary,
  type ReconcileReport,
} from '@/lib/ipc';

/**
 * Harness 展示名与能力矩阵的呈现顺序。
 * 顺序与 Rust `HarnessCapabilities` 的字段声明顺序一致，便于逐项对照。
 */
const CAPABILITY_FIELDS: ReadonlyArray<[keyof HarnessCapabilities, string]> = [
  ['launch', '启动'],
  ['terminal', '终端'],
  ['resume', '恢复'],
  ['usage', 'Usage'],
  ['replay', '回放'],
  ['toolCalls', '工具调用'],
  ['subagents', '子代理'],
  ['liveState', '实时状态'],
  ['worktree', 'Worktree'],
];

type PageState =
  | { kind: 'loading' }
  | { kind: 'ipc-unavailable' }
  | { kind: 'error'; message: string }
  | { kind: 'ready'; summaries: HarnessSummary[] };

/**
 * Harnesses 页面。
 *
 * 数据全部来自 Rust Control Plane 的真实检测（`list_harnesses`，**纯读**）。
 * 本页**不做**任何自己的二进制探测，也不缓存检测结果：
 * 用户可能在应用运行期间安装或卸载 Harness。
 *
 * 把检测结果写进 SQLite 是**另一个明确的操作**：用户点「重新检测并同步」按钮
 * 才调用 `refresh_harnesses`（见 ADR-0007：inspect 不改状态，refresh 才改）。
 */
export function HarnessesPage() {
  const [state, setState] = useState<PageState>({ kind: 'loading' });
  const [syncing, setSyncing] = useState(false);
  const [report, setReport] = useState<ReconcileReport | null>(null);
  const connected = isTauriRuntime();

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const result = await listHarnesses();
      if (cancelled) return;

      if (result.ok) {
        setState({ kind: 'ready', summaries: result.data });
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

  const refresh = async () => {
    setSyncing(true);
    setReport(null);

    const synced = await refreshHarnesses();
    if (!synced.ok) {
      setSyncing(false);
      setState(
        synced.error === NOT_IN_TAURI
          ? { kind: 'ipc-unavailable' }
          : { kind: 'error', message: synced.error },
      );
      return;
    }
    setReport(synced.data);

    const listed = await listHarnesses();
    setSyncing(false);
    if (listed.ok) {
      setState({ kind: 'ready', summaries: listed.data });
    } else if (listed.error !== NOT_IN_TAURI) {
      setState({ kind: 'error', message: listed.error });
    }
  };

  return (
    <div>
      <PageHeader
        title="Harnesses"
        description="本机已安装的 AI Coding Harness：检测结果、能力矩阵与只读数据目录。"
        actions={
          <Button onClick={() => void refresh()} disabled={!connected || syncing}>
            {syncing ? '同步中…' : '重新检测并同步'}
          </Button>
        }
      />

      {report ? (
        <p className="pb-4 text-[11px] text-content-muted">
          已同步：{report.harnesses} 个 Harness 定义 / {report.installations} 个安装（写入本地
          SQLite）。
        </p>
      ) : null}

      {state.kind === 'loading' ? (
        <Card>
          <CardTitle>正在检测本机 Harness…</CardTitle>
          <CardDescription>
            扫描 PATH 与已知数据目录，只读访问，不修改任何外部文件。
          </CardDescription>
        </Card>
      ) : null}

      {state.kind === 'ipc-unavailable' ? (
        <Card>
          <CardTitle>IPC 不可用（浏览器模式）</CardTitle>
          <CardDescription>
            请用 pnpm tauri dev 启动桌面应用。Harness 检测只能在 Rust Control Plane 内进行，
            浏览器里没有可用的后端，因此这里不会显示任何结果。
          </CardDescription>
        </Card>
      ) : null}

      {state.kind === 'error' ? (
        <Card className="border-signal-error/40">
          <CardTitle>检测失败</CardTitle>
          <CardDescription>IPC 错误：{state.message}</CardDescription>
        </Card>
      ) : null}

      {state.kind === 'ready' && state.summaries.length === 0 ? (
        <Card>
          <CardTitle>未检测到任何 Harness</CardTitle>
          <CardDescription>
            本页只展示已检测并注册的 AI Coding Harness，不预设系统里应该有哪几个。
            若这里为空，说明没有在 PATH 中找到可执行文件；这不影响其他功能，
            安装后重新打开本页即可。
          </CardDescription>
        </Card>
      ) : null}

      {state.kind === 'ready' && state.summaries.length > 0 ? (
        <div className="space-y-4">
          {state.summaries.map((summary) => (
            <HarnessCard key={summary.id} summary={summary} />
          ))}
          <p className="text-[11px] text-content-muted">
            ✓ = 已实现该能力；— = <strong className="font-medium">当前未支持</strong>
            （实现尚未落地）。能力逐项独立演进，不因某台机器的环境问题（binary 缺失、 auth
            过期等）而改变 —— 那属于 readiness，见 ADR-0005。
          </p>
        </div>
      ) : null}
    </div>
  );
}

function HarnessCard({ summary }: { summary: HarnessSummary }) {
  return (
    <section aria-label={summary.displayName}>
      <Card>
        <div className="flex items-start justify-between gap-4">
          <div>
            <CardTitle>{summary.displayName}</CardTitle>
            <CardDescription>Harness ID：{summary.id}</CardDescription>
          </div>
          <StatusPill installed={summary.installed} />
        </div>

        <dl className="mt-4 grid gap-3 text-xs sm:grid-cols-3">
          <Field label="版本" value={summary.version ?? '版本未知'} />
          <Field label="可执行文件" value={summary.binaryPath ?? '未检测到可执行文件'} />
          <Field
            label="数据目录（只读）"
            value={summary.dataPaths.length > 0 ? summary.dataPaths.join('、') : '未检测到数据目录'}
          />
        </dl>

        <div className="mt-4 border-t border-border-subtle pt-4">
          <p className="text-xs font-medium text-content-muted">能力矩阵</p>
          <ul className="mt-2 flex flex-wrap gap-1.5">
            {CAPABILITY_FIELDS.map(([key, label]) => {
              const supported = summary.capabilities[key];

              return (
                <li
                  key={key}
                  data-testid={`capability-${key}`}
                  title={supported ? `${label}：已实现` : `${label}：当前未支持（实现尚未落地）`}
                  aria-label={`${label}：${supported ? '已实现' : '当前未支持'}`}
                  className={cn(
                    'inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-[11px]',
                    supported
                      ? 'border-signal-ok/40 bg-signal-ok/10 text-content-primary'
                      : 'border-border-subtle text-content-muted',
                  )}
                >
                  <span>{label}</span>
                  <span aria-hidden>{supported ? '✓' : '—'}</span>
                </li>
              );
            })}
          </ul>
        </div>
      </Card>
    </section>
  );
}

function StatusPill({ installed }: { installed: boolean }) {
  return (
    <span
      className={cn(
        'shrink-0 rounded-md border px-2 py-1 text-[11px] font-medium',
        installed
          ? 'border-signal-ok/40 bg-signal-ok/10 text-signal-ok'
          : 'border-signal-warn/40 bg-signal-warn/10 text-signal-warn',
      )}
    >
      {installed ? '已安装' : '不可用'}
    </span>
  );
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt className="text-content-muted">{label}</dt>
      <dd className="mt-0.5 break-all font-mono text-[11px] text-content-primary">{value}</dd>
    </div>
  );
}

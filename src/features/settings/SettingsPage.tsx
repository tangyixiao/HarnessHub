import { Loader2, Settings } from 'lucide-react';
import { useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PlaceholderCard } from '@/components/ui/placeholder-card';
import { PageHeader } from '@/components/ui/page-header';
import { discoverProfile, isTauriRuntime, type ProfileSummary } from '@/lib/ipc';

/**
 * 可发现配置现状的 Harness 清单。id 与 Rust 侧 `profile_discover` 的 match
 * 分支一一对应（src-tauri/src/commands.rs，E14）；未实现的 harness 不提供按钮，
 * 避免「注定失败的调用」。
 */
const PROFILE_HARNESS_OPTIONS: ReadonlyArray<{ id: string; label: string }> = [
  { id: 'claude', label: 'Claude Code' },
  { id: 'codex', label: 'Codex' },
  { id: 'gemini', label: 'Gemini CLI' },
];

/** Rust 层 redact 后的占位值（src-tauri/src/profile/mod.rs）；出现时加「已脱敏」徽标。 */
const REDACTED_VALUE = '<redacted>';

type ProfileState =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'missing' }
  | { kind: 'ready'; summary: ProfileSummary }
  | { kind: 'error'; message: string };

export function SettingsPage() {
  return (
    <div>
      <PageHeader
        title="Settings"
        description="本地优先设置：数据目录、网络开关、Secret 句柄与审计。"
      />
      <ProviderProfileCard />
      <PlaceholderCard
        title="尚未接入"
        icon={Settings}
        hint="目标模块：src-tauri/src/permissions/、src-tauri/src/hhar/、src/features/settings/"
      >
        计划包含：数据目录与导出（HHAR）、网络能力显式开关、Config Doctor、Secret 句柄状态。
        所有网络相关能力默认关闭。
      </PlaceholderCard>
    </div>
  );
}

/**
 * Provider 配置现状（ROADMAP E14）：**只读**展示各 Harness 的当前配置形态。
 *
 * 纪律（spec 2026-10-25-profile-registry-design.md）：
 * - secret 值已在 Rust 层脱敏，这里原样展示 `<redacted>` 并加视觉标注；
 * - 本区不提供任何「修改 / 保存 / 应用」入口 —— 配置投影（原子写 + 备份恢复）
 *   是 E15 的独立验收，绝不在这里偷跑；
 * - 只在用户点击按钮时才读取，渲染路径不做任何 IPC 调用。
 */
function ProviderProfileCard() {
  const [activeHarness, setActiveHarness] = useState<string | null>(null);
  const [state, setState] = useState<ProfileState>({ kind: 'idle' });
  const connected = isTauriRuntime();

  const discover = async (harnessId: string) => {
    setActiveHarness(harnessId);
    setState({ kind: 'loading' });

    const result = await discoverProfile(harnessId);
    if (!result.ok) {
      setState({ kind: 'error', message: result.error });
      return;
    }
    setState(result.data === null ? { kind: 'missing' } : { kind: 'ready', summary: result.data });
  };

  return (
    <Card className="mb-6">
      <CardTitle>Provider 配置现状</CardTitle>
      <CardDescription>
        只读读取各 Harness 的现有配置文件（Provider 指向、模型偏好等），Secret 值已在后端脱敏。
        本区仅展示、不可修改；配置投影与切换在后续版本提供。
      </CardDescription>

      <div className="mt-3 flex flex-wrap gap-2">
        {PROFILE_HARNESS_OPTIONS.map((option) => (
          <Button
            key={option.id}
            variant={activeHarness === option.id ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => void discover(option.id)}
            disabled={!connected || state.kind === 'loading'}
          >
            {option.label}
          </Button>
        ))}
      </div>

      <div className="mt-4" aria-live="polite">
        {state.kind === 'idle' ? (
          <p className="text-xs text-content-muted">
            选择一个 Harness 查看其配置现状；不会读取未列出的内容，也不会写入任何文件。
          </p>
        ) : null}

        {state.kind === 'loading' ? (
          <p className="flex items-center gap-2 text-xs text-content-muted">
            <Loader2 aria-hidden className="size-3.5 animate-spin text-accent" />
            正在读取{activeHarness ? ` ${activeHarness} ` : ''}的配置文件…
          </p>
        ) : null}

        {state.kind === 'missing' ? (
          <div>
            <p className="text-xs font-medium text-content-secondary">未找到配置文件（未初始化）</p>
            <p className="mt-1 text-xs text-content-muted">
              该 Harness 还没有写过自己的配置文件；这不代表安装失败，初始化后会出现在这里。
            </p>
          </div>
        ) : null}

        {state.kind === 'error' ? (
          <p className="text-xs text-signal-error" role="alert">
            读取失败：{state.message}
          </p>
        ) : null}

        {state.kind === 'ready' ? (
          Object.keys(state.summary.fields).length === 0 ? (
            <p className="text-xs text-content-muted">配置文件可读，但没有可展示的键值。</p>
          ) : (
            <ProfileFields summary={state.summary} />
          )
        ) : null}
      </div>
    </Card>
  );
}

/**
 * 键值明细：后端给的顺序原样展示（Rust BTreeMap 已按键排序，前端不重排）。
 * `<redacted>` 行的值照实渲染（绝不伪造明文），并加暗色小徽标说明。
 */
function ProfileFields({ summary }: { summary: ProfileSummary }) {
  return (
    <div>
      <ul className="overflow-hidden rounded-md border border-border-subtle bg-white/[0.03]">
        {Object.entries(summary.fields).map(([key, value]) => (
          <li
            key={key}
            data-testid={`profile-field-${key}`}
            className="flex items-start gap-2 border-b border-border-subtle/60 px-3 py-1.5 text-[11px] last:border-b-0"
          >
            <span className="break-all font-mono text-content-primary">{key}</span>
            <span aria-hidden className="shrink-0 font-mono text-content-muted">
              =
            </span>
            <span className="min-w-0 break-all font-mono text-content-secondary">{value}</span>
            {value === REDACTED_VALUE ? (
              <span
                data-testid="profile-redacted-badge"
                title="该值含 Secret，已在后端脱敏"
                className="ml-auto shrink-0 rounded border border-border-subtle bg-white/[0.05] px-1.5 py-0.5 text-[10px] font-medium text-content-muted"
              >
                已脱敏
              </span>
            ) : null}
          </li>
        ))}
      </ul>
      <p className="mt-3 text-[11px] text-content-muted">
        来源：<span className="font-mono">{summary.sourcePath}</span>
        <span aria-hidden className="mx-2">
          ·
        </span>
        解析时间：<span className="font-mono">{summary.parsedAt}</span>
      </p>
    </div>
  );
}

import { AlertTriangle, CheckCircle2, KeyRound, Loader2, Settings, XCircle } from 'lucide-react';
import { useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PlaceholderCard } from '@/components/ui/placeholder-card';
import { PageHeader } from '@/components/ui/page-header';
import {
  discoverProfile,
  isTauriRuntime,
  listCredentials,
  listProfileBackups,
  projectProfile,
  registerCredential,
  removeCredential,
  restoreProfileBackup,
  runConfigDoctor,
  type BackupInfo,
  type CredentialRef,
  type DoctorCheckItem,
  type ProfileSummary,
  type ProjectionReport,
} from '@/lib/ipc';

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
      <ConfigProjectionCard />
      <CredentialsCard />
      <ConfigDoctorCard />
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
 * - 本区**仍不提供**修改入口 —— 投影 / 凭据 / 体检各自是下面的独立区块
 *   （E15/E16/E17），现状展示永远只读；
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
    <Card className="mb-6" data-testid="provider-profile-card">
      <CardTitle>Provider 配置现状</CardTitle>
      <CardDescription>
        只读读取各 Harness 的现有配置文件（Provider 指向、模型偏好等），Secret 值已在后端脱敏。
        本区仅展示；要修改请使用下方的「安全投影」区块。
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

const INPUT_CLASS =
  'min-w-0 flex-1 rounded-md border border-white/10 bg-white/[0.04] px-3 py-2 font-mono text-xs text-content-primary outline-none placeholder:text-content-muted focus:border-accent/60';

/**
 * 安全投影（ROADMAP E15）：把编辑的字段写进 `~/.claude/settings.json`。
 *
 * 纪律（spec 2026-10-30-profile-plane-wiring-design.md）：
 * - 仅 claude（E15 范围）；**显式写动作**：confirm 文案必须含目标文件路径；
 * - 空输入 = 不动该键（不发送）；投影前自动备份，坏结果绝不落盘（原子管线在 Rust 层）；
 * - 备份列表显式加载，恢复目标永远来自后端备份记录。
 */
function ConfigProjectionCard() {
  const connected = isTauriRuntime();
  const [baseUrl, setBaseUrl] = useState('');
  const [model, setModel] = useState('');
  const [token, setToken] = useState('');
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<ProjectionReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [backups, setBackups] = useState<BackupInfo[] | null>(null);

  const apply = async () => {
    const fields: Record<string, string> = {};
    if (baseUrl.trim().length > 0) {
      fields['env.ANTHROPIC_BASE_URL'] = baseUrl.trim();
    }
    if (model.trim().length > 0) {
      fields['env.ANTHROPIC_MODEL'] = model.trim();
    }
    if (token.length > 0) {
      fields['env.ANTHROPIC_AUTH_TOKEN'] = token;
    }
    if (Object.keys(fields).length === 0) {
      setError('没有要写入的字段：填至少一项，留空的项不会被修改。');
      return;
    }

    const confirmed = window.confirm(
      '将修改 ~/.claude/settings.json（写入前自动备份到 Hub 备份目录，原文件其余内容保持不变）。继续？',
    );
    if (!confirmed) {
      return;
    }

    setBusy(true);
    setError(null);
    setReport(null);
    const result = await projectProfile('claude', fields);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setReport(result.data);
    setToken('');
    setBackups(null);
  };

  const loadBackups = async () => {
    setBusy(true);
    setError(null);
    const result = await listProfileBackups('claude');
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setBackups(result.data);
  };

  const restore = async (backupId: string) => {
    const confirmed = window.confirm('将用这份备份覆盖当前的 ~/.claude/settings.json。继续？');
    if (!confirmed) {
      return;
    }
    setBusy(true);
    setError(null);
    const result = await restoreProfileBackup('claude', backupId);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setReport(null);
    setBackups(null);
    setError(null);
  };

  return (
    <Card className="mb-6" data-testid="projection-card">
      <CardTitle>安全投影（Claude Code）</CardTitle>
      <CardDescription>
        把下面的字段合并进 ~/.claude/settings.json：同名键覆盖、其余内容原样保留、 env
        块深合并。写入走「读 → 解析 → 备份 → 临时文件 → 校验 → 原子替换」管线，
        任何一步失败都不会落盘。当前仅支持 Claude Code。
      </CardDescription>

      <div className="mt-3 space-y-2">
        <input
          type="text"
          value={baseUrl}
          onChange={(event) => setBaseUrl(event.target.value)}
          placeholder="env.ANTHROPIC_BASE_URL（留空 = 不修改）"
          aria-label="ANTHROPIC_BASE_URL"
          className={INPUT_CLASS}
        />
        <input
          type="text"
          value={model}
          onChange={(event) => setModel(event.target.value)}
          placeholder="env.ANTHROPIC_MODEL（留空 = 不修改）"
          aria-label="ANTHROPIC_MODEL"
          className={INPUT_CLASS}
        />
        <input
          type="password"
          value={token}
          onChange={(event) => setToken(event.target.value)}
          placeholder="env.ANTHROPIC_AUTH_TOKEN（留空 = 不修改）"
          aria-label="ANTHROPIC_AUTH_TOKEN"
          autoComplete="off"
          className={INPUT_CLASS}
        />
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button
          variant="primary"
          size="sm"
          onClick={() => void apply()}
          disabled={!connected || busy}
          data-testid="projection-apply"
        >
          写入配置
        </Button>
        <Button variant="secondary" size="sm" onClick={() => void loadBackups()} disabled={busy}>
          查看备份
        </Button>
      </div>

      <div className="mt-3" aria-live="polite">
        {error ? (
          <p className="text-xs text-signal-error" role="alert">
            {error}
          </p>
        ) : null}

        {report ? (
          <div data-testid="projection-report" className="text-xs text-content-secondary">
            <p>
              已写入 <span className="font-mono">{report.targetPath}</span>
              （变更 {report.changedKeys} 处），备份 id
              <span className="font-mono"> {report.backupId}</span>。可在「查看备份」里恢复。
            </p>
          </div>
        ) : null}

        {backups ? (
          backups.length === 0 ? (
            <p className="text-xs text-content-muted">还没有备份（投影一次就会出现）。</p>
          ) : (
            <ul
              className="overflow-hidden rounded-md border border-border-subtle bg-white/[0.03]"
              data-testid="projection-backups"
            >
              {backups.map((backup) => (
                <li
                  key={backup.id}
                  className="flex items-center gap-2 border-b border-border-subtle/60 px-3 py-1.5 text-[11px] last:border-b-0"
                >
                  <span className="font-mono text-content-secondary">{backup.createdAt}</span>
                  <span className="font-mono text-content-muted">{backup.id}</span>
                  <Button
                    variant="secondary"
                    size="sm"
                    className="ml-auto"
                    onClick={() => void restore(backup.id)}
                    data-testid={`projection-restore-${backup.id}`}
                  >
                    恢复
                  </Button>
                </li>
              ))}
            </ul>
          )
        ) : null}
      </div>
    </Card>
  );
}

/**
 * 凭据登记（ROADMAP E16）：值进 Secret Store，Hub 只存句柄。
 *
 * 纪律：值用 password 输入框、不做任何回显；列表只有句柄元数据
 * （label / storeKind / createdAt），**任何路径都不展示值**；删除需 confirm。
 */
function CredentialsCard() {
  const connected = isTauriRuntime();
  const [activeHarness, setActiveHarness] = useState<string | null>(null);
  const [items, setItems] = useState<CredentialRef[] | null>(null);
  const [label, setLabel] = useState('');
  const [value, setValue] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = async (harnessId: string) => {
    setBusy(true);
    setError(null);
    const result = await listCredentials(harnessId);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setItems(result.data);
  };

  const select = async (harnessId: string) => {
    setActiveHarness(harnessId);
    setItems(null);
    await load(harnessId);
  };

  const register = async () => {
    if (label.trim().length === 0 || value.length === 0 || activeHarness === null) {
      setError('需要 harness、标签和值。');
      return;
    }
    setBusy(true);
    setError(null);
    const result = await registerCredential(activeHarness, label.trim(), value);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setLabel('');
    setValue('');
    await load(activeHarness);
  };

  const remove = async (id: string, itemLabel: string) => {
    const confirmed = window.confirm(`删除凭据「${itemLabel}」？值会从 Secret Store 一并删除。`);
    if (!confirmed) {
      return;
    }
    setBusy(true);
    setError(null);
    const result = await removeCredential(id);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    if (activeHarness !== null) {
      await load(activeHarness);
    }
  };

  return (
    <Card className="mb-6" data-testid="credentials-card">
      <CardTitle>凭据句柄</CardTitle>
      <CardDescription>
        API Key 等凭据只进 Secret Store（OS 密钥链抽象，文件兜底），Hub 数据库只存句柄。
        值登记后不再可见；列表与导出都不会包含值。
      </CardDescription>

      <div className="mt-3 flex flex-wrap gap-2">
        {PROFILE_HARNESS_OPTIONS.map((option) => (
          <Button
            key={option.id}
            variant={activeHarness === option.id ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => void select(option.id)}
            disabled={!connected || busy}
            data-testid={`credential-tab-${option.id}`}
          >
            {option.label}
          </Button>
        ))}
      </div>

      {activeHarness !== null ? (
        <form
          className="mt-3 space-y-2 rounded-md border border-white/[0.06] bg-white/[0.02] p-3"
          onSubmit={(event) => {
            event.preventDefault();
            void register();
          }}
        >
          <div className="flex flex-col gap-2 sm:flex-row">
            <input
              type="text"
              value={label}
              onChange={(event) => setLabel(event.target.value)}
              placeholder="标签，例如 deepseek-key"
              aria-label="凭据标签"
              className={INPUT_CLASS}
            />
            <input
              type="password"
              value={value}
              onChange={(event) => setValue(event.target.value)}
              placeholder="值（登记后不可见）"
              aria-label="凭据值"
              autoComplete="off"
              className={INPUT_CLASS}
            />
            <Button type="submit" variant="primary" size="sm" disabled={!connected || busy}>
              登记
            </Button>
          </div>
        </form>
      ) : null}

      <div className="mt-3" aria-live="polite">
        {error ? (
          <p className="text-xs text-signal-error" role="alert">
            {error}
          </p>
        ) : null}

        {items !== null && activeHarness !== null ? (
          items.length === 0 ? (
            <p className="text-xs text-content-muted">这个 Harness 还没有登记凭据。</p>
          ) : (
            <ul className="overflow-hidden rounded-md border border-border-subtle bg-white/[0.03]">
              {items.map((item) => (
                <li
                  key={item.id}
                  data-testid={`credential-row-${item.label}`}
                  className="flex items-center gap-2 border-b border-border-subtle/60 px-3 py-1.5 text-[11px] last:border-b-0"
                >
                  <KeyRound aria-hidden className="size-3 shrink-0 text-content-muted" />
                  <span className="font-mono text-content-primary">{item.label}</span>
                  <span className="text-content-muted">{item.storeKind}</span>
                  <span className="font-mono text-content-muted">{item.createdAt}</span>
                  <Button
                    variant="secondary"
                    size="sm"
                    className="ml-auto"
                    onClick={() => void remove(item.id, item.label)}
                    data-testid={`credential-remove-${item.label}`}
                  >
                    删除
                  </Button>
                </li>
              ))}
            </ul>
          )
        ) : null}
      </div>
    </Card>
  );
}

const DOCTOR_STATUS_META: Record<
  DoctorCheckItem['status'],
  { icon: typeof CheckCircle2; className: string }
> = {
  ok: { icon: CheckCircle2, className: 'text-signal-ok' },
  warn: { icon: AlertTriangle, className: 'text-signal-warn' },
  error: { icon: XCircle, className: 'text-signal-error' },
};

/**
 * 配置体检（ROADMAP E17）：只读诊断 + 建议，**没有 Fix 按钮**——
 * 修复动作走上面的投影 / 备份恢复（显式动作）。
 */
function ConfigDoctorCard() {
  const connected = isTauriRuntime();
  const [activeHarness, setActiveHarness] = useState<string | null>(null);
  const [items, setItems] = useState<DoctorCheckItem[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (harnessId: string) => {
    setActiveHarness(harnessId);
    setItems(null);
    setBusy(true);
    setError(null);
    const result = await runConfigDoctor(harnessId);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setItems(result.data);
  };

  return (
    <Card className="mb-6" data-testid="doctor-card">
      <CardTitle>配置体检</CardTitle>
      <CardDescription>
        只读检查：配置可解析、凭据可达、Provider 指向。发现问题只给建议，
        不会自动修复；修复请使用「安全投影」与「恢复备份」（显式动作）。
      </CardDescription>

      <div className="mt-3 flex flex-wrap gap-2">
        {PROFILE_HARNESS_OPTIONS.map((option) => (
          <Button
            key={option.id}
            variant={activeHarness === option.id ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => void run(option.id)}
            disabled={!connected || busy}
            data-testid={`doctor-run-${option.id}`}
          >
            {option.label}
          </Button>
        ))}
      </div>

      <div className="mt-3" aria-live="polite">
        {busy ? (
          <p className="flex items-center gap-2 text-xs text-content-muted">
            <Loader2 aria-hidden className="size-3.5 animate-spin text-accent" />
            正在体检…
          </p>
        ) : null}

        {error ? (
          <p className="text-xs text-signal-error" role="alert">
            {error}
          </p>
        ) : null}

        {items !== null && activeHarness !== null && !busy ? (
          <ul className="space-y-1.5" data-testid="doctor-items">
            {items.map((item) => {
              const meta = DOCTOR_STATUS_META[item.status];
              const Icon = meta.icon;
              return (
                <li
                  key={item.check}
                  data-testid={`doctor-item-${item.check}`}
                  className="flex items-start gap-2 text-[11px]"
                >
                  <Icon aria-hidden className={`mt-0.5 size-3.5 shrink-0 ${meta.className}`} />
                  <div className="min-w-0">
                    <p className="text-content-secondary">
                      <span className="font-mono text-content-primary">{item.check}</span>
                      <span aria-hidden className="mx-2">
                        ·
                      </span>
                      {item.message}
                    </p>
                    {item.suggestion !== null ? (
                      <p className="mt-0.5 text-content-muted">建议：{item.suggestion}</p>
                    ) : null}
                  </div>
                </li>
              );
            })}
          </ul>
        ) : null}
      </div>
    </Card>
  );
}

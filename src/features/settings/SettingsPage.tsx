import { AlertTriangle, CheckCircle2, KeyRound, Loader2, Settings, XCircle } from 'lucide-react';
import { useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PlaceholderCard } from '@/components/ui/placeholder-card';
import { PageHeader } from '@/components/ui/page-header';
import {
  addMcpBinding,
  applyMcpSync,
  discoverProfile,
  isTauriRuntime,
  importMcpConfig,
  listCredentials,
  listMcpBindings,
  listMcpServers,
  listProfileBackups,
  listSkills,
  projectProfile,
  registerCredential,
  removeCredential,
  restoreProfileBackup,
  runConfigDoctor,
  sidecarRestart,
  sidecarStart,
  sidecarStatus,
  sidecarStop,
  previewMcpSync,
  removeMcpBinding,
  removeMcpServer,
  upsertMcpServer,
  type BackupInfo,
  type CredentialRef,
  type DoctorCheckItem,
  type McpBinding,
  type McpImportReport,
  type McpServer,
  type McpServerInput,
  type McpSyncPreview,
  type McpSyncReport,
  type McpTransport,
  type IpcResult,
  type ProfileSummary,
  type ProjectionReport,
  type SidecarState,
  type SidecarStatus,
  type SkillsReport,
} from '@/lib/ipc';
import { McpRuntimeCard } from '@/features/settings/McpRuntimeCard';
import { ProviderGatewayCard } from '@/features/settings/ProviderGatewayCard';

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
      <McpCard />
      <McpRuntimeCard />
      <SkillsCard />
      <SidecarCard />
      <ProviderGatewayCard />
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

/* ------------------------------------------------------------------ *
 * MCP Servers（ROADMAP E18）
 * ------------------------------------------------------------------ */

/** 各 harness 的 MCP 配置文件位置（展示用；实际绝对路径由后端解析 home 目录）。 */
const MCP_TARGET_HINTS: Record<string, string> = {
  claude: '~/.claude.json',
  codex: '~/.codex/config.toml',
  gemini: '~/.gemini/settings.json',
};

/** 把逗号分隔的输入解析成字符串列表（去空白、丢空段）。 */
function splitCommaList(raw: string): string[] {
  return raw
    .split(',')
    .map((item) => item.trim())
    .filter((item) => item.length > 0);
}

/**
 * MCP Binding Registry（ROADMAP E18）：Hub 侧 source-of-truth 登记处。
 *
 * 纪律（spec docs/specs/，E18）：
 * - **渲染路径零 IPC**：列表加载也是显式动作（「加载列表」按钮），与本页其它区块一致；
 * - 绑定勾选只是 Registry 内的意图声明，**不写文件**，不需要 confirm；
 * - 同步 = 预览差异（纯读）→ window.confirm（文案必须含目标文件路径）→ 原子写入；
 * - 导入是显式动作，confirm 说明「导入后 Hub Registry 成为 source-of-truth」；
 * - Hub 只管 command / args / url，条目内用户手写的 env / headers 由后端保留。
 */
function McpCard() {
  const connected = isTauriRuntime();
  const [servers, setServers] = useState<McpServer[] | null>(null);
  const [bindings, setBindings] = useState<McpBinding[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 新增表单
  const [name, setName] = useState('');
  const [transport, setTransport] = useState<McpTransport>('stdio');
  const [command, setCommand] = useState('');
  const [url, setUrl] = useState('');
  const [args, setArgs] = useState('');
  const [envKeys, setEnvKeys] = useState('');
  const [enabled, setEnabled] = useState(true);

  // 同步区：一次只保留一份预览 / 报告 / 导入结果（最新操作覆盖旧的）
  const [preview, setPreview] = useState<McpSyncPreview | null>(null);
  const [report, setReport] = useState<McpSyncReport | null>(null);
  const [importReport, setImportReport] = useState<McpImportReport | null>(null);

  /** 变更成功后刷新两个列表（servers + bindings）。 */
  const reloadLists = async () => {
    const [serversResult, bindingsResult] = await Promise.all([
      listMcpServers(),
      listMcpBindings(),
    ]);
    if (!serversResult.ok) {
      setError(serversResult.error);
      return;
    }
    setServers(serversResult.data);
    if (!bindingsResult.ok) {
      setError(bindingsResult.error);
      return;
    }
    setBindings(bindingsResult.data);
    setError(null);
  };

  // 列表加载是显式动作（「加载列表」按钮 / 各操作成功后的刷新）；
  // 渲染路径与挂载路径都不发起 IPC，与本页其它区块同一纪律。

  /** 某个 server 在给定 scope 下是否已有绑定（勾选态完全由列表数据推导）。 */
  const isBound = (serverId: string, scope: string, harnessId: string | null): boolean =>
    (bindings ?? []).some(
      (binding) =>
        binding.serverId === serverId &&
        binding.scope === scope &&
        (scope === 'global' ? binding.harnessId === null : binding.harnessId === harnessId),
    );

  /** 勾 = 增加绑定；取消勾 = 删除绑定。绑定不写文件，所以不需要 confirm。 */
  const toggleBinding = async (
    server: McpServer,
    scope: 'global' | 'harness',
    harnessId: string | null,
    checked: boolean,
  ) => {
    setBusy(true);
    setError(null);
    if (checked) {
      const result = await addMcpBinding(server.id, scope, harnessId);
      if (!result.ok) {
        setBusy(false);
        setError(result.error);
        return;
      }
    } else {
      const binding = (bindings ?? []).find(
        (item) =>
          item.serverId === server.id &&
          item.scope === scope &&
          (scope === 'global' ? item.harnessId === null : item.harnessId === harnessId),
      );
      if (binding === undefined) {
        setBusy(false);
        return;
      }
      const result = await removeMcpBinding(binding.id);
      if (!result.ok) {
        setBusy(false);
        setError(result.error);
        return;
      }
    }
    const listResult = await listMcpBindings();
    setBusy(false);
    if (!listResult.ok) {
      setError(listResult.error);
      return;
    }
    setBindings(listResult.data);
  };

  const submit = async () => {
    const trimmedName = name.trim();
    if (trimmedName.length === 0) {
      setError('需要 Server 名称。');
      return;
    }
    if (transport === 'stdio' && command.trim().length === 0) {
      setError('stdio 传输需要命令（command）。');
      return;
    }
    if (transport === 'http' && url.trim().length === 0) {
      setError('http 传输需要 URL。');
      return;
    }

    const input: McpServerInput = {
      id: null,
      name: trimmedName,
      transport,
      command: transport === 'stdio' ? command.trim() : null,
      args: transport === 'stdio' ? splitCommaList(args) : [],
      url: transport === 'http' ? url.trim() : null,
      envKeys: splitCommaList(envKeys),
      enabled,
    };

    setBusy(true);
    setError(null);
    const result = await upsertMcpServer(input);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setName('');
    setCommand('');
    setUrl('');
    setArgs('');
    setEnvKeys('');
    setEnabled(true);
    await reloadLists();
  };

  const removeServer = async (server: McpServer) => {
    const confirmed = window.confirm(
      `从 Hub Registry 删除 MCP Server「${server.name}」？只删除 Hub 侧登记，不会修改任何 harness 配置文件。继续？`,
    );
    if (!confirmed) {
      return;
    }
    setBusy(true);
    setError(null);
    const result = await removeMcpServer(server.id);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    await reloadLists();
  };

  /** 预览差异：**纯读**，可以反复调用；确认要写入时再走确认同步。 */
  const runPreview = async (harnessId: string) => {
    setBusy(true);
    setError(null);
    setReport(null);
    setImportReport(null);
    const result = await previewMcpSync(harnessId);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setPreview(result.data);
  };

  /** 确认同步：**显式写动作**，confirm 文案必须含预览返回的目标文件路径。 */
  const applySync = async (harnessId: string) => {
    const target = preview !== null && preview.harnessId === harnessId ? preview : null;
    if (target === null || target.targetPath.length === 0) {
      return;
    }
    const confirmed = window.confirm(
      `将把 Hub Registry 的 MCP 配置写入 ${target.targetPath}（写入前自动备份；Hub 管理的条目会被覆盖，条目内用户手写的 env / headers 保留）。继续？`,
    );
    if (!confirmed) {
      return;
    }
    setBusy(true);
    setError(null);
    const result = await applyMcpSync(harnessId);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setReport(result.data);
  };

  /** 导入现有：**显式动作**，导入后 Hub Registry 成为 source-of-truth。 */
  const runImport = async (harnessId: string) => {
    const confirmed = window.confirm(
      `从 ${harnessId} 的现有配置导入 MCP Server 到 Hub Registry？导入后 Hub Registry 成为 source-of-truth，之后的同步会以 Hub 为准覆盖配置文件。继续？`,
    );
    if (!confirmed) {
      return;
    }
    setBusy(true);
    setError(null);
    setPreview(null);
    setReport(null);
    const result = await importMcpConfig(harnessId);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setImportReport(result.data);
    await reloadLists();
  };

  return (
    <Card className="mb-6" data-testid="mcp-card">
      <CardTitle>MCP Servers</CardTitle>
      <CardDescription>
        Hub 侧的 MCP 配置 source-of-truth 登记处：这里只登记 command / args / url 与环境变量**名**，
        同步走「预览差异 → 确认 → 原子写入」，把登记内容投影进 harness 配置（claude ~/.claude.json、
        codex ~/.codex/config.toml、gemini ~/.gemini/settings.json）。条目内用户手写的 env / headers
        一律保留；绑定勾选只是 Registry 内声明，不写文件。
      </CardDescription>

      <div className="mt-3" aria-live="polite">
        {busy ? (
          <p className="flex items-center gap-2 text-xs text-content-muted">
            <Loader2 aria-hidden className="size-3.5 animate-spin text-accent" />
            处理中…
          </p>
        ) : null}

        {error !== null ? (
          <p className="text-xs text-signal-error" role="alert">
            {error}
          </p>
        ) : null}

        {servers === null ? (
          connected ? (
            <div className="flex flex-wrap items-center gap-2">
              <p className="text-xs text-content-muted">
                点击「加载列表」读取已登记的 MCP Server 与绑定；不会自动发起任何调用。
              </p>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => void reloadLists()}
                disabled={busy}
                data-testid="mcp-load"
              >
                加载列表
              </Button>
            </div>
          ) : (
            <p className="text-xs text-content-muted">
              未在 Tauri 环境中运行：列表不可用，本区按钮已禁用。
            </p>
          )
        ) : servers.length === 0 ? (
          <p className="text-xs text-content-muted">
            还没有登记任何 MCP Server；用下面的表单新增，或用「导入现有」从 harness 配置导入。
          </p>
        ) : (
          <ul
            className="overflow-hidden rounded-md border border-border-subtle bg-white/[0.03]"
            data-testid="mcp-server-list"
          >
            {servers.map((server) => (
              <li
                key={server.id}
                data-testid={`mcp-server-row-${server.name}`}
                className="border-b border-border-subtle/60 px-3 py-2 text-[11px] last:border-b-0"
              >
                <div className="flex items-center gap-2">
                  <span className="font-mono text-content-primary">{server.name}</span>
                  <span className="shrink-0 rounded border border-border-subtle bg-white/[0.05] px-1.5 py-0.5 text-[10px] font-medium text-content-muted">
                    {server.transport}
                  </span>
                  <span className="min-w-0 truncate font-mono text-content-muted">
                    {server.transport === 'stdio' ? server.command : server.url}
                  </span>
                  <span className={server.enabled ? 'text-signal-ok' : 'text-content-muted'}>
                    {server.enabled ? '已启用' : '已停用'}
                  </span>
                  <Button
                    variant="secondary"
                    size="sm"
                    className="ml-auto"
                    onClick={() => void removeServer(server)}
                    disabled={!connected || busy}
                    data-testid={`mcp-server-remove-${server.name}`}
                  >
                    删除
                  </Button>
                </div>
                <div className="mt-1.5 flex flex-wrap items-center gap-3">
                  <label className="flex w-fit items-center gap-1.5 text-content-secondary">
                    <input
                      type="checkbox"
                      checked={isBound(server.id, 'global', null)}
                      onChange={(event) =>
                        void toggleBinding(server, 'global', null, event.target.checked)
                      }
                      disabled={!connected || busy}
                    />
                    Global
                  </label>
                  {PROFILE_HARNESS_OPTIONS.map((option) => (
                    <label
                      key={option.id}
                      className="flex w-fit items-center gap-1.5 text-content-secondary"
                    >
                      <input
                        type="checkbox"
                        checked={isBound(server.id, 'harness', option.id)}
                        onChange={(event) =>
                          void toggleBinding(server, 'harness', option.id, event.target.checked)
                        }
                        disabled={!connected || busy}
                      />
                      {option.label}
                    </label>
                  ))}
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      <form
        className="mt-3 space-y-2 rounded-md border border-white/[0.06] bg-white/[0.02] p-3"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <p className="text-xs font-medium text-content-secondary">登记新的 MCP Server</p>
        <div className="flex flex-col gap-2 sm:flex-row">
          <input
            type="text"
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="名称（写入 harness 配置的键），例如 filesystem"
            aria-label="MCP Server 名称"
            className={INPUT_CLASS}
            data-testid="mcp-name-input"
            disabled={!connected || busy}
          />
          <select
            value={transport}
            onChange={(event) => setTransport(event.target.value === 'http' ? 'http' : 'stdio')}
            aria-label="传输方式"
            className={INPUT_CLASS}
            data-testid="mcp-transport-select"
            disabled={!connected || busy}
          >
            <option value="stdio">stdio</option>
            <option value="http">http</option>
          </select>
        </div>
        {transport === 'stdio' ? (
          <input
            type="text"
            value={command}
            onChange={(event) => setCommand(event.target.value)}
            placeholder="命令，例如 npx"
            aria-label="命令"
            className={INPUT_CLASS}
            data-testid="mcp-command-input"
            disabled={!connected || busy}
          />
        ) : (
          <input
            type="text"
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            placeholder="URL，例如 https://example.com/mcp"
            aria-label="URL"
            className={INPUT_CLASS}
            data-testid="mcp-url-input"
            disabled={!connected || busy}
          />
        )}
        <div className="flex flex-col gap-2 sm:flex-row">
          <input
            type="text"
            value={args}
            onChange={(event) => setArgs(event.target.value)}
            placeholder="参数（逗号分隔），例如 -y, @modelcontextprotocol/server-filesystem"
            aria-label="参数（逗号分隔）"
            className={INPUT_CLASS}
            data-testid="mcp-args-input"
            disabled={!connected || busy}
          />
          <input
            type="text"
            value={envKeys}
            onChange={(event) => setEnvKeys(event.target.value)}
            placeholder="环境变量名（逗号分隔；只登记名字，值不进 Hub）"
            aria-label="环境变量名（逗号分隔）"
            className={INPUT_CLASS}
            data-testid="mcp-envkeys-input"
            disabled={!connected || busy}
          />
        </div>
        <div className="flex items-center gap-3">
          <label className="flex w-fit items-center gap-1.5 text-xs text-content-secondary">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(event) => setEnabled(event.target.checked)}
              disabled={!connected || busy}
              aria-label="启用该 Server"
            />
            启用
          </label>
          <Button
            type="submit"
            variant="primary"
            size="sm"
            disabled={!connected || busy}
            data-testid="mcp-submit"
          >
            登记到 Hub
          </Button>
        </div>
      </form>

      <div className="mt-4">
        <p className="text-xs font-medium text-content-secondary">同步到 harness 配置</p>
        <div className="mt-2 space-y-2">
          {PROFILE_HARNESS_OPTIONS.map((option) => (
            <div
              key={option.id}
              className="flex flex-wrap items-center gap-2 rounded-md border border-white/[0.06] bg-white/[0.02] px-3 py-2 text-[11px]"
              data-testid={`mcp-sync-row-${option.id}`}
            >
              <span className="text-content-secondary">{option.label}</span>
              <span className="font-mono text-content-muted">
                {MCP_TARGET_HINTS[option.id] ?? ''}
              </span>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => void runPreview(option.id)}
                disabled={!connected || busy}
                data-testid={`mcp-preview-${option.id}`}
              >
                预览差异
              </Button>
              {preview !== null && preview.harnessId === option.id ? (
                <Button
                  variant="primary"
                  size="sm"
                  onClick={() => void applySync(option.id)}
                  disabled={!connected || busy}
                  data-testid={`mcp-apply-${option.id}`}
                >
                  确认同步
                </Button>
              ) : null}
              <Button
                variant="secondary"
                size="sm"
                className="ml-auto"
                onClick={() => void runImport(option.id)}
                disabled={!connected || busy}
                data-testid={`mcp-import-${option.id}`}
              >
                导入现有
              </Button>
            </div>
          ))}
        </div>

        <div className="mt-3" aria-live="polite">
          {preview !== null ? (
            <div data-testid="mcp-preview-result" className="text-xs text-content-secondary">
              <p>
                目标文件：
                <span className="font-mono text-content-primary">{preview.targetPath}</span>
              </p>
              <p className="mt-1">新增（{preview.additions.length}）</p>
              {preview.additions.length === 0 ? (
                <p className="text-content-muted">无新增</p>
              ) : (
                <ul className="ml-4 list-disc font-mono">
                  {preview.additions.map((addition) => (
                    <li key={addition}>{addition}</li>
                  ))}
                </ul>
              )}
              <p className="mt-1">更新（{preview.updates.length}）</p>
              {preview.updates.length === 0 ? (
                <p className="text-content-muted">无更新</p>
              ) : (
                <ul className="ml-4 list-disc">
                  {preview.updates.map((entry) => (
                    <li key={entry.name}>
                      <span className="font-mono text-content-primary">{entry.name}</span>
                      {entry.summary.length > 0 ? (
                        <span className="ml-2 text-content-muted">{entry.summary}</span>
                      ) : null}
                    </li>
                  ))}
                </ul>
              )}
              <p className="mt-1">移除（{preview.removals.length}）</p>
              {preview.removals.length === 0 ? (
                <p className="text-content-muted">无移除</p>
              ) : (
                <ul className="ml-4 list-disc">
                  {preview.removals.map((entry) => (
                    <li key={entry.name}>
                      <span className="font-mono text-content-primary">{entry.name}</span>
                      {entry.summary.length > 0 ? (
                        <span className="ml-2 text-content-muted">{entry.summary}</span>
                      ) : null}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          ) : null}

          {report !== null ? (
            <div data-testid="mcp-sync-report" className="text-xs text-content-secondary">
              <p>
                已写入 <span className="font-mono text-content-primary">{report.targetPath}</span>
                ，备份 id <span className="font-mono text-content-primary">{report.backupId}</span>
                ；新增 {report.added}、更新 {report.updated}、移除 {report.removed}。
              </p>
            </div>
          ) : null}

          {importReport !== null ? (
            <div data-testid="mcp-import-result" className="text-xs text-content-secondary">
              <p>导入成功（{importReport.imported.length}）</p>
              {importReport.imported.length === 0 ? (
                <p className="text-content-muted">无</p>
              ) : (
                <ul className="ml-4 list-disc font-mono">
                  {importReport.imported.map((imported) => (
                    <li key={imported}>{imported}</li>
                  ))}
                </ul>
              )}
              <p className="mt-1">跳过（{importReport.skipped.length}）</p>
              {importReport.skipped.length === 0 ? (
                <p className="text-content-muted">无</p>
              ) : (
                <ul className="ml-4 list-disc font-mono">
                  {importReport.skipped.map((skipped) => (
                    <li key={skipped}>{skipped}</li>
                  ))}
                </ul>
              )}
            </div>
          ) : null}
        </div>
      </div>
    </Card>
  );
}

/* ------------------------------------------------------------------ *
 * Skills 只读发现（ROADMAP E19 缩范围）
 * ------------------------------------------------------------------ */

/**
 * Skills 只读发现（ROADMAP E19 缩范围）：Hub 只扫描并展示
 * `~/.claude/skills/<name>/SKILL.md` 的 name / description。
 *
 * 纪律（spec 2026-10-31-skills-prompts-tray-design.md Non-goals）：
 * - claude 自己是 skills 的加载方（这是它的原生目录），Hub **不复制投影**、
 *   不修改任何 skill 文件，本区没有任何写操作按钮；
 * - 扫描是**纯读**，不需要 confirm；但渲染路径零 IPC —— 扫描也是显式动作，
 *   与本页其它区块同一纪律。
 */
function SkillsCard() {
  const connected = isTauriRuntime();
  const [report, setReport] = useState<SkillsReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const scan = async () => {
    setBusy(true);
    setError(null);
    const result = await listSkills();
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setReport(result.data);
  };

  return (
    <Card className="mb-6" data-testid="skills-card">
      <CardTitle>Skills（只读）</CardTitle>
      <CardDescription>
        {
          'Hub 只发现展示 ~/.claude/skills/<name>/SKILL.md：claude 自己是加载方，Hub 不复制投影、也不修改任何 skill 文件。'
        }
      </CardDescription>

      <div className="mt-3">
        <Button
          variant="secondary"
          size="sm"
          onClick={() => void scan()}
          disabled={!connected || busy}
          data-testid="skills-scan"
        >
          扫描 Skills
        </Button>
      </div>

      <div className="mt-3" aria-live="polite">
        {busy ? (
          <p className="flex items-center gap-2 text-xs text-content-muted">
            <Loader2 aria-hidden className="size-3.5 animate-spin text-accent" />
            正在扫描…
          </p>
        ) : null}

        {error !== null ? (
          <p className="text-xs text-signal-error" role="alert">
            {error}
          </p>
        ) : null}

        {report !== null && !busy ? (
          <>
            {report.skills.length === 0 ? (
              <p className="text-xs text-content-muted">
                没有发现任何 skill（~/.claude/skills 不存在或为空）
              </p>
            ) : (
              <ul
                className="overflow-hidden rounded-md border border-border-subtle bg-white/[0.03]"
                data-testid="skills-list"
              >
                {report.skills.map((skill) => (
                  <li
                    key={skill.name}
                    data-testid={`skills-row-${skill.name}`}
                    className="border-b border-border-subtle/60 px-3 py-1.5 text-[11px] last:border-b-0"
                  >
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-content-primary">{skill.name}</span>
                      <span className="min-w-0 truncate font-mono text-content-muted">
                        {skill.path}
                      </span>
                    </div>
                    {skill.description.length > 0 ? (
                      <p className="mt-0.5 text-content-secondary">{skill.description}</p>
                    ) : null}
                  </li>
                ))}
              </ul>
            )}

            {report.skipped.length > 0 ? (
              <p className="mt-2 text-xs text-content-muted" data-testid="skills-skipped">
                无法解析的条目（{report.skipped.length}）：
                <span className="font-mono">{report.skipped.join('、')}</span>
              </p>
            ) : null}
          </>
        ) : null}
      </div>
    </Card>
  );
}

/* ------------------------------------------------------------------ *
 * Python Sidecar 生命周期状态（F20，spec §5）
 * ------------------------------------------------------------------ */

const SIDECAR_STATE_STYLE: Record<SidecarState, string> = {
  stopped: 'border-white/10 bg-white/[0.05] text-content-muted',
  ready: 'border-signal-ok/30 bg-signal-ok/12 text-signal-ok',
  degraded: 'border-signal-error/30 bg-signal-error/12 text-signal-error',
};

const SIDECAR_ACTIONS: ReadonlyArray<{
  label: string;
  testId: string;
  run: () => Promise<IpcResult<SidecarStatus>>;
}> = [
  { label: '刷新', testId: 'sidecar-refresh', run: sidecarStatus },
  { label: '启动', testId: 'sidecar-start', run: sidecarStart },
  { label: '停止', testId: 'sidecar-stop', run: sidecarStop },
  { label: '重启', testId: 'sidecar-restart', run: sidecarRestart },
];

/**
 * Sidecar 状态卡片（F20）：只读状态 + 四个显式动作。
 *
 * 纪律（spec §5 / AGENTS.md）：
 * - **渲染路径零 IPC**：只在用户点按钮时才调用，与本页其它卡片同一纪律；
 * - 组件内**禁止** `invoke()`，全部走 `@/lib/ipc` 的 wrapper；
 * - Rust 侧所有失败都收敛为 `SidecarStatus{state:degraded}`，不向上传播为
 *   IPC 错误 —— degraded 只影响本卡片，主 Harness 功能不受影响。
 */
function SidecarCard() {
  const connected = isTauriRuntime();
  const [status, setStatus] = useState<SidecarStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const act = async (run: () => Promise<IpcResult<SidecarStatus>>) => {
    setBusy(true);
    setError(null);
    const result = await run();
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setStatus(result.data);
  };

  return (
    <Card className="mb-6" data-testid="sidecar-card">
      <CardTitle>Python Sidecar 状态</CardTitle>
      <CardDescription>
        stdio JSON-RPC 运行时（F20）。状态只在点击按钮时读取；所有失败降级为状态，不影响主 Harness
        功能。
      </CardDescription>

      <div className="mt-3 flex flex-wrap gap-2">
        {SIDECAR_ACTIONS.map(({ label, testId, run }) => (
          <Button
            key={testId}
            variant="secondary"
            size="sm"
            onClick={() => void act(run)}
            disabled={!connected || busy}
            data-testid={testId}
          >
            {label}
          </Button>
        ))}
      </div>

      <div className="mt-4" aria-live="polite">
        {busy ? (
          <p className="flex items-center gap-2 text-xs text-content-muted">
            <Loader2 aria-hidden className="size-3.5 animate-spin text-accent" />
            正在执行…
          </p>
        ) : null}

        {error !== null ? (
          <p className="text-xs text-signal-error" role="alert">
            {error}
          </p>
        ) : null}

        {status !== null && !busy ? (
          <div className="space-y-2 text-xs" data-testid="sidecar-status">
            <div className="flex items-center gap-2">
              <span
                className={`rounded border px-1.5 py-0.5 font-mono font-medium ${SIDECAR_STATE_STYLE[status.state]}`}
                data-testid="sidecar-state"
              >
                {status.state}
              </span>
              {status.state === 'degraded' ? (
                <span className="text-signal-error" data-testid="sidecar-degraded-warning">
                  Python sidecar 不可用，主 Harness 功能不受影响
                </span>
              ) : null}
            </div>

            <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
              <dt className="text-content-muted">PID</dt>
              <dd className="font-mono text-content-primary" data-testid="sidecar-pid">
                {status.pid ?? '—'}
              </dd>
              <dt className="text-content-muted">Runtime 版本</dt>
              <dd className="font-mono text-content-primary" data-testid="sidecar-runtime-version">
                {status.runtime?.version ?? '—'}
              </dd>
              <dt className="text-content-muted">Python 版本</dt>
              <dd className="font-mono text-content-primary" data-testid="sidecar-python">
                {status.runtime?.python ?? '—'}
              </dd>
              <dt className="text-content-muted">协议版本</dt>
              <dd className="font-mono text-content-primary" data-testid="sidecar-protocol">
                {status.protocolVersion ?? '—'}
              </dd>
              <dt className="text-content-muted">协商能力</dt>
              <dd className="font-mono text-content-primary" data-testid="sidecar-capabilities">
                {status.negotiatedCapabilities.length > 0
                  ? status.negotiatedCapabilities.join(', ')
                  : '—'}
              </dd>
              <dt className="text-content-muted">重启次数</dt>
              <dd className="font-mono text-content-primary" data-testid="sidecar-restart-count">
                {status.restartCount}
              </dd>
            </dl>

            {status.lastError !== null ? (
              <p
                className="text-xs text-signal-error"
                role="alert"
                data-testid="sidecar-last-error"
              >
                {status.lastError}
              </p>
            ) : null}
          </div>
        ) : null}
      </div>
    </Card>
  );
}

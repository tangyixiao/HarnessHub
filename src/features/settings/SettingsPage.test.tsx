import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { SettingsPage } from '@/features/settings/SettingsPage';

type Internals = { __TAURI_INTERNALS__?: unknown };

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

/**
 * 按命令名分派，便于验证「哪个命令在什么时候被调用」。
 *
 * 值可以是直接返回的数据，也可以是**惰性求值的函数**：要模拟失败必须用函数形式，
 * 否则 `Promise.reject(...)` 在桩定义时就被创建，等到真正 await 之前已是孤儿拒绝，
 * vitest 会报 unhandled rejection。
 */
function stubCommands(
  handlers: Record<string, unknown | (() => unknown)>,
): ReturnType<typeof vi.fn> {
  const invoke = vi.fn(async (command: string) => {
    if (!(command in handlers)) {
      throw new Error(`未预期的命令：${command}`);
    }
    const handler = handlers[command];
    return typeof handler === 'function' ? (handler as () => unknown)() : handler;
  });
  (window as unknown as Internals).__TAURI_INTERNALS__ = { invoke };
  return invoke;
}

/**
 * 与 Rust `ProfileSummary`（serde camelCase）同形，复刻自 Rust 契约测试
 * `profile_summary_serializes_with_camel_case_and_redaction`（E14）。
 */
const CLAUDE_PROFILE_PAYLOAD = {
  harnessId: 'claude',
  sourcePath: 'C:/Users/dev/.claude/settings.json',
  fields: {
    'env.ANTHROPIC_AUTH_TOKEN': '<redacted>',
    'env.ANTHROPIC_BASE_URL': 'https://api.example.com',
    'env.ANTHROPIC_MODEL': 'claude-sonnet-4-6',
  },
  parsedAt: '2026-10-25T10:00:00Z',
};

describe('SettingsPage — Provider 配置现状（E14，只读）', () => {
  /** E14 卡片内作用域：页面里其它区块（E15/E16/E17）也有同名 harness 按钮。 */
  function profileCard(): HTMLElement {
    return screen.getByTestId('provider-profile-card');
  }

  it('点击 Claude Code 后调 profile_discover 并传 harnessId', async () => {
    const invoke = stubCommands({ profile_discover: CLAUDE_PROFILE_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.click(within(profileCard()).getByRole('button', { name: 'Claude Code' }));

    expect(await screen.findByText('env.ANTHROPIC_BASE_URL')).toBeInTheDocument();

    expect(invoke.mock.calls.map((call) => call[0])).toEqual(['profile_discover']);
    expect(invoke.mock.calls[0]?.[1]).toEqual({ harnessId: 'claude' });
  });

  it('渲染 key=value 等宽行 + sourcePath 底注 + parsedAt', async () => {
    stubCommands({ profile_discover: CLAUDE_PROFILE_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.click(within(profileCard()).getByRole('button', { name: 'Claude Code' }));

    expect(await screen.findByText('env.ANTHROPIC_AUTH_TOKEN')).toBeInTheDocument();
    expect(screen.getByText('env.ANTHROPIC_BASE_URL')).toBeInTheDocument();
    expect(screen.getByText('env.ANTHROPIC_MODEL')).toBeInTheDocument();
    expect(screen.getByText('https://api.example.com')).toBeInTheDocument();
    expect(screen.getByText('claude-sonnet-4-6')).toBeInTheDocument();
    expect(screen.getByText('C:/Users/dev/.claude/settings.json')).toBeInTheDocument();
    expect(screen.getByText('2026-10-25T10:00:00Z')).toBeInTheDocument();
  });

  it('值为 <redacted> 的行原样展示值并加「已脱敏」徽标，非脱敏行不加', async () => {
    stubCommands({ profile_discover: CLAUDE_PROFILE_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.click(within(profileCard()).getByRole('button', { name: 'Claude Code' }));

    expect(await screen.findByText('<redacted>')).toBeInTheDocument();
    const badges = screen.getAllByTestId('profile-redacted-badge');
    expect(badges).toHaveLength(1);
    expect(badges[0]).toHaveTextContent('已脱敏');

    // 徽标只出现在 AUTH_TOKEN 那一行：用行级 testid 限定作用域。
    const tokenRow = screen.getByTestId('profile-field-env.ANTHROPIC_AUTH_TOKEN');
    expect(within(tokenRow).getByTestId('profile-redacted-badge')).toBeInTheDocument();
    const modelRow = screen.getByTestId('profile-field-env.ANTHROPIC_MODEL');
    expect(within(modelRow).queryByTestId('profile-redacted-badge')).not.toBeInTheDocument();
  });

  it('后端返回 null（配置文件不存在）时显示未初始化空态', async () => {
    stubCommands({ profile_discover: null });

    render(<SettingsPage />);
    await userEvent.click(within(profileCard()).getByRole('button', { name: 'Codex' }));

    expect(await screen.findByText('未找到配置文件（未初始化）')).toBeInTheDocument();
    expect(screen.queryByTestId(/^profile-field-/)).not.toBeInTheDocument();
  });

  it('invoke reject 时行内展示错误原因，而不是白屏', async () => {
    stubCommands({
      profile_discover: () => {
        throw new Error('claude 的配置发现尚未实现（当前支持 claude/codex/gemini）');
      },
    });

    render(<SettingsPage />);
    await userEvent.click(within(profileCard()).getByRole('button', { name: 'Claude Code' }));

    expect(await screen.findByText(/claude 的配置发现尚未实现/)).toBeInTheDocument();
  });

  it('本区只读：卡片内不出现修改 / 保存 / 应用类按钮，也不出现输入控件', async () => {
    stubCommands({ profile_discover: CLAUDE_PROFILE_PAYLOAD });

    render(<SettingsPage />);
    await screen.findByText('Provider 配置现状');

    // E15/E16/E17 区块有它们自己的编辑入口；本卡片必须保持纯展示。
    const card = profileCard();
    for (const forbidden of ['修改', '保存', '应用', '写入配置', '登记']) {
      expect(within(card).queryByRole('button', { name: forbidden })).not.toBeInTheDocument();
    }
    expect(within(card).queryByRole('textbox')).not.toBeInTheDocument();
  });

  it('浏览器模式下按钮不可点，避免发起注定失败的调用', async () => {
    render(<SettingsPage />);
    await screen.findByText('Provider 配置现状');

    for (const name of ['Claude Code', 'Codex', 'Gemini CLI']) {
      // E14/E16/E17 三个区块各有一组；同名按钮必须全部禁用。
      for (const button of screen.getAllByRole('button', { name })) {
        expect(button).toBeDisabled();
      }
    }
  });

  it('读取完成后按钮恢复可用（加载期间禁用防重复触发）', async () => {
    stubCommands({ profile_discover: CLAUDE_PROFILE_PAYLOAD });

    render(<SettingsPage />);
    const claudeButton = within(profileCard()).getByRole('button', { name: 'Claude Code' });
    await userEvent.click(claudeButton);
    await screen.findByText('env.ANTHROPIC_BASE_URL');

    expect(within(profileCard()).getByRole('button', { name: 'Codex' })).toBeEnabled();
    expect(claudeButton).toBeEnabled();
  });
});

/* ------------------------------------------------------------------ *
 * 安全投影（E15）/ 凭据（E16）/ 体检（E17）
 * ------------------------------------------------------------------ */

const PROJECTION_REPORT_PAYLOAD = {
  targetPath: 'C:/Users/dev/.claude/settings.json',
  backupId: '00000000000000000000-000001',
  changedKeys: 1,
};

const BACKUP_PAYLOAD = {
  id: '00000000000000000000-000001',
  harnessId: 'claude',
  createdAt: '2026-10-30T00:00:00Z',
  path: 'C:/Users/dev/.harness-hub/backups/claude/00000000000000000000-000001.json',
  targetPath: 'C:/Users/dev/.claude/settings.json',
};

const CREDENTIAL_PAYLOAD = {
  id: 'ref-1',
  label: 'deepseek-key',
  harnessId: 'codex',
  storeKind: 'file',
  createdAt: '2026-10-30T00:00:00Z',
};

const CHECK_ITEMS_PAYLOAD = [
  {
    check: 'config_parseable',
    status: 'ok',
    message: 'settings.json 可解析',
    suggestion: null,
  },
  {
    check: 'credentials_reachable',
    status: 'warn',
    message: '未登记凭据句柄（使用官方登录则无需登记）',
    suggestion: '如需 API Key Provider，在凭据区登记后重新体检',
  },
];

describe('SettingsPage — 安全投影（E15，显式写）', () => {
  function projectionCard(): HTMLElement {
    return screen.getByTestId('projection-card');
  }

  it('填一项字段 → confirm 文案含目标路径 → profile_project 只带填了的键', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const invoke = stubCommands({ profile_project: PROJECTION_REPORT_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.type(
      within(projectionCard()).getByLabelText('ANTHROPIC_BASE_URL'),
      'https://api.example.com',
    );
    await userEvent.click(within(projectionCard()).getByTestId('projection-apply'));

    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(String(confirmSpy.mock.calls[0]?.[0])).toContain('~/.claude/settings.json');

    expect(invoke.mock.calls[0]?.[0]).toBe('profile_project');
    expect(invoke.mock.calls[0]?.[1]).toEqual({
      harnessId: 'claude',
      fields: { 'env.ANTHROPIC_BASE_URL': 'https://api.example.com' },
    });
    expect(await within(projectionCard()).findByTestId('projection-report')).toBeInTheDocument();
  });

  it('全部留空 → 不发起调用，行内提示而不是错误', async () => {
    const invoke = stubCommands({ profile_project: PROJECTION_REPORT_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.click(within(projectionCard()).getByTestId('projection-apply'));

    expect(invoke.mock.calls).toHaveLength(0);
    expect(await within(projectionCard()).findByText(/没有要写入的字段/)).toBeInTheDocument();
  });

  it('用户取消 confirm → 不写盘', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    const invoke = stubCommands({ profile_project: PROJECTION_REPORT_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.type(
      within(projectionCard()).getByLabelText('ANTHROPIC_MODEL'),
      'claude-sonnet-4-6',
    );
    await userEvent.click(within(projectionCard()).getByTestId('projection-apply'));

    expect(invoke.mock.calls).toHaveLength(0);
  });

  it('查看备份 → profile_backups；恢复 → confirm 后 profile_restore', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const invoke = stubCommands({
      profile_backups: [BACKUP_PAYLOAD],
      profile_restore: null,
    });

    render(<SettingsPage />);
    await userEvent.click(within(projectionCard()).getByRole('button', { name: '查看备份' }));

    expect(invoke.mock.calls[0]?.[0]).toBe('profile_backups');
    expect(invoke.mock.calls[0]?.[1]).toEqual({ harnessId: 'claude' });
    expect(await screen.findByTestId('projection-backups')).toBeInTheDocument();

    await userEvent.click(screen.getByTestId('projection-restore-00000000000000000000-000001'));

    expect(invoke.mock.calls[1]?.[0]).toBe('profile_restore');
    expect(invoke.mock.calls[1]?.[1]).toEqual({
      harnessId: 'claude',
      backupId: '00000000000000000000-000001',
    });
  });
});

describe('SettingsPage — 凭据句柄（E16，值只进不出）', () => {
  function credentialsCard(): HTMLElement {
    return screen.getByTestId('credentials-card');
  }

  it('选中 harness → credential_list 拉句柄列表（无值字段）', async () => {
    const invoke = stubCommands({ credential_list: [CREDENTIAL_PAYLOAD] });

    render(<SettingsPage />);
    await userEvent.click(within(credentialsCard()).getByTestId('credential-tab-codex'));

    expect(invoke.mock.calls[0]?.[0]).toBe('credential_list');
    expect(invoke.mock.calls[0]?.[1]).toEqual({ harnessId: 'codex' });
    expect(await screen.findByTestId('credential-row-deepseek-key')).toBeInTheDocument();
    expect(screen.queryByText('sk-secret-123')).not.toBeInTheDocument();
  });

  it('登记 → credential_register 收到值（请求方向），登记后输入清空并刷新列表', async () => {
    const invoke = stubCommands({
      credential_register: CREDENTIAL_PAYLOAD,
      credential_list: [CREDENTIAL_PAYLOAD],
    });

    render(<SettingsPage />);
    await userEvent.click(within(credentialsCard()).getByTestId('credential-tab-codex'));
    await screen.findByTestId('credential-row-deepseek-key');

    await userEvent.type(within(credentialsCard()).getByLabelText('凭据标签'), 'deepseek-key');
    await userEvent.type(within(credentialsCard()).getByLabelText('凭据值'), 'sk-secret-123');
    await userEvent.click(within(credentialsCard()).getByRole('button', { name: '登记' }));

    expect(invoke.mock.calls[1]?.[0]).toBe('credential_register');
    expect(invoke.mock.calls[1]?.[1]).toEqual({
      harnessId: 'codex',
      label: 'deepseek-key',
      value: 'sk-secret-123',
    });
    // 值的输入框在登记后清空：不做任何回显。
    expect((within(credentialsCard()).getByLabelText('凭据值') as HTMLInputElement).value).toBe('');
  });

  it('删除 → confirm 提到 store 一并删除 → credential_remove', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const invoke = stubCommands({
      credential_list: [CREDENTIAL_PAYLOAD],
      credential_remove: true,
    });

    render(<SettingsPage />);
    await userEvent.click(within(credentialsCard()).getByTestId('credential-tab-codex'));
    await screen.findByTestId('credential-row-deepseek-key');
    await userEvent.click(screen.getByTestId('credential-remove-deepseek-key'));

    expect(confirmSpy.mock.calls[0]?.[0]).toContain('Secret Store');
    expect(invoke.mock.calls[1]?.[0]).toBe('credential_remove');
    expect(invoke.mock.calls[1]?.[1]).toEqual({ id: 'ref-1' });
  });
});

describe('SettingsPage — 配置体检（E17，只读 + 建议）', () => {
  function doctorCard(): HTMLElement {
    return screen.getByTestId('doctor-card');
  }

  it('点击 harness → config_doctor → 检查项逐条渲染（含建议）', async () => {
    const invoke = stubCommands({ config_doctor: CHECK_ITEMS_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.click(within(doctorCard()).getByTestId('doctor-run-claude'));

    expect(invoke.mock.calls[0]?.[0]).toBe('config_doctor');
    expect(invoke.mock.calls[0]?.[1]).toEqual({ harnessId: 'claude' });

    expect(await screen.findByTestId('doctor-item-config_parseable')).toBeInTheDocument();
    expect(screen.getByTestId('doctor-item-credentials_reachable')).toBeInTheDocument();
    expect(screen.getByText('settings.json 可解析')).toBeInTheDocument();
    expect(screen.getByText(/建议：如需 API Key Provider/)).toBeInTheDocument();
  });

  it('本区没有 Fix 按钮：修复只给建议，不提供自动动作', async () => {
    stubCommands({ config_doctor: CHECK_ITEMS_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.click(within(doctorCard()).getByTestId('doctor-run-claude'));
    await screen.findByTestId('doctor-item-config_parseable');

    expect(within(doctorCard()).queryByRole('button', { name: 'Fix' })).not.toBeInTheDocument();
    expect(within(doctorCard()).queryByRole('button', { name: '修复' })).not.toBeInTheDocument();
  });

  it('体检失败时行内展示错误原因，而不是白屏', async () => {
    stubCommands({
      config_doctor: () => {
        throw new Error('无法确定 home 目录');
      },
    });

    render(<SettingsPage />);
    await userEvent.click(within(doctorCard()).getByTestId('doctor-run-claude'));

    expect(await within(doctorCard()).findByText(/无法确定 home 目录/)).toBeInTheDocument();
    expect(screen.queryByTestId('doctor-items')).not.toBeInTheDocument();
  });
});

/* ------------------------------------------------------------------ *
 * MCP Binding Registry（E18）fixtures —— 与 Rust 契约（serde camelCase）
 * 同形，复刻自 src/lib/ipc.test.ts 的 MCP_SERVER_PAYLOAD 系列。
 * ------------------------------------------------------------------ */

/** `mcp_server_list` 的返回项之一：stdio 传输。 */
const MCP_SERVER_STDIO_PAYLOAD = {
  id: 'srv-1',
  name: 'filesystem',
  transport: 'stdio',
  command: 'npx',
  args: ['-y', '@modelcontextprotocol/server-filesystem', 'C:/project'],
  url: null,
  envKeys: ['FILESYSTEM_ROOT'],
  enabled: true,
  source: 'hub',
  createdAt: '2026-10-25T10:00:00Z',
  updatedAt: '2026-11-01T09:30:00Z',
};

/** `mcp_server_list` 的返回项之一：http 传输、已停用。 */
const MCP_SERVER_HTTP_PAYLOAD = {
  id: 'srv-2',
  name: 'fetch',
  transport: 'http',
  command: null,
  args: [],
  url: 'https://mcp.example.com/sse',
  envKeys: [],
  enabled: false,
  source: 'imported:claude',
  createdAt: '2026-10-25T10:00:00Z',
  updatedAt: '2026-11-01T09:30:00Z',
};

/** `mcp_binding_list` 的返回项之一：global 作用域绑定（harnessId 为 null）。 */
const MCP_BINDING_GLOBAL_PAYLOAD = {
  id: 'bind-1',
  serverId: 'srv-1',
  serverName: 'filesystem',
  scope: 'global',
  harnessId: null,
  createdAt: '2026-11-01T09:30:00Z',
};

/** `mcp_sync_preview` 的返回项（updates / removals 是带 summary 的 diff 条目）。 */
const MCP_SYNC_PREVIEW_PAYLOAD = {
  harnessId: 'claude',
  targetPath: 'C:/Users/dev/.claude.json',
  additions: ['fetch'],
  updates: [{ name: 'filesystem', summary: '命令从 npx 改为 bunx' }],
  removals: [{ name: 'legacy', summary: 'Hub 侧已删除该 server' }],
};

/** `mcp_sync_apply` 的返回项（写前备份的 id 用于回溯）。 */
const MCP_SYNC_REPORT_PAYLOAD = {
  targetPath: 'C:/Users/dev/.claude.json',
  backupId: 'bkp-20261101',
  added: 1,
  updated: 1,
  removed: 1,
};

/** `mcp_import` 的返回项（imported / skipped 都是 harness 配置里的键名）。 */
const MCP_IMPORT_REPORT_PAYLOAD = {
  harnessId: 'claude',
  imported: ['legacy'],
  skipped: ['bad key!'],
};

describe('SettingsPage — MCP Servers（E18）', () => {
  it('点「加载列表」调 mcp_server_list + mcp_binding_list，并渲染 stdio / http 服务器行与绑定勾选', async () => {
    const invoke = stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD, MCP_SERVER_HTTP_PAYLOAD],
      mcp_binding_list: [MCP_BINDING_GLOBAL_PAYLOAD],
    });

    render(<SettingsPage />);
    // 渲染路径零 IPC：列表加载是显式动作。
    expect(invoke.mock.calls.every((call) => !call[0].startsWith('mcp_'))).toBe(true);
    await userEvent.click(screen.getByTestId('mcp-load'));

    const list = await screen.findByTestId('mcp-server-list');
    expect(invoke.mock.calls.map((call) => call[0])).toEqual([
      'mcp_server_list',
      'mcp_binding_list',
    ]);

    const stdioRow = within(list).getByTestId('mcp-server-row-filesystem');
    expect(within(stdioRow).getByText('filesystem')).toBeInTheDocument();
    expect(within(stdioRow).getByText('stdio')).toBeInTheDocument();
    expect(within(stdioRow).getByText('npx')).toBeInTheDocument();
    expect(within(stdioRow).getByText('已启用')).toBeInTheDocument();
    // global 绑定已勾选；harness 绑定未勾选（勾选态完全由 binding 列表推导）。
    expect(within(stdioRow).getByRole('checkbox', { name: 'Global' })).toBeChecked();
    expect(within(stdioRow).getByRole('checkbox', { name: 'Claude Code' })).not.toBeChecked();

    const httpRow = within(list).getByTestId('mcp-server-row-fetch');
    expect(within(httpRow).getByText('http')).toBeInTheDocument();
    expect(within(httpRow).getByText('https://mcp.example.com/sse')).toBeInTheDocument();
    expect(within(httpRow).getByText('已停用')).toBeInTheDocument();
  });

  it('预览差异：展示 targetPath 与 additions / updates / removals 分组（removals 逐条）', async () => {
    const invoke = stubCommands({
      mcp_server_list: [],
      mcp_binding_list: [],
      mcp_sync_preview: MCP_SYNC_PREVIEW_PAYLOAD,
    });

    render(<SettingsPage />);
    await userEvent.click(await screen.findByTestId('mcp-preview-claude'));

    const result = await screen.findByTestId('mcp-preview-result');
    expect(within(result).getByText('C:/Users/dev/.claude.json')).toBeInTheDocument();
    expect(within(result).getByText('fetch')).toBeInTheDocument();
    expect(within(result).getByText('filesystem')).toBeInTheDocument();
    expect(within(result).getByText('命令从 npx 改为 bunx')).toBeInTheDocument();
    expect(within(result).getByText('legacy')).toBeInTheDocument();
    expect(within(result).getByText('Hub 侧已删除该 server')).toBeInTheDocument();

    const previewCall = invoke.mock.calls.find((call) => call[0] === 'mcp_sync_preview');
    expect(previewCall?.[1]).toEqual({ harnessId: 'claude' });
  });

  it('预览差异：removals 为空时显示「无移除」', async () => {
    stubCommands({
      mcp_server_list: [],
      mcp_binding_list: [],
      mcp_sync_preview: { ...MCP_SYNC_PREVIEW_PAYLOAD, harnessId: 'codex', removals: [] },
    });

    render(<SettingsPage />);
    await userEvent.click(await screen.findByTestId('mcp-preview-codex'));

    expect(await screen.findByText('无移除')).toBeInTheDocument();
  });

  it('确认同步：confirm 文案含 targetPath，mcp_sync_apply 带 harnessId，报告含 backupId', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const invoke = stubCommands({
      mcp_server_list: [],
      mcp_binding_list: [],
      mcp_sync_preview: MCP_SYNC_PREVIEW_PAYLOAD,
      mcp_sync_apply: MCP_SYNC_REPORT_PAYLOAD,
    });

    render(<SettingsPage />);
    await userEvent.click(await screen.findByTestId('mcp-preview-claude'));
    await screen.findByTestId('mcp-preview-result');
    await userEvent.click(screen.getByTestId('mcp-apply-claude'));

    expect(String(confirmSpy.mock.calls[0]?.[0])).toContain('C:/Users/dev/.claude.json');

    const applyCall = invoke.mock.calls.find((call) => call[0] === 'mcp_sync_apply');
    expect(applyCall?.[0]).toBe('mcp_sync_apply');
    expect(applyCall?.[1]).toEqual({ harnessId: 'claude' });

    const report = await screen.findByTestId('mcp-sync-report');
    expect(within(report).getByText('bkp-20261101')).toBeInTheDocument();
    expect(within(report).getByText('C:/Users/dev/.claude.json')).toBeInTheDocument();
  });

  it('导入现有：confirm（含 source-of-truth 说明）后调 mcp_import 并渲染 imported / skipped', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const invoke = stubCommands({
      mcp_server_list: [],
      mcp_binding_list: [],
      mcp_import: MCP_IMPORT_REPORT_PAYLOAD,
    });

    render(<SettingsPage />);
    await userEvent.click(await screen.findByTestId('mcp-import-claude'));

    expect(String(confirmSpy.mock.calls[0]?.[0])).toContain('source-of-truth');

    const importCall = invoke.mock.calls.find((call) => call[0] === 'mcp_import');
    expect(importCall?.[1]).toEqual({ harnessId: 'claude' });

    const result = await screen.findByTestId('mcp-import-result');
    expect(within(result).getByText('legacy')).toBeInTheDocument();
    expect(within(result).getByText('bad key!')).toBeInTheDocument();
  });

  it('新增表单：transport 切换 command / url，stdio 提交时 args / envKeys 按逗号解析', async () => {
    const invoke = stubCommands({
      mcp_server_list: [],
      mcp_binding_list: [],
      mcp_server_upsert: MCP_SERVER_STDIO_PAYLOAD,
    });

    render(<SettingsPage />);
    await userEvent.click(screen.getByTestId('mcp-load'));
    await screen.findByText(
      '还没有登记任何 MCP Server；用下面的表单新增，或用「导入现有」从 harness 配置导入。',
    );

    // 默认 stdio：command 输入可见、url 输入不可见。
    expect(screen.getByTestId('mcp-command-input')).toBeInTheDocument();
    expect(screen.queryByTestId('mcp-url-input')).not.toBeInTheDocument();
    await userEvent.type(screen.getByTestId('mcp-name-input'), 'filesystem');

    // 切到 http：url 输入出现、command 消失。
    await userEvent.selectOptions(screen.getByTestId('mcp-transport-select'), 'http');
    expect(screen.getByTestId('mcp-url-input')).toBeInTheDocument();
    expect(screen.queryByTestId('mcp-command-input')).not.toBeInTheDocument();

    // 切回 stdio 后提交，验证输入形状（逗号分隔解析、http 字段为 null）。
    await userEvent.selectOptions(screen.getByTestId('mcp-transport-select'), 'stdio');
    await userEvent.type(screen.getByTestId('mcp-command-input'), 'npx');
    await userEvent.type(
      screen.getByTestId('mcp-args-input'),
      '-y, @modelcontextprotocol/server-filesystem',
    );
    await userEvent.type(screen.getByTestId('mcp-envkeys-input'), 'FILESYSTEM_ROOT, API_KEY');
    await userEvent.click(screen.getByTestId('mcp-submit'));

    const upsertCall = invoke.mock.calls.find((call) => call[0] === 'mcp_server_upsert');
    expect(upsertCall?.[1]).toEqual({
      input: {
        id: null,
        name: 'filesystem',
        transport: 'stdio',
        command: 'npx',
        args: ['-y', '@modelcontextprotocol/server-filesystem'],
        url: null,
        envKeys: ['FILESYSTEM_ROOT', 'API_KEY'],
        enabled: true,
      },
    });
  });

  it('删除服务器：confirm 后调 mcp_server_remove', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const invoke = stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_binding_list: [],
      mcp_server_remove: true,
    });

    render(<SettingsPage />);
    await userEvent.click(screen.getByTestId('mcp-load'));
    await userEvent.click(await screen.findByTestId('mcp-server-remove-filesystem'));

    expect(String(confirmSpy.mock.calls[0]?.[0])).toContain('filesystem');
    const removeCall = invoke.mock.calls.find((call) => call[0] === 'mcp_server_remove');
    expect(removeCall?.[1]).toEqual({ id: 'srv-1' });
  });

  it('浏览器模式：MCP 区块按钮禁用、不发调用', async () => {
    render(<SettingsPage />);
    await screen.findByTestId('mcp-card');

    expect(screen.getByTestId('mcp-preview-claude')).toBeDisabled();
    expect(screen.getByTestId('mcp-import-codex')).toBeDisabled();
    expect(screen.getByTestId('mcp-submit')).toBeDisabled();
  });
});

/* ------------------------------------------------------------------ *
 * Skills 只读发现（E19 缩范围）fixture —— 与 Rust `SkillsReport`
 * （serde camelCase）同形，复刻自 src/lib/ipc.test.ts 的 SKILLS_REPORT_PAYLOAD。
 * ------------------------------------------------------------------ */

/** `skills_list` 的返回项：一个正常 skill + 一个无法解析的目录。 */
const SKILLS_REPORT_PAYLOAD = {
  skills: [
    {
      name: 'pdf-tools',
      description: '处理 PDF 的工具',
      path: 'C:/Users/dev/.claude/skills/pdf-tools/SKILL.md',
    },
    {
      name: 'commit-helper',
      description: '',
      path: 'C:/Users/dev/.claude/skills/commit-helper/SKILL.md',
    },
  ],
  skipped: ['broken-dir'],
};

describe('SettingsPage — Skills 只读发现（E19 缩范围）', () => {
  function skillsCard(): HTMLElement {
    return screen.getByTestId('skills-card');
  }

  it('扫描 → skills_list 命令名（无参），列表渲染 name / description 与 skipped 计数行', async () => {
    const invoke = stubCommands({ skills_list: SKILLS_REPORT_PAYLOAD });

    render(<SettingsPage />);
    // 渲染路径零 IPC：扫描是显式动作。
    expect(invoke.mock.calls).toHaveLength(0);
    await userEvent.click(within(skillsCard()).getByTestId('skills-scan'));

    expect(invoke.mock.calls[0]?.[0]).toBe('skills_list');

    const list = await within(skillsCard()).findByTestId('skills-list');
    const pdfRow = within(list).getByTestId('skills-row-pdf-tools');
    expect(within(pdfRow).getByText('pdf-tools')).toBeInTheDocument();
    expect(within(pdfRow).getByText('处理 PDF 的工具')).toBeInTheDocument();
    const commitRow = within(list).getByTestId('skills-row-commit-helper');
    expect(within(commitRow).getByText('commit-helper')).toBeInTheDocument();

    const skippedLine = within(skillsCard()).getByTestId('skills-skipped');
    expect(skippedLine).toHaveTextContent('无法解析的条目（1）');
    expect(skippedLine).toHaveTextContent('broken-dir');
  });

  it('空目录：显示空态文案，不渲染列表', async () => {
    stubCommands({ skills_list: { skills: [], skipped: [] } });

    render(<SettingsPage />);
    await userEvent.click(within(skillsCard()).getByTestId('skills-scan'));

    expect(
      await within(skillsCard()).findByText('没有发现任何 skill（~/.claude/skills 不存在或为空）'),
    ).toBeInTheDocument();
    expect(within(skillsCard()).queryByTestId('skills-list')).not.toBeInTheDocument();
  });

  it('扫描失败时行内展示错误原因，而不是白屏', async () => {
    stubCommands({
      skills_list: () => {
        throw new Error('无法确定 home 目录');
      },
    });

    render(<SettingsPage />);
    await userEvent.click(within(skillsCard()).getByTestId('skills-scan'));

    expect(await within(skillsCard()).findByText(/无法确定 home 目录/)).toBeInTheDocument();
    expect(within(skillsCard()).queryByTestId('skills-list')).not.toBeInTheDocument();
  });

  it('本区纯读：只有「扫描 Skills」一个按钮，没有任何写操作按钮与输入控件', async () => {
    stubCommands({ skills_list: SKILLS_REPORT_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.click(within(skillsCard()).getByTestId('skills-scan'));
    await within(skillsCard()).findByTestId('skills-list');

    const card = skillsCard();
    expect(within(card).getAllByRole('button')).toHaveLength(1);
    expect(within(card).getByRole('button', { name: '扫描 Skills' })).toBeInTheDocument();
    for (const forbidden of ['写入', '保存', '应用', '删除', '投影']) {
      expect(within(card).queryByRole('button', { name: forbidden })).not.toBeInTheDocument();
    }
    expect(within(card).queryByRole('textbox')).not.toBeInTheDocument();
  });

  it('浏览器模式：扫描按钮禁用', async () => {
    render(<SettingsPage />);
    await screen.findByTestId('skills-card');

    expect(screen.getByTestId('skills-scan')).toBeDisabled();
  });
});

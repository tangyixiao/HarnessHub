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
  it('点击 Claude Code 后调 profile_discover 并传 harnessId', async () => {
    const invoke = stubCommands({ profile_discover: CLAUDE_PROFILE_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.click(screen.getByRole('button', { name: 'Claude Code' }));

    expect(await screen.findByText('env.ANTHROPIC_BASE_URL')).toBeInTheDocument();

    expect(invoke.mock.calls.map((call) => call[0])).toEqual(['profile_discover']);
    expect(invoke.mock.calls[0]?.[1]).toEqual({ harnessId: 'claude' });
  });

  it('渲染 key=value 等宽行 + sourcePath 底注 + parsedAt', async () => {
    stubCommands({ profile_discover: CLAUDE_PROFILE_PAYLOAD });

    render(<SettingsPage />);
    await userEvent.click(screen.getByRole('button', { name: 'Claude Code' }));

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
    await userEvent.click(screen.getByRole('button', { name: 'Claude Code' }));

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
    await userEvent.click(screen.getByRole('button', { name: 'Codex' }));

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
    await userEvent.click(screen.getByRole('button', { name: 'Claude Code' }));

    expect(await screen.findByText(/claude 的配置发现尚未实现/)).toBeInTheDocument();
  });

  it('本区只读：不出现修改 / 保存 / 应用类按钮（投影是 E15）', async () => {
    stubCommands({ profile_discover: CLAUDE_PROFILE_PAYLOAD });

    render(<SettingsPage />);
    await screen.findByText('Provider 配置现状');

    for (const forbidden of ['修改', '保存', '应用']) {
      expect(screen.queryByRole('button', { name: forbidden })).not.toBeInTheDocument();
    }
    // 数据区里也不允许出现输入控件：现状是展示，不是编辑。
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  });

  it('浏览器模式下按钮不可点，避免发起注定失败的调用', async () => {
    render(<SettingsPage />);
    await screen.findByText('Provider 配置现状');

    for (const name of ['Claude Code', 'Codex', 'Gemini CLI']) {
      expect(screen.getByRole('button', { name })).toBeDisabled();
    }
  });

  it('读取完成后按钮恢复可用（加载期间禁用防重复触发）', async () => {
    stubCommands({ profile_discover: CLAUDE_PROFILE_PAYLOAD });

    render(<SettingsPage />);
    const claudeButton = screen.getByRole('button', { name: 'Claude Code' });
    await userEvent.click(claudeButton);
    await screen.findByText('env.ANTHROPIC_BASE_URL');

    expect(screen.getByRole('button', { name: 'Codex' })).toBeEnabled();
    expect(claudeButton).toBeEnabled();
  });
});

import userEvent from '@testing-library/user-event';
import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ReplayPage } from '@/features/replay/ReplayPage';

type Internals = { __TAURI_INTERNALS__?: unknown };

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

function stubTauri(
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>,
): void {
  (window as unknown as Internals).__TAURI_INTERNALS__ = { invoke };
}

/**
 * 与 Rust `HarnessRegistry::summaries()`（serde camelCase）同形：
 * claude 的 capability replay = true（B6 已实现），codex 为 false。
 */
const CLAUDE_HARNESS = {
  id: 'claude',
  displayName: 'Claude Code',
  installationId: 'claude@local',
  installed: true,
  binaryPath: 'C:/Users/dev/.local/bin/claude.exe',
  version: '2.0.76',
  capabilities: {
    launch: true,
    terminal: true,
    resume: true,
    usage: true,
    replay: true,
    toolCalls: false,
    subagents: false,
    liveState: false,
    worktree: false,
  },
  dataPaths: ['C:/Users/dev/.claude'],
};

const CODEX_HARNESS = {
  id: 'codex',
  displayName: 'Codex',
  installationId: 'codex@local',
  installed: true,
  binaryPath: 'D:/npm-global/codex.cmd',
  version: '0.152.1',
  capabilities: {
    launch: true,
    terminal: true,
    resume: true,
    usage: false,
    replay: false,
    toolCalls: false,
    subagents: false,
    liveState: false,
    worktree: false,
  },
  dataPaths: ['C:/Users/dev/.codex'],
};

/**
 * 与 Rust `ReplaySessionSummary` / `ReplaySession`（serde camelCase）同形，
 * 形状手工复刻自 Rust 契约测试 `replay_dtos_serialize_with_camel_case_keys`；
 * B8 起含 toolCalls / subagentThreads 嵌套形状。
 */
const SESSION_SUMMARY = {
  harnessId: 'claude',
  sessionId: 'b3a1b3a1-0000-4000-8000-000000000003',
  projectDir: 'D--work-demo',
  sizeBytes: 1024,
  modifiedMs: 1_761_000_000_000,
  firstUserPreview: '帮我看看这个报错',
  startedAt: '2026-10-25T10:00:00.000Z',
  lastAt: '2026-10-25T10:01:00.000Z',
};

const REPLAY_SESSION = {
  harnessId: 'claude',
  sessionId: 'b3a1b3a1-0000-4000-8000-000000000003',
  sourcePath: 'C:/Users/dev/.claude/projects/D--work-demo/b3a1.jsonl',
  messages: [
    {
      seq: 0,
      role: 'user',
      sidechain: false,
      timestamp: '2026-10-25T10:00:00.000Z',
      textParts: ['帮我看看这个报错'],
      thinkingParts: [],
      model: null,
      toolCalls: [],
      // B8 起 tool_use 展开为 toolCalls；仍不展开的种类（如 image）才进计数。
      unsupportedContentKinds: { image: 1 },
    },
    {
      seq: 1,
      role: 'assistant',
      sidechain: true,
      timestamp: '2026-10-25T10:00:05.000Z',
      textParts: ['回答正文'],
      thinkingParts: ['先分析一下报错原因'],
      model: 'claude-sonnet-4-6',
      toolCalls: [
        {
          id: 'call_1',
          name: 'Grep',
          inputJson: '{"pattern":"TODO"}',
          resultJson: 'src/a.rs',
          resultTruncated: false,
          inputTruncated: false,
        },
      ],
      unsupportedContentKinds: {},
    },
  ],
  subagentThreads: [
    {
      messageSeqs: [1],
      startedAt: '2026-10-25T10:00:05.000Z',
      lastAt: '2026-10-25T10:00:09.000Z',
    },
  ],
  // BTreeMap 序列化按键排序：file-history-snapshot 在 summary 之前。
  skippedKindCounts: { 'file-history-snapshot': 1, summary: 1 },
  malformedLines: 2,
};

/**
 * 与 Rust `SessionRecord`（serde camelCase）同形：B9 `resume_terminal` 的返回。
 * `launchMode = "resume"`、`sourceSessionId` 为传入的外部会话 id。
 */
const RESUMED_SESSION_RECORD = {
  hubSessionId: 'hub-resume-1',
  sourceSessionId: 'b3a1b3a1-0000-4000-8000-000000000003',
  harnessId: 'claude',
  installationId: 'claude@local',
  projectId: null,
  runtimeTargetId: 'local',
  parentSessionId: null,
  status: 'running',
  launchMode: 'resume',
  cwd: null,
  worktreePath: null,
  startedAt: '2026-10-25T11:00:00.000Z',
  endedAt: null,
  exitCode: null,
  terminationReason: null,
  pid: 4242,
};

/**
 * B9 的 resume_terminal 走 Tauri Channel：stub 里必须补 transformCallback
 * （真实环境由 Tauri 注入），否则 `new Channel()` 在 jsdom 里直接抛。
 */
function stubTauriWithChannel(
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>,
): void {
  (window as unknown as Internals).__TAURI_INTERNALS__ = {
    invoke,
    transformCallback: () => 1,
  };
}

describe('ReplayPage', () => {
  it('有 replay 能力时渲染会话列表，点击后渲染只读时间线（消息正文、思考、子代理徽标、skipped 脚注）', async () => {
    const user = userEvent.setup();
    const invokeSpy = vi.fn(async (cmd: string, _args?: Record<string, unknown>) => {
      if (cmd === 'list_harnesses') return [CODEX_HARNESS, CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return REPLAY_SESSION;
      return null;
    });
    stubTauri(invokeSpy);

    render(<ReplayPage />);

    // 左列：按后端顺序渲染会话摘要。
    expect(await screen.findByText('D--work-demo')).toBeInTheDocument();
    expect(screen.getByText('帮我看看这个报错')).toBeInTheDocument();
    expect(screen.getByText(/2026-10-25 10:00:00 ~ 2026-10-25 10:01:00/)).toBeInTheDocument();
    expect(screen.getByText(/1\.0 KiB/)).toBeInTheDocument();

    // 点击会话 → 右侧时间线。
    await user.click(screen.getByRole('button', { name: /D--work-demo/ }));

    expect(await screen.findByText('回答正文')).toBeInTheDocument();
    // user 消息正文同时出现在列表预览与时间线里，共两处。
    expect(await screen.findAllByText('帮我看看这个报错')).toHaveLength(2);
    // thinkingParts 渲染为「思考」标注区，不与正文混排。
    expect(screen.getByText('思考')).toBeInTheDocument();
    expect(screen.getByText('先分析一下报错原因')).toBeInTheDocument();
    // sidechain 消息带「子代理」徽标。
    expect(screen.getByText('子代理')).toBeInTheDocument();
    // B8：toolCalls 展开为等宽工具卡（工具名 + 原样 inputJson + 「→ 结果」）。
    expect(screen.getByRole('list', { name: '工具调用' })).toBeInTheDocument();
    expect(screen.getByText('Grep')).toBeInTheDocument();
    expect(screen.getByText('{"pattern":"TODO"}')).toBeInTheDocument();
    expect(screen.getByText('→ 结果')).toBeInTheDocument();
    expect(screen.getByText('src/a.rs')).toBeInTheDocument();
    // 子代理线程区在时间线下方渲染（seq 列表 + 起止时间）。
    expect(screen.getByRole('region', { name: '子代理线程' })).toBeInTheDocument();
    expect(
      screen.getByRole('button', {
        name: /线程 1 · #1 · 2026-10-25 10:00:05 ~ 2026-10-25 10:00:09/,
      }),
    ).toBeInTheDocument();
    // unsupportedContentKinds 非空时保留计数脚注（tool_use 已展开，剩下的才是未展开）。
    expect(screen.getByText(/未展开内容：image ×1/)).toBeInTheDocument();
    // 底部脚注如实渲染 skippedKindCounts 与 malformedLines。
    expect(
      screen.getByText(/已跳过 2 行非消息记录（file-history-snapshot ×1、summary ×1）/),
    ).toBeInTheDocument();
    expect(screen.getByText(/无法解析 2 行/)).toBeInTheDocument();

    // 契约：按 session_id 读取，前端传不了文件系统路径。
    const readCall = invokeSpy.mock.calls.find(([cmd]) => cmd === 'replay_read_session');
    expect(readCall?.[1]).toEqual({
      harnessId: 'claude',
      sessionId: 'b3a1b3a1-0000-4000-8000-000000000003',
    });
  });

  /* ---------------------------------------------------------------- *
   * 工具调用卡与子代理线程（ROADMAP B8）
   * ---------------------------------------------------------------- */

  it('工具卡标注截断：结果截断与输入截断分别显示，无结果的调用不渲染「→ 结果」', async () => {
    const user = userEvent.setup();
    const truncatedSession = {
      ...REPLAY_SESSION,
      subagentThreads: [],
      messages: [
        {
          seq: 0,
          role: 'assistant',
          sidechain: false,
          timestamp: '2026-10-25T10:00:05.000Z',
          textParts: ['查一下 TODO'],
          thinkingParts: [],
          model: 'claude-sonnet-4-6',
          toolCalls: [
            {
              id: 'call_1',
              name: 'Grep',
              inputJson: '{"pattern":"TODO"}',
              resultJson: 'src/a.rs:1: TODO…',
              resultTruncated: true,
              inputTruncated: false,
            },
            {
              id: 'call_2',
              name: 'Bash',
              inputJson: '{"command":"…被截断的命令…"}',
              resultJson: null,
              resultTruncated: false,
              inputTruncated: true,
            },
          ],
          unsupportedContentKinds: {},
        },
      ],
    };
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return truncatedSession;
      return null;
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    expect(await screen.findByRole('list', { name: '工具调用' })).toBeInTheDocument();
    expect(screen.getByText('Grep')).toBeInTheDocument();
    expect(screen.getByText('Bash')).toBeInTheDocument();
    expect(screen.getByText('{"pattern":"TODO"}')).toBeInTheDocument();
    expect(screen.getByText('{"command":"…被截断的命令…"}')).toBeInTheDocument();
    // 只有 call_1 有结果：恰好一个「→ 结果」，结果原文 + 截断标注都在。
    expect(screen.getAllByText('→ 结果')).toHaveLength(1);
    expect(screen.getByText('src/a.rs:1: TODO…')).toBeInTheDocument();
    expect(screen.getByText('结果已截断')).toBeInTheDocument();
    // call_2 的输入被截断，单独标注。
    expect(screen.getByText('输入已截断')).toBeInTheDocument();
  });

  it('点击子代理线程滚动到首条消息并高亮线程内消息（尽力而为）', async () => {
    // jsdom 不实现 scrollIntoView：手动挂一个 spy 验证「找到消息时确实滚动」。
    const scrollSpy = vi.fn();
    Object.defineProperty(Element.prototype, 'scrollIntoView', {
      value: scrollSpy,
      configurable: true,
      writable: true,
    });
    const user = userEvent.setup();
    const threadedSession = {
      ...REPLAY_SESSION,
      messages: [
        REPLAY_SESSION.messages[0],
        { ...REPLAY_SESSION.messages[1], textParts: ['子代理回答'] },
        {
          seq: 2,
          role: 'assistant',
          sidechain: true,
          timestamp: '2026-10-25T10:00:07.000Z',
          textParts: ['子代理继续'],
          thinkingParts: [],
          model: 'claude-sonnet-4-6',
          toolCalls: [],
          unsupportedContentKinds: {},
        },
      ],
      subagentThreads: [
        {
          messageSeqs: [1, 2],
          startedAt: '2026-10-25T10:00:05.000Z',
          lastAt: '2026-10-25T10:00:09.000Z',
        },
      ],
    };
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return threadedSession;
      return null;
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    await user.click(
      await screen.findByRole('button', { name: /线程 1 · #1、#2 · 2026-10-25 10:00:05/ }),
    );

    // 线程内两条消息都命中：滚动一次（到首条），高亮覆盖全部命中消息。
    expect(scrollSpy).toHaveBeenCalledTimes(1);
    expect(screen.getByText('子代理回答').closest('article')).toHaveClass(
      'ring-1',
      'ring-accent/40',
    );
    expect(screen.getByText('子代理继续').closest('article')).toHaveClass(
      'ring-1',
      'ring-accent/40',
    );
    // 线程外的消息不高亮。
    expect(screen.getByLabelText('用户消息')).not.toHaveClass('ring-1');
  });

  it('无工具调用与无子代理线程的会话不渲染空区块（空区块纪律）', async () => {
    const user = userEvent.setup();
    const plainSession = {
      ...REPLAY_SESSION,
      subagentThreads: [],
      messages: [
        { ...REPLAY_SESSION.messages[0], unsupportedContentKinds: {} },
        { ...REPLAY_SESSION.messages[1], sidechain: false, toolCalls: [] },
      ],
    };
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return plainSession;
      return null;
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    expect(await screen.findByText('回答正文')).toBeInTheDocument();
    // 无工具调用 → 不渲染工具卡列表；无线程 → 整个子代理区都不出现。
    expect(screen.queryByRole('list', { name: '工具调用' })).not.toBeInTheDocument();
    expect(screen.queryByRole('region', { name: '子代理线程' })).not.toBeInTheDocument();
    // 没有未展开内容时计数脚注同样不渲染。
    expect(screen.queryByText(/未展开内容/)).not.toBeInTheDocument();
  });

  it('会话列表保持后端返回顺序（mtime 倒序），前端不重排', async () => {
    const newer = {
      ...SESSION_SUMMARY,
      sessionId: 'newer-1',
      projectDir: 'D--work-beta',
      modifiedMs: 1_761_000_000_999,
    };
    const older = {
      ...SESSION_SUMMARY,
      sessionId: 'older-2',
      projectDir: 'D--work-alpha',
      modifiedMs: 1_761_000_000_000,
    };
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [newer, older];
      return null;
    });

    render(<ReplayPage />);

    const items = await screen.findAllByRole('button', { name: /D--work-(alpha|beta)/ });
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent('D--work-beta');
    expect(items[1]).toHaveTextContent('D--work-alpha');
  });

  it('没有任何 replay 能力的 Harness 时渲染说明卡，且不发起会话列表查询', async () => {
    const invokeSpy = vi.fn(async (cmd: string) =>
      cmd === 'list_harnesses' ? [CODEX_HARNESS] : [],
    );
    stubTauri(invokeSpy);

    render(<ReplayPage />);

    expect(await screen.findByText(/还没有支持回放的 Harness/)).toBeInTheDocument();
    // 说明卡要讲清楚门控依据：capability 矩阵里 replay = false 就不渲染入口。
    expect(screen.getByText(/capabilities\.replay = true/)).toBeInTheDocument();
    expect(invokeSpy.mock.calls.some(([cmd]) => cmd === 'replay_list_sessions')).toBe(false);
  });

  it('读取失败时渲染错误卡，而不是白屏', async () => {
    const user = userEvent.setup();
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      throw new Error('转录超过 16 MiB 上限');
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    // B7 的搜索设置区读不到设置时也会显示同一错误串；这里限定时间线错误卡。
    expect(await screen.findByText(/IPC 错误：转录超过 16 MiB 上限/)).toBeInTheDocument();
  });

  it('session 未命中发现结果（后端返回 null）时如实说明，不渲染空时间线', async () => {
    const user = userEvent.setup();
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      return null;
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    expect(await screen.findByText(/已不在发现列表中/)).toBeInTheDocument();
  });

  it('messages 为空时渲染空态说明', async () => {
    const user = userEvent.setup();
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session')
        return { ...REPLAY_SESSION, messages: [], subagentThreads: [] };
      return null;
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    expect(await screen.findByText(/没有可回放的消息/)).toBeInTheDocument();
  });

  it('发现列表为空时给出说明，不报错', async () => {
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      return [];
    });

    render(<ReplayPage />);

    expect(await screen.findByText(/没有发现可回放的 Claude 会话/)).toBeInTheDocument();
  });

  it('浏览器模式下如实说明 IPC 不可用', async () => {
    render(<ReplayPage />);

    expect(await screen.findByText(/请用 pnpm tauri dev 启动桌面应用/)).toBeInTheDocument();
  });

  /* ---------------------------------------------------------------- *
   * 搜索与索引（ROADMAP B7）
   * ---------------------------------------------------------------- */

  it('索引开关默认关闭：搜索框禁用并说明原因，隐私文案注明本地存储可关闭清除', async () => {
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_search_settings_get') {
        return { enabled: false, excludedDirs: ['D--work-beta'] };
      }
      return null;
    });

    render(<ReplayPage />);

    const toggle = await screen.findByRole('checkbox', { name: /启用回放索引/ });
    expect(toggle).not.toBeChecked();
    expect(screen.getByLabelText('搜索已索引的会话内容')).toBeDisabled();
    expect(screen.getByText(/索引已关闭：打开并重建索引后才能搜索/)).toBeInTheDocument();
    // 隐私文案纪律：索引在本地数据库、可关闭清除；绝不出现「上传 / 云端」类表述。
    expect(screen.getByText(/索引内容保存在本地数据库，可随时关闭清除/)).toBeInTheDocument();
  });

  it('打开开关即保存设置（沿用排除名单），点击重建索引后显示审计计数', async () => {
    const user = userEvent.setup();
    const invokeSpy = vi.fn(async (cmd: string, _args?: Record<string, unknown>) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_search_settings_get') return { enabled: false, excludedDirs: [] };
      if (cmd === 'replay_search_settings_set') return null;
      if (cmd === 'replay_search_reindex') {
        return { scanned: 68, indexed: 7, unchanged: 60, excluded: 1, removed: 0 };
      }
      return null;
    });
    stubTauri(invokeSpy);

    render(<ReplayPage />);

    await user.click(await screen.findByRole('checkbox', { name: /启用回放索引/ }));

    // 开关即保存：settings_set 收到 enabled: true，排除名单原样沿用。
    await waitFor(() =>
      expect(invokeSpy.mock.calls.some(([cmd]) => cmd === 'replay_search_settings_set')).toBe(true),
    );
    const setCall = invokeSpy.mock.calls.find(([cmd]) => cmd === 'replay_search_settings_set');
    expect(setCall?.[1]).toEqual({ settings: { enabled: true, excludedDirs: [] } });

    // 重建索引是显式用户动作：未点击按钮前不得调用。
    expect(invokeSpy.mock.calls.some(([cmd]) => cmd === 'replay_search_reindex')).toBe(false);
    const reindexButton = screen.getByRole('button', { name: '重建索引' });
    await waitFor(() => expect(reindexButton).toBeEnabled());
    await user.click(reindexButton);

    expect(
      await screen.findByText(/扫描 68 · 索引 7 · 未变 60 · 排除 1 · 清理 0/),
    ).toBeInTheDocument();
    expect(invokeSpy.mock.calls.filter(([cmd]) => cmd === 'replay_search_reindex')).toHaveLength(1);
  });

  it('搜索渲染命中片段（原样含 «» 标记）与 sessionId+seq 副标题，点击命中选中对应会话', async () => {
    const user = userEvent.setup();
    const invokeSpy = vi.fn(async (cmd: string, _args?: Record<string, unknown>) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return REPLAY_SESSION;
      if (cmd === 'replay_search_settings_get') return { enabled: true, excludedDirs: [] };
      if (cmd === 'replay_search') {
        return [
          {
            harnessId: 'claude',
            sessionId: 'b3a1b3a1-0000-4000-8000-000000000003',
            seq: 3,
            role: 'user',
            snippet: '…帮我«排查»报错…',
            firstUserPreview: null,
          },
        ];
      }
      return null;
    });
    stubTauri(invokeSpy);

    render(<ReplayPage />);

    await user.type(await screen.findByLabelText('搜索已索引的会话内容'), '排查报错');
    await user.click(screen.getByRole('button', { name: '搜索' }));

    // snippet 原样渲染：«» 标记不解释、不转义、不重新排版。
    expect(await screen.findByText('…帮我«排查»报错…')).toBeInTheDocument();
    expect(
      screen.getByText(/用户 · claude · 会话 b3a1b3a1-0000-4000-8000-000000000003 · #3/),
    ).toBeInTheDocument();

    const searchCall = invokeSpy.mock.calls.find(([cmd]) => cmd === 'replay_search');
    expect(searchCall?.[1]).toEqual({ query: '排查报错' });

    // 点击命中 → 左侧列表选中对应会话（sessionId 命中），右侧加载时间线。
    await user.click(screen.getByText('…帮我«排查»报错…'));
    expect(await screen.findByText('回答正文')).toBeInTheDocument();
  });

  it('「<3 字符」「未启用」类后端 reject 显示为行内提示，而不是「搜索失败」错误卡', async () => {
    const user = userEvent.setup();
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_search_settings_get') return { enabled: true, excludedDirs: [] };
      if (cmd === 'replay_search') {
        throw new Error('搜索词至少需要 3 个字符（trigram 分词器的下限）');
      }
      return null;
    });

    render(<ReplayPage />);

    await user.type(await screen.findByLabelText('搜索已索引的会话内容'), 'abc');
    await user.click(screen.getByRole('button', { name: '搜索' }));

    expect(await screen.findByText(/至少需要 3 个字符/)).toBeInTheDocument();
    expect(screen.queryByText(/搜索失败/)).not.toBeInTheDocument();

    // 索引未启用（例如设置被并发关闭）同理：提示，不是崩溃卡。
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_search_settings_get') return { enabled: true, excludedDirs: [] };
      if (cmd === 'replay_search') throw new Error('回放索引未启用：先在设置里打开');
      return null;
    });
    await user.click(screen.getByRole('button', { name: '搜索' }));

    expect(await screen.findByText(/回放索引未启用/)).toBeInTheDocument();
    expect(screen.queryByText(/搜索失败/)).not.toBeInTheDocument();
  });

  /* ---------------------------------------------------------------- *
   * 在终端中恢复（ROADMAP B9）
   * ---------------------------------------------------------------- */

  it('claude（resume=true）渲染「在终端中恢复」：点击后成功提示含新会话 id，exited/error 事件落成一行状态', async () => {
    const user = userEvent.setup();
    const invokeSpy = vi.fn(async (cmd: string, _args?: Record<string, unknown>) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return REPLAY_SESSION;
      if (cmd === 'resume_terminal') return RESUMED_SESSION_RECORD;
      return null;
    });
    stubTauriWithChannel(invokeSpy);

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    await user.click(await screen.findByRole('button', { name: '在终端中恢复' }));

    // 成功提示含新终端会话的 id（返回 SessionRecord 的 hubSessionId）。
    expect(await screen.findByText(/已在新终端会话恢复（hub-resume-1）/)).toBeInTheDocument();

    // resume_terminal 收到的正是 claude 的 installationId 与本会话 id；
    // cwd 传 null（编码后的 projectDir 不可靠还原），cols/rows 用 120/30 默认值。
    const resumeCall = invokeSpy.mock.calls.find(([cmd]) => cmd === 'resume_terminal');
    expect(resumeCall).toBeDefined();
    const resumeArgs = resumeCall?.[1] as Record<string, unknown>;
    expect(resumeArgs).toMatchObject({
      installationId: 'claude@local',
      sourceSessionId: 'b3a1b3a1-0000-4000-8000-000000000003',
      cwd: null,
      cols: 120,
      rows: 30,
    });

    // onEvent 的 exited / error 落成一行简短状态（output 在回放页无处渲染，忽略）。
    const output = resumeArgs.output as { onmessage: (event: unknown) => void };
    act(() => {
      output.onmessage({
        kind: 'exited',
        sessionId: 'hub-resume-1',
        exitCode: 0,
        reason: 'natural_exit',
      });
    });
    expect(screen.getByText(/终端会话已结束（正常退出 · 退出码 0）/)).toBeInTheDocument();

    // 最新一条事件覆盖上一条：状态行只有一行。
    act(() => {
      output.onmessage({ kind: 'error', sessionId: 'hub-resume-1', message: 'pty io error' });
    });
    expect(screen.getByText('终端会话错误：pty io error')).toBeInTheDocument();
    expect(screen.queryByText(/终端会话已结束/)).not.toBeInTheDocument();
  });

  it('capabilities.resume 不为 true 时不渲染「在终端中恢复」按钮（不置灰、不给假入口）', async () => {
    const user = userEvent.setup();
    stubTauri(async (cmd: string) => {
      if (cmd === 'list_harnesses') {
        return [
          { ...CLAUDE_HARNESS, capabilities: { ...CLAUDE_HARNESS.capabilities, resume: false } },
        ];
      }
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return REPLAY_SESSION;
      return null;
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));

    // 时间线照常渲染，恢复入口整个不出现。
    expect(await screen.findByText('回答正文')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '在终端中恢复' })).not.toBeInTheDocument();
  });

  it('重复恢复的后端 reject（唯一索引）显示为行内提示「该外部会话已有恢复记录」', async () => {
    const user = userEvent.setup();
    stubTauriWithChannel(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return REPLAY_SESSION;
      if (cmd === 'resume_terminal') {
        throw new Error('UNIQUE constraint failed: sessions.source_session_id');
      }
      return null;
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));
    await user.click(await screen.findByRole('button', { name: '在终端中恢复' }));

    expect(await screen.findByText('该外部会话已有恢复记录')).toBeInTheDocument();
    // 使用方式问题渲染成行内提示，绝不套「恢复失败」错误卡。
    expect(screen.queryByText(/恢复失败/)).not.toBeInTheDocument();
  });

  it('其它恢复错误原样显示', async () => {
    const user = userEvent.setup();
    stubTauriWithChannel(async (cmd: string) => {
      if (cmd === 'list_harnesses') return [CLAUDE_HARNESS];
      if (cmd === 'replay_list_sessions') return [SESSION_SUMMARY];
      if (cmd === 'replay_read_session') return REPLAY_SESSION;
      if (cmd === 'resume_terminal') throw new Error('spawn pty failed: permission denied');
      return null;
    });

    render(<ReplayPage />);
    await user.click(await screen.findByRole('button', { name: /D--work-demo/ }));
    await user.click(await screen.findByRole('button', { name: '在终端中恢复' }));

    expect(
      await screen.findByText(/恢复失败：spawn pty failed: permission denied/),
    ).toBeInTheDocument();
    expect(screen.queryByText('该外部会话已有恢复记录')).not.toBeInTheDocument();
  });
});

import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { McpRuntimeCard } from '@/features/settings/McpRuntimeCard';

type Internals = { __TAURI_INTERNALS__?: unknown };

/**
 * 按命令名分派，便于验证「哪个命令在什么时候被调用」（与 SettingsPage.test.tsx
 * 的 stubCommands 同款）。值可以是直接返回的数据，也可以是惰性求值的函数。
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

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

/* ------------------------------------------------------------------ *
 * fixtures —— 与 Rust serde camelCase 契约同形，复刻自 src/lib/ipc.test.ts
 * 的 F22 MCP_SERVER_STATE_PAYLOAD 系列与 SettingsPage.test.tsx 的 E18 系列。
 * ------------------------------------------------------------------ */

/** `mcp_server_list` 的返回项之一：stdio 传输、已启用。 */
const MCP_SERVER_STDIO_PAYLOAD = {
  id: 'srv-1',
  name: 'filesystem',
  transport: 'stdio',
  command: 'npx',
  args: ['-y', '@modelcontextprotocol/server-filesystem'],
  url: null,
  envKeys: ['FILESYSTEM_ROOT'],
  enabled: true,
  source: 'hub',
  createdAt: '2026-10-25T10:00:00Z',
  updatedAt: '2026-11-01T09:30:00Z',
};

/** `mcp_server_list` 的返回项之一：http 传输、已停用（不可 connect）。 */
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

/** `mcp_runtime_status`：sidecar 就绪，filesystem 尚未连接。 */
const MCP_RUNTIME_STATUS_PAYLOAD = {
  available: true,
  unavailableReason: null,
  servers: [],
};

/** sidecar 未就绪时的形状。 */
const MCP_RUNTIME_UNAVAILABLE_PAYLOAD = {
  available: false,
  unavailableReason: 'Python sidecar 未就绪（启动失败：python 不存在）',
  servers: [],
};

/** `mcp_runtime_tools_list` 的返回项。 */
const MCP_TOOLS_LIST_PAYLOAD = [
  { name: 'get_forecast', description: '获取天气预报', inputSchemaKeys: ['location', 'days'] },
  { name: 'list_cities', description: '', inputSchemaKeys: [] },
];

/** `mcp_runtime_tool_call` 的返回项（成功、有截断）。 */
const MCP_TOOL_CALL_OUTCOME_PAYLOAD = {
  allowed: true,
  reason: 'readonly_by_default',
  isError: false,
  contentCount: 1,
  contentTypes: ['text'],
  textPreview: '上海晴，26°C',
  truncated: true,
};

/** `mcp_runtime_connect` 的返回项（spec §3.3：单 server 状态形状）。 */
const MCP_CONNECT_READY_PAYLOAD = {
  name: 'filesystem',
  transport: 'stdio',
  state: 'ready',
  protocolVersion: '2025-06-18',
  serverInfo: { name: 'filesystem-server', version: '1.0.0' },
  toolCount: 0,
  lastError: null,
  envKeys: ['FILESYSTEM_ROOT'],
  envMissingKeys: [],
};

function runtimeCard(): HTMLElement {
  return screen.getByTestId('mcp-runtime-card');
}

describe('McpRuntimeCard（F22 §5）', () => {
  it('渲染路径零 IPC：挂载后、点击前不发起任何 invoke 调用', () => {
    const invoke = stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_STATUS_PAYLOAD,
    });

    render(<McpRuntimeCard />);

    expect(runtimeCard()).toBeInTheDocument();
    expect(within(runtimeCard()).getByTestId('mcp-runtime-refresh')).toBeInTheDocument();
    expect(invoke.mock.calls).toHaveLength(0);
  });

  it('UI 明示连接行为：卡片描述含「连接将启动外部 MCP server 进程或发起网络连接」', () => {
    stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_STATUS_PAYLOAD,
    });

    render(<McpRuntimeCard />);

    expect(
      within(runtimeCard()).getByText(/连接将启动外部 MCP server 进程或发起网络连接/),
    ).toBeInTheDocument();
  });

  it('点击「加载状态」调 mcp_server_list + mcp_runtime_status，渲染 server 行与状态徽标', async () => {
    const invoke = stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD, MCP_SERVER_HTTP_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_STATUS_PAYLOAD,
    });

    render(<McpRuntimeCard />);
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-refresh'));

    expect(await within(runtimeCard()).findByTestId('mcp-runtime-servers')).toBeInTheDocument();
    expect(invoke.mock.calls.map((call) => call[0])).toEqual([
      'mcp_server_list',
      'mcp_runtime_status',
    ]);

    const stdioRow = within(runtimeCard()).getByTestId('mcp-runtime-server-row-filesystem');
    expect(within(stdioRow).getByText('filesystem')).toBeInTheDocument();
    expect(within(stdioRow).getByText('stdio')).toBeInTheDocument();
    expect(within(stdioRow).getByTestId('mcp-runtime-state-filesystem')).toHaveTextContent(
      'disconnected',
    );
    expect(within(stdioRow).getByText('已启用')).toBeInTheDocument();

    const httpRow = within(runtimeCard()).getByTestId('mcp-runtime-server-row-fetch');
    expect(within(httpRow).getByText('http')).toBeInTheDocument();
    expect(within(httpRow).getByText('已停用')).toBeInTheDocument();
  });

  it('connect 只对 enabled server 可用；点击后无 confirm 直接调 mcp_runtime_connect 并带 serverId，状态徽标随返回更新', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const invoke = stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD, MCP_SERVER_HTTP_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_STATUS_PAYLOAD,
      mcp_runtime_connect: MCP_CONNECT_READY_PAYLOAD,
    });

    render(<McpRuntimeCard />);
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-refresh'));
    await within(runtimeCard()).findByTestId('mcp-runtime-servers');

    // 已停用的 server 不可 connect。
    expect(within(runtimeCard()).getByTestId('mcp-runtime-connect-fetch')).toBeDisabled();

    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-connect-filesystem'));

    expect(confirmSpy).not.toHaveBeenCalled(); // 连接本身只读，不需要 confirm
    const connectCall = invoke.mock.calls.find((call) => call[0] === 'mcp_runtime_connect');
    expect(connectCall?.[1]).toEqual({ serverId: 'srv-1' });

    // connect 返回 ready 状态，合并进本地快照后徽标更新。
    expect(
      await within(runtimeCard()).findByTestId('mcp-runtime-state-filesystem'),
    ).toHaveTextContent('ready');
  });

  it('disconnect 调 mcp_runtime_disconnect 并带 serverId', async () => {
    const invoke = stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_STATUS_PAYLOAD,
      mcp_runtime_disconnect: { state: 'disconnected', name: 'filesystem' },
    });

    render(<McpRuntimeCard />);
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-refresh'));
    await within(runtimeCard()).findByTestId('mcp-runtime-servers');
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-disconnect-filesystem'));

    const disconnectCall = invoke.mock.calls.find((call) => call[0] === 'mcp_runtime_disconnect');
    expect(disconnectCall?.[1]).toEqual({ serverId: 'srv-1' });
  });

  it('展示 lastError 与 envMissingKeys 提示（值不进 Hub，仅提示宿主环境变量名）', async () => {
    stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_runtime_status: {
        available: true,
        unavailableReason: null,
        servers: [
          {
            name: 'filesystem',
            transport: 'stdio',
            state: 'degraded',
            protocolVersion: null,
            serverInfo: { name: 'filesystem-server', version: '1.0.0' },
            toolCount: 0,
            lastError: '握手失败：进程已退出',
            envKeys: ['FILESYSTEM_ROOT'],
            envMissingKeys: ['FILESYSTEM_ROOT'],
          },
        ],
      },
    });

    render(<McpRuntimeCard />);
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-refresh'));

    expect(
      await within(runtimeCard()).findByTestId('mcp-runtime-last-error-filesystem'),
    ).toHaveTextContent('握手失败：进程已退出');
    expect(
      within(runtimeCard()).getByTestId('mcp-runtime-env-missing-filesystem'),
    ).toHaveTextContent('FILESYSTEM_ROOT');
    expect(within(runtimeCard()).getByTestId('mcp-runtime-state-filesystem')).toHaveTextContent(
      'degraded',
    );
  });

  it('sidecar 未就绪（available=false）：显示 unavailableReason，禁用全部运行时操作', async () => {
    stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_UNAVAILABLE_PAYLOAD,
    });

    render(<McpRuntimeCard />);
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-refresh'));

    expect(await within(runtimeCard()).findByTestId('mcp-runtime-unavailable')).toHaveTextContent(
      'Python sidecar 未就绪（启动失败：python 不存在）',
    );

    for (const testid of [
      'mcp-runtime-select-filesystem',
      'mcp-runtime-connect-filesystem',
      'mcp-runtime-disconnect-filesystem',
    ]) {
      expect(within(runtimeCard()).getByTestId(testid)).toBeDisabled();
    }
    expect(within(runtimeCard()).queryByTestId('mcp-runtime-tools')).not.toBeInTheDocument();
  });

  it('选中 server → 列出工具：mcp_runtime_tools_list 带 serverId，渲染工具名/描述/参数键', async () => {
    const invoke = stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_STATUS_PAYLOAD,
      mcp_runtime_tools_list: MCP_TOOLS_LIST_PAYLOAD,
    });

    render(<McpRuntimeCard />);
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-refresh'));
    await userEvent.click(
      await within(runtimeCard()).findByTestId('mcp-runtime-select-filesystem'),
    );
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-tools-load'));

    const toolsCall = invoke.mock.calls.find((call) => call[0] === 'mcp_runtime_tools_list');
    expect(toolsCall?.[1]).toEqual({ serverId: 'srv-1' });

    const list = await within(runtimeCard()).findByTestId('mcp-runtime-tools-list');
    const forecastRow = within(list).getByTestId('mcp-runtime-tool-row-get_forecast');
    expect(within(forecastRow).getByText('get_forecast')).toBeInTheDocument();
    expect(within(forecastRow).getByText('获取天气预报')).toBeInTheDocument();
    expect(within(forecastRow).getByText('参数：location, days')).toBeInTheDocument();

    const citiesRow = within(list).getByTestId('mcp-runtime-tool-row-list_cities');
    expect(within(citiesRow).getByText('（无描述）')).toBeInTheDocument();
  });

  it('调用工具：confirm 文案含「将连接外部 MCP server 并调用其工具 <name>」，mcp_runtime_tool_call 带 serverId/tool/arguments，结果展示 textPreview 与截断标注', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const invoke = stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_STATUS_PAYLOAD,
      mcp_runtime_tools_list: MCP_TOOLS_LIST_PAYLOAD,
      mcp_runtime_tool_call: MCP_TOOL_CALL_OUTCOME_PAYLOAD,
    });

    render(<McpRuntimeCard />);
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-refresh'));
    await userEvent.click(
      await within(runtimeCard()).findByTestId('mcp-runtime-select-filesystem'),
    );
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-tools-load'));
    await within(runtimeCard()).findByTestId('mcp-runtime-tools-list');

    // 工具下拉默认选中第一个工具；填 JSON 参数。
    // （user-event 的 type 把 { / [ 当键盘描述符起始符，字面输入必须双写转义。）
    const argsInput = within(runtimeCard()).getByTestId('mcp-runtime-args-input');
    await userEvent.clear(argsInput);
    await userEvent.type(argsInput, '{{"location": "Shanghai"}');
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-call'));

    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(String(confirmSpy.mock.calls[0]?.[0])).toContain(
      '将连接外部 MCP server 并调用其工具 get_forecast',
    );

    const callEntry = invoke.mock.calls.find((call) => call[0] === 'mcp_runtime_tool_call');
    expect(callEntry?.[1]).toEqual({
      serverId: 'srv-1',
      tool: 'get_forecast',
      arguments: { location: 'Shanghai' },
    });

    const result = await within(runtimeCard()).findByTestId('mcp-runtime-call-result');
    expect(result).toHaveTextContent('上海晴，26°C');
    expect(within(runtimeCard()).getByTestId('mcp-runtime-truncated')).toBeInTheDocument();
  });

  it('deny 结果展示 reason（如 not_readonly_by_default），不渲染调用结果', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_STATUS_PAYLOAD,
      mcp_runtime_tools_list: MCP_TOOLS_LIST_PAYLOAD,
      mcp_runtime_tool_call: { allowed: false, reason: 'not_readonly_by_default' },
    });

    render(<McpRuntimeCard />);
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-refresh'));
    await userEvent.click(
      await within(runtimeCard()).findByTestId('mcp-runtime-select-filesystem'),
    );
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-tools-load'));
    await within(runtimeCard()).findByTestId('mcp-runtime-tools-list');
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-call'));

    expect(await within(runtimeCard()).findByTestId('mcp-runtime-deny-reason')).toHaveTextContent(
      'not_readonly_by_default',
    );
    expect(within(runtimeCard()).queryByTestId('mcp-runtime-call-result')).not.toBeInTheDocument();
  });

  it('用户取消 confirm → 不发调用', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    const invoke = stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_STATUS_PAYLOAD,
      mcp_runtime_tools_list: MCP_TOOLS_LIST_PAYLOAD,
    });

    render(<McpRuntimeCard />);
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-refresh'));
    await userEvent.click(
      await within(runtimeCard()).findByTestId('mcp-runtime-select-filesystem'),
    );
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-tools-load'));
    await within(runtimeCard()).findByTestId('mcp-runtime-tools-list');
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-call'));

    expect(invoke.mock.calls.every((call) => call[0] !== 'mcp_runtime_tool_call')).toBe(true);
  });

  it('JSON 参数非法（非对象 / 坏 JSON）→ 行内报错，不发调用', async () => {
    const invoke = stubCommands({
      mcp_server_list: [MCP_SERVER_STDIO_PAYLOAD],
      mcp_runtime_status: MCP_RUNTIME_STATUS_PAYLOAD,
      mcp_runtime_tools_list: MCP_TOOLS_LIST_PAYLOAD,
    });

    render(<McpRuntimeCard />);
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-refresh'));
    await userEvent.click(
      await within(runtimeCard()).findByTestId('mcp-runtime-select-filesystem'),
    );
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-tools-load'));
    await within(runtimeCard()).findByTestId('mcp-runtime-tools-list');

    const argsInput = within(runtimeCard()).getByTestId('mcp-runtime-args-input');

    // 非对象：JSON 数组（user-event 需双写 [ 才能输入字面量）。
    await userEvent.clear(argsInput);
    await userEvent.type(argsInput, '[[1,2]');
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-call'));
    expect(await within(runtimeCard()).findByText(/参数必须是 JSON 对象/)).toBeInTheDocument();

    // 坏 JSON。
    await userEvent.clear(argsInput);
    await userEvent.type(argsInput, '{{oops');
    await userEvent.click(within(runtimeCard()).getByTestId('mcp-runtime-call'));
    expect(await within(runtimeCard()).findByText(/参数 JSON 无法解析/)).toBeInTheDocument();

    expect(invoke.mock.calls.every((call) => call[0] !== 'mcp_runtime_tool_call')).toBe(true);
  });

  it('浏览器模式（无 Tauri runtime）：加载与运行时按钮全部禁用', async () => {
    render(<McpRuntimeCard />);
    await screen.findByTestId('mcp-runtime-card');

    expect(within(runtimeCard()).getByTestId('mcp-runtime-refresh')).toBeDisabled();
    expect(within(runtimeCard()).queryByTestId('mcp-runtime-connect-filesystem')).toBeNull();
  });
});

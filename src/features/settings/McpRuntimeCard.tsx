import { useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import {
  isTauriRuntime,
  listMcpServers,
  mcpRuntimeConnect,
  mcpRuntimeDisconnect,
  mcpRuntimeStatus,
  mcpRuntimeToolCall,
  mcpRuntimeToolsList,
  type McpRuntimeStatus,
  type McpServer,
  type McpServerRuntimeState,
  type McpServerState,
  type McpToolCallOutcome,
  type McpToolDescriptor,
} from '@/lib/ipc';

const MCP_STATE_STYLE: Record<McpServerRuntimeState, string> = {
  ready: 'border-signal-ok/30 bg-signal-ok/12 text-signal-ok',
  degraded: 'border-signal-error/30 bg-signal-error/12 text-signal-error',
  connecting: 'border-white/10 bg-white/[0.05] text-content-muted',
  disconnected: 'border-white/10 bg-white/[0.05] text-content-muted',
};

/** 把单 server 状态合并进已加载的运行时快照（同名覆盖，不存在则追加；connect/disconnect 后本地刷新）。 */
function mergeServerState(runtime: McpRuntimeStatus, next: McpServerState): McpRuntimeStatus {
  const servers = runtime.servers.some((entry) => entry.name === next.name)
    ? runtime.servers.map((entry) => (entry.name === next.name ? next : entry))
    : [...runtime.servers, next];
  return { ...runtime, servers };
}

/**
 * MCP Runtime 卡片（F22 spec §5）：连接已登记的 MCP Server、发现工具并调用只读工具。
 *
 * 纪律（AGENTS.md / spec §3.8 / §5）：
 * - 渲染路径零 IPC：无 useEffect 自动拉取，只在用户点按钮时调用 @/lib/ipc 的 wrapper；
 * - 组件内禁止 invoke()，全部走 @/lib/ipc；
 * - connect 不需要 confirm（连接本身不执行工具），但 UI 明示「将启动外部 MCP server
 *   进程或发起网络连接」；工具调用是显式动作，必须 confirm（文案含工具名）；
 * - sidecar 未就绪（available=false）时显示 unavailableReason，禁用全部运行时操作；
 * - 状态不轮询：只在加载/操作后刷新，与 ProviderGatewayCard 一致。
 */
export function McpRuntimeCard() {
  const connected = isTauriRuntime();
  const [servers, setServers] = useState<McpServer[] | null>(null);
  const [runtime, setRuntime] = useState<McpRuntimeStatus | null>(null);
  const [selectedServerId, setSelectedServerId] = useState<string | null>(null);
  const [tools, setTools] = useState<McpToolDescriptor[] | null>(null);
  const [toolName, setToolName] = useState('');
  const [argsText, setArgsText] = useState('{}');
  const [argsError, setArgsError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<McpToolCallOutcome | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const runtimeReady = runtime !== null && runtime.available;
  const selectedServer = servers?.find((server) => server.id === selectedServerId) ?? null;

  const refresh = async () => {
    setBusy(true);
    setError(null);
    const [serverResult, statusResult] = await Promise.all([listMcpServers(), mcpRuntimeStatus()]);
    setBusy(false);
    if (!serverResult.ok) {
      setError(serverResult.error);
      return;
    }
    if (!statusResult.ok) {
      setError(statusResult.error);
      return;
    }
    const nextServers = serverResult.data;
    setServers(nextServers);
    setRuntime(statusResult.data);
    setSelectedServerId((prev) =>
      prev !== null && nextServers.some((server) => server.id === prev) ? prev : null,
    );
    // 状态可能已变化：工具列表与上次调用结果随之失效。
    setTools(null);
    setToolName('');
    setOutcome(null);
  };

  const connect = async (server: McpServer) => {
    setBusy(true);
    setError(null);
    const result = await mcpRuntimeConnect(server.id);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    // 失败也收敛为 degraded 状态（spec §3.3 不抛 RPC 错），合并后由状态徽标如实展示。
    setRuntime((prev) => (prev ? mergeServerState(prev, result.data) : prev));
  };

  const disconnect = async (server: McpServer) => {
    setBusy(true);
    setError(null);
    const result = await mcpRuntimeDisconnect(server.id);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setRuntime((prev) => (prev ? mergeServerState(prev, result.data) : prev));
  };

  const loadTools = async () => {
    if (selectedServer === null) {
      return;
    }
    setBusy(true);
    setError(null);
    const result = await mcpRuntimeToolsList(selectedServer.id);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setTools(result.data);
    setToolName(result.data[0]?.name ?? '');
    setOutcome(null);
  };

  const callTool = async () => {
    if (selectedServer === null || toolName.length === 0) {
      return;
    }

    const text = argsText.trim();
    let parsedArgs: Record<string, unknown> = {};
    if (text.length > 0) {
      try {
        const parsed: unknown = JSON.parse(text);
        if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
          setArgsError('参数必须是 JSON 对象（如 {"location": "Shanghai"}）');
          return;
        }
        parsedArgs = parsed as Record<string, unknown>;
      } catch {
        setArgsError('参数 JSON 无法解析');
        return;
      }
    }
    setArgsError(null);

    // 显式动作：confirm 文案明示将连接外部 MCP server 并调用其工具。
    const confirmed = window.confirm(
      `将连接外部 MCP server 并调用其工具 ${toolName}。参数：${text.length > 0 ? text : '{}'}。确认执行？`,
    );
    if (!confirmed) {
      return;
    }

    setBusy(true);
    setError(null);
    const result = await mcpRuntimeToolCall(selectedServer.id, toolName, parsedArgs);
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setOutcome(result.data);
  };

  return (
    <Card className="mb-6" data-testid="mcp-runtime-card">
      <CardTitle>MCP Runtime</CardTitle>
      <CardDescription>
        MCP client 运行时（F22）：连接 Registry 里登记的 server、发现工具并调用只读工具。
        连接将启动外部 MCP server 进程或发起网络连接；工具调用仅放行只读策略命中的工具。
      </CardDescription>

      <div className="mt-3 flex flex-wrap gap-2">
        <Button
          variant="secondary"
          size="sm"
          onClick={() => void refresh()}
          disabled={!connected || busy}
          data-testid="mcp-runtime-refresh"
        >
          加载状态
        </Button>
      </div>

      <div className="mt-4" aria-live="polite">
        {busy ? <p className="text-xs text-content-muted">正在执行…</p> : null}

        {error !== null ? (
          <p className="text-xs text-signal-error" role="alert">
            {error}
          </p>
        ) : null}

        {runtime !== null && !runtime.available ? (
          <p className="text-xs text-signal-error" data-testid="mcp-runtime-unavailable">
            {runtime.unavailableReason ?? 'MCP 运行时不可用'}
          </p>
        ) : null}

        {runtime !== null && servers !== null ? (
          <div className="space-y-2 text-xs" data-testid="mcp-runtime-servers">
            {servers.length === 0 ? (
              <p className="text-content-muted">
                还没有登记任何 MCP Server；请先在上面的 MCP Servers 区新增。
              </p>
            ) : null}

            {servers.map((server) => {
              const state = runtime.servers.find((entry) => entry.name === server.name) ?? null;
              const stateValue = state?.state ?? 'disconnected';
              return (
                <div
                  key={server.id}
                  className="flex flex-wrap items-center gap-2"
                  data-testid={`mcp-runtime-server-row-${server.name}`}
                >
                  <button
                    type="button"
                    className={`rounded border px-1.5 py-0.5 font-mono ${
                      selectedServerId === server.id
                        ? 'border-white/40 bg-white/10 text-content-primary'
                        : 'border-white/10 text-content-muted'
                    }`}
                    onClick={() => {
                      setSelectedServerId(server.id);
                      setTools(null);
                      setToolName('');
                      setOutcome(null);
                      setArgsError(null);
                    }}
                    disabled={!connected || busy || !runtimeReady}
                    data-testid={`mcp-runtime-select-${server.name}`}
                    title="选中后可在下方发现并调用该 server 的工具"
                  >
                    {server.name}
                  </button>
                  <span className="text-content-muted">{server.transport}</span>
                  <span
                    className={`rounded border px-1.5 py-0.5 font-mono font-medium ${MCP_STATE_STYLE[stateValue]}`}
                    data-testid={`mcp-runtime-state-${server.name}`}
                  >
                    {stateValue}
                  </span>
                  <span className="text-content-muted">工具 {state?.toolCount ?? 0}</span>
                  <span className={server.enabled ? 'text-signal-ok' : 'text-content-muted'}>
                    {server.enabled ? '已启用' : '已停用'}
                  </span>

                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => void connect(server)}
                    disabled={!connected || busy || !runtimeReady || !server.enabled}
                    data-testid={`mcp-runtime-connect-${server.name}`}
                  >
                    连接
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => void disconnect(server)}
                    disabled={!connected || busy || !runtimeReady}
                    data-testid={`mcp-runtime-disconnect-${server.name}`}
                  >
                    断开
                  </Button>

                  {state?.lastError !== null && state?.lastError !== undefined ? (
                    <p
                      className="w-full text-signal-error"
                      role="alert"
                      data-testid={`mcp-runtime-last-error-${server.name}`}
                    >
                      {state.lastError}
                    </p>
                  ) : null}

                  {state !== null && state.envMissingKeys.length > 0 ? (
                    <p
                      className="w-full text-content-muted"
                      data-testid={`mcp-runtime-env-missing-${server.name}`}
                    >
                      提示：宿主环境需设置 {state.envMissingKeys.join(', ')}
                      （值不进 Hub，由子进程直接继承）
                    </p>
                  ) : null}
                </div>
              );
            })}
          </div>
        ) : null}

        {runtimeReady && selectedServer !== null ? (
          <div className="mt-4 space-y-2 text-xs" data-testid="mcp-runtime-tools">
            <p className="text-content-muted">
              已选中：{selectedServer.name}（{selectedServer.transport}）
            </p>

            <Button
              variant="secondary"
              size="sm"
              onClick={() => void loadTools()}
              disabled={!connected || busy}
              data-testid="mcp-runtime-tools-load"
            >
              列出工具
            </Button>

            {tools !== null ? (
              tools.length === 0 ? (
                <p className="text-content-muted">该 server 没有发现任何工具。</p>
              ) : (
                <div className="space-y-1" data-testid="mcp-runtime-tools-list">
                  {tools.map((tool) => (
                    <div
                      key={tool.name}
                      className="flex flex-wrap items-center gap-2 font-mono"
                      data-testid={`mcp-runtime-tool-row-${tool.name}`}
                    >
                      <span className="text-content-primary">{tool.name}</span>
                      <span className="text-content-muted">
                        {tool.description.length > 0 ? tool.description : '（无描述）'}
                      </span>
                      {tool.inputSchemaKeys.length > 0 ? (
                        <span className="text-content-muted">
                          参数：{tool.inputSchemaKeys.join(', ')}
                        </span>
                      ) : null}
                    </div>
                  ))}
                </div>
              )
            ) : null}

            <div className="mt-2 space-y-1">
              <label className="block text-content-muted" htmlFor="mcp-runtime-tool-select">
                工具
              </label>
              <select
                id="mcp-runtime-tool-select"
                className="rounded border border-white/20 bg-white/[0.05] px-2 py-1 font-mono"
                value={toolName}
                onChange={(event) => {
                  setToolName(event.target.value);
                  setOutcome(null);
                }}
                data-testid="mcp-runtime-tool-select"
              >
                {(tools ?? []).map((tool) => (
                  <option key={tool.name} value={tool.name}>
                    {tool.name}
                  </option>
                ))}
              </select>

              <label className="block text-content-muted" htmlFor="mcp-runtime-args-input">
                JSON 参数
              </label>
              <textarea
                id="mcp-runtime-args-input"
                className="w-96 rounded border border-white/20 bg-white/[0.05] px-2 py-1 font-mono"
                rows={3}
                value={argsText}
                onChange={(event) => {
                  setArgsText(event.target.value);
                  setArgsError(null);
                }}
                data-testid="mcp-runtime-args-input"
              />
              {argsError !== null ? (
                <p className="text-signal-error" role="alert">
                  {argsError}
                </p>
              ) : null}

              <Button
                variant="secondary"
                size="sm"
                onClick={() => void callTool()}
                disabled={!connected || busy || tools === null || toolName.length === 0}
                data-testid="mcp-runtime-call"
              >
                调用工具
              </Button>
            </div>

            {outcome !== null && !outcome.allowed ? (
              <div className="text-signal-error" data-testid="mcp-runtime-deny" role="alert">
                已拒绝执行：
                <span data-testid="mcp-runtime-deny-reason">
                  {outcome.reason.length > 0 ? outcome.reason : '未知原因'}
                </span>
              </div>
            ) : null}

            {outcome !== null && outcome.allowed ? (
              <div className="space-y-1" data-testid="mcp-runtime-call-result">
                {outcome.isError ? (
                  <p
                    className="text-signal-error"
                    role="alert"
                    data-testid="mcp-runtime-call-error"
                  >
                    工具返回错误（isError）
                  </p>
                ) : null}
                <pre className="whitespace-pre-wrap font-mono text-content-primary">
                  {outcome.textPreview ?? '（无文本结果）'}
                </pre>
                {outcome.truncated ? (
                  <span className="text-content-muted" data-testid="mcp-runtime-truncated">
                    结果已截断（仅展示前 2048 字符预览）
                  </span>
                ) : null}
              </div>
            ) : null}
          </div>
        ) : null}
      </div>
    </Card>
  );
}

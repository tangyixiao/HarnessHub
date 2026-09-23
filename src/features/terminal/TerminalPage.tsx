import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import { useCallback, useEffect, useRef, useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import {
  NOT_IN_TAURI,
  killTerminal,
  listHarnesses,
  resizeTerminal,
  startTerminal,
  writeTerminal,
  type HarnessSummary,
  type SessionRecord,
  type TerminationReason,
} from '@/lib/ipc';

const TERMINATION_LABEL: Record<TerminationReason, string> = {
  natural_exit: '正常退出',
  user_killed: '用户主动结束',
  launch_failed: '启动失败',
  runtime_error: '运行故障',
  host_shutdown: 'Harness Hub 关闭',
  lost: '失去联系',
};

/**
 * Terminal 页状态机（**Session Launcher**，不是「打开页面就开 shell」）。
 *
 * ```text
 * loading → idle → starting → running → ended
 * ```
 *
 * - `installed = 0` → `error`（没有可启动的 Harness）
 * - `installed >= 1` → `idle`：**不再自动 spawn**
 *
 * 为什么取消「单 Harness 自动启动」：启动其实有多个启动期参数
 * （`installationId`、`cwd`，以后还会 env / profile / runtime target 之类），
 * mount 即 spawn 会让用户**没有机会**设置它们 —— 而且单 Harness 机器上尤其明显。
 * 形态因此从 `mount → spawn` 变成 `configure → explicit launch`。
 */
type Phase =
  | { kind: 'loading' }
  | { kind: 'idle' }
  | { kind: 'starting' }
  | { kind: 'running'; session: SessionRecord }
  | { kind: 'ended'; status: string }
  | { kind: 'unavailable' }
  | { kind: 'error'; message: string };

/** 空白（含纯空格）视为「未指定」，后端因此收到 `None`、`sessions.cwd` 记 NULL。 */
function normalizeCwd(raw: string): string | undefined {
  const trimmed = raw.trim();
  return trimmed.length === 0 ? undefined : trimmed;
}

/**
 * Terminal 页面。
 *
 * **ready-before-spawn**（ADR-0009）：严格按
 * `new Terminal → open → Channel 回调 → onData/onBinary → resize → start_terminal`
 * 的顺序初始化。顺序错了，交互式 TUI 首屏的 DSR 就会早于 responder 就绪而永久卡住。
 *
 * **启动期参数与运行期句柄严格分离**：
 * - `selectedInstallationId` / `selectedCwd`：**下一次**启动要用什么（用户可改）；
 * - `activeInstallationId` / `activeCwd`：当前 session **实际**用的是什么（只读事实）；
 * - `sessionIdRef`：write / resize / kill 的**唯一**控制句柄。
 *
 * 运行中三个输入都禁用，因此改「下一次」的参数绝不会污染当前会话。
 *
 * **卸载 ≠ kill**：组件卸载只释放浏览器侧资源（订阅、xterm、observer）。
 */
export function TerminalPage() {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  /** 唯一的 spawn 路径。 */
  const startRef = useRef<(installationId: string, cwd?: string) => void>(() => {});
  /** 防止双击 / 重入产生两个 session。 */
  const startingRef = useRef(false);

  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [installedHarnesses, setInstalledHarnesses] = useState<HarnessSummary[]>([]);
  const [selectedInstallationId, setSelectedInstallationId] = useState<string | null>(null);
  const [selectedCwd, setSelectedCwd] = useState('');
  const [activeInstallationId, setActiveInstallationId] = useState<string | null>(null);
  const [activeCwd, setActiveCwd] = useState<string | undefined>(undefined);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }

    const terminal = new Terminal({
      convertEol: false,
      fontSize: 13,
      fontFamily: 'Consolas, "Cascadia Mono", monospace',
      theme: { background: '#16181d', foreground: '#e6e8ee' },
      scrollback: 2000,
    });
    const fitAddon = new FitAddon();

    // ① 先 open：xterm 必须已经挂到 DOM
    terminal.open(container);
    terminal.loadAddon(fitAddon);

    // ② 输入路径先就绪：onData 是 UTF-8 文本（含终端协议响应），onBinary 是原始字节
    const dataSubscription = terminal.onData((text) => {
      const sessionId = sessionIdRef.current;
      if (!sessionId) return;
      void writeTerminal(sessionId, new TextEncoder().encode(text));
    });
    const binarySubscription = terminal.onBinary((data) => {
      const sessionId = sessionIdRef.current;
      if (!sessionId) return;
      // onBinary 给的是「每个 code unit 的低 8 位」，必须按原始字节还原，
      // 不能当字符串编码 —— 否则二进制序列会被破坏。
      const bytes = new Uint8Array(data.length);
      for (let index = 0; index < data.length; index += 1) {
        bytes[index] = data.charCodeAt(index) & 0xff;
      }
      void writeTerminal(sessionId, bytes);
    });

    // ③ resize 先就绪
    let observer: ResizeObserver | null = null;
    if (typeof ResizeObserver !== 'undefined') {
      observer = new ResizeObserver(() => {
        try {
          fitAddon.fit();
        } catch {
          // 容器尚未完成布局时 fit 会抛，忽略即可
        }
        const sessionId = sessionIdRef.current;
        if (sessionId && terminal.cols > 0 && terminal.rows > 0) {
          void resizeTerminal(sessionId, terminal.cols, terminal.rows);
        }
      });
      observer.observe(container);
    }

    // ④ 唯一的启动路径
    const start = (installationId: string, cwd?: string) => {
      if (startingRef.current || sessionIdRef.current !== null) {
        return; // 已在启动 / 已有会话：忽略，避免双击产生两个 session
      }
      startingRef.current = true;
      setPhase({ kind: 'starting' });
      setActiveInstallationId(installationId);
      setActiveCwd(cwd);

      void (async () => {
        const started = await startTerminal({
          installationId,
          cwd,
          cols: terminal.cols,
          rows: terminal.rows,
          onEvent: (event) => {
            switch (event.kind) {
              case 'output':
                // 原始字节直接交给 xterm：它的解码器是跨 chunk 有状态的
                terminal.write(new Uint8Array(event.data));
                break;
              case 'started':
                break;
              case 'exited':
                sessionIdRef.current = null;
                setPhase({
                  kind: 'ended',
                  status: `${TERMINATION_LABEL[event.reason]}${
                    event.exitCode === null ? '' : ` · 退出码 ${event.exitCode}`
                  }`,
                });
                break;
              case 'error':
                terminal.write(`\r\n[Harness Hub] ${event.message}\r\n`);
                break;
            }
          },
        });

        startingRef.current = false;

        if (!started.ok) {
          setActiveInstallationId(null);
          setActiveCwd(undefined);
          setPhase(
            started.error === NOT_IN_TAURI
              ? { kind: 'unavailable' }
              : // 启动失败：保留 installation 与 cwd 选择，用户可以重试
                { kind: 'error', message: started.error },
          );
          return;
        }

        sessionIdRef.current = started.data.hubSessionId;
        setPhase({ kind: 'running', session: started.data });
      })();
    };
    startRef.current = start;

    // ⑤ 只探测可选项，**不自动启动**
    let cancelled = false;
    void (async () => {
      const listed = await listHarnesses();
      if (cancelled) return;

      if (!listed.ok) {
        setPhase(
          listed.error === NOT_IN_TAURI
            ? { kind: 'unavailable' }
            : { kind: 'error', message: listed.error },
        );
        return;
      }

      // **不得按 Harness 名字分支**：只认 DTO（installed / installationId / displayName）。
      const choices = listed.data.filter((item) => item.installed && item.installationId !== null);
      setInstalledHarnesses(choices);

      if (choices.length === 0) {
        setPhase({ kind: 'error', message: '没有可用于启动终端的已安装 Harness' });
        return;
      }

      setSelectedInstallationId(choices[0]?.installationId ?? null);
      setPhase({ kind: 'idle' });
    })();

    return () => {
      cancelled = true;
      // 只释放前端资源。**不 kill**：离开页面不等于结束会话。
      dataSubscription.dispose();
      binarySubscription.dispose();
      observer?.disconnect();
      terminal.dispose();
    };
  }, []);

  const kill = useCallback(async () => {
    const sessionId = sessionIdRef.current;
    if (!sessionId) return;
    const result = await killTerminal(sessionId);
    if (!result.ok) {
      setPhase({ kind: 'error', message: result.error });
    }
  }, []);

  const activeHarness = installedHarnesses.find(
    (item) => item.installationId === activeInstallationId,
  );
  const busy = phase.kind === 'starting' || phase.kind === 'running';
  const canLaunch =
    selectedInstallationId !== null &&
    (phase.kind === 'idle' || phase.kind === 'ended' || phase.kind === 'error');
  const launch = () => {
    if (selectedInstallationId === null) return;
    startRef.current(selectedInstallationId, normalizeCwd(selectedCwd));
  };

  return (
    <div>
      <PageHeader
        title="Terminal"
        description="选择一个已安装的 Harness 并显式启动 PTY 会话：原始字节流、可交互、可调整尺寸。"
        actions={
          phase.kind === 'running' ? (
            <Button variant="secondary" onClick={() => void kill()}>
              结束会话
            </Button>
          ) : null
        }
      />

      {/*
        启动选项：**只认 DTO**（installationId 是 value、displayName 是 label），
        没有任何 codex / claude 之类的名字分支，因此加第三个 Harness 不需要改这里。
        运行中全部禁用：改「下一次」的启动参数绝不污染当前会话。
      */}
      {phase.kind !== 'unavailable' && installedHarnesses.length > 0 ? (
        <div className="mb-2 flex flex-wrap items-end gap-3 text-[11px] text-content-muted">
          <label className="flex flex-col gap-1">
            启动的 Harness
            <select
              aria-label="启动的 Harness"
              className="rounded border border-border-subtle bg-transparent px-2 py-1 text-xs text-content-primary"
              value={selectedInstallationId ?? ''}
              disabled={busy}
              onChange={(event) => setSelectedInstallationId(event.target.value)}
            >
              {installedHarnesses.map((item) => (
                <option key={item.installationId} value={item.installationId ?? ''}>
                  {item.displayName}
                </option>
              ))}
            </select>
          </label>

          <label className="flex flex-col gap-1">
            工作目录（可选）
            <input
              aria-label="工作目录（可选）"
              className="w-80 rounded border border-border-subtle bg-transparent px-2 py-1 text-xs text-content-primary"
              placeholder="留空 = 不指定（sessions.cwd 记为 NULL）"
              value={selectedCwd}
              disabled={busy}
              onChange={(event) => setSelectedCwd(event.target.value)}
            />
          </label>

          <Button variant="primary" onClick={launch} disabled={!canLaunch}>
            {phase.kind === 'ended' ? '重新启动' : '启动'}
          </Button>

          {phase.kind === 'running' ? (
            <span>
              运行中 · {activeHarness?.displayName ?? ''}
              {activeCwd ? ` · cwd ${activeCwd}` : ''}
              {phase.session.pid !== null ? ` · pid ${phase.session.pid}` : ''}
              {` · session ${phase.session.hubSessionId}`}
            </span>
          ) : null}
          {phase.kind === 'ended' ? <span>已结束 · {phase.status}</span> : null}
        </div>
      ) : null}

      {phase.kind === 'loading' ? (
        <p className="pb-2 text-[11px] text-content-muted">
          正在探测本机已安装的 Harness（准备 xterm、挂好输入与 resize）…
        </p>
      ) : null}

      {phase.kind === 'idle' ? (
        <p className="pb-2 text-[11px] text-content-muted">
          选择 Harness（可选填工作目录）后点「启动」。页面不会自动替你启动 —— 启动参数必须在 spawn
          之前确定。
        </p>
      ) : null}

      {phase.kind === 'starting' ? (
        <p className="pb-2 text-[11px] text-content-muted">
          正在启动 {activeHarness?.displayName ?? ''}…
        </p>
      ) : null}

      {phase.kind === 'unavailable' ? (
        <Card className="mb-2">
          <CardTitle>IPC 不可用（浏览器模式）</CardTitle>
          <CardDescription>
            请用 pnpm tauri dev 启动桌面应用。终端需要真实的 PTY 后端。
          </CardDescription>
        </Card>
      ) : null}

      {phase.kind === 'error' ? (
        <Card className="mb-2 border-signal-error/40">
          <CardTitle>终端启动失败</CardTitle>
          <CardDescription>{phase.message}</CardDescription>
        </Card>
      ) : null}

      {/*
        容器**必须始终存在**：xterm 要在 effect 里 open 它，
        而 effect 只在挂载时运行一次。条件渲染会让首次 effect 拿到 null 而永远起不来。
      */}
      <div
        ref={containerRef}
        data-testid="terminal-container"
        className="h-[480px] overflow-hidden rounded-md border border-border-subtle bg-[#16181d] p-1"
      />
    </div>
  );
}

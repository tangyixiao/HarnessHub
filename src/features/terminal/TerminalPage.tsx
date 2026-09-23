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
 * Terminal 页状态机。
 *
 * `loading`（正在探测本机有哪些 Harness）
 *   → `idle`（**多个**已安装：等用户显式启动，mount 不 spawn）
 *   → `starting`（启动中：禁止再次启动，防双击）
 *   → `running`（有 session）
 *   → `ended`（终态，可重新启动 → 新的 session id）
 *
 * 没有 `running`（也就是 ≥2 installed 的场景）时 mount **不**自动启动：
 * 否则「第一个 installation」会变成隐式的默认目标，用户根本没机会选别的。
 */
type Phase =
  | { kind: 'loading' }
  | { kind: 'idle' }
  | { kind: 'starting' }
  | { kind: 'running'; session: SessionRecord }
  | { kind: 'ended'; status: string }
  | { kind: 'unavailable' }
  | { kind: 'error'; message: string };

/**
 * Terminal 页面。
 *
 * **ready-before-spawn**（ADR-0009）：严格按
 * `new Terminal → open → Channel 回调 → onData/onBinary → resize → start_terminal`
 * 的顺序初始化。顺序错了，交互式 TUI 首屏的 DSR 就会早于 responder 就绪而永久卡住。
 *
 * **三个 id 各司其职，绝不混用**：
 * - `selectedInstallationId`：下一次准备启动谁（用户可选）；
 * - `activeInstallationId`：当前 session **实际**由谁启动（只读事实）；
 * - `sessionIdRef`：write / resize / kill 的**唯一**控制句柄。
 *
 * 运行中禁止切换目标：切换只影响下一次启动，绝不会把当前会话的控制对象换掉。
 *
 * **卸载 ≠ kill**：组件卸载只释放浏览器侧资源（订阅、xterm、observer）。
 */
export function TerminalPage() {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  /** 唯一的 spawn 路径（单 Harness 自动启动与多 Harness 手动启动都走它）。 */
  const startRef = useRef<(installationId: string) => void>(() => {});
  /** 防止 mount 自动启动被重复触发 / 双击启动产生两个 session。 */
  const startingRef = useRef(false);

  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [installedHarnesses, setInstalledHarnesses] = useState<HarnessSummary[]>([]);
  const [selectedInstallationId, setSelectedInstallationId] = useState<string | null>(null);
  const [activeInstallationId, setActiveInstallationId] = useState<string | null>(null);

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
    const start = (installationId: string) => {
      if (startingRef.current || sessionIdRef.current !== null) {
        return; // 已在启动 / 已有会话：忽略，避免双击产生两个 session
      }
      startingRef.current = true;
      setPhase({ kind: 'starting' });
      setActiveInstallationId(installationId);

      void (async () => {
        const started = await startTerminal({
          installationId,
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
          setPhase(
            started.error === NOT_IN_TAURI
              ? { kind: 'unavailable' }
              : // 启动失败：保留选择，用户可以重试
                { kind: 'error', message: started.error },
          );
          return;
        }

        sessionIdRef.current = started.data.hubSessionId;
        setPhase({ kind: 'running', session: started.data });
      })();
    };
    startRef.current = start;

    // ⑤ 最后才决定要不要 spawn
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

      const first = choices[0];
      if (!first || first.installationId === null) {
        setPhase({ kind: 'error', message: '没有可用于启动终端的已安装 Harness' });
        return;
      }
      setSelectedInstallationId(first.installationId);

      if (choices.length === 1) {
        // 唯一选择：自动启动**恰好一次**
        start(first.installationId);
      } else {
        // 多个选择：交给用户，mount 不 spawn
        setPhase({ kind: 'idle' });
      }
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

  return (
    <div>
      <PageHeader
        title="Terminal"
        description="在 Harness Hub 内直接运行 Harness 的 PTY 会话：原始字节流、可交互、可调整尺寸。"
        actions={
          phase.kind === 'running' ? (
            <Button variant="secondary" onClick={() => void kill()}>
              结束会话
            </Button>
          ) : null
        }
      />

      {installedHarnesses.length > 1 ? (
        <label className="mb-2 flex items-center gap-2 text-[11px] text-content-muted">
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
          {phase.kind === 'running' ? '（会话运行中，切换只影响下一次启动）' : ''}
        </label>
      ) : null}

      {phase.kind === 'idle' || (phase.kind === 'error' && installedHarnesses.length > 0) ? (
        <div className="mb-2 flex items-center gap-2">
          <Button
            variant="primary"
            onClick={() => {
              if (selectedInstallationId !== null) startRef.current(selectedInstallationId);
            }}
          >
            启动
          </Button>
          <span className="text-[11px] text-content-muted">
            本机有多个已安装 Harness，先选择再启动（不会自动替你选）。
          </span>
        </div>
      ) : null}

      {phase.kind === 'ended' ? (
        <div className="mb-2 flex items-center gap-2">
          <Button
            variant="primary"
            onClick={() => {
              if (selectedInstallationId !== null) startRef.current(selectedInstallationId);
            }}
          >
            重新启动
          </Button>
          <span className="text-[11px] text-content-muted">已结束 · {phase.status}</span>
        </div>
      ) : null}

      {phase.kind === 'loading' ? (
        <p className="pb-2 text-[11px] text-content-muted">
          正在探测本机已安装的 Harness（准备 xterm、挂好输入与 resize，然后才 spawn）…
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

      {phase.kind === 'running' ? (
        <p className="pb-2 text-[11px] text-content-muted">
          {`运行中 · ${activeHarness?.displayName ?? ''} · session ${phase.session.hubSessionId}`}
          {phase.session.pid !== null ? ` · pid ${phase.session.pid}` : ''}
        </p>
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

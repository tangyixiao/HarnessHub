import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import { useEffect, useRef, useState } from 'react';

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

type Phase =
  | { kind: 'connecting' }
  | { kind: 'running'; session: SessionRecord }
  | { kind: 'finished'; status: string }
  | { kind: 'unavailable' }
  | { kind: 'error'; message: string };

/**
 * Terminal 页面。
 *
 * **ready-before-spawn**（ADR-0009）：严格按
 * `new Terminal → open → Channel 回调 → onData/onBinary → resize → start_terminal`
 * 的顺序初始化。顺序错了，Codex 首屏的 DSR 就会早于 responder 就绪而永久卡住。
 *
 * **卸载 ≠ kill**：组件卸载只释放浏览器侧资源（订阅、xterm、observer）。
 * 正在运行的 Codex 不会被杀 —— 切页面不是结束 Session 的意思。
 * 结束会话只能通过显式的「结束会话」按钮。
 */
export function TerminalPage() {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: 'connecting' });
  const [harness, setHarness] = useState<HarnessSummary | null>(null);

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

    // ④ 最后才 spawn
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

      const target = listed.data.find((item) => item.installed && item.installationId !== null);
      if (!target || target.installationId === null) {
        setPhase({ kind: 'error', message: '没有可用于启动终端的已安装 Harness' });
        return;
      }
      setHarness(target);

      const started = await startTerminal({
        installationId: target.installationId,
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
              setPhase({
                kind: 'finished',
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

      if (cancelled) return;

      if (!started.ok) {
        setPhase(
          started.error === NOT_IN_TAURI
            ? { kind: 'unavailable' }
            : { kind: 'error', message: started.error },
        );
        return;
      }

      sessionIdRef.current = started.data.hubSessionId;
      setPhase({ kind: 'running', session: started.data });
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

  const kill = async () => {
    const sessionId = sessionIdRef.current;
    if (!sessionId) return;
    const result = await killTerminal(sessionId);
    if (!result.ok) {
      setPhase({ kind: 'error', message: result.error });
    }
  };

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

      {phase.kind === 'connecting' ? (
        <p className="pb-2 text-[11px] text-content-muted">
          正在启动终端（准备 xterm、挂好输入与 resize，然后才 spawn Harness）…
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

      {phase.kind === 'running' || phase.kind === 'finished' ? (
        <p className="pb-2 text-[11px] text-content-muted">
          {phase.kind === 'running'
            ? `运行中 · ${harness?.displayName ?? ''} · session ${phase.session.hubSessionId}`
            : `已结束 · ${phase.status}`}
          {phase.kind === 'running' && phase.session.pid !== null
            ? ` · pid ${phase.session.pid}`
            : ''}
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

import { useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { gatewayStatus, isTauriRuntime, type GatewayState, type GatewayStatus } from '@/lib/ipc';

const GATEWAY_STATE_STYLE: Record<GatewayState, string> = {
  ready: 'border-signal-ok/30 bg-signal-ok/12 text-signal-ok',
  degraded: 'border-signal-error/30 bg-signal-error/12 text-signal-error',
  unavailable: 'border-white/10 bg-white/[0.05] text-content-muted',
};

const GATEWAY_SIDECAR_STATE_STYLE: Record<GatewayStatus['sidecarState'], string> = {
  stopped: 'border-white/10 bg-white/[0.05] text-content-muted',
  ready: 'border-signal-ok/30 bg-signal-ok/12 text-signal-ok',
  degraded: 'border-signal-error/30 bg-signal-error/12 text-signal-error',
};

/**
 * Provider Gateway 状态卡片（F21 §7.3）：只读状态 + 一个显式「刷新」动作。
 *
 * 纪律（AGENTS.md / spec §9）：
 * - 渲染路径零 IPC：只在用户点按钮时调用 gatewayStatus()，无 useEffect 自动拉取；
 * - 组件内禁止 invoke()，全部走 @/lib/ipc 的 wrapper；
 * - 网关不可用（degraded / unavailable）只影响本卡片，主 Harness 功能不受影响。
 */
export function ProviderGatewayCard() {
  const connected = isTauriRuntime();
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    setBusy(true);
    setError(null);
    const result = await gatewayStatus();
    setBusy(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setStatus(result.data);
  };

  const unhealthy =
    status !== null && (status.state === 'degraded' || status.state === 'unavailable');

  return (
    <Card className="mb-6" data-testid="gateway-card">
      <CardTitle>Provider Gateway 状态</CardTitle>
      <CardDescription>
        统一网关与 tokenization 状态（F21）。状态只在点击按钮时读取。
      </CardDescription>

      <div className="mt-3 flex flex-wrap gap-2">
        <Button
          variant="secondary"
          size="sm"
          onClick={() => void refresh()}
          disabled={!connected || busy}
          data-testid="gateway-refresh"
        >
          刷新
        </Button>
      </div>

      <div className="mt-4" aria-live="polite">
        {busy ? <p className="text-xs text-content-muted">正在读取…</p> : null}

        {error !== null ? (
          <p className="text-xs text-signal-error" role="alert">
            {error}
          </p>
        ) : null}

        {status !== null && !busy ? (
          <div className="space-y-2 text-xs" data-testid="gateway-status">
            <div className="flex items-center gap-2">
              <span
                className={`rounded border px-1.5 py-0.5 font-mono font-medium ${GATEWAY_STATE_STYLE[status.state]}`}
                data-testid="gateway-state"
              >
                {status.state}
              </span>
              <span
                className={`rounded border px-1.5 py-0.5 font-mono font-medium ${GATEWAY_SIDECAR_STATE_STYLE[status.sidecarState]}`}
                data-testid="gateway-sidecar-state"
              >
                {status.sidecarState}
              </span>
              {unhealthy ? (
                <span className="text-signal-error" data-testid="gateway-warning">
                  Provider Gateway 不可用，主 Harness 功能不受影响
                </span>
              ) : null}
            </div>

            <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
              <dt className="text-content-muted">模式</dt>
              <dd className="font-mono text-content-primary" data-testid="gateway-mode">
                {status.mode}
              </dd>
              <dt className="text-content-muted">能力数</dt>
              <dd
                className="font-mono text-content-primary"
                data-testid="gateway-capabilities-count"
              >
                {status.capabilities.length}
              </dd>
              <dt className="text-content-muted">Token 优先级</dt>
              <dd className="font-mono text-content-primary" data-testid="gateway-precedence">
                {status.tokenization.precedence.join(', ')}
              </dd>
              <dt className="text-content-muted">协议版本</dt>
              <dd className="font-mono text-content-primary" data-testid="gateway-protocol-version">
                {status.protocolVersion ?? '—'}
              </dd>
            </dl>

            <div className="space-y-1">
              {status.providers.map((provider) => (
                <div
                  key={provider.id}
                  className="flex items-center gap-2 font-mono"
                  data-testid={`gateway-provider-${provider.id}`}
                >
                  <span className="text-content-primary">{provider.id}</span>
                  <span className="text-content-muted">{provider.mode}</span>
                  <span className={provider.importable ? 'text-signal-ok' : 'text-content-muted'}>
                    {provider.importable ? '可用' : '未安装'}
                  </span>
                </div>
              ))}
            </div>

            {status.lastError !== null ? (
              <p
                className="text-xs text-signal-error"
                role="alert"
                data-testid="gateway-last-error"
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

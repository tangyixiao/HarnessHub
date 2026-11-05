import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ProviderGatewayCard } from '@/features/settings/ProviderGatewayCard';

type Internals = { __TAURI_INTERNALS__?: unknown };

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

const F21_CAPABILITIES = [
  'ping',
  'runtime.info',
  'shutdown',
  'gateway.status',
  'gateway.routing',
  'gateway.streaming',
  'gateway.errors',
  'tokenization.precedence',
  'tokenization.estimate',
  'tokenization.revision',
];

const GATEWAY_READY_PAYLOAD = {
  state: 'ready',
  sidecarState: 'ready',
  mode: 'unified',
  providers: [
    { id: 'litellm', mode: 'unified', importable: false, detail: 'optional SDK not installed' },
    { id: 'openai', mode: 'native', importable: false, detail: 'optional SDK not installed' },
    { id: 'anthropic', mode: 'native', importable: false, detail: 'optional SDK not installed' },
    { id: 'google', mode: 'native', importable: false, detail: 'optional SDK not installed' },
  ],
  tokenization: {
    precedence: ['provider_reported', 'provider_count_api', 'local_exact', 'estimated'],
    levels: ['local_exact', 'estimated'],
    revisionBinding: true,
  },
  capabilities: F21_CAPABILITIES,
  protocolVersion: '1.0',
  lastError: null,
};

const GATEWAY_DEGRADED_PAYLOAD = {
  ...GATEWAY_READY_PAYLOAD,
  state: 'degraded',
  sidecarState: 'degraded',
  lastError: 'gateway.status 探测失败',
};

function gatewayCard(): HTMLElement {
  return screen.getByTestId('gateway-card');
}

describe('ProviderGatewayCard（F21 §7.3）', () => {
  it('渲染路径零 IPC：挂载后、点击前不发起任何 invoke 调用', () => {
    const invoke = stubCommands({ gateway_status: GATEWAY_READY_PAYLOAD });

    render(<ProviderGatewayCard />);

    expect(gatewayCard()).toBeInTheDocument();
    expect(within(gatewayCard()).getByTestId('gateway-refresh')).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalled();
    expect(invoke.mock.calls).toHaveLength(0);
  });

  it('点击「刷新」调用 gateway_status，并展示 state/mode/sidecarState/能力数/providers/未安装/precedence/protocolVersion', async () => {
    const invoke = stubCommands({ gateway_status: GATEWAY_READY_PAYLOAD });
    render(<ProviderGatewayCard />);

    expect(invoke).not.toHaveBeenCalled();
    await userEvent.click(within(gatewayCard()).getByTestId('gateway-refresh'));

    expect(invoke.mock.calls).toHaveLength(1);
    // invoke 收 3 个参数（command / args / options），只断言命令名，别断言参数个数。
    expect(invoke.mock.calls[0]?.[0]).toBe('gateway_status');

    const card = gatewayCard();
    expect(await within(card).findByTestId('gateway-state')).toHaveTextContent('ready');
    expect(within(card).getByTestId('gateway-mode')).toHaveTextContent('unified');
    expect(within(card).getByTestId('gateway-sidecar-state')).toHaveTextContent('ready');
    expect(within(card).getByTestId('gateway-capabilities-count')).toHaveTextContent('10');
    expect(within(card).getByTestId('gateway-precedence')).toHaveTextContent('provider_reported');
    expect(within(card).getByTestId('gateway-protocol-version')).toHaveTextContent('1.0');
    expect(within(card).getByTestId('gateway-provider-litellm')).toHaveTextContent('litellm');
    expect(within(card).getByTestId('gateway-provider-openai')).toHaveTextContent('未安装');
    expect(within(card).getAllByText('未安装')).toHaveLength(4);
    expect(within(card).queryByTestId('gateway-warning')).not.toBeInTheDocument();
  });

  it('importable:true 的 provider 显示「可用」', async () => {
    stubCommands({
      gateway_status: {
        ...GATEWAY_READY_PAYLOAD,
        providers: [{ id: 'litellm', mode: 'unified', importable: true, detail: 'ready' }],
      },
    });
    render(<ProviderGatewayCard />);

    await userEvent.click(within(gatewayCard()).getByTestId('gateway-refresh'));

    expect(await within(gatewayCard()).findByTestId('gateway-provider-litellm')).toHaveTextContent(
      '可用',
    );
  });

  for (const state of ['degraded', 'unavailable'] as const) {
    it(`${state} 状态渲染 gateway-warning 与「Provider Gateway 不可用，主 Harness 功能不受影响」`, async () => {
      stubCommands({ gateway_status: { ...GATEWAY_READY_PAYLOAD, state } });
      render(<ProviderGatewayCard />);

      await userEvent.click(within(gatewayCard()).getByTestId('gateway-refresh'));

      const warning = await within(gatewayCard()).findByTestId('gateway-warning');
      expect(warning).toHaveTextContent('Provider Gateway 不可用，主 Harness 功能不受影响');
    });
  }

  it('degraded 时展示非空 lastError', async () => {
    stubCommands({ gateway_status: GATEWAY_DEGRADED_PAYLOAD });
    render(<ProviderGatewayCard />);

    await userEvent.click(within(gatewayCard()).getByTestId('gateway-refresh'));

    expect(await within(gatewayCard()).findByTestId('gateway-last-error')).toHaveTextContent(
      'gateway.status 探测失败',
    );
  });
});

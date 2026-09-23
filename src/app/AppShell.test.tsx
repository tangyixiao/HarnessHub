import { render, screen } from '@testing-library/react';
import { RouterProvider, createMemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';

import { routes } from '@/app/routes';

function renderAt(path: string) {
  const router = createMemoryRouter(routes, { initialEntries: [path] });
  return render(<RouterProvider router={router} />);
}

const NAV_LABELS = [
  'Dashboard',
  'Harnesses',
  'Projects',
  'Sessions',
  'Terminal',
  'Activity',
  'Settings',
];

describe('AppShell', () => {
  it('渲染全部主导航项', async () => {
    renderAt('/');

    for (const label of NAV_LABELS) {
      expect(screen.getByRole('link', { name: label })).toBeInTheDocument();
    }

    // Dashboard 的 IPC 探测是异步的；等它落地，避免 React act() 警告。
    expect(await screen.findByText(/请用 pnpm tauri dev 启动桌面应用/)).toBeInTheDocument();
  });

  it('浏览器模式下明确提示 IPC 不可用，而不是静默失败', async () => {
    renderAt('/');

    expect(screen.getByText('浏览器模式 · IPC 不可用')).toBeInTheDocument();
    expect(await screen.findByText(/请用 pnpm tauri dev 启动桌面应用/)).toBeInTheDocument();
  });

  it('每个导航目标都能渲染对应页面标题', () => {
    const paths: Array<[string, string]> = [
      ['/harnesses', 'Harnesses'],
      ['/projects', 'Projects'],
      ['/sessions', 'Sessions'],
      ['/activity', 'Activity'],
      ['/settings', 'Settings'],
      ['/', 'Dashboard'],
    ];

    for (const [path, heading] of paths) {
      const { unmount } = renderAt(path);
      expect(screen.getByRole('heading', { level: 1, name: heading })).toBeInTheDocument();
      unmount();
    }
  });

  /**
   * Terminal 是懒加载路由：首屏只出 fallback，chunk 到达后才渲染真实页面。
   * 这条测试同时锁住「切分生效」与「行为不变」两件事。
   */
  it('Terminal 路由懒加载：chunk 到达后渲染真实页面', async () => {
    renderAt('/terminal');

    expect(await screen.findByRole('heading', { level: 1, name: 'Terminal' })).toBeInTheDocument();
  });

  it('未接入的功能页面如实说明尚未接入，不展示假数据', () => {
    // Harnesses 与 Terminal 都已接入真实功能，用仍未实现的 Settings 页做这条断言。
    renderAt('/settings');
    expect(screen.getByText('尚未接入')).toBeInTheDocument();
  });

  it('Harnesses 页不再显示占位文案', () => {
    renderAt('/harnesses');

    expect(screen.queryByText('尚未接入')).not.toBeInTheDocument();
  });
});

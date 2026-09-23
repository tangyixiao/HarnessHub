import { Suspense, lazy } from 'react';
import { createHashRouter, type RouteObject } from 'react-router-dom';

import { AppShell } from '@/app/AppShell';
import { ActivityPage } from '@/features/history/ActivityPage';
import { DashboardPage } from '@/features/dashboard/DashboardPage';
import { HarnessesPage } from '@/features/harnesses/HarnessesPage';
import { ProjectsPage } from '@/features/projects/ProjectsPage';
import { SessionsPage } from '@/features/sessions/SessionsPage';
import { SettingsPage } from '@/features/settings/SettingsPage';

/**
 * Terminal 路由**懒加载**。
 *
 * `@xterm/xterm` 只在 Terminal 页面用到，而它把首屏主 chunk 从约 360 kB 推到
 * 约 705 kB。普通页面（Dashboard / Harnesses / Sessions…）不该付这个成本，
 * 所以这里用 dynamic import 把它切出去 —— **只改加载边界，不改任何行为**：
 * 进入 Terminal 后的一切（ready-before-spawn、原始字节、卸载不 kill）完全不变。
 *
 * 刻意**不**做 preload：那等于把成本又拉回首屏，抵消这次切分。
 * 也刻意不放进 barrel/index：静态再导出会把 xterm 拉回主依赖链。
 */
const TerminalPage = lazy(async () => {
  const module = await import('@/features/terminal/TerminalPage');
  return { default: module.TerminalPage };
});

function TerminalRouteFallback() {
  return <p className="text-sm text-content-muted">正在加载终端模块…</p>;
}

/**
 * 使用 Hash 路由：Tauri 的 webview 通过自定义协议加载前端，
 * hash 路由不需要服务器侧 rewrite，打包后行为最稳定。
 */
export const routes: RouteObject[] = [
  {
    path: '/',
    element: <AppShell />,
    children: [
      { index: true, element: <DashboardPage /> },
      { path: 'harnesses', element: <HarnessesPage /> },
      { path: 'projects', element: <ProjectsPage /> },
      { path: 'sessions', element: <SessionsPage /> },
      {
        path: 'terminal',
        element: (
          <Suspense fallback={<TerminalRouteFallback />}>
            <TerminalPage />
          </Suspense>
        ),
      },
      { path: 'activity', element: <ActivityPage /> },
      { path: 'settings', element: <SettingsPage /> },
    ],
  },
];

export function createAppRouter() {
  return createHashRouter(routes);
}

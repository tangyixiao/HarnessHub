import { createHashRouter, type RouteObject } from 'react-router-dom';

import { AppShell } from '@/app/AppShell';
import { ActivityPage } from '@/features/history/ActivityPage';
import { DashboardPage } from '@/features/dashboard/DashboardPage';
import { HarnessesPage } from '@/features/harnesses/HarnessesPage';
import { ProjectsPage } from '@/features/projects/ProjectsPage';
import { SessionsPage } from '@/features/sessions/SessionsPage';
import { SettingsPage } from '@/features/settings/SettingsPage';
import { TerminalPage } from '@/features/terminal/TerminalPage';

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
      { path: 'terminal', element: <TerminalPage /> },
      { path: 'activity', element: <ActivityPage /> },
      { path: 'settings', element: <SettingsPage /> },
    ],
  },
];

export function createAppRouter() {
  return createHashRouter(routes);
}

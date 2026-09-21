import {
  Boxes,
  CalendarRange,
  FolderGit2,
  History,
  LayoutDashboard,
  Settings,
  Terminal,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { NavLink, Outlet } from 'react-router-dom';

import { isTauriRuntime } from '@/lib/ipc';
import { cn } from '@/lib/utils';

type NavItem = {
  to: string;
  label: string;
  icon: LucideIcon;
  end?: boolean;
};

const NAV_ITEMS: NavItem[] = [
  { to: '/', label: 'Dashboard', icon: LayoutDashboard, end: true },
  { to: '/harnesses', label: 'Harnesses', icon: Boxes },
  { to: '/projects', label: 'Projects', icon: FolderGit2 },
  { to: '/sessions', label: 'Sessions', icon: History },
  { to: '/terminal', label: 'Terminal', icon: Terminal },
  { to: '/activity', label: 'Activity', icon: CalendarRange },
  { to: '/settings', label: 'Settings', icon: Settings },
];

export function AppShell() {
  const connected = isTauriRuntime();

  return (
    <div className="flex h-full">
      <aside className="flex w-56 shrink-0 flex-col border-r border-border-subtle bg-surface-raised/40">
        <div className="px-4 py-5">
          <p className="text-sm font-semibold tracking-tight">Harness Hub</p>
          <p className="mt-0.5 text-[11px] text-content-muted">AI Development Control Plane</p>
        </div>

        <nav aria-label="主导航" className="flex-1 space-y-0.5 px-2">
          {NAV_ITEMS.map(({ to, label, icon: Icon, end }) => (
            <NavLink
              key={to}
              to={to}
              end={end}
              className={({ isActive }) =>
                cn(
                  'flex items-center gap-2.5 rounded-md px-3 py-2 text-sm transition-colors',
                  isActive
                    ? 'bg-surface-overlay text-content-primary'
                    : 'text-content-muted hover:bg-surface-raised hover:text-content-primary',
                )
              }
            >
              <Icon aria-hidden className="size-4" />
              {label}
            </NavLink>
          ))}
        </nav>

        <div className="border-t border-border-subtle px-4 py-3 text-[11px] text-content-muted">
          <span
            aria-hidden
            className={cn(
              'mr-1.5 inline-block size-1.5 rounded-full align-middle',
              connected ? 'bg-signal-ok' : 'bg-signal-warn',
            )}
          />
          {connected ? 'Tauri 运行时已连接' : '浏览器模式 · IPC 不可用'}
        </div>
      </aside>

      <main className="min-w-0 flex-1 overflow-y-auto px-8 py-7">
        <Outlet />
      </main>
    </div>
  );
}

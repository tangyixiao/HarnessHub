import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ProjectsPage } from '@/features/projects/ProjectsPage';

type Internals = { __TAURI_INTERNALS__?: unknown };

afterEach(() => {
  delete (window as unknown as Internals).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

function stubInvoke(handler: (command: string, args?: Record<string, unknown>) => unknown): void {
  (window as unknown as Internals).__TAURI_INTERNALS__ = {
    invoke: vi.fn(async (command: string, args?: Record<string, unknown>) =>
      handler(command, args),
    ),
  };
}

/** 与 Rust `ProjectRecord`（serde camelCase）同形。 */
const PROJECT = {
  id: 'proj-1',
  name: 'demo',
  repoRoot: 'D:/work/demo',
  createdAt: '2026-10-12T00:00:00Z',
  updatedAt: '2026-10-12T00:00:01Z',
  lastOpenedAt: '2026-10-12T00:00:01Z',
};

describe('ProjectsPage', () => {
  it('已登记的项目按最近打开倒序显示 repo root 与时间', async () => {
    stubInvoke((command) => {
      if (command === 'list_projects') {
        return [
          PROJECT,
          {
            ...PROJECT,
            id: 'proj-2',
            name: 'older',
            repoRoot: 'D:/work/older',
            lastOpenedAt: null,
          },
        ];
      }
      return null;
    });

    render(<ProjectsPage />);

    const first = await screen.findByRole('region', { name: 'demo' });
    expect(within(first).getByText('D:/work/demo')).toBeInTheDocument();
    expect(within(first).getByText('2026-10-12 00:00:01')).toBeInTheDocument();

    const second = screen.getByRole('region', { name: 'older' });
    // lastOpenedAt 为 null 显示 —；session 历史拉取前也短暂为 —，所以用 getAllByText。
    expect(within(second).getAllByText('—').length).toBeGreaterThanOrEqual(1);
  });

  it('登记项目：输入路径后调用 register_project 并刷新列表', async () => {
    const user = userEvent.setup();
    const registered = { ...PROJECT, name: 'fresh', repoRoot: 'D:/work/fresh' };
    let projects: unknown[] = [];
    stubInvoke((command, args) => {
      if (command === 'list_projects') {
        return projects;
      }
      if (command === 'register_project') {
        projects = [registered];
        expect(args).toEqual({ path: 'D:\\work\\fresh', name: null });
        return registered;
      }
      return null;
    });

    render(<ProjectsPage />);

    await user.type(await screen.findByLabelText('仓库路径'), 'D:\\work\\fresh');
    await user.click(screen.getByRole('button', { name: '登记项目' }));

    expect(await screen.findByRole('region', { name: 'fresh' })).toBeInTheDocument();
    expect(screen.queryByRole('region', { name: 'demo' })).not.toBeInTheDocument();
  });

  it('登记失败（不是 Git 仓库）时显示错误而不清空输入', async () => {
    const user = userEvent.setup();
    stubInvoke((command) => {
      if (command === 'list_projects') {
        return [];
      }
      if (command === 'register_project') {
        throw new Error('「D:/not-a-repo」不是有效的本地 Git 仓库：找不到 repo root');
      }
      return null;
    });

    render(<ProjectsPage />);

    await user.type(await screen.findByLabelText('仓库路径'), 'D:/not-a-repo');
    await user.click(screen.getByRole('button', { name: '登记项目' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(/不是有效的本地 Git 仓库/);
    expect(screen.getByLabelText('仓库路径')).toHaveValue('D:/not-a-repo');
    expect(screen.queryByRole('region')).not.toBeInTheDocument();
  });

  it('删除项目调用 remove_project 后刷新列表', async () => {
    const user = userEvent.setup();
    let projects: unknown[] = [PROJECT];
    stubInvoke((command, args) => {
      if (command === 'list_projects') {
        return projects;
      }
      if (command === 'remove_project') {
        expect(args).toEqual({ projectId: 'proj-1' });
        projects = [];
        return true;
      }
      return null;
    });

    render(<ProjectsPage />);

    await screen.findByRole('region', { name: 'demo' });
    await user.click(screen.getByRole('button', { name: '删除' }));

    expect(await screen.findByText('还没有登记任何项目')).toBeInTheDocument();
  });

  it('没有任何项目时显示空态', async () => {
    stubInvoke((command) => (command === 'list_projects' ? [] : null));

    render(<ProjectsPage />);

    expect(await screen.findByText('还没有登记任何项目')).toBeInTheDocument();
  });

  it('IPC 报错时显示错误卡片', async () => {
    stubInvoke(() => {
      throw new Error('database is locked');
    });

    render(<ProjectsPage />);

    expect(await screen.findByText(/database is locked/)).toBeInTheDocument();
  });

  it('浏览器模式显示引导文案', async () => {
    render(<ProjectsPage />);

    expect(await screen.findByText(/请用 pnpm tauri dev 启动桌面应用/)).toBeInTheDocument();
  });
});

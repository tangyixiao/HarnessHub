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

/** 与 Rust `WorktreeEntry`（serde camelCase）同形：主树 / 链接树 / detached 各一。 */
const WORKTREE_LIST = [
  {
    path: 'D:/work/demo',
    head: '1234567890abcdef1234567890abcdef12345678',
    branch: 'main',
    main: true,
    detached: false,
    bare: false,
  },
  {
    path: 'D:/wt/feature',
    head: 'abcdef1234567890abcdef1234567890abcdef12',
    branch: 'feature',
    main: false,
    detached: false,
    bare: false,
  },
  {
    path: 'D:/wt/detached',
    head: null,
    branch: null,
    main: false,
    detached: true,
    bare: false,
  },
];

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

  describe('Worktrees 区（ROADMAP C10）', () => {
    it('默认折叠不发请求；展开后渲染路径 / 分支 / detached / HEAD 前 7 位 / 主树徽标', async () => {
      const user = userEvent.setup();
      const invokeSpy = vi.fn(async (command: string) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return WORKTREE_LIST;
        return null;
      });
      stubInvoke(invokeSpy);

      render(<ProjectsPage />);
      await screen.findByRole('region', { name: 'demo' });

      const toggle = screen.getByRole('button', { name: 'Worktrees' });
      expect(toggle).toHaveAttribute('aria-expanded', 'false');
      // 折叠时绝不发 worktree_list（避免每张卡片自动请求）。
      expect(invokeSpy).not.toHaveBeenCalledWith('worktree_list', expect.anything());

      await user.click(toggle);

      const region = screen.getByRole('region', { name: 'demo' });
      expect(await within(region).findByText('D:/wt/feature')).toBeInTheDocument();
      expect(within(region).getByText('feature')).toBeInTheDocument();
      expect(within(region).getByText('abcdef1')).toBeInTheDocument(); // HEAD 前 7 位
      expect(within(region).getByText('detached')).toBeInTheDocument();
      expect(within(region).getByText('主工作树')).toBeInTheDocument();
      expect(invokeSpy).toHaveBeenCalledWith('worktree_list', { repoRoot: 'D:/work/demo' });
    });

    it('提交创建前 confirm 被调用且文案含「将在磁盘上创建目录并检出代码」与目标路径；成功后刷新并高亮', async () => {
      const user = userEvent.setup();
      const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
      let worktrees: unknown[] = [];
      const invokeSpy = vi.fn(async (command: string, args?: Record<string, unknown>) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return worktrees;
        if (command === 'worktree_add') {
          expect(args).toEqual({
            repoRoot: 'D:/work/demo',
            path: 'D:/wt/feature',
            branch: 'feature',
            createBranch: true,
          });
          worktrees = [...worktrees, WORKTREE_LIST[1]];
          return WORKTREE_LIST[1];
        }
        return null;
      });
      stubInvoke(invokeSpy);

      render(<ProjectsPage />);
      await screen.findByRole('region', { name: 'demo' });
      await user.click(screen.getByRole('button', { name: 'Worktrees' }));
      await user.click(await screen.findByRole('button', { name: '新建 Worktree' }));

      await user.type(screen.getByLabelText('Worktree 路径'), 'D:/wt/feature');
      await user.type(screen.getByLabelText('分支名'), 'feature');
      // 复选框「创建新分支」默认勾选。
      expect(screen.getByRole('checkbox', { name: '创建新分支' })).toBeChecked();

      await user.click(screen.getByRole('button', { name: '创建' }));

      expect(confirmSpy).toHaveBeenCalledTimes(1);
      expect(confirmSpy.mock.calls[0]?.[0]).toContain('将在磁盘上创建目录并检出代码');
      expect(confirmSpy.mock.calls[0]?.[0]).toContain('D:/wt/feature');

      // 创建成功：列表刷新出新条目并带高亮样式。
      const region = screen.getByRole('region', { name: 'demo' });
      const row = await within(region).findByText('D:/wt/feature');
      expect(row.closest('li')).toHaveClass('ring-accent/40');
    });

    it('confirm 取消时不调用 worktree_add', async () => {
      const user = userEvent.setup();
      vi.spyOn(window, 'confirm').mockReturnValue(false);
      const invokeSpy = vi.fn(async (command: string) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return [];
        return null;
      });
      stubInvoke(invokeSpy);

      render(<ProjectsPage />);
      await screen.findByRole('region', { name: 'demo' });
      await user.click(screen.getByRole('button', { name: 'Worktrees' }));
      await user.click(screen.getByRole('button', { name: '新建 Worktree' }));
      await user.type(screen.getByLabelText('Worktree 路径'), 'D:/wt/feature');
      await user.type(screen.getByLabelText('分支名'), 'feature');
      await user.click(screen.getByRole('button', { name: '创建' }));

      expect(invokeSpy).not.toHaveBeenCalledWith('worktree_add', expect.anything());
    });

    it('列表加载失败（未登记的项目根）显示行内错误', async () => {
      const user = userEvent.setup();
      stubInvoke((command) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') {
          throw new Error('未登记的项目根：D:/work/demo（先在 Projects 页登记）');
        }
        return null;
      });

      render(<ProjectsPage />);
      await screen.findByRole('region', { name: 'demo' });
      await user.click(screen.getByRole('button', { name: 'Worktrees' }));

      const region = screen.getByRole('region', { name: 'demo' });
      expect(await within(region).findByRole('alert')).toHaveTextContent(/未登记的项目根/);
    });

    it('创建失败（路径已占用）在表单内原样显示错误', async () => {
      const user = userEvent.setup();
      vi.spyOn(window, 'confirm').mockReturnValue(true);
      stubInvoke((command) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return [];
        if (command === 'worktree_add') {
          throw new Error("fatal: 'D:/wt/feature' already exists");
        }
        return null;
      });

      render(<ProjectsPage />);
      await screen.findByRole('region', { name: 'demo' });
      await user.click(screen.getByRole('button', { name: 'Worktrees' }));
      await user.click(screen.getByRole('button', { name: '新建 Worktree' }));
      await user.type(screen.getByLabelText('Worktree 路径'), 'D:/wt/feature');
      await user.type(screen.getByLabelText('分支名'), 'feature');
      await user.click(screen.getByRole('button', { name: '创建' }));

      expect(await screen.findByRole('alert')).toHaveTextContent(
        "fatal: 'D:/wt/feature' already exists",
      );
    });
  });
});

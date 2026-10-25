import { render, screen, waitFor, within } from '@testing-library/react';
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

  /**
   * 定位包含指定路径文本的 worktree 行（行内查询徽标 / 按钮要用 within 缩小范围，
   * 因为项目卡片自己也带一个「删除」按钮；主树路径与 repo root 相同时卡片描述里
   * 也有同文文本，所以要只认 <li> 内的匹配）。
   */
  function worktreeRow(path: string): HTMLElement {
    const row = screen
      .getAllByText(path)
      .map((el) => el.closest('li'))
      .find((li) => li !== null);
    if (!(row instanceof HTMLElement)) {
      throw new Error(`找不到 worktree 行：${path}`);
    }
    return row;
  }

  /** 展开前先等到列表出现（行挂载后才会各自惰性查询状态）。 */
  async function expandWorktrees(user: ReturnType<typeof userEvent.setup>): Promise<void> {
    await screen.findByRole('region', { name: 'demo' });
    await user.click(screen.getByRole('button', { name: 'Worktrees' }));
    await screen.findByText('D:/wt/feature');
  }

  describe('Worktrees 区升级（ROADMAP C11）', () => {
    it('每行惰性查询状态：干净显示「干净」，有修改显示「修改 N 项」；主树行没有操作按钮', async () => {
      const user = userEvent.setup();
      const invokeSpy = vi.fn(async (command: string, args?: Record<string, unknown>) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return WORKTREE_LIST;
        if (command === 'worktree_status') {
          // C11 契约：worktree_status 传 repoRoot + worktreePath。
          expect(args).toEqual({
            repoRoot: 'D:/work/demo',
            worktreePath: expect.any(String),
          });
          if (args?.worktreePath === 'D:/wt/feature') {
            return { stagedFiles: 2, unstagedFiles: 1, untrackedFiles: 3, conflictedFiles: 0 };
          }
          return { stagedFiles: 0, unstagedFiles: 0, untrackedFiles: 0, conflictedFiles: 0 };
        }
        return null;
      });
      stubInvoke(invokeSpy);

      render(<ProjectsPage />);
      await expandWorktrees(user);

      const featureRow = worktreeRow('D:/wt/feature');
      // N = 四类计数之和：2 + 1 + 3 + 0 = 6。
      expect(await within(featureRow).findByText('修改 6 项')).toBeInTheDocument();
      expect(within(featureRow).getByRole('button', { name: '合并到主分支' })).toBeInTheDocument();
      expect(within(featureRow).getByRole('button', { name: '删除' })).toBeInTheDocument();

      // 干净的行（主树 + detached）显示「干净」徽标。
      const mainRow = worktreeRow('D:/work/demo');
      expect(await within(mainRow).findByText('干净')).toBeInTheDocument();
      const detachedRow = worktreeRow('D:/wt/detached');
      expect(await within(detachedRow).findByText('干净')).toBeInTheDocument();

      // 主树行不显示操作按钮（合并/删除只属于链接树）。
      expect(
        within(mainRow).queryByRole('button', { name: '合并到主分支' }),
      ).not.toBeInTheDocument();
      expect(within(mainRow).queryByRole('button', { name: '删除' })).not.toBeInTheDocument();
      // detached 行没有分支 → 没有合并按钮，但删除按钮在。
      expect(
        within(detachedRow).queryByRole('button', { name: '合并到主分支' }),
      ).not.toBeInTheDocument();
      expect(within(detachedRow).getByRole('button', { name: '删除' })).toBeInTheDocument();
    });

    it('状态查询失败显示「状态未知」徽标与行内错误，不阻塞按钮', async () => {
      const user = userEvent.setup();
      stubInvoke((command) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return WORKTREE_LIST;
        if (command === 'worktree_status') {
          throw new Error('git status 失败：退出码 128');
        }
        return null;
      });

      render(<ProjectsPage />);
      await expandWorktrees(user);

      const row = worktreeRow('D:/wt/feature');
      expect(await within(row).findByText('状态未知')).toBeInTheDocument();
      expect(await within(row).findByRole('alert')).toHaveTextContent(
        'git status 失败：退出码 128',
      );
      // 徽标失败不阻塞整行：操作按钮仍然可用。
      expect(within(row).getByRole('button', { name: '删除' })).toBeEnabled();
    });

    it('合并：confirm 文案含「快进合并」与分支名与「非快进会被拒绝」；成功显示「已合并 <前 7 位>」并刷新列表', async () => {
      const user = userEvent.setup();
      const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
      const invokeSpy = vi.fn(async (command: string, args?: Record<string, unknown>) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return WORKTREE_LIST;
        if (command === 'worktree_status') {
          return { stagedFiles: 0, unstagedFiles: 0, untrackedFiles: 0, conflictedFiles: 0 };
        }
        if (command === 'worktree_merge') {
          expect(args).toEqual({ repoRoot: 'D:/work/demo', branch: 'feature' });
          return {
            previousHead: '1234567890abcdef1234567890abcdef12345678',
            newHead: 'fedcba9876543210fedcba9876543210fedcba98',
          };
        }
        return null;
      });
      stubInvoke(invokeSpy);

      render(<ProjectsPage />);
      await expandWorktrees(user);
      await user.click(
        within(worktreeRow('D:/wt/feature')).getByRole('button', { name: '合并到主分支' }),
      );

      expect(confirmSpy).toHaveBeenCalledTimes(1);
      expect(confirmSpy.mock.calls[0]?.[0]).toContain('快进合并');
      expect(confirmSpy.mock.calls[0]?.[0]).toContain('feature');
      expect(confirmSpy.mock.calls[0]?.[0]).toContain('非快进会被拒绝');

      // 成功提示与列表刷新（worktree_list 被再次调用）。
      expect(await screen.findByText('已合并 fedcba9')).toBeInTheDocument();
      expect(invokeSpy).toHaveBeenCalledWith('worktree_list', { repoRoot: 'D:/work/demo' });
    });

    it('confirm 取消合并不调用 worktree_merge', async () => {
      const user = userEvent.setup();
      vi.spyOn(window, 'confirm').mockReturnValue(false);
      const invokeSpy = vi.fn(async (command: string) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return WORKTREE_LIST;
        if (command === 'worktree_status') {
          return { stagedFiles: 0, unstagedFiles: 0, untrackedFiles: 0, conflictedFiles: 0 };
        }
        return null;
      });
      stubInvoke(invokeSpy);

      render(<ProjectsPage />);
      await expandWorktrees(user);
      await user.click(
        within(worktreeRow('D:/wt/feature')).getByRole('button', { name: '合并到主分支' }),
      );

      expect(invokeSpy).not.toHaveBeenCalledWith('worktree_merge', expect.anything());
    });

    it('合并失败（主树脏被拒）在行内原样展示错误', async () => {
      const user = userEvent.setup();
      vi.spyOn(window, 'confirm').mockReturnValue(true);
      stubInvoke((command) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return WORKTREE_LIST;
        if (command === 'worktree_status') {
          return { stagedFiles: 0, unstagedFiles: 0, untrackedFiles: 0, conflictedFiles: 0 };
        }
        if (command === 'worktree_merge') {
          throw new Error('主工作树有未提交修改，拒绝合并（暂存 1 / 未暂存 0 / 未跟踪 0）');
        }
        return null;
      });

      render(<ProjectsPage />);
      await expandWorktrees(user);
      await user.click(
        within(worktreeRow('D:/wt/feature')).getByRole('button', { name: '合并到主分支' }),
      );

      expect(await within(worktreeRow('D:/wt/feature')).findByRole('alert')).toHaveTextContent(
        '主工作树有未提交修改，拒绝合并（暂存 1 / 未暂存 0 / 未跟踪 0）',
      );
      // 失败后行仍在，可以重试。
      expect(
        within(worktreeRow('D:/wt/feature')).getByRole('button', { name: '合并到主分支' }),
      ).toBeEnabled();
    });

    it('删除：confirm 文案含「删除目录」与路径与「有未提交修改会被拒绝」；成功后行从列表移除', async () => {
      const user = userEvent.setup();
      const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
      let worktrees: unknown[] = WORKTREE_LIST;
      stubInvoke((command, args) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return worktrees;
        if (command === 'worktree_status') {
          return { stagedFiles: 0, unstagedFiles: 0, untrackedFiles: 0, conflictedFiles: 0 };
        }
        if (command === 'worktree_remove') {
          expect(args).toEqual({ repoRoot: 'D:/work/demo', worktreePath: 'D:/wt/feature' });
          worktrees = worktrees.filter(
            (entry) => (entry as { path: string }).path !== 'D:/wt/feature',
          );
          return null;
        }
        return null;
      });

      render(<ProjectsPage />);
      await expandWorktrees(user);
      await user.click(within(worktreeRow('D:/wt/feature')).getByRole('button', { name: '删除' }));

      expect(confirmSpy).toHaveBeenCalledTimes(1);
      expect(confirmSpy.mock.calls[0]?.[0]).toContain('删除目录');
      expect(confirmSpy.mock.calls[0]?.[0]).toContain('D:/wt/feature');
      expect(confirmSpy.mock.calls[0]?.[0]).toContain('有未提交修改会被拒绝');

      // 成功后刷新列表，该行消失。
      await waitFor(() => {
        expect(screen.queryByText('D:/wt/feature')).not.toBeInTheDocument();
      });
    });

    it('confirm 取消删除不调用 worktree_remove', async () => {
      const user = userEvent.setup();
      vi.spyOn(window, 'confirm').mockReturnValue(false);
      const invokeSpy = vi.fn(async (command: string) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return WORKTREE_LIST;
        if (command === 'worktree_status') {
          return { stagedFiles: 0, unstagedFiles: 0, untrackedFiles: 0, conflictedFiles: 0 };
        }
        return null;
      });
      stubInvoke(invokeSpy);

      render(<ProjectsPage />);
      await expandWorktrees(user);
      await user.click(within(worktreeRow('D:/wt/feature')).getByRole('button', { name: '删除' }));

      expect(invokeSpy).not.toHaveBeenCalledWith('worktree_remove', expect.anything());
    });

    it('删除失败（脏树被拒）在行内原样展示错误', async () => {
      const user = userEvent.setup();
      vi.spyOn(window, 'confirm').mockReturnValue(true);
      stubInvoke((command) => {
        if (command === 'list_projects') return [PROJECT];
        if (command === 'get_project_sessions') return [];
        if (command === 'worktree_list') return WORKTREE_LIST;
        if (command === 'worktree_status') {
          return { stagedFiles: 0, unstagedFiles: 0, untrackedFiles: 0, conflictedFiles: 0 };
        }
        if (command === 'worktree_remove') {
          throw new Error(
            'worktree 有未提交修改，拒绝删除（暂存 2 / 未暂存 1 / 未跟踪 3 / 冲突 0）——请先处理或手动清理',
          );
        }
        return null;
      });

      render(<ProjectsPage />);
      await expandWorktrees(user);
      await user.click(within(worktreeRow('D:/wt/feature')).getByRole('button', { name: '删除' }));

      expect(await within(worktreeRow('D:/wt/feature')).findByRole('alert')).toHaveTextContent(
        'worktree 有未提交修改，拒绝删除',
      );
      // 失败后行仍在。
      expect(screen.getByText('D:/wt/feature')).toBeInTheDocument();
    });
  });
});

import { useCallback, useEffect, useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import {
  addWorktree,
  getProjectSessions,
  listProjects,
  listWorktrees,
  registerProject,
  removeProject,
  type ProjectRecord,
  type WorktreeEntry,
} from '@/lib/ipc';

type PageState =
  | { kind: 'loading' }
  | { kind: 'ipc-unavailable' }
  | { kind: 'error'; message: string }
  | { kind: 'ready'; projects: ProjectRecord[] };

type RegisterState =
  { kind: 'idle' } | { kind: 'registering' } | { kind: 'failed'; message: string };

function formatTimestamp(value: string): string {
  return value.replace('T', ' ').replace(/\.\d+/, '').replace('Z', '');
}

/** 每张项目卡片单独拉一次该项目的历史来显示条数；失败时按 0 处理（readiness 不阻塞列表）。 */
function ProjectCard({
  project,
  onRemove,
}: {
  project: ProjectRecord;
  onRemove: (id: string) => void;
}) {
  const [sessionCount, setSessionCount] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const result = await getProjectSessions(project.id);
      if (!cancelled && result.ok) {
        setSessionCount(result.data.length);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [project.id]);

  return <ProjectCardBody project={project} sessionCount={sessionCount} onRemove={onRemove} />;
}

export function ProjectsPage() {
  const [state, setState] = useState<PageState>({ kind: 'loading' });
  const [pathInput, setPathInput] = useState('');
  const [registerState, setRegisterState] = useState<RegisterState>({ kind: 'idle' });

  const loadProjects = useCallback(async (): Promise<ProjectRecord[] | null> => {
    const result = await listProjects();
    if (!result.ok) {
      if (result.error === 'not-running-in-tauri') {
        setState({ kind: 'ipc-unavailable' });
      } else {
        setState({ kind: 'error', message: result.error });
      }
      return null;
    }
    setState({ kind: 'ready', projects: result.data });
    return result.data;
  }, []);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const projects = await listProjects();
      if (cancelled) {
        return;
      }
      if (!projects.ok) {
        if (projects.error === 'not-running-in-tauri') {
          setState({ kind: 'ipc-unavailable' });
        } else {
          setState({ kind: 'error', message: projects.error });
        }
        return;
      }
      setState({ kind: 'ready', projects: projects.data });
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  const handleRegister = async () => {
    const path = pathInput.trim();
    if (!path || registerState.kind === 'registering') {
      return;
    }

    setRegisterState({ kind: 'registering' });
    const result = await registerProject(path);
    if (!result.ok) {
      setRegisterState({ kind: 'failed', message: result.error });
      return;
    }

    setPathInput('');
    setRegisterState({ kind: 'idle' });
    await loadProjects();
  };

  const handleRemove = async (projectId: string) => {
    const result = await removeProject(projectId);
    if (!result.ok) {
      setRegisterState({ kind: 'failed', message: result.error });
      return;
    }
    await loadProjects();
  };

  return (
    <div className="mx-auto w-full max-w-5xl px-8 py-7">
      <PageHeader
        title="Projects"
        description="本地 Git 仓库注册表：登记时自动识别 repo root，按 root 幂等去重。"
      />

      <Card className="mt-6">
        <CardTitle>登记本地项目</CardTitle>
        <CardDescription>输入仓库目录的本地路径；只读取 Git 元数据，不改写仓库。</CardDescription>
        <div className="mt-4 flex gap-3">
          <input
            type="text"
            value={pathInput}
            onChange={(event) => setPathInput(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                void handleRegister();
              }
            }}
            placeholder="例如 D:\work\demo"
            aria-label="仓库路径"
            className="min-w-0 flex-1 rounded-md border border-white/10 bg-white/[0.04] px-3 py-2 font-mono text-xs text-content-primary outline-none placeholder:text-content-muted focus:border-accent/60"
          />
          <Button
            onClick={() => void handleRegister()}
            disabled={registerState.kind === 'registering'}
          >
            {registerState.kind === 'registering' ? '登记中…' : '登记项目'}
          </Button>
        </div>
        {registerState.kind === 'failed' ? (
          <p role="alert" className="mt-3 text-xs text-red-400">
            登记失败：{registerState.message}
          </p>
        ) : null}
      </Card>

      {state.kind === 'loading' ? (
        <Card className="mt-4">
          <CardTitle>加载中…</CardTitle>
          <CardDescription>正在读取项目列表。</CardDescription>
        </Card>
      ) : null}

      {state.kind === 'ipc-unavailable' ? (
        <Card className="mt-4">
          <CardTitle>浏览器模式</CardTitle>
          <CardDescription>请用 pnpm tauri dev 启动桌面应用后再查看项目列表。</CardDescription>
        </Card>
      ) : null}

      {state.kind === 'error' ? (
        <Card className="mt-4">
          <CardTitle>加载失败</CardTitle>
          <CardDescription>{state.message}</CardDescription>
        </Card>
      ) : null}

      {state.kind === 'ready' && state.projects.length === 0 ? (
        <Card className="mt-4">
          <CardTitle>还没有登记任何项目</CardTitle>
          <CardDescription>
            在上方输入本地 Git 仓库路径即可登记；登记后可在此查看最近打开时间与 Session 历史。
          </CardDescription>
        </Card>
      ) : null}

      {state.kind === 'ready' && state.projects.length > 0 ? (
        <div className="mt-4 space-y-3">
          {state.projects.map((project) => (
            <section key={project.id} aria-label={project.name || project.repoRoot}>
              <ProjectCard project={project} onRemove={(id) => void handleRemove(id)} />
            </section>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function ProjectCardBody({
  project,
  sessionCount,
  onRemove,
}: {
  project: ProjectRecord;
  sessionCount: number | null;
  onRemove: (id: string) => void;
}) {
  return (
    <Card>
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <CardTitle className="truncate">{project.name}</CardTitle>
          <CardDescription className="truncate font-mono">{project.repoRoot}</CardDescription>
        </div>
        <Button variant="ghost" size="sm" onClick={() => onRemove(project.id)}>
          删除
        </Button>
      </div>
      <dl className="mt-4 grid gap-3 text-xs sm:grid-cols-3">
        <div>
          <dt className="text-content-muted">最近打开</dt>
          <dd className="mt-0.5 font-mono text-[11px]">
            {project.lastOpenedAt ? formatTimestamp(project.lastOpenedAt) : '—'}
          </dd>
        </div>
        <div>
          <dt className="text-content-muted">登记时间</dt>
          <dd className="mt-0.5 font-mono text-[11px]">{formatTimestamp(project.createdAt)}</dd>
        </div>
        <div>
          <dt className="text-content-muted">Session 历史</dt>
          <dd className="mt-0.5 font-mono text-[11px]">
            {sessionCount === null ? '—' : `${sessionCount} 条`}
          </dd>
        </div>
      </dl>
      <ProjectWorktrees repoRoot={project.repoRoot} />
    </Card>
  );
}

type WorktreeListState =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ready'; entries: WorktreeEntry[] }
  | { kind: 'failed'; message: string };

/**
 * 卡片内的 Worktrees 区：默认折叠，展开时才发 `worktree_list`（避免每张
 * 卡片自动请求）。创建是**显式用户动作**：提交前 window.confirm 二次确认
 * 「将在磁盘上创建目录并检出代码」，失败原样展示后端错误。
 */
function ProjectWorktrees({ repoRoot }: { repoRoot: string }) {
  const [expanded, setExpanded] = useState(false);
  const [listState, setListState] = useState<WorktreeListState>({ kind: 'idle' });
  const [formOpen, setFormOpen] = useState(false);
  const [pathInput, setPathInput] = useState('');
  const [branchInput, setBranchInput] = useState('');
  const [createBranch, setCreateBranch] = useState(true);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  const [highlightPath, setHighlightPath] = useState<string | null>(null);

  const loadWorktrees = useCallback(async () => {
    setListState({ kind: 'loading' });
    const result = await listWorktrees(repoRoot);
    if (result.ok) {
      setListState({ kind: 'ready', entries: result.data });
    } else {
      setListState({ kind: 'failed', message: result.error });
    }
  }, [repoRoot]);

  const handleToggle = () => {
    const next = !expanded;
    setExpanded(next);
    // 只有第一次展开才拉列表；折叠不重置，避免反复展开来回打请求。
    if (next && listState.kind === 'idle') {
      void loadWorktrees();
    }
  };

  const handleCreate = async () => {
    const path = pathInput.trim();
    const branch = branchInput.trim();
    if (!path || !branch || creating) {
      return;
    }

    // 写磁盘动作：确认文案必须让用户知道目录会被真实创建在目标路径。
    const confirmed = window.confirm(
      `将在磁盘上创建目录并检出代码：\n${path}\n` +
        `（分支：${branch}，${createBranch ? '新建分支' : '使用已有分支'}）`,
    );
    if (!confirmed) {
      return;
    }

    setCreating(true);
    setCreateError(null);
    const result = await addWorktree(repoRoot, path, branch, createBranch);
    if (!result.ok) {
      setCreating(false);
      setCreateError(result.error);
      return;
    }

    setHighlightPath(result.data.path);
    setCreating(false);
    setFormOpen(false);
    setPathInput('');
    setBranchInput('');
    setCreateBranch(true);
    await loadWorktrees();
  };

  return (
    <div className="mt-4 border-t border-white/[0.06] pt-3">
      <div className="flex items-center justify-between gap-2">
        <button
          type="button"
          aria-expanded={expanded}
          onClick={handleToggle}
          className="rounded-sm text-xs text-content-muted outline-none hover:text-content-primary focus-visible:outline-2 focus-visible:outline-accent"
        >
          Worktrees
        </button>
        {expanded ? (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              setFormOpen((open) => !open);
              setCreateError(null);
            }}
          >
            新建 Worktree
          </Button>
        ) : null}
      </div>

      {expanded ? (
        <div className="mt-2">
          {listState.kind === 'loading' ? (
            <p className="text-xs text-content-muted">加载中…</p>
          ) : null}

          {listState.kind === 'failed' ? (
            <p role="alert" className="text-xs text-red-400">
              Worktree 列表加载失败：{listState.message}
            </p>
          ) : null}

          {listState.kind === 'ready' && listState.entries.length === 0 ? (
            <p className="text-xs text-content-muted">没有可显示的 worktree。</p>
          ) : null}

          {listState.kind === 'ready' && listState.entries.length > 0 ? (
            <ul className="space-y-1">
              {listState.entries.map((entry) => {
                const highlighted = entry.path === highlightPath;
                return (
                  <li
                    key={entry.path}
                    className={`flex items-center gap-2 rounded-md px-2 py-1 text-xs ring-1 ${
                      highlighted ? 'bg-accent/[0.12] ring-accent/40' : 'ring-transparent'
                    }`}
                  >
                    <span
                      className="min-w-0 flex-1 truncate font-mono text-[11px]"
                      title={entry.path}
                    >
                      {entry.path}
                    </span>
                    <span className="shrink-0 font-mono text-[11px] text-content-secondary">
                      {entry.branch ?? (entry.detached ? 'detached' : '—')}
                    </span>
                    <span className="shrink-0 font-mono text-[11px] text-content-muted">
                      {entry.head ? entry.head.slice(0, 7) : '—'}
                    </span>
                    {entry.main ? (
                      <span className="shrink-0 rounded-sm bg-white/[0.06] px-1.5 py-0.5 text-[10px] text-content-secondary">
                        主工作树
                      </span>
                    ) : null}
                  </li>
                );
              })}
            </ul>
          ) : null}

          {formOpen ? (
            <form
              className="mt-3 space-y-2 rounded-md border border-white/[0.06] bg-white/[0.02] p-3"
              onSubmit={(event) => {
                event.preventDefault();
                void handleCreate();
              }}
            >
              <div className="flex flex-col gap-2 sm:flex-row">
                <input
                  type="text"
                  value={pathInput}
                  onChange={(event) => setPathInput(event.target.value)}
                  placeholder="新 worktree 路径，例如 D:\wt\feature"
                  aria-label="Worktree 路径"
                  className="min-w-0 flex-1 rounded-md border border-white/10 bg-white/[0.04] px-3 py-2 font-mono text-xs text-content-primary outline-none placeholder:text-content-muted focus:border-accent/60"
                />
                <input
                  type="text"
                  value={branchInput}
                  onChange={(event) => setBranchInput(event.target.value)}
                  placeholder="分支名，例如 feature"
                  aria-label="分支名"
                  className="min-w-0 flex-1 rounded-md border border-white/10 bg-white/[0.04] px-3 py-2 font-mono text-xs text-content-primary outline-none placeholder:text-content-muted focus:border-accent/60"
                />
              </div>
              <label className="flex w-fit items-center gap-2 text-xs text-content-secondary">
                <input
                  type="checkbox"
                  checked={createBranch}
                  onChange={(event) => setCreateBranch(event.target.checked)}
                />
                创建新分支
              </label>
              <div className="flex items-center gap-2">
                <Button type="submit" size="sm" disabled={creating}>
                  {creating ? '创建中…' : '创建'}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    setFormOpen(false);
                    setCreateError(null);
                  }}
                >
                  取消
                </Button>
              </div>
              {createError !== null ? (
                <p role="alert" className="text-xs text-red-400">
                  创建失败：{createError}
                </p>
              ) : null}
            </form>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

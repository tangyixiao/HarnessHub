import { useCallback, useEffect, useState } from 'react';

import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';
import {
  getProjectSessions,
  listProjects,
  registerProject,
  removeProject,
  type ProjectRecord,
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
    </Card>
  );
}

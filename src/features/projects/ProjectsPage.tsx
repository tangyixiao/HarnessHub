import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';

export function ProjectsPage() {
  return (
    <div>
      <PageHeader
        title="Projects"
        description="本地 Git 仓库注册表：repo root、最近打开、项目级 Harness 设置与 Session 历史。"
      />
      <Card>
        <CardTitle>尚未接入</CardTitle>
        <CardDescription>
          依赖 Project Registry 与 SQLite 的 projects 表（迁移 0001 已建表）。添加项目时会自动识别
          repo root，并只读取 Git 元数据。
        </CardDescription>
        <p className="mt-3 text-[11px] text-content-muted">
          目标模块：src-tauri/src/git/、src-tauri/src/session/、src/features/projects/
        </p>
      </Card>
    </div>
  );
}

import { PlaceholderCard } from '@/components/ui/placeholder-card';
import { PageHeader } from '@/components/ui/page-header';

export function ProjectsPage() {
  return (
    <div>
      <PageHeader
        title="Projects"
        description="本地 Git 仓库注册表：repo root、最近打开、项目级 Harness 设置与 Session 历史。"
      />
      <PlaceholderCard
        title="尚未接入"
        hint="目标模块：src-tauri/src/git/、src-tauri/src/session/、src/features/projects/"
      >
        依赖 Project Registry 与 SQLite 的 projects 表（迁移 0001 已建表）。添加项目时会自动识别
        repo root，并只读取 Git 元数据。
      </PlaceholderCard>
    </div>
  );
}

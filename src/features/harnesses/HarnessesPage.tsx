import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';

export function HarnessesPage() {
  return (
    <div>
      <PageHeader
        title="Harnesses"
        description="本机已安装的 AI Coding Harness：binary 路径、版本、能力矩阵与数据路径。"
      />
      <Card>
        <CardTitle>尚未接入</CardTitle>
        <CardDescription>
          Phase 1 Task「Codex 检测」起开始实现。能力不是 boolean，而是可逐项灰度的矩阵
          （launch / terminal / resume / usage / replay / tool_calls / live_state / worktree）。
        </CardDescription>
        <p className="mt-3 text-[11px] text-content-muted">
          目标模块：src-tauri/src/harness/（adapter.rs / registry.rs / adapters/）
        </p>
      </Card>
    </div>
  );
}

import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';

export function SessionsPage() {
  return (
    <div>
      <PageHeader
        title="Sessions"
        description="Harness Hub 统一 Session 索引：hub_session_id 与外部 source_session_id 分离管理。"
      />
      <Card>
        <CardTitle>尚未接入</CardTitle>
        <CardDescription>
          sessions 表已在迁移 0001 中建立（含 runtime_target_id、parent_session_id、worktree_path）。
          待 PTY 启动链路完成后填充数据，并支持退出重启后仍可查看历史。
        </CardDescription>
        <p className="mt-3 text-[11px] text-content-muted">
          目标模块：src-tauri/src/session/、src-tauri/src/pty/
        </p>
      </Card>
    </div>
  );
}

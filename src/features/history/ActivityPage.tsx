import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';

export function ActivityPage() {
  return (
    <div>
      <PageHeader
        title="Activity"
        description="长期 Coding Activity Timeline：Date × Project × Harness × Model × Session × Token × Git 变化。"
      />
      <Card>
        <CardTitle>尚未接入</CardTitle>
        <CardDescription>
          Phase 3 的差异化功能。依赖已建好的 usage_events / git_events 表与幂等导入链路。
        </CardDescription>
        <p className="mt-3 text-[11px] text-content-muted">
          目标模块：src/features/history/、src-tauri/src/usage/、src-tauri/src/watcher/
        </p>
      </Card>
    </div>
  );
}

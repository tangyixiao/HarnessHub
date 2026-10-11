import { CalendarRange } from 'lucide-react';

import { PlaceholderCard } from '@/components/ui/placeholder-card';
import { PageHeader } from '@/components/ui/page-header';

export function ActivityPage() {
  return (
    <div>
      <PageHeader
        title="Activity"
        description="长期 Coding Activity Timeline：Date × Project × Harness × Model × Session × Token × Git 变化。"
      />
      <PlaceholderCard
        title="尚未接入"
        icon={CalendarRange}
        hint="目标模块：src/features/history/、src-tauri/src/usage/、src-tauri/src/watcher/"
      >
        Phase 3 的差异化功能。依赖已建好的 usage_events / git_events 表与幂等导入链路。
      </PlaceholderCard>
    </div>
  );
}

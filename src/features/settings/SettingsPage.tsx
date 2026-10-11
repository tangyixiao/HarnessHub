import { Settings } from 'lucide-react';

import { PlaceholderCard } from '@/components/ui/placeholder-card';
import { PageHeader } from '@/components/ui/page-header';

export function SettingsPage() {
  return (
    <div>
      <PageHeader
        title="Settings"
        description="本地优先设置：数据目录、网络开关、Secret 句柄与审计。"
      />
      <PlaceholderCard
        title="尚未接入"
        icon={Settings}
        hint="目标模块：src-tauri/src/permissions/、src-tauri/src/hhar/、src/features/settings/"
      >
        计划包含：数据目录与导出（HHAR）、网络能力显式开关、Config Doctor、Secret 句柄状态。
        所有网络相关能力默认关闭。
      </PlaceholderCard>
    </div>
  );
}

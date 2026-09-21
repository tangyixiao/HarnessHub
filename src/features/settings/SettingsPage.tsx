import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';

export function SettingsPage() {
  return (
    <div>
      <PageHeader title="Settings" description="本地优先设置：数据目录、网络开关、Secret 句柄与审计。" />
      <Card>
        <CardTitle>尚未接入</CardTitle>
        <CardDescription>
          计划包含：数据目录与导出（HHAR）、网络能力显式开关、Config Doctor、Secret 句柄状态。
          所有网络相关能力默认关闭。
        </CardDescription>
        <p className="mt-3 text-[11px] text-content-muted">
          目标模块：src-tauri/src/permissions/、src-tauri/src/hhar/、src/features/settings/
        </p>
      </Card>
    </div>
  );
}

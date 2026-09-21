import { Card, CardDescription, CardTitle } from '@/components/ui/card';
import { PageHeader } from '@/components/ui/page-header';

export function TerminalPage() {
  return (
    <div>
      <PageHeader
        title="Terminal"
        description="在 Harness Hub 内直接启动 Harness 的 PTY 会话，支持多 Session 与重连。"
      />
      <Card>
        <CardTitle>尚未接入</CardTitle>
        <CardDescription>
          该页面需要一个真实 PTY 后端（launch / kill / reconnect / 多 Session）。在 PTY 链路实现前
          不放置假终端，避免误导。
        </CardDescription>
        <p className="mt-3 text-[11px] text-content-muted">
          目标模块：src-tauri/src/process/、src-tauri/src/pty/
        </p>
      </Card>
    </div>
  );
}

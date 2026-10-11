import type { ReactNode } from 'react';
import { Hammer } from 'lucide-react';

import { cn } from '@/lib/utils';

/**
 * 「尚未接入」占位卡：虚线边 + 图标章，与真实内容卡在视觉上一眼可辨。
 * 文案由调用方给出（各页描述自己的依赖模块）。
 */
export function PlaceholderCard({
  title,
  children,
  hint,
  icon: Icon = Hammer,
  className,
}: {
  title: string;
  children: ReactNode;
  hint?: ReactNode;
  icon?: typeof Hammer;
  className?: string;
}) {
  return (
    <div
      className={cn(
        'rounded-xl border border-dashed border-border-subtle bg-surface-raised/30 p-5',
        className,
      )}
    >
      <div className="flex items-center gap-2.5">
        <span
          aria-hidden
          className="flex size-8 shrink-0 items-center justify-center rounded-md bg-white/[0.04] text-content-muted ring-1 ring-white/[0.06]"
        >
          <Icon className="size-4" />
        </span>
        <h3 className="text-sm font-semibold text-content-primary">{title}</h3>
      </div>
      <div className="mt-2 text-xs text-content-muted">{children}</div>
      {hint ? <p className="mt-3 font-mono text-[11px] text-content-muted/80">{hint}</p> : null}
    </div>
  );
}

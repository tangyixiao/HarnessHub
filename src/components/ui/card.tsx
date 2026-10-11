import type { HTMLAttributes } from 'react';

import { cn } from '@/lib/utils';

/**
 * 卡片：半透明底 + hairline 描边。深色界面的立体感来自亮度分层与描边，
 * 不来自投影（研究依据见 docs/specs/2026-10-11-ui-visual-polish-design.md）。
 */
export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        'rounded-lg border border-border-subtle bg-surface-raised/45',
        'shadow-[0_0_0_1px_rgba(255,255,255,0.015)]',
        className,
      )}
      {...props}
    />
  );
}

export function CardTitle({ className, ...props }: HTMLAttributes<HTMLHeadingElement>) {
  return <h3 className={cn('text-sm font-medium text-content-primary', className)} {...props} />;
}

export function CardDescription({ className, ...props }: HTMLAttributes<HTMLParagraphElement>) {
  return <p className={cn('mt-1 text-xs text-content-muted', className)} {...props} />;
}

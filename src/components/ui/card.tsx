import type { HTMLAttributes } from 'react';

import { cn } from '@/lib/utils';

/**
 * 卡片：微透明的悬浮面 + 顶部 1px 内高光，是「深色界面上的一层纸」。
 * 内容卡用默认内边距；做布局容器时调用方可用 className 覆盖。
 */
export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        'rounded-xl border border-border-subtle bg-surface-raised/55 p-5',
        'shadow-[inset_0_1px_0_0_rgba(255,255,255,0.035),0_1px_2px_rgba(0,0,0,0.25)]',
        className,
      )}
      {...props}
    />
  );
}

export function CardTitle({ className, ...props }: HTMLAttributes<HTMLHeadingElement>) {
  return <h3 className={cn('text-sm font-semibold text-content-primary', className)} {...props} />;
}

export function CardDescription({ className, ...props }: HTMLAttributes<HTMLParagraphElement>) {
  return <p className={cn('mt-1 text-xs text-content-muted', className)} {...props} />;
}

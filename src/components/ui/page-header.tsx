import type { ReactNode } from 'react';

import { cn } from '@/lib/utils';

export type PageHeaderProps = {
  title: string;
  description?: string;
  actions?: ReactNode;
  className?: string;
};

export function PageHeader({ title, description, actions, className }: PageHeaderProps) {
  return (
    <header className={cn('flex items-start justify-between gap-4 pb-6', className)}>
      <div>
        <h1 className="text-[22px] font-semibold leading-7 tracking-tight text-content-primary">
          {title}
        </h1>
        {description ? (
          <p className="mt-1 max-w-3xl text-[13px] text-content-muted">{description}</p>
        ) : null}
      </div>
      {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
    </header>
  );
}

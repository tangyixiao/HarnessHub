import { cva, type VariantProps } from 'class-variance-authority';
import type { ButtonHTMLAttributes } from 'react';

import { cn } from '@/lib/utils';

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-2 rounded-lg text-sm font-medium transition-all ' +
    'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent ' +
    'active:translate-y-px disabled:pointer-events-none disabled:opacity-50',
  {
    variants: {
      variant: {
        // 主按钮：accent 底 + 顶部内高光，压下去有 1px 的物理反馈
        primary:
          'bg-accent text-surface-base shadow-[inset_0_1px_0_0_rgba(255,255,255,0.25),0_1px_2px_rgba(0,0,0,0.3)] hover:bg-accent-strong',
        secondary: 'bg-surface-overlay text-content-primary hover:bg-surface-raised',
        ghost: 'text-content-muted hover:bg-surface-raised hover:text-content-primary',
      },
      size: {
        sm: 'h-8 px-3',
        md: 'h-9 px-4',
        icon: 'size-9',
      },
    },
    defaultVariants: {
      variant: 'primary',
      size: 'md',
    },
  },
);

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> &
  VariantProps<typeof buttonVariants>;

export function Button({ className, variant, size, type = 'button', ...props }: ButtonProps) {
  return (
    <button type={type} className={cn(buttonVariants({ variant, size }), className)} {...props} />
  );
}

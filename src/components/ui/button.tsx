import { cva, type VariantProps } from 'class-variance-authority';
import type { ButtonHTMLAttributes } from 'react';

import { cn } from '@/lib/utils';

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-2 rounded-md text-sm font-medium transition-colors duration-150 ' +
    'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent ' +
    'active:translate-y-px disabled:pointer-events-none disabled:opacity-50',
  {
    variants: {
      variant: {
        // 主按钮：accent 实底 + 白字，hover 提亮（accent 唯一的「实底」用途）
        primary: 'bg-accent text-white hover:bg-accent-strong',
        // 次级/幽灵：白透明底（2–5%），hover 轻微抬升
        secondary:
          'bg-white/[0.05] text-content-secondary ring-1 ring-white/[0.06] hover:bg-white/[0.08] hover:text-content-primary',
        ghost: 'text-content-muted hover:bg-white/[0.05] hover:text-content-primary',
      },
      size: {
        sm: 'h-7 px-2.5 text-[13px]',
        md: 'h-8 px-3.5 text-[13px]',
        icon: 'size-8',
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

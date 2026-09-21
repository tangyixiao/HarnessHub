import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

/**
 * 合并 Tailwind class：clsx 处理条件，twMerge 处理冲突（后者胜出）。
 * 所有 UI 组件的 className 都必须经过它，避免调用方覆盖不掉默认样式。
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

/**
 * 合并类名：clsx 处理条件与数组，tailwind-merge 让后写的同类工具类赢过先写的。
 *
 * 组件库统一用它拼接 className —— 调用方传进来的类要能覆盖组件默认类，
 * 靠 tailwind-merge 按「同类属性」判定而不是靠 CSS 顺序（工具类都在
 * @layer utilities 里，顺序由构建产物决定，不能指望）。
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs))
}

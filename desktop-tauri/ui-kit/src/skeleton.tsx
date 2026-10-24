import * as React from 'react'
import { cn } from './lib/cn'

/**
 * 骨架屏占位块（标准形态，与 shadcn/ui 的 Skeleton 一致）。
 *
 * 底色取 --ui-skeleton-base（浅色近白、深色比卡片亮一档），配 pulse 呼吸。
 * 尺寸与圆角交给调用方（w-/h-/rounded-*），组件只负责底色与动画。
 */
function Skeleton({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot='skeleton'
      className={cn('animate-pulse rounded-md bg-skeleton-base', className)}
      {...props}
    />
  )
}

export { Skeleton }

import * as React from 'react'
import { cn } from './lib/cn'

/**
 * 表单标签（标准形态，与 shadcn/ui 的 Label 一致：`data-slot="label"`）。
 *
 * 样式对齐 ui/css/components.css 的 `.field .label`：11.5px、第三档文字色。
 * 不用 Base UI 的 Field.Label —— 那要连同 Field 根组件一起用，
 * 现有表单是命令式脚本拼的 DOM，逐级迁移时再换。
 */
function Label({ className, ...props }: React.ComponentProps<'label'>) {
  return (
    <label
      data-slot='label'
      className={cn('flex items-center gap-[6px] text-[11.5px] text-muted-foreground', className)}
      {...props}
    />
  )
}

export { Label }

import * as React from 'react'
import { useRender } from '@base-ui/react/use-render'
import { mergeProps } from '@base-ui/react/merge-props'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from './lib/cn'

/**
 * 徽章 / 状态标签。
 *
 * 形态与 shadcn/ui 的 Badge 对齐：variant 取 default / secondary / destructive /
 * outline / ghost / link，命名与标准一致；后面七个是本项目的扩展档
 * （success / warning / info / brand / cn / intl / sensitive）—— 项目的账号状态、
 * 版本标签、敏感词命中需要这组语义色，但它们仍然是 variant 的取值，不另起一套 prop。
 *
 * 与既有界面的对应：ui/css 的 `.badge`（控件底 + 控件描边）＝ `outline`，
 * `.badge.ok` ＝ `success`，`.badge.warn` ＝ `warning`，`.badge.bad` ＝ `destructive`，
 * `.badge.brand` ＝ `brand`，`.badge.tag` ＝ `shape="tag"`。
 *
 * shape 是第二个维度（与 Button 的 size 同理）：pill 是默认的胶囊徽章，
 * tag 是行内状态标签（更小更方，取 --r-xs）。
 */

const badgeVariants = cva(
  'inline-flex w-fit shrink-0 items-center justify-center gap-[5px] overflow-hidden whitespace-nowrap border font-semibold',
  {
    variants: {
      variant: {
        default: 'border-transparent bg-primary text-primary-foreground',
        secondary: 'border-transparent bg-secondary text-secondary-foreground',
        destructive: 'border-destructive-bd bg-destructive-soft text-destructive',
        outline: 'border-control-border bg-control text-subtle',
        ghost: 'border-transparent bg-transparent text-muted-foreground',
        link: 'border-transparent bg-transparent text-primary underline-offset-4 hover:underline',
        /* 以下为项目扩展档：状态与版本语义色 */
        success: 'border-success-bd bg-success-soft text-success',
        warning: 'border-warning-bd bg-warning-soft text-warning',
        info: 'border-info-bd bg-info-soft text-info',
        brand: 'border-primary-bd bg-primary-soft text-primary-fg',
        cn: 'border-cn-bd bg-cn-soft text-cn',
        intl: 'border-intl-bd bg-intl-soft text-intl',
        /* 敏感词命中：既不是「失败」（红）也不是「国际版」（靛蓝），
           它是第三类事实，所以另起一档紫（取值见 theme.css 的 --ui-sensitive*） */
        sensitive: 'border-sensitive-bd bg-sensitive-soft text-sensitive',
      },
      shape: {
        pill: 'min-h-[21px] rounded-pill px-[9px] py-px text-[11.5px]',
        tag: 'min-h-[19px] rounded-xs px-2 py-0 text-[11px]',
      },
    },
    defaultVariants: { variant: 'default', shape: 'pill' },
  }
)

type BadgeProps = useRender.ComponentProps<'span'> & VariantProps<typeof badgeVariants>

function Badge({ className, variant, shape, render, ...props }: BadgeProps) {
  /**
   * 走 Base UI 的 useRender，于是支持 `render` —— 传一个元素（如
   * `render={<button type='button' />}`）就能把徽章渲染成别的标签，事件处理器
   * 由 Base UI 做合并（不是覆盖）。这是 shadcn 的 `asChild` 在 Base UI 这一支
   * 的对应写法。
   *
   * 为什么徽章需要它：请求日志里的「重试」「敏」两枚标签是**悬停面板的锚点**，
   * 必须是真的可聚焦元素（键盘要能 Tab 到、悬停委托要能接到），
   * 只当装饰用的 span 做不到。以前只能放弃徽章样式、手写一个 button。
   *
   * className 自己先合并好再交给 useRender：两个来源（变体给的 + 调用方给的）
   * 在这里就已经拼成一条，不依赖 mergeProps 对 className 的合并规则。
   */
  return useRender({
    render,
    defaultTagName: 'span',
    // 用 mergeProps 而不是 useRender 的 props 数组形态：后者在运行期同样被合并，
    // 但公开类型只收单个对象，传数组要断言，白白丢掉类型检查。
    props: mergeProps({ 'data-slot': 'badge' }, props, {
      className: cn(badgeVariants({ variant, shape }), className),
    }),
  })
}

/** 徽章里的状态圆点：颜色继承徽章文字色 */
function BadgeDot({ className, ...props }: React.ComponentProps<'span'>) {
  return <span className={cn('size-1.5 flex-none rounded-full bg-current', className)} {...props} />
}

export { Badge, BadgeDot, badgeVariants, type BadgeProps }

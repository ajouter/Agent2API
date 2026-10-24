import * as React from 'react'
import { Toggle as BaseToggle } from '@base-ui/react/toggle'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from './lib/cn'

/**
 * 开关按钮（标准形态，与 shadcn/ui 的 Toggle 一致）。
 *
 * 与 Switch 的分工：Switch 是「设置项开着还是关着」（有独立标签、放在表单里），
 * Toggle 是「这一条筛选/这个视图现在生效吗」（按钮本身就是要筛的东西，
 * 按下去就是生效）。两者语义不同，不要互相顶替。
 *
 * 与 SegmentedControl 的分工：那个是「多档里选一档」，不可取消；
 * Toggle 只有两态、且**可以按回未激活** —— 「仅看进行中」这类开关型筛选要的正是这个。
 *
 * 形态与 shadcn 对齐：variant 取 default / outline，size 取 default / sm / xs / lg。
 * 激活态统一走品牌浅底（--ui-primary-soft + --ui-primary-fg + --ui-primary-bd），
 * 与分段控件、导航项的选中态是同一套观感。
 *
 * 交互交给 Base UI 的 Toggle：`<button aria-pressed>`，键盘 Space/Enter 内建。
 */

const toggleVariants = cva(
  [
    'inline-flex shrink-0 cursor-pointer items-center justify-center gap-1.5 whitespace-nowrap border',
    'font-medium transition-[background-color,border-color,color] duration-150 ease-out',
    'outline-none select-none disabled:pointer-events-none disabled:opacity-45',
    '[&_svg]:pointer-events-none [&_svg]:shrink-0',
    // 激活态：品牌浅底。三个属性一起给，未激活时的透明描边保证按下不跳位
    'data-pressed:border-primary-bd data-pressed:bg-primary-soft data-pressed:font-semibold data-pressed:text-primary-fg',
    'data-pressed:hover:bg-primary-tint',
  ],
  {
    variants: {
      variant: {
        /** 平铺型：未激活时只是一行文字（列表工具条里的开关型筛选） */
        default: 'border-transparent bg-transparent text-subtle hover:bg-nav-hover hover:text-foreground',
        /** 控件型：未激活时与同行的按钮同脸 */
        outline:
          'border-control-border bg-control text-foreground shadow-1 hover:border-control-border-hover hover:bg-control-hover',
      },
      size: {
        default: 'h-[30px] rounded-md px-3 text-[12.5px]',
        xs: 'h-6 rounded-sm px-2 text-[11.5px]',
        sm: 'h-[26px] rounded-sm px-2.5 text-[12px]',
        lg: 'h-9 rounded-md px-4 text-[13px]',
      },
    },
    defaultVariants: { variant: 'default', size: 'default' },
  }
)

type ToggleProps = Omit<React.ComponentProps<typeof BaseToggle>, 'className'> &
  VariantProps<typeof toggleVariants> & {
    className?: string
  }

function Toggle({ className, variant, size, ...props }: ToggleProps) {
  return (
    <BaseToggle
      data-slot='toggle'
      className={cn(toggleVariants({ variant, size }), className)}
      {...props}
    />
  )
}

export { Toggle, toggleVariants, type ToggleProps }

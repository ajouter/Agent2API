import * as React from 'react'
import { Tooltip as BaseTooltip } from '@base-ui/react/tooltip'
import { cn } from './lib/cn'

/**
 * 提示气泡。
 *
 * 视觉对齐 ui/css/tooltip.css 的 `.tip-bubble`：主色底 + 近白字、--r-md 圆角、
 * max-width 288px、11.5px 字号、--shadow-3 投影，进场淡入 + 轻微上浮收 4%。
 *
 * 交互交给 Base UI 的 Tooltip：悬停延迟、移开宽限、Esc 收起、`aria-describedby`
 * 关联、触摸设备长按，都是内建的 —— 既有 tooltip.js 手写的那套（150ms 显示 /
 * 80ms 收起 / 点击钉住）是同一个目标的更简陋版本。需要「点击钉住」的场景
 * 改用 Popover。
 *
 * 浮层 z-index 取 35：在弹窗遮罩（30）之上、轻提示（40）之下，与既有编排一致。
 * 箭头默认不画 —— 既有气泡的箭头是 JS 按锚点位置实时算 x 坐标的，
 * Base UI 的 Arrow 由定位器托管，视觉上更稳；要箭头就传 <TooltipArrow />。
 */

/** 四个部件都按标准包一层，只为挂上 data-slot 标记（样式钩子与调试的锚点） */
function TooltipProvider(props: BaseTooltip.Provider.Props) {
  return <BaseTooltip.Provider data-slot='tooltip-provider' {...props} />
}

function Tooltip(props: BaseTooltip.Root.Props) {
  return <BaseTooltip.Root data-slot='tooltip' {...props} />
}

function TooltipTrigger(props: BaseTooltip.Trigger.Props) {
  return <BaseTooltip.Trigger data-slot='tooltip-trigger' {...props} />
}

type TooltipContentProps = Omit<React.ComponentProps<typeof BaseTooltip.Popup>, 'className'> & {
  className?: string
  /** 与锚点的距离，默认 8px */
  sideOffset?: number
  /** 锚定方向，默认 top（气泡在锚点上方） */
  side?: 'top' | 'right' | 'bottom' | 'left'
}

function TooltipContent({ className, sideOffset = 8, side = 'top', children, ...props }: TooltipContentProps) {
  return (
    <BaseTooltip.Portal>
      <BaseTooltip.Positioner sideOffset={sideOffset} side={side} className='z-[35]'>
        <BaseTooltip.Popup
          data-slot='tooltip-content'
          className={cn(
            'max-w-[288px] rounded-md bg-primary px-2.5 py-2 text-[11.5px] leading-[1.55] text-primary-on shadow-3',
            'break-words outline-none',
            'transition-[opacity,transform] duration-150 ease-out',
            'data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95',
            'data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95',
            className
          )}
          {...props}
        >
          {children}
        </BaseTooltip.Popup>
      </BaseTooltip.Positioner>
    </BaseTooltip.Portal>
  )
}

/** 气泡箭头：与气泡同色的 45° 小方块 */
function TooltipArrow({ className, ...props }: React.ComponentProps<typeof BaseTooltip.Arrow>) {
  return (
    <BaseTooltip.Arrow
      className={cn(
        'size-2 rotate-45 rounded-[1px] bg-primary',
        'data-[side=top]:-bottom-1 data-[side=bottom]:-top-1',
        className
      )}
      {...props}
    />
  )
}

export {
  Tooltip,
  TooltipProvider,
  TooltipTrigger,
  TooltipContent,
  TooltipArrow,
  type TooltipContentProps,
}

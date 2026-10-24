import * as React from 'react'
import { Popover as PopoverPrimitive } from '@base-ui/react/popover'
import { cn } from './lib/cn'

/**
 * 浮层（标准形态，与 shadcn/ui 的 Popover 一致）。
 *
 * 部件划分照标准：Popover / PopoverTrigger / PopoverContent /
 * PopoverHeader / PopoverTitle / PopoverDescription / PopoverClose。
 * PopoverContent 自带 Portal + Positioner + Popup 三层，所以调用方只写一个
 * `<PopoverContent>` 就够了，不必自己拼 Base UI 的三件套。
 *
 * 没有 PopoverAnchor：Base UI 的 Popover 把「锚点」交给了 Trigger 自己
 * （触发器就是锚点），没有单独的 Anchor 部件 —— 那是 Radix 的划分。
 * 需要「浮层挂在别的元素上」时用 PopoverContent 的 `anchor` 属性
 * （传元素 / ref / 返回元素的函数），不必把 Trigger 包过去。
 *
 * 视觉对齐 ui/css/components.css 的浮层一族（与 SelectContent 同一张脸）：
 * 浮层底（--ui-raised）+ 强描边 + --shadow-3、--r-lg 圆角、z-35。
 * z 取 35 而不是弹窗的 30：浮层要压在弹窗之上（表在弹窗里也要能调列），
 * 轻提示（40）仍在它上面。
 *
 * 与 Select / Tooltip 的分工：
 *   · 需要「从一组里选一个」→ 用 Select（它自带选中语义与键盘 typeahead）；
 *   · 需要「挂一块任意内容」→ 用 Popover（本件）；
 *   · 需要「悬停时说明一下」→ 用 Tooltip。
 *
 * 交互交给 Base UI 的 Popover：点外部关闭、Esc 关闭、焦点归位、
 * 按视口自动翻向与贴边（floating-ui）全部内建 —— 这几条正是自绘浮层最容易做漏的。
 *
 * 用法：
 *   <Popover>
 *     <PopoverTrigger render={<Button variant='outline' />}>打开</PopoverTrigger>
 *     <PopoverContent align='end'>…</PopoverContent>
 *   </Popover>
 */

function Popover(props: PopoverPrimitive.Root.Props) {
  return <PopoverPrimitive.Root data-slot='popover' {...props} />
}

function PopoverTrigger(props: PopoverPrimitive.Trigger.Props) {
  return <PopoverPrimitive.Trigger data-slot='popover-trigger' {...props} />
}

function PopoverClose(props: PopoverPrimitive.Close.Props) {
  return <PopoverPrimitive.Close data-slot='popover-close' {...props} />
}

type PopoverContentProps = Omit<PopoverPrimitive.Popup.Props, 'className'> & {
  className?: string
  /** 与锚点的间距，默认 6（与 SelectContent 的 4 略有差别：浮层里通常有内边距，贴太近会像粘住） */
  sideOffset?: number
  align?: PopoverPrimitive.Positioner.Props['align']
  side?: PopoverPrimitive.Positioner.Props['side']
  alignOffset?: number
  /** 贴边时保留的余量，默认 8（四个方向共用） */
  collisionPadding?: PopoverPrimitive.Positioner.Props['collisionPadding']
  /**
   * 把浮层锚在一个**外部元素**上，而不是 PopoverTrigger。
   *
   * 存在的理由：调用方的触发按钮有时不归 React 管（命令式建出来的原生按钮，
   * 插在 legacy 的页面骨架里）。没有这条时只能靠「盖一层透明 Trigger 当锚点」
   * 绕过去 —— 那样会挡住原按钮的 hover / title，得不偿失。
   * 传元素、ref 或返回元素的函数都行（Base UI 的 VirtualElement 也接受）。
   */
  anchor?: PopoverPrimitive.Positioner.Props['anchor']
}

function PopoverContent({
  className,
  sideOffset = 6,
  align = 'start',
  side,
  alignOffset,
  collisionPadding = 8,
  anchor,
  children,
  ...props
}: PopoverContentProps) {
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Positioner
        className='z-[35] outline-none'
        sideOffset={sideOffset}
        align={align}
        side={side}
        alignOffset={alignOffset}
        // 浮层自己要能拿到宽度约束：max-w 用 --available-width 才不会顶出视口
        collisionPadding={collisionPadding}
        anchor={anchor}
      >
        <PopoverPrimitive.Popup
          data-slot='popover-content'
          className={cn(
            'flex max-h-[min(70vh,560px)] flex-col overflow-hidden rounded-lg border border-border-strong bg-raised text-foreground shadow-3',
            'outline-none',
            // 入场：淡入 + 轻微上移。不做缩放 —— 浮层常常要「打开后立刻量尺寸摆位置」，
            // transform 会污染坐标（与 SelectContent 同一条取舍）
            'data-open:animate-in data-open:fade-in-0 data-open:slide-in-from-top-1',
            'data-closed:animate-out data-closed:fade-out-0',
            className
          )}
          {...props}
        >
          {children}
        </PopoverPrimitive.Popup>
      </PopoverPrimitive.Positioner>
    </PopoverPrimitive.Portal>
  )
}

/** 浮层头部：标题一行，底部一条 hairline。要放操作按钮自己往里塞。 */
function PopoverHeader({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot='popover-header'
      className={cn('flex items-center gap-2 border-b border-hairline px-3 py-2', className)}
      {...props}
    />
  )
}

function PopoverTitle({ className, ...props }: PopoverPrimitive.Title.Props) {
  return (
    <PopoverPrimitive.Title
      data-slot='popover-title'
      className={cn('text-[12.5px] font-semibold text-foreground', className)}
      {...props}
    />
  )
}

function PopoverDescription({ className, ...props }: PopoverPrimitive.Description.Props) {
  return (
    <PopoverPrimitive.Description
      data-slot='popover-description'
      className={cn('text-[11.5px] leading-[1.55] text-muted-foreground', className)}
      {...props}
    />
  )
}

export {
  Popover,
  PopoverTrigger,
  PopoverClose,
  PopoverContent,
  PopoverHeader,
  PopoverTitle,
  PopoverDescription,
}

import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from './lib/cn'

/**
 * 导航 / 列表项（项目扩展，shadcn 标准里没有对应件）。
 *
 * 为什么需要它：左栏那种「一列可点的条目」用 Button 做不了 —— Button 的
 * 工具类带 `!important` 且分层，会把 `.pv.on` 那类**选中态底色整个盖掉**
 * （组件库的 utilities 优先于 ui/css 的未分层规则），于是要么放弃选中态、
 * 要么把选中态写成一长串内联工具类散落在调用点。封成组件后选中态只有一处定义。
 *
 * 形态取自侧边栏的 `.nav-item` 与模型管理页左栏的 `.pv`（两者本来就是同一形态）：
 * 行高由 padding 撑、圆角 --r-sm、左侧可选图标、右侧可选计数。
 *
 * 三种变体：
 *   · default  普通条目（可选中）
 *   · add      虚线描边的「＋ 新建…」入口：它不是一家、点了也不会有选中态，
 *              虚线正是为了与上面那些真条目分开
 *
 * 注意：条目本身是 `<button>`，**里面不能再嵌按钮**。要在右侧挂一枚独立的
 * 操作按钮（如删除 ×），请由调用方在它外面套一层定位容器、把那枚按钮放成兄弟节点
 * —— HTML 不允许 button 嵌套，硬塞进去浏览器会把内层拆出来。
 */

const navItemVariants = cva(
  [
    'relative flex w-full items-center gap-2.5 text-left',
    'border border-transparent rounded-sm',
    // 显式清掉投影：ui/css/components.css 的通用 `button { box-shadow: var(--shadow-1) }`
    // 会漏到本组件上（深色主题下那道 40% 黑在平铺的导航列表里很显眼）。
    // 既有的 .nav-item / .pv 都在 CSS 里各清过一次，组件库这一层清掉就不必每个调用点再写。
    'shadow-none',
    'transition-[background-color,border-color,color] duration-150 ease-out',
    'outline-none focus-visible:shadow-focus',
    'disabled:pointer-events-none disabled:opacity-45',
    '[&_svg]:pointer-events-none [&_svg]:shrink-0',
  ],
  {
    variants: {
      variant: {
        default: [
          'cursor-pointer px-2.5 py-2',
          'text-[12.5px] font-medium text-sidebar-fg',
          'hover:bg-nav-hover hover:text-foreground',
          // 选中态：底色 + 描边 + 字重三件套（与分段控件、导航项的选中态同一套观感）
          'data-[active]:border-nav-selected-border data-[active]:bg-nav-selected-bg data-[active]:font-semibold data-[active]:text-nav-selected-fg',
        ],
        add: [
          'cursor-pointer justify-center px-2 py-1.5',
          'border-dashed border-border-strong bg-transparent',
          'text-[11.5px] text-muted-foreground',
          'hover:border-primary hover:text-primary-fg',
        ],
      },
    },
    defaultVariants: { variant: 'default' },
  }
)

type NavItemProps = Omit<React.ComponentProps<'button'>, 'className'> &
  VariantProps<typeof navItemVariants> & {
    /** 选中态。写成 data-active 而不是类名切换，是为了让 Tailwind 变体能命中它 */
    active?: boolean
    /** 右侧计数（家数 / 条数）；传 undefined 不渲染 */
    count?: number | string
    /** 左侧图标 */
    icon?: React.ReactNode
    /** 右侧附加内容（计数之外的东西，如状态点） */
    trailing?: React.ReactNode
    className?: string
  }

function NavItem({
  className,
  variant,
  active = false,
  count,
  icon,
  trailing,
  children,
  ...props
}: NavItemProps) {
  return (
    <button
      type='button'
      data-slot='nav-item'
      data-active={active ? '' : undefined}
      aria-current={active ? 'true' : undefined}
      className={cn(navItemVariants({ variant }), className)}
      {...props}
    >
      {icon}
      <span data-slot='nav-item-label' className='min-w-0 flex-1 truncate'>
        {children}
      </span>
      {trailing}
      {count !== undefined && count !== null && (
        <span
          data-slot='nav-item-count'
          className={cn(
            'flex-none text-[10.5px] text-sidebar-muted [font-variant-numeric:tabular-nums]',
            // 选中态下计数跟随文字色，否则它在深色底上会糊成一团
            active && 'text-inherit'
          )}
        >
          {count}
        </span>
      )}
    </button>
  )
}

export { NavItem, navItemVariants, type NavItemProps }

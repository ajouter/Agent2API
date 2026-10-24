import * as React from 'react'
import { Input, InputGroup, InputGroupAddon, InputGroupInput, cn } from '@ui'

/**
 * 岛的输入控件适配层：把「岛自己的调用约定」翻译成组件库的标准组件。
 *
 * 组件库侧的形状是 shadcn 的标准件（Input / InputGroup），这里只做两件事的转换：
 * `domRef` 对象 ↔ 标准 ref、`onInput(value)` ↔ 原生事件；其余 props 原样透传。
 *
 * 有图标时渲染 InputGroup（图标走标准的 InputGroupAddon），没有图标时就是
 * 一个标准 Input —— 不再有自造的 icon prop 参与组件结构。
 *
 * 两处类名的去向要分清：
 *   · `className`      → 挂在**容器**上（InputGroup）。业务壳的宽度与 flex 规则
 *                        （如 `.fm-search { width: 240px }`）落在这里才生效。
 *   · `inputClassName` → 挂在**控件**上（原 `<input>` 自带的类名）。
 *
 * 还有一处必须由适配层补的：**布局类**。组件库的 Input / InputGroup 默认 `w-full`
 * （shadcn 的用法是块级独占一行或放进固定宽度容器），而项目的输入框一直靠
 * `input[type=…] { flex: 1 1 auto }` 在工具条里与相邻控件同行自适应 —— 少了这一步，
 * 模型页那种 `flex-wrap` 的工具条里输入框会撑满整行、把右侧按钮挤到下一行。
 */

/**
 * 与老 CSS 等价的布局：`flex: 1 1 auto` + `width: auto`。
 * `w-auto` 是为了压掉组件库的 `w-full`（tailwind-merge 按同类属性判胜，后者生效）。
 */
const layoutClass = 'w-auto flex-auto'

type InputControlProps = {
  id?: string
  /** 原生 input 类型，默认 search（带原生清除按钮） */
  type?: string
  /** 初始值（非受控，之后由浏览器持有） */
  defaultValue?: string
  /** 出厂就藏起来的输入框（如「自定义等级」），显隐由业务侧直接写 DOM 属性 */
  hidden?: boolean
  placeholder?: string
  /** 挂在容器（InputGroup）上的类名：业务壳的布局规则 */
  className?: string
  /** 原 `<input>` 带来的类名，转给控件本身 */
  inputClassName?: string
  min?: string | number
  max?: string | number
  step?: string | number
  maxLength?: number
  inputMode?: React.HTMLAttributes<HTMLInputElement>['inputMode']
  autoComplete?: string
  spellCheck?: boolean
  disabled?: boolean
  readOnly?: boolean
  name?: string
  title?: string
  'aria-label'?: string
  /** 前缀图标字符（如 ⌕）；不传就渲染一个不带 InputGroup 的普通输入框 */
  icon?: string
  onInput?: (value: string) => void
  /** 岛靠它拿到真实 DOM，用于命令式设值与聚焦 */
  domRef: { current: HTMLInputElement | null }
}

function InputControl({
  onInput,
  domRef,
  icon,
  className,
  inputClassName,
  ...props
}: InputControlProps) {
  const attach = (node: HTMLElement | null) => {
    domRef.current = node as HTMLInputElement | null
  }
  const handleInput = (event: React.FormEvent<HTMLInputElement>) => {
    onInput?.(event.currentTarget.value)
  }

  if (!icon) {
    return (
      <Input
        ref={attach}
        className={cn(layoutClass, inputClassName)}
        onInput={handleInput}
        {...props}
      />
    )
  }

  return (
    <InputGroup className={cn(layoutClass, className)}>
      {/* 控件排在 addon 之前：Tab 顺序才是「先输入框后 addon」，视觉位置由 align 决定 */}
      <InputGroupInput
        ref={attach}
        className={inputClassName}
        onInput={handleInput}
        {...props}
      />
      <InputGroupAddon aria-hidden='true'>{icon}</InputGroupAddon>
    </InputGroup>
  )
}

export { InputControl, type InputControlProps }

import { mountInput, observeInputs } from './input'
import { mountSegmented } from './segmented'
// 岛样式（组件库 + Tailwind 产物）：由 Vite 抽成 ui/islands/ui.css，
// index.html 以 <link> 引入。写在这里是为了让样式跟着这个唯一的 JS 入口一起构建。
import './styles/islands.css'

/**
 * 岛的统一入口。
 *
 * 所有岛打进同一个 bundle 是刻意的：React 与 react-dom 只能有一份实例
 * （各打各的会把 React 装两遍，体积翻倍，而且两边的调度器互不认识）。
 * 于是 index.html 里也只有一个 <script>，所有岛共用它。
 *
 * 岛的三种接入形态：
 *   · 挂载点式（分段筛选）—— 宿主元素用 `.island`（display: contents，
 *     本身不生成盒子），岛渲染出来的根元素直接参与父级布局，接入前后布局等价；
 *   · 自动增强式（输入框）—— 给原生 `<input>` 加 `data-island-input` 就地升级；
 *   · 命令式弹窗（并发上限弹窗、确认弹窗…）—— 模块 import 时就注册好 window 上的
 *     接口，业务侧按原有调用方式用，点击时才建 DOM、关闭即移除。
 */
declare global {
  interface Window {
    /** 分段筛选：见 segmented.tsx */
    wbSegmented?: { mount: typeof mountSegmented }
    /** 搜索 / 文本输入框：见 input.tsx */
    wbInput?: { mount: typeof mountInput }
  }
}

/**
 * 命令式弹窗岛：扫 `./islands/` 目录自动全部引入，**加一个岛只需在
 * src/islands/ 下放一个文件**，不必回来改这里。
 *
 * 这样安排是为了让「迁移一个弹窗」成为一个自足的改动 —— 一个文件、零个共享
 * 编辑点，多个迁移可以并行做而不在同一个入口文件上撞车（历史上每加一个岛都要
 * 在这里插一行 import，谁都在改同一个文件）。
 *
 * 每个岛文件必须在模块顶层把自己挂到 window 上（如
 * `window.wbConfirm = { ask }`），import 的副作用即注册，不需要导出任何东西。
 * 执行顺序按文件名的字典序，但因为彼此独立，顺序不影响结果。
 */
const islandModules = import.meta.glob('./islands/*.tsx', { eager: true })

// 让打包器不会因为「变量没被读取」而把上面的 import 去掉
void islandModules

window.wbSegmented = { mount: mountSegmented }
window.wbInput = { mount: mountInput }

// 自动增强：页面里带 data-island-input 的原生输入框（含后续动态生成的）在这里
// 就完成升级，业务模块随后加载时拿到的已经是岛渲染的控件。
observeInputs()

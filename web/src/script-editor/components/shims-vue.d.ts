/**
 * .vue 单文件组件的 TS 模块声明（tsc --noEmit 自检用；vite/vitest 由插件转译，不经过此声明）。
 */
declare module '*.vue' {
  import type { DefineComponent } from 'vue'
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const component: DefineComponent<Record<string, any>, Record<string, any>, any>
  export default component
}

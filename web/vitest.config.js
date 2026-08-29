import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

// 独立于 vite.config.js：测试只覆盖纯 JS 模块（脚本语言模块 + 鉴权会话层 + 脚本编辑器 fixture），
// node 环境即可运行（不需要 Android 设备 / ffmpeg / 浏览器；fetch/localStorage/location 由用例内 stub）。
// 组件测试（script-editor/components/**）在文件内用 `// @vitest-environment happy-dom` pragma 切环境；
// 此处全局 environment 保持 node 不变，仅挂 vue 插件以转译 .vue 单文件组件。
export default defineConfig({
  plugins: [vue()],
  test: {
    environment: 'node',
    include: ['src/*.test.js', 'src/script-editor/**/*.test.js', 'src/recording/**/*.test.js'],
  },
})

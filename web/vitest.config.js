import { defineConfig } from 'vitest/config'

// 独立于 vite.config.js：测试只覆盖纯 JS 模块（脚本语言模块 + 鉴权会话层 + 脚本编辑器 fixture），
// node 环境即可运行（不需要 Android 设备 / ffmpeg / 浏览器；fetch/localStorage/location 由用例内 stub）
export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/script-language/**/*.test.js', 'src/*.test.js', 'src/script-editor/**/*.test.js'],
  },
})

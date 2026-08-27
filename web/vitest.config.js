import { defineConfig } from 'vitest/config'

// 独立于 vite.config.js：测试只覆盖脚本语言纯函数模块，node 环境即可运行
//（不需要 Android 设备 / ffmpeg / 浏览器）
export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/script-language/**/*.test.js'],
  },
})

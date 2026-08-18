import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  build: {
    // 构建产物直接输出到后端静态托管目录（server 服务于 ./web-dist）
    outDir: '../server/web-dist',
    emptyOutDir: true
  },
  server: {
    port: 5173,
    host: true,
    proxy: {
      // Rust 服务端 API 代理
      '/api': {
        target: process.env.VITE_PROXY_TARGET || 'http://localhost:8443',
        changeOrigin: true
      },
      '/ws': {
        target: process.env.VITE_PROXY_TARGET || 'ws://localhost:8443',
        ws: true
      }
    }
  }
})

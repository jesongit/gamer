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
      // Rust 服务端 API 代理。
      // changeOrigin 必须保持 false：后端对 POST/PUT/DELETE 做 Origin↔Host 同源校验，
      // 改写 Host 后与浏览器 Origin(5173) 不一致会被 403 forbidden_origin 拒掉。
      '/api': {
        target: process.env.VITE_PROXY_TARGET || 'http://localhost:8443',
        changeOrigin: false
      },
      '/ws': {
        target: process.env.VITE_PROXY_TARGET || 'ws://localhost:8443',
        ws: true
      }
    }
  }
})

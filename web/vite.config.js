import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
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

import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { readFileSync } from 'node:fs'

// WEB-006 混包告警的前端侧版本源：构建期把 web/package.json version 注入为全局
// __APP_VERSION__（CI 由 tools/check-version.ps1 保证它与 server/Cargo.toml 同源一致）。
// 运行时 MainLayout 拿它与 /api/system/info 的 app.version 比对，不一致显示混包警告条。
const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8'))

export default defineConfig({
  plugins: [
    {
      name: 'gamer-prebuilt-plugin-ui',
      configureServer(server) {
        // These are independently built ES modules. Vite adds ?import to dynamic
        // imports, which its public middleware otherwise rejects as source code.
        server.middlewares.use((req, _res, next) => {
          if (req.url?.startsWith('/plugin-ui/')) {
            const url = new URL(req.url, 'http://localhost')
            if (/\.m?js$/.test(url.pathname) && url.searchParams.has('import')) {
              url.searchParams.delete('import')
              req.url = url.pathname + url.search
            }
          }
          next()
        })
      },
    },
    vue(),
  ],
  resolve: { dedupe: ["vue", "vue-router"] },
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version)
  },
  build: {
    // 构建产物直接输出到后端静态托管目录（server 服务于 ./web-dist）
    outDir: '../server/web-dist',
    // 发布包校验与后端静态路由依赖这一目录契约，避免资源被平铺。
    assetsDir: 'assets',
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

import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { currentPublicAssetsPlugin } from './retired-public-assets.mjs'
import { assertMatchingBuildVersion } from './build-version.mjs'

// WEB-006：构建和开发启动都先校验 web/package.json 与服务端权威版本，
// 再注入真实前端构建版本；运行时继续检测部署资源与服务端是否混包。
const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8'))
const appVersion = assertMatchingBuildVersion(
  pkg.version,
  readFileSync(new URL('../server/Cargo.toml', import.meta.url), 'utf8'),
)

export default defineConfig({
  plugins: [
    currentPublicAssetsPlugin(),
    {
      name: 'gamer-prebuilt-plugin-ui',
      configureServer(server) {
        // These are independently built ES modules. Vite adds ?import to dynamic
        // imports, which its public middleware otherwise rejects as source code.
        server.middlewares.use((req, _res, next) => {
          if (req.url?.startsWith('/plugin-ui/') || /^\/api\/extensions\/[a-z0-9][a-z0-9._-]*\/ui\//.test(req.url || '')) {
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
    __APP_VERSION__: JSON.stringify(appVersion)
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
    fs: {
      // Host dialogs share this source module with plugin UIs. Keep the
      // allow list narrow instead of exposing the whole repository/data root.
      allow: [
        fileURLToPath(new URL('./', import.meta.url)),
        fileURLToPath(new URL('../plugins/ui-shared/', import.meta.url)),
      ],
    },
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

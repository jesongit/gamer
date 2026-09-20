import { defineConfig } from "vitest/config"
import vue from "@vitejs/plugin-vue"
export default defineConfig({ plugins: [vue()], resolve: { dedupe: ["vue", "vue-router"] }, test: { environment: "node", include: ["src/**/*.test.js"], passWithNoTests: true } })

/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

// @tauri-apps/cli 在 dev 时会注入以下 env：
//   TAURI_ENV_PLATFORM / TAURI_ENV_ARCH / TAURI_ENV_FAMILY
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],

  // Tauri 期望前端产物在 dist/
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },

  // Vite 的开发服务器配置。Tauri 通过此 host:port 加载前端。
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 让 Tauri 监听不到的目录不触发 HMR 重载
      ignored: ["**/src-tauri/**"],
    },
  },

  // 单元测试（vitest）：只覆盖纯逻辑 + store/组件，无 Tauri 依赖
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: ["src/test-setup.ts"],
    globals: false,
    restoreMocks: true,
  },
});

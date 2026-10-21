import { defineConfig } from "vite";

// Tauri 会把前端当作本地页面加载，固定端口避免 devUrl 对不上。
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
  },
});

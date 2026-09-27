import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { readFileSync } from "node:fs";

// 設定の「このアプリについて」に出す版(版上げで package.json と tauri.conf.json をそろえて上げる)
const pkg = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf-8"));

// Tauri のテンプレートに準拠: ポート固定、src-tauri は監視対象外
export default defineConfig({
  plugins: [svelte()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});

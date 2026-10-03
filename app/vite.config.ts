import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { readFileSync } from "node:fs";
import { execSync } from "node:child_process";

// 設定の「このアプリについて」に出す版(版上げで package.json と tauri.conf.json をそろえて上げる)
const pkg = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf-8"));

// ビルド番号 = Git のコミットの通し番号(同じコミットからは同じ番号、新しいほど大きい)と、短いコミット ID。
// Git が使えない所でビルドしたときは空(版は 3 桁だけ出す)
function git(args: string): string {
  try {
    return execSync(`git ${args}`, { cwd: new URL(".", import.meta.url), stdio: ["ignore", "pipe", "ignore"] })
      .toString()
      .trim();
  } catch {
    return "";
  }
}
const build = git("rev-list --count HEAD");
const commit = git("rev-parse --short HEAD");

// Tauri のテンプレートに準拠: ポート固定、src-tauri は監視対象外
export default defineConfig({
  plugins: [svelte()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
    __APP_BUILD__: JSON.stringify(/^\d+$/.test(build) ? build : ""),
    __APP_COMMIT__: JSON.stringify(commit),
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

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { spzCspPlugin } from "./scripts/spz-csp-plugin.mjs";

export default defineConfig({
  plugins: [react(), spzCspPlugin()],
  worker: { plugins: () => [spzCspPlugin()] },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Windows directory watchers can block cache-directory renames. These
      // generated pipeline/validation assets never participate in HMR.
      ignored: ["**/src-tauri/**", "**/.cache/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  optimizeDeps: {
    // The repository also contains standalone experiment pages with their own
    // dependencies. Only the desktop application's entry belongs to this dev
    // server; scanning every HTML file produces unrelated missing-dependency
    // errors (for example, optional Three.js experiments).
    entries: ["index.html"],
  },
  build: {
    // Phase one is Windows-only; WebView2 tracks Chromium.
    target: "chrome105",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
  },
});

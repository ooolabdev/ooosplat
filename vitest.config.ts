import { defineConfig } from "vitest/config";

export default defineConfig({
  esbuild: { jsx: "automatic" },
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_ENV_"],
  test: {
    // Only run the current application's tests, not archived worktrees or artifacts.
    include: ["src/**/*.test.{ts,tsx}", "html-viewer/**/*.test.{ts,tsx}"],
  },
});

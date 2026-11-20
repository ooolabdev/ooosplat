import { defineConfig } from "vitest/config";

export default defineConfig({
  esbuild: { jsx: "automatic" },
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_ENV_"],
  test: {
    // The development test run includes frontend unit tests. Native Windows E2E is opt-in via npm run test:e2e.
    include: ["src/**/*.test.{ts,tsx}", "html-viewer/**/*.test.{ts,tsx}"],
  },
});

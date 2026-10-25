import tailwindcss from "@tailwindcss/vite";
import vue from "@vitejs/plugin-vue";
import { playwright } from "@vitest/browser-playwright";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  optimizeDeps: { include: ["vue-router", "class-variance-authority"] },
  test: {
    include: ["src/**/*.browser.test.ts"],
    attachmentsDir: "../../.cache/browser-tests",
    browser: {
      enabled: true,
      provider: playwright({ launchOptions: { chromiumSandbox: true } }),
      headless: true,
      instances: [{ browser: "chromium" }],
    },
  },
});

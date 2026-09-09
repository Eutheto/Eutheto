import vue from "@vitejs/plugin-vue";
import { configDefaults, defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [vue()],
  test: {
    environment: "node",
    exclude: [...configDefaults.exclude, "e2e/**", "src/**/*.browser.test.ts"],
  },
});

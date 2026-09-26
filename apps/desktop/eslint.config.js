import { withVueTs, vueTsConfigs } from "@vue/eslint-config-typescript";
import pluginVue from "eslint-plugin-vue";

const readonly = "readonly";

const browserGlobals = {
  AbortController: readonly,
  Blob: readonly,
  CustomEvent: readonly,
  Document: readonly,
  Element: readonly,
  Event: readonly,
  EventTarget: readonly,
  File: readonly,
  FileReader: readonly,
  FormData: readonly,
  HTMLElement: readonly,
  IntersectionObserver: readonly,
  MutationObserver: readonly,
  Node: readonly,
  ResizeObserver: readonly,
  URL: readonly,
  URLSearchParams: readonly,
  WebSocket: readonly,
  cancelAnimationFrame: readonly,
  clearInterval: readonly,
  clearTimeout: readonly,
  console: readonly,
  document: readonly,
  fetch: readonly,
  localStorage: readonly,
  navigator: readonly,
  queueMicrotask: readonly,
  requestAnimationFrame: readonly,
  sessionStorage: readonly,
  setInterval: readonly,
  setTimeout: readonly,
  window: readonly,
};

const nodeGlobals = {
  Buffer: readonly,
  clearImmediate: readonly,
  clearInterval: readonly,
  clearTimeout: readonly,
  console: readonly,
  global: readonly,
  process: readonly,
  queueMicrotask: readonly,
  setImmediate: readonly,
  setInterval: readonly,
  setTimeout: readonly,
};

const vitestGlobals = {
  afterAll: readonly,
  afterEach: readonly,
  assert: readonly,
  beforeAll: readonly,
  beforeEach: readonly,
  describe: readonly,
  expect: readonly,
  it: readonly,
  suite: readonly,
  test: readonly,
  vi: readonly,
};

export default withVueTs(
  {
    rootDir: import.meta.dirname,
  },
  {
    name: "eutheto/ignores",
    ignores: ["node_modules/**", "dist/**", ".vite/**", "coverage/**", "src-tauri/target/**"],
  },
  pluginVue.configs["flat/recommended"],
  vueTsConfigs.strictTypeChecked,
  {
    name: "eutheto/tauri-api-boundary",
    files: ["**/*.{js,mjs,cjs,ts,mts,cts,tsx,vue}"],
    ignores: ["src/api/**"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              regex: "^@tauri-apps/api(?:/.*)?$",
              message: "Import the generated adapter from src/api instead.",
            },
          ],
        },
      ],
    },
  },
  {
    name: "eutheto/vue-formatting",
    files: ["src/**/*.vue"],
    rules: {
      "vue/html-self-closing": [
        "warn",
        {
          html: {
            void: "always",
            normal: "always",
            component: "always",
          },
          svg: "always",
          math: "always",
        },
      ],
      "vue/max-attributes-per-line": "off",
      "vue/singleline-html-element-content-newline": "off",
      // Prettier owns layout; Vue's alternatives disagree on multiline inline content.
      "vue/html-indent": "off",
      "vue/html-closing-bracket-newline": "off",
      "vue/multiline-html-element-content-newline": "off",
    },
  },
  {
    name: "eutheto/browser-globals",
    files: ["src/**/*.{ts,vue}"],
    languageOptions: {
      globals: browserGlobals,
    },
  },
  {
    name: "eutheto/node-globals",
    files: ["*.config.{js,ts}", "**/*.test.ts", "e2e/**/*.mjs"],
    languageOptions: {
      globals: nodeGlobals,
    },
  },
  {
    name: "eutheto/vitest-globals",
    files: ["**/*.test.ts"],
    languageOptions: {
      globals: vitestGlobals,
    },
  },
);

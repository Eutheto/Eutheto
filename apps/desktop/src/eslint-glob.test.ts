import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, relative, sep } from "node:path";
import * as esmConfig from "@vue/eslint-config-typescript";
import { describe, expect, it, onTestFinished } from "vitest";

const loadCommonJs = createRequire(import.meta.url);
const cjsConfig = loadCommonJs("@vue/eslint-config-typescript") as typeof esmConfig;
const migrationCli = join(dirname(loadCommonJs.resolve("@vue/eslint-config-typescript")), "bin.js");
const typedVue = '<script setup lang="ts">const count: number = 1;</script>\n';
const plainVue = "<template><p>Plain component</p></template>\n";
const migratedConfig =
  'import { withVueTs } from "@vue/eslint-config-typescript";\nexport default withVueTs({});\n';

function fixture(files: Record<string, string> = {}) {
  // Metacharacters in the root also exercise escaping of absolute ignore paths.
  const root = mkdtempSync(join(tmpdir(), "eutheto-eslint-[square](paren)-"));
  onTestFinished(() => {
    rmSync(root, { recursive: true, force: true });
  });
  for (const [filename, contents] of Object.entries(files)) {
    const absoluteFilename = join(root, filename);
    mkdirSync(dirname(absoluteFilename), { recursive: true });
    writeFileSync(absoluteFilename, contents);
  }
  return root;
}

async function classify(
  config: typeof esmConfig,
  rootDir: string,
  includeDotFolders = false,
  ignores: string[] = [],
) {
  const configs = await config.withVueTs(
    { rootDir, includeDotFolders },
    { name: "fixture/ignores", ignores },
    config.vueTsConfigs.strictTypeChecked,
  );
  return {
    typed: (
      configs.find(
        (entry) => entry.name === "@vue/typescript/default-project-service-for-vue-files",
      )?.files ?? []
    ).toSorted(),
    // The spread disableTypeChecked config overwrites the helper's Vue-specific name.
    plain: (
      configs.find(
        (entry) =>
          entry.name === "typescript-eslint/disable-type-checked" &&
          entry.files?.every((matcher) => typeof matcher === "string" && matcher.endsWith(".vue")),
      )?.files ?? []
    ).toSorted(),
  };
}

// Exercise the actual package exports, rather than a reimplementation of its glob calls.
describe.each([
  { format: "ESM", config: esmConfig },
  { format: "CommonJS", config: cjsConfig },
])("Vue ESLint file discovery ($format)", ({ config }) => {
  it("classifies nested Vue files relative to rootDir and skips directories and vendor files", async () => {
    const root = fixture({
      "src/deep/Typed.vue": typedVue,
      "src/deep/Plain.vue": plainVue,
      "src/[square]/Typed.vue": typedVue,
      "src/(paren)/Plain.vue": plainVue,
      "node_modules/vendor/Excluded.vue": typedVue,
      "src/node_modules/vendor/Excluded.vue": plainVue,
      ".git/Excluded.vue": typedVue,
      "src/.git/Excluded.vue": plainVue,
    });
    mkdirSync(join(root, "fake.vue"));

    expect(root).not.toBe(process.cwd());
    expect(await classify(config, root, true)).toEqual({
      typed: ["src/[[]square[]]/Typed.vue", "src/deep/Typed.vue"],
      plain: ["src/[(]paren[)]/Plain.vue", "src/deep/Plain.vue"],
    });
  });

  it.each([false, true])("respects includeDotFolders=%s", async (includeDotFolders) => {
    const root = fixture({
      "Visible.vue": typedVue,
      ".hidden/Typed.vue": typedVue,
      "src/.hidden/Plain.vue": plainVue,
      "src/.Plain.vue": plainVue,
      ".git/Excluded.vue": typedVue,
      "node_modules/.hidden/Excluded.vue": plainVue,
    });

    expect(await classify(config, root, includeDotFolders)).toEqual({
      typed: includeDotFolders ? [".hidden/Typed.vue", "Visible.vue"] : ["Visible.vue"],
      plain: includeDotFolders ? ["src/.Plain.vue", "src/.hidden/Plain.vue"] : [],
    });
  });

  it.each(["native", "forward slash"])(
    "honors exact cwd-relative ignores with %s separators and glob metacharacters",
    async (separators) => {
      const ignoredFiles = {
        "src/[square]/(paren)/IgnoreTyped.vue": typedVue,
        "src/[square]/(paren)/IgnorePlain.vue": plainVue,
        "src/{brace}/!bang/+plus/@at/[Ignore].vue": typedVue,
      };
      const root = fixture({
        "src/Keep.vue": typedVue,
        // These lookalike paths must not be treated as matches for literal ignores.
        "src/s/paren/IgnoreTyped.vue": typedVue,
        ...ignoredFiles,
      });
      const ignores = Object.keys(ignoredFiles).map((filename) => {
        const cwdRelative = relative(process.cwd(), join(root, filename));
        return separators === "native" ? cwdRelative : cwdRelative.split(sep).join("/");
      });

      expect(await classify(config, root, false, ignores)).toEqual({
        typed: ["src/Keep.vue", "src/s/paren/IgnoreTyped.vue"],
        plain: [],
      });
    },
  );

  it("follows directory symlinks and keeps paths relative to the Vue project", async () => {
    const target = fixture({ "nested/Typed.vue": typedVue, "Plain.vue": plainVue });
    const root = fixture();
    // Junctions also work on Windows without requiring file-symlink privileges.
    symlinkSync(target, join(root, "linked"), "junction");

    expect(await classify(config, root)).toEqual({
      typed: ["linked/nested/Typed.vue"],
      plain: ["linked/Plain.vue"],
    });
  });
});

function discoverMigrationFiles(root: string, patterns: string[] = []) {
  return execFileSync(process.execPath, [migrationCli, "migrate-to-with-vue-ts", ...patterns], {
    cwd: root,
    encoding: "utf8",
    timeout: 10_000,
  });
}

describe("Vue ESLint migration CLI file discovery", () => {
  it.each(["relative", "absolute"])(
    "deduplicates overlapping brace globs and %s explicit files while retaining default ignores",
    (explicitPaths) => {
      const files = {
        "eslint.config.mjs": migratedConfig,
        "packages/one/eslint.config.ts": migratedConfig,
        "packages/[square]/(paren)/eslint.config.ts": migratedConfig,
        "node_modules/vendor/eslint.config.ts": migratedConfig,
        "packages/one/node_modules/vendor/eslint.config.ts": migratedConfig,
        ".git/eslint.config.ts": migratedConfig,
        "dist/eslint.config.ts": migratedConfig,
        "packages/one/dist/eslint.config.ts": migratedConfig,
        ".hidden/eslint.config.ts": migratedConfig,
      };
      const root = fixture(files);
      mkdirSync(join(root, "packages/one/fake.config.ts"));

      const explicitFiles = ["eslint.config.mjs", "packages/[square]/(paren)/eslint.config.ts"];
      const output = discoverMigrationFiles(root, [
        "**/*config.{mjs,ts}",
        "packages/**/eslint.config.ts",
        ...explicitFiles.flatMap((filename) => {
          const absolute = join(root, filename);
          const native = explicitPaths === "absolute" ? absolute : relative(root, absolute);
          return [native, native.split(sep).join("/")];
        }),
      ]);

      expect(output).toContain("Found 3 ESLint config file(s).");
      expect(output).toContain("No migration needed.");
      for (const [filename, contents] of Object.entries(files)) {
        expect(readFileSync(join(root, filename), "utf8")).toBe(contents);
      }
    },
  );

  it("finds the default config extensions and accepts literal metacharacters in explicit paths", () => {
    const root = fixture({
      "eslint.config.js": migratedConfig,
      "packages/[square]/(paren)/eslint.config.mts": migratedConfig,
      "packages/one/other.ts": migratedConfig,
    });

    expect(discoverMigrationFiles(root)).toContain("Found 2 ESLint config file(s).");
    expect(discoverMigrationFiles(root, ["packages/[square]/(paren)/eslint.config.mts"])).toContain(
      "Found 1 ESLint config file(s).",
    );
  });

  it.each(["missing/**/*.ts", "packages", "packages/one/eslint.config.ts"])(
    "does not expand a missing pattern or bare directory: %s",
    (pattern) => {
      const root = fixture({ "packages/one/eslint.config.mjs": migratedConfig });
      mkdirSync(join(root, "packages/one/eslint.config.ts"));

      expect(discoverMigrationFiles(root, [pattern])).toContain(
        "No matching ESLint config files found.",
      );
      expect(readFileSync(join(root, "packages/one/eslint.config.mjs"), "utf8")).toBe(
        migratedConfig,
      );
    },
  );
});

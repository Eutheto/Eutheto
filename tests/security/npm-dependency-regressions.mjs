// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const desktopRequire = createRequire(new URL('../../apps/desktop/package.json', import.meta.url));
const eslintRequire = createRequire(desktopRequire.resolve('eslint'));
const { Minimatch, minimatch } = eslintRequire('minimatch');

if (process.argv.includes('--hostile-pattern')) {
  // GHSA-g3hv-75wg-m249: **/.. preprocessing must respect the expansion budget.
  const matcher = new Minimatch('**/../a/b/'.repeat(8) + 'file.ts', {
    optimizationLevel: 2,
    braceExpandMax: 16,
  });
  assert.ok(matcher.globParts.length <= 16);
} else {
  const testUtilsRequire = createRequire(desktopRequire.resolve('@vue/test-utils'));
  const beautifyRequire = createRequire(testUtilsRequire.resolve('js-beautify'));
  const { glob, globSync } = beautifyRequire('glob');
  const root = await mkdtemp(path.join(tmpdir(), 'eutheto-npm-regression-'));
  try {
    for (const directory of ['src/nested', 'node_modules/pkg', '.git']) {
      await mkdir(path.join(root, directory), { recursive: true });
    }
    for (const file of ['src/one.js', 'src/nested/two.js', 'node_modules/pkg/ignored.js', '.git/ignored.js']) {
      await writeFile(path.join(root, file), '');
    }
    // Match js-beautify's CLI options through its actual resolved dependency.
    assert.deepEqual(globSync('**/*.js', {
      cwd: root,
      absolute: true,
      ignore: ['**/node_modules/**', '**/.git/**'],
    }).sort(), [path.join(root, 'src/nested/two.js'), path.join(root, 'src/one.js')].sort());

    let reads = 0;
    const abort = new Error('regression-abort');
    const trackedFs = {
      readdir(...args) { reads++; return fs.readdir(...args); },
      readdirSync(...args) { reads++; return fs.readdirSync(...args); },
      promises: {
        readdir(...args) { reads++; return fs.promises.readdir(...args); },
      },
    };
    // GHSA-3g3m-2fjr-6pw4: rejection must also stop filesystem traversal.
    await assert.rejects(glob('**/*', {
      cwd: root,
      signal: AbortSignal.abort(abort),
      fs: trackedFs,
    }), (error) => error === abort);
    await new Promise((resolve) => setImmediate(resolve));
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(reads, 0);

    const controller = new AbortController();
    const abortingFs = {
      readdir(directory, options, callback) {
        reads++;
        const entries = fs.readdirSync(directory, options);
        queueMicrotask(() => {
          controller.abort(abort);
          callback(null, entries);
        });
      },
    };
    // Abort at the first directory response, where the old walker kept recursing.
    await assert.rejects(glob('**/*', {
      cwd: root,
      signal: controller.signal,
      fs: abortingFs,
    }), (error) => error === abort);
    await new Promise((resolve) => setImmediate(resolve));
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(reads, 1);

    assert.equal(minimatch('src/nested/two.js', '**/*.js', { dot: true }), true);
    assert.equal(new Minimatch('src/**/*.js', { dot: true }).match('src/nested', true), true);
    assert.equal(new Minimatch('**/*.js', { matchBase: true, dot: true }).match('src/one.ts'), false);
    const hostile = spawnSync(process.execPath, [fileURLToPath(import.meta.url), '--hostile-pattern'], {
      encoding: 'utf8',
      timeout: 5000,
    });
    assert.ifError(hostile.error);
    assert.equal(hostile.status, 0, hostile.stderr);
    console.log('npm dependency regressions passed');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

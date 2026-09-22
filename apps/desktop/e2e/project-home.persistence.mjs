import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { mkdir, readFile, readdir, rename, rm, writeFile } from "node:fs/promises";
import { createConnection } from "node:net";
import { dirname, isAbsolute, join } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const driverHost = "127.0.0.1";
const driverPort = 4_444;
const nativeDriverPort = 4_445;
const timeout = 15_000;
const elementKey = "element-6066-11e4-a52e-4f735466cecf";
const application = fileURLToPath(
  new URL("../../../.cache/cargo-target/debug/eutheto-desktop", import.meta.url),
);
const tauriDriverExecutable = process.env.EUTHETO_TAURI_DRIVER;
assert.equal(
  typeof tauriDriverExecutable,
  "string",
  "EUTHETO_TAURI_DRIVER must name the config-owned tauri-driver executable",
);
assert(
  isAbsolute(tauriDriverExecutable),
  "EUTHETO_TAURI_DRIVER must be an absolute executable path",
);
const nativeDriverExecutable = process.env.EUTHETO_NATIVE_DRIVER;
assert.equal(
  typeof nativeDriverExecutable,
  "string",
  "EUTHETO_NATIVE_DRIVER must name the config-owned WebKitWebDriver executable",
);
assert(
  isAbsolute(nativeDriverExecutable),
  "EUTHETO_NATIVE_DRIVER must be an absolute executable path",
);
const xdotoolExecutable = process.env.EUTHETO_XDOTOOL;
assert.equal(
  typeof xdotoolExecutable,
  "string",
  "EUTHETO_XDOTOOL must name the config-owned X11 interaction tool",
);
assert(isAbsolute(xdotoolExecutable));
const executeFile = promisify(execFile);

let tauriDriver;
let activeSessionId;

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function isPortOpen(port) {
  return new Promise((resolve) => {
    const socket = createConnection({ host: driverHost, port });
    const finish = (open) => {
      socket.destroy();
      resolve(open);
    };
    socket.setTimeout(250, () => finish(false));
    socket.once("connect", () => finish(true));
    socket.once("error", () => finish(false));
  });
}

async function waitForDriver() {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (tauriDriver?.exitCode !== null || tauriDriver.signalCode !== null) {
      const detail =
        tauriDriver?.exitCode === null
          ? `signal ${tauriDriver.signalCode ?? "unknown"}`
          : `code ${tauriDriver?.exitCode.toString() ?? "unknown"}`;
      throw new Error(`tauri-driver exited before becoming ready (${detail})`);
    }
    if ((await isPortOpen(driverPort)) && (await isPortOpen(nativeDriverPort))) return;
    await sleep(100);
  }
  throw new Error(
    `tauri-driver did not open ports ${driverPort.toString()}, ${nativeDriverPort.toString()} within 15 seconds`,
  );
}

async function command(method, path, body) {
  const response = await fetch(`http://${driverHost}:${driverPort.toString()}${path}`, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(timeout),
  });
  const payload = await response.json();
  const value = payload?.value;
  if (!response.ok || (value !== null && typeof value === "object" && "error" in value)) {
    const message =
      value !== null && typeof value === "object" && typeof value.message === "string"
        ? value.message
        : JSON.stringify(payload);
    throw new Error(`WebDriver ${method} ${path} failed: ${message}`);
  }
  return value;
}

async function createSession() {
  const value = await command("POST", "/session", {
    capabilities: {
      alwaysMatch: {
        "tauri:options": { application },
      },
    },
  });
  assert(value !== null && typeof value === "object", "WebDriver returned no session payload");
  assert.equal(typeof value.sessionId, "string", "WebDriver returned no session ID");
  activeSessionId = value.sessionId;
  return value.sessionId;
}

async function deleteSession() {
  const sessionId = activeSessionId;
  activeSessionId = undefined;
  if (sessionId === undefined) return;
  await command("DELETE", `/session/${encodeURIComponent(sessionId)}`);
}

async function findElement(sessionId, selector) {
  const value = await command("POST", `/session/${encodeURIComponent(sessionId)}/element`, {
    using: "css selector",
    value: selector,
  });
  assert(value !== null && typeof value === "object", `No element payload for ${selector}`);
  const id = value[elementKey] ?? value.ELEMENT;
  assert.equal(typeof id, "string", `No element ID for ${selector}`);
  return id;
}

async function waitForElement(sessionId, selector) {
  const deadline = Date.now() + timeout;
  let lastError;
  while (Date.now() < deadline) {
    try {
      return await findElement(sessionId, selector);
    } catch (error) {
      lastError = error;
      await sleep(100);
    }
  }
  throw new Error(`Element ${selector} was not available within 15 seconds`, { cause: lastError });
}

async function setValue(sessionId, selector, text) {
  const id = await waitForElement(sessionId, selector);
  const elementPath = `/session/${encodeURIComponent(sessionId)}/element/${encodeURIComponent(id)}`;
  const type = await evaluate(sessionId, "return document.querySelector(arguments[0]).type;", [
    selector,
  ]);
  if (type === "date" && text.length !== 0) {
    await command("POST", `${elementPath}/clear`, {});
  } else {
    // Native text editing must notify Vue. WebKit clear also rejects some formerly-readonly fields.
    const keys = "\uE009a\uE000\uE003";
    await command("POST", `${elementPath}/value`, { text: keys, value: Array.from(keys) });
  }
  if (text.length !== 0)
    await command("POST", `${elementPath}/value`, { text, value: Array.from(text) });
}

async function getText(sessionId, selector) {
  const id = await waitForElement(sessionId, selector);
  return command(
    "GET",
    `/session/${encodeURIComponent(sessionId)}/element/${encodeURIComponent(id)}/text`,
  );
}

async function evaluate(sessionId, script, args = []) {
  return command("POST", `/session/${encodeURIComponent(sessionId)}/execute/sync`, {
    script,
    args,
  });
}

async function waitFor(sessionId, script, args = []) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const result = await evaluate(sessionId, script, args);
    if (result) return result;
    await sleep(100);
  }
  throw new Error(`Native view did not reach the expected state: ${script}`);
}

async function activateElement(sessionId, element) {
  await command("POST", `/session/${encodeURIComponent(sessionId)}/execute/async`, {
    script:
      "const done = arguments[arguments.length - 1]; Promise.all(document.getAnimations().filter(animation => animation.effect?.getComputedTiming().iterations !== Infinity).map(animation => animation.finished.catch(() => {}))).then(() => requestAnimationFrame(() => done(null)));",
    args: [],
  });
  const key = await evaluate(
    sessionId,
    "const field = arguments[0]; field.scrollIntoView({block:'center', behavior:'instant'}); field.focus(); if(document.activeElement !== field) throw new Error('Native control could not receive focus'); return field.matches('a,button') ? '\\uE007' : ' ';",
    [element],
  );
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "activation-keyboard",
        actions: [
          { type: "keyDown", value: key },
          { type: "keyUp", value: key },
        ],
      },
    ],
  });
}

async function activate(sessionId, selector) {
  const id = await waitForElement(sessionId, selector);
  await activateElement(sessionId, { [elementKey]: id });
}

async function activateButton(sessionId, title, root = "body") {
  const element = await waitFor(
    sessionId,
    `return [...(document.querySelector(arguments[1])?.querySelectorAll('button') ?? [])].find(button => button.textContent.trim() === arguments[0] && !button.disabled) ?? null;`,
    [title, root],
  );
  await activateElement(sessionId, element);
}

async function activateCheckbox(sessionId, title, root = "body") {
  const element = await waitFor(
    sessionId,
    `return [...(document.querySelector(arguments[1])?.querySelectorAll('label') ?? [])].find(label => label.textContent.trim() === arguments[0])?.querySelector('input[type="checkbox"]:not(:disabled)') ?? null;`,
    [title, root],
  );
  await activateElement(sessionId, element);
}

async function navigate(sessionId, path) {
  await activate(sessionId, `a[href=${JSON.stringify(`#${path}`)}]`);
  await waitFor(sessionId, "return window.location.hash === arguments[0];", [`#${path}`]);
}

async function selectValue(sessionId, selector, value) {
  await waitForElement(sessionId, selector);
  await evaluate(
    sessionId,
    "const field = document.querySelector(arguments[0]); field.value = arguments[1]; field.dispatchEvent(new Event('change', {bubbles:true}));",
    [selector, value],
  );
}

async function submitForm(sessionId, field) {
  await evaluate(
    sessionId,
    "document.querySelector(arguments[0]).closest('form').requestSubmit();",
    [field],
  );
}

async function idle(sessionId) {
  await waitFor(sessionId, "return !document.querySelector('.operation-panel');");
}

let requestSequence = 100;
function requestId() {
  requestSequence += 1;
  return `01900000-0000-7000-8000-${requestSequence.toString(16).padStart(12, "0")}`;
}

async function nativeRequest(sessionId, name, request) {
  const response = await command(
    "POST",
    `/session/${encodeURIComponent(sessionId)}/execute/async`,
    {
      script: `const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke(arguments[0], {request: arguments[1]}).then(value => done({ok: value}), error => done({failure: typeof error === 'string' ? error : JSON.stringify(error)}));`,
      args: [name, { requestId: requestId(), ...request }],
    },
  );
  assert.equal(response.failure, undefined, `Native ${name} failed: ${response.failure ?? ""}`);
  return response.ok;
}

async function projects(sessionId) {
  return (await nativeRequest(sessionId, "project_list", { schemaVersion: 1, scope: "all" }))
    .result;
}

async function settingsSnapshot(sessionId) {
  return (await nativeRequest(sessionId, "settings_get", { schemaVersion: 1 })).result;
}

async function screenshot(sessionId, name) {
  const image = await command("GET", `/session/${encodeURIComponent(sessionId)}/screenshot`);
  assert.equal(typeof image, "string", "Native screenshot must contain encoded PNG bytes");
  const artifacts = new URL("../../../.cache/e2e/", import.meta.url);
  await mkdir(artifacts, { recursive: true });
  await writeFile(new URL(name, artifacts), Buffer.from(image, "base64"));
}

async function nativeChooser(title, path = null) {
  const deadline = Date.now() + timeout;
  let windowId;
  while (Date.now() < deadline) {
    try {
      const result = await executeFile(xdotoolExecutable, [
        "search",
        "--onlyvisible",
        "--name",
        `^${title}$`,
      ]);
      windowId = result.stdout.trim().split(/\s+/u)[0];
      if (windowId) break;
    } catch {
      /* A chooser may not have been created yet. */
    }
    await sleep(100);
  }
  assert(windowId, `Native chooser did not appear: ${title}`);
  await executeFile(xdotoolExecutable, ["windowfocus", "--sync", windowId]);
  if (path === null) {
    await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "Escape"]);
    return;
  }
  await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "ctrl+l", "ctrl+a"]);
  await executeFile(xdotoolExecutable, ["type", "--clearmodifiers", "--delay", "1", "--", path]);
  // GTK resolves and completes location-entry text asynchronously before default activation.
  await sleep(500);
  await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "Return"]);
}

async function waitForOutcome(sessionId, text) {
  await waitFor(
    sessionId,
    "return document.querySelector('.workspace-outcome')?.textContent.includes(arguments[0]) && !document.querySelector('.operation-panel');",
    [text],
  );
}

async function previewSuccessiveBackups(sessionId) {
  await navigate(sessionId, "/settings/backup-restore");
  for (let index = 1; index <= 4; index += 1) {
    await setValue(
      sessionId,
      "#portable-backup-title",
      `Native portable review ${index.toString()}`,
    );
    await submitForm(sessionId, "#portable-backup-title");
    await waitForElement(sessionId, "#portable-review-heading");
    await idle(sessionId);
    const digest = await evaluate(
      sessionId,
      "return [...document.querySelectorAll('dt')].find(item => item.textContent === 'Prepared file digest')?.nextElementSibling.textContent.trim();",
    );
    assert.match(digest, /^[a-f0-9]{64}$/u);
    if (index === 4) await screenshot(sessionId, "portable-preview.png");
    await activateButton(sessionId, "Discard review");
    await waitForElement(sessionId, "#portable-backup-title");
  }
}

async function settingsAcceptance(sessionId, directory) {
  await navigate(sessionId, "/settings");
  await waitFor(
    sessionId,
    "return document.querySelector('#settings-locale') && !document.querySelector('#settings-locale').disabled && !document.querySelector('#settings-locale').closest('fieldset').disabled;",
  );
  await setValue(sessionId, "#settings-locale", "fr-CA");
  const before = await settingsSnapshot(sessionId);
  await nativeRequest(sessionId, "settings_update", {
    schemaVersion: 1,
    key: "units",
    value: "us-customary",
    expectedLibraryRevision: before.libraryRevision,
  });
  await waitFor(
    sessionId,
    "return document.querySelector('[aria-labelledby=\"settings-locale-title\"] .inline-alert');",
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector('#settings-locale').value;"),
    "fr-CA",
    "External native changes must preserve the dirty locale draft",
  );
  await activateButton(
    sessionId,
    "Discard draft and reload saved section",
    '[aria-labelledby="settings-locale-title"]',
  );
  await setValue(sessionId, "#settings-locale", "fr-CA");
  await submitForm(sessionId, "#settings-locale");
  await waitForOutcome(sessionId, "committed");
  await selectValue(sessionId, "#settings-units", "metric");
  await selectValue(sessionId, "#settings-theme", "dark");
  await submitForm(sessionId, "#settings-theme");
  await waitFor(
    sessionId,
    "return document.documentElement.dataset.theme === 'dark' && !document.querySelector('.operation-panel');",
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector('#settings-units').value;"),
    "metric",
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelector('#settings-units').closest('form').querySelector('[type=submit]').disabled;",
    ),
    false,
    "An exact appearance self-commit must not strand the unaffected units draft",
  );
  await submitForm(sessionId, "#settings-units");
  await waitForOutcome(sessionId, "committed");
  const saved = await settingsSnapshot(sessionId);
  assert.equal(saved.settings.appearance.value.theme, "dark");
  assert.equal(saved.settings.locale.value, "fr-CA");
  assert.equal(saved.settings.units.value, "metric");
  const settingsPath = join(directory, "nonsecret-settings.json");
  await activateButton(sessionId, "Export saved nonsecret settings");
  await nativeChooser("Save nonsecret application settings", settingsPath);
  await waitForOutcome(sessionId, "were exported");
  await readFile(settingsPath);
  await activateButton(sessionId, "Reset section", '[aria-labelledby="settings-appearance-title"]');
  await idle(sessionId);
  assert.equal((await settingsSnapshot(sessionId)).settings.appearance, null);
  await activateButton(sessionId, "Choose settings file to review");
  await nativeChooser("Choose nonsecret application settings", settingsPath);
  await waitForOutcome(sessionId, "review is ready");
  await activateButton(sessionId, "Apply reviewed settings changes");
  await waitForOutcome(sessionId, "settings import committed");
  assert.equal((await settingsSnapshot(sessionId)).settings.appearance.value.theme, "dark");
  await screenshot(sessionId, "settings.png");
}

async function saveReviewed(sessionId, title, path) {
  await activateButton(sessionId, "Save reviewed file");
  await nativeChooser(title, path);
  await waitForOutcome(sessionId, "File saved as");
}

async function chooseCollisions(sessionId, action) {
  await evaluate(
    sessionId,
    "for (const select of document.querySelectorAll('[id^=portable-collision-]')) { select.value = arguments[0]; select.dispatchEvent(new Event('change', {bubbles:true})); } for (const select of document.querySelectorAll('[id^=portable-supplemental-]')) { select.value = 'skip'; select.dispatchEvent(new Event('change', {bubbles:true})); }",
    [action],
  );
}

async function confirmPortable(sessionId, replacing = false, bypass = false) {
  await activateButton(
    sessionId,
    bypass ? "Review replacement without a safety backup" : "Review and confirm application",
  );
  await waitForElement(sessionId, '[role="dialog"]');
  if (replacing) await activate(sessionId, '[role="dialog"] input[type="checkbox"]');
  if (bypass) await setValue(sessionId, "#portable-bypass-phrase", "REPLACE WITHOUT BACKUP");
  await activateButton(
    sessionId,
    bypass ? "Replace without a safety backup" : "Apply reviewed changes",
    '[role="dialog"]',
  );
}

async function chooseRestore(sessionId, path, replacing = false, safety = false) {
  await navigate(sessionId, "/settings/backup-restore");
  await activate(
    sessionId,
    `input[type=radio][value=${JSON.stringify(replacing ? "replace-library" : "add-backup")}]`,
  );
  if (safety) await activate(sessionId, 'input[type=radio][value="safetyBackups"]');
  await activateButton(sessionId, "Choose backup to review");
  await nativeChooser("Choose an Eutheto backup to restore", path);
  await waitForElement(sessionId, "#portable-review-heading");
  await idle(sessionId);
}

async function portableAcceptance(sessionId, scenarioId, directory, backupDirectory) {
  await navigate(sessionId, "/projects");
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  const before = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  assert(before && typeof before.lastOpenedAt === "string");
  await screenshot(sessionId, "setup.png");
  await evaluate(sessionId, "document.querySelector('#setup-validation').scrollIntoView();");
  await screenshot(sessionId, "validation.png");
  await navigate(sessionId, `/project/${scenarioId}/export`);
  await waitForElement(sessionId, "#portable-workspace-heading");
  const after = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  assert.equal(
    after.lastOpenedAt,
    before.lastOpenedAt,
    "Moving between project subviews must not record another opening",
  );
  assert.equal(after.revision, before.revision);
  const exportPath = join(directory, "editable-scenario.eutheto");
  await activateButton(sessionId, "Preview editable scenario export");
  await waitForElement(sessionId, "#portable-review-heading");
  await idle(sessionId);
  await saveReviewed(sessionId, "Save Eutheto export", exportPath);

  await navigate(sessionId, "/projects");
  await navigate(sessionId, "/projects/import");
  await activateButton(sessionId, "Choose file to inspect unopened");
  await nativeChooser("Choose an unopened Eutheto bundle to inspect", exportPath);
  await waitForElement(sessionId, "#portable-review-heading");
  await idle(sessionId);
  const beforeInspect = (await projects(sessionId)).map((project) => project.scenarioId);
  const exactPath = join(directory, "exact-reexport.eutheto");
  await activateButton(sessionId, "Save exact unopened re-export");
  await nativeChooser("Save exact unopened Eutheto bundle", exactPath);
  await waitForOutcome(sessionId, "File saved as");
  assert.deepEqual(
    await readFile(exactPath),
    await readFile(exportPath),
    "Unopened re-export must preserve the exact original bytes",
  );
  assert.deepEqual(
    (await projects(sessionId)).map((project) => project.scenarioId),
    beforeInspect,
    "Inspection and exact re-export must not import a project",
  );

  await navigate(sessionId, "/settings/backup-restore");
  const backupPath = join(directory, "whole-library.eutheto");
  await setValue(sessionId, "#portable-backup-title", "Native recovery baseline");
  await submitForm(sessionId, "#portable-backup-title");
  await waitForElement(sessionId, "#portable-review-heading");
  await idle(sessionId);
  await saveReviewed(sessionId, "Save Eutheto backup", backupPath);

  await navigate(sessionId, "/projects");
  await navigate(sessionId, "/projects/import");
  await activateButton(sessionId, "Choose import file");
  await nativeChooser("Choose an Eutheto file to import", exportPath);
  await waitForElement(sessionId, `#portable-collision-${scenarioId}`);
  await idle(sessionId);
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      `#portable-collision-${scenarioId}`,
    ]),
    "",
    "Existing identity collisions must start without an implicit action",
  );
  await chooseCollisions(sessionId, "create-copy");
  await confirmPortable(sessionId);
  await waitForOutcome(sessionId, "Reviewed changes applied");
  const imported = (await projects(sessionId)).find((project) => project.scenarioId !== scenarioId);
  assert(imported, "The explicitly reviewed copy must have a distinct saved identity");

  await chooseRestore(sessionId, backupPath);
  await chooseCollisions(sessionId, "skip");
  const beforeAdd = (await projects(sessionId)).map((project) => project.scenarioId).sort();
  await confirmPortable(sessionId);
  await waitForOutcome(sessionId, "Reviewed changes applied");
  assert.deepEqual(
    (await projects(sessionId)).map((project) => project.scenarioId).sort(),
    beforeAdd,
    "Additive restore with Skip must not remove existing projects",
  );

  await chooseRestore(sessionId, backupPath, true);
  assert(
    (await getText(sessionId, '[aria-labelledby="portable-review-heading"]')).includes(
      imported.scenarioId,
    ),
    "Replacement review must disclose the actual project identity being removed",
  );
  await confirmPortable(sessionId, true);
  await waitForOutcome(sessionId, "Safety backup saved and verified as");
  const verifiedNotice = await getText(sessionId, ".workspace-outcome");
  assert.deepEqual(
    (await projects(sessionId)).map((project) => project.scenarioId),
    [scenarioId],
  );
  const safetyFiles = (await readdir(backupDirectory)).filter((name) => name.endsWith(".eutheto"));
  assert.equal(
    safetyFiles.length,
    1,
    "The successful replacement must publish its actual safety backup",
  );
  assert(
    verifiedNotice.includes(safetyFiles[0]),
    "The UI must preserve the actual verified artifact basename",
  );

  await navigate(sessionId, "/projects");
  await navigate(sessionId, `/projects?project=${scenarioId}`);
  await setValue(sessionId, "#duplicate-title", "Native safety failure removal");
  await submitForm(sessionId, "#duplicate-title");
  await waitForOutcome(sessionId, "Duplicated");
  const beforeFailure = (await projects(sessionId)).map((project) => project.scenarioId).sort();
  const retainedDirectory = `${backupDirectory}-retained`;
  await rename(backupDirectory, retainedDirectory);
  await writeFile(backupDirectory, "Native E2E deliberately blocks the private backup directory.");
  try {
    await chooseRestore(sessionId, backupPath, true);
    const revisionBeforeFailure = (await settingsSnapshot(sessionId)).libraryRevision;
    await confirmPortable(sessionId, true);
    await waitFor(
      sessionId,
      "return [...document.querySelectorAll('button')].some(button => button.textContent.trim() === 'Review replacement without a safety backup' && !button.disabled);",
    );
    assert.deepEqual(
      (await projects(sessionId)).map((project) => project.scenarioId).sort(),
      beforeFailure,
      "A real safety-backup failure must preserve the library",
    );
    assert.equal((await settingsSnapshot(sessionId)).libraryRevision, revisionBeforeFailure);
    assert(
      !(await getText(sessionId, "main")).includes(backupDirectory),
      "Native filesystem failures must not expose the private path in renderer text",
    );
    await confirmPortable(sessionId, true, true);
    await waitForOutcome(sessionId, "without a safety backup after explicit confirmation");
    assert.deepEqual(
      (await projects(sessionId)).map((project) => project.scenarioId),
      [scenarioId],
    );
  } finally {
    await rm(backupDirectory);
    await rename(retainedDirectory, backupDirectory);
  }
  await chooseRestore(sessionId, join(backupDirectory, safetyFiles[0]), false, true);
  await chooseCollisions(sessionId, "skip");
  await confirmPortable(sessionId);
  await waitForOutcome(sessionId, "Reviewed changes applied");
  assert.deepEqual(
    (await projects(sessionId)).map((project) => project.scenarioId).sort(),
    [scenarioId, imported.scenarioId].sort(),
    "The actual safety backup must restore the previously removed project through the ordinary reviewed recovery path",
  );
  return imported.scenarioId;
}

async function keyboardChord(sessionId, shift = false) {
  const actions = [{ type: "keyDown", value: "\uE009" }];
  if (shift) actions.push({ type: "keyDown", value: "\uE008" });
  actions.push({ type: "keyDown", value: "z" }, { type: "keyUp", value: "z" });
  if (shift) actions.push({ type: "keyUp", value: "\uE008" });
  actions.push({ type: "keyUp", value: "\uE009" });
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [{ type: "key", id: "scenario-keyboard", actions }],
  });
}

async function historyRouteAcceptance(sessionId, scenarioId, settingsCommandId, revisionBefore) {
  const historyRoot = '[aria-labelledby="history-heading"]';
  const entryRoot = `${historyRoot} ol[aria-label="Recorded changes"] > li:first-child`;
  const locale = (await settingsSnapshot(sessionId)).settings.locale.value;
  async function observeHistory(applied, previousRevision = null) {
    const project = (await projects(sessionId)).find((item) => item.scenarioId === scenarioId);
    assert(project && !project.archived && project.domainPackId === "official.workforce");
    if (previousRevision !== null) {
      assert.equal(project.revision, previousRevision + 1, "History actions must commit one step");
    }
    const page = (
      await nativeRequest(sessionId, "scenario_get_history_page", {
        schemaVersion: 1,
        scenarioId,
        expectedRevision: project.revision,
        limit: 50,
        continuation: null,
      })
    ).result;
    assert.equal(page.revision, project.revision);
    const entry = page.entries[0];
    assert.equal(entry?.id, settingsCommandId, "Undo/redo must retain the settings journal entry");
    assert.equal(entry.revisionBefore, revisionBefore);
    assert.equal(entry.revisionAfter, revisionBefore + 1);
    assert.equal(entry.applied, applied);
    assert.equal(page.redoAvailable, !applied);
    await waitFor(
      sessionId,
      `const status = document.querySelector(arguments[0] + ' [role="status"]');
      return status?.getAttribute('aria-busy') === 'false'
        && status.textContent.trim() === 'History captured at revision '
          + new Intl.NumberFormat(arguments[2]).format(arguments[1]) + '.';`,
      [historyRoot, project.revision, locale],
    );
    await waitFor(
      sessionId,
      `const entry = document.querySelector(arguments[0]);
      return entry?.querySelector('dd')?.textContent.trim() === arguments[1]
        && entry.querySelector('.field-help')?.textContent.includes(arguments[2]);`,
      [entryRoot, settingsCommandId, applied ? "Applied" : "Undone"],
    );
    await activate(sessionId, `${entryRoot} details > summary`);
    await waitFor(sessionId, "return document.querySelector(arguments[0]).open;", [
      `${entryRoot} details`,
    ]);
    const details = await getText(sessionId, `${entryRoot} details`);
    assert(
      details.includes(settingsCommandId),
      "History must expose the committed change identity",
    );
    const number = new Intl.NumberFormat(locale);
    assert(
      details.includes(
        `${number.format(entry.revisionBefore)} → ${number.format(entry.revisionAfter)}`,
      ),
      "Recorded revisions must remain the original commit, not the current undo/redo revision",
    );
    return project.revision;
  }
  async function observeSavedLocale(expected) {
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await waitForElement(sessionId, "#setup-calendar");
    assert(
      (await getText(sessionId, '[aria-labelledby="setup-calendar"]')).includes(expected),
      `A fresh native setup read must expose the persisted scenario locale ${expected}`,
    );
  }
  // The shortcut exercise already restored this command. Reuse its journal tail rather than
  // creating another change or treating an audit row as an individually replayable command.
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  assert((await getText(sessionId, '[aria-labelledby="setup-calendar"]')).includes("en-GB"));
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await waitForElement(sessionId, "#history-heading");
  const committedRevision = await observeHistory(true);
  await screenshot(sessionId, "history.png");
  await activateButton(sessionId, "Undo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario undo committed");
  const undoneRevision = await observeHistory(false, committedRevision);
  await observeSavedLocale("en-US");
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await waitForElement(sessionId, "#history-heading");
  await activateButton(sessionId, "Redo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario redo committed");
  await observeHistory(true, undoneRevision);
  await observeSavedLocale("en-GB");
  await navigate(sessionId, "/projects");
}

async function deletionAndHistoryAcceptance(sessionId, scenarioId, copyId, windowTitle) {
  await navigate(sessionId, "/projects");
  await navigate(sessionId, `/projects?project=${scenarioId}`);
  await activateButton(sessionId, "Delete project");
  await waitForElement(sessionId, '[role="dialog"]');
  await activateButton(sessionId, "Export editable scenario first", '[role="dialog"]');
  await waitForElement(sessionId, "#portable-workspace-heading");
  await activateButton(sessionId, "Preview editable scenario export");
  await waitForElement(sessionId, "#portable-review-heading");
  await idle(sessionId);
  await activateButton(sessionId, "Save reviewed file");
  await nativeChooser("Save Eutheto export");
  await waitForOutcome(sessionId, "Operation cancelled");
  await navigate(sessionId, `/projects?project=${scenarioId}`);
  await waitForElement(sessionId, '[role="dialog"]');
  assert(
    (await projects(sessionId)).some((project) => project.scenarioId === scenarioId),
    "A cancelled export must not delete the source project",
  );
  const original = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  const settingsCommandId = requestId();
  await nativeRequest(sessionId, "scenario_apply_command", {
    commandId: settingsCommandId,
    scenarioId,
    expectedRevision: original.revision,
    actor: { actorId: null, displayName: "Native E2E external change" },
    truncateRedo: false,
    command: {
      type: "setScenarioSettings",
      payload: {
        restoration: null,
        settings: {
          timeZone: "UTC",
          locale: "en-GB",
          units: "metric",
          horizon: { start: "2030-01-01T00:00:00Z", end: "2030-02-01T00:00:00Z" },
          gapPolicy: "reject",
          overlapPolicy: "earlier",
        },
      },
    },
  });
  await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[role=dialog] button')].some(button => button.textContent.trim() === 'Review current saved project');",
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll('[role=dialog] button')].find(button => button.textContent.trim() === 'Delete permanently').disabled;",
    ),
    true,
    "A changed revision must invalidate the earlier deletion confirmation",
  );
  await activateButton(sessionId, "Review current saved project", '[role="dialog"]');
  await activateButton(sessionId, "Keep project", '[role="dialog"]');
  await waitFor(sessionId, "return !document.querySelector('[role=dialog]');");
  const revision = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  const nativeWindow = await executeFile(xdotoolExecutable, [
    "search",
    "--onlyvisible",
    "--name",
    `^${windowTitle}$`,
  ]);
  await executeFile(xdotoolExecutable, [
    "windowfocus",
    "--sync",
    nativeWindow.stdout.trim().split(/\s+/u)[0],
  ]);
  await evaluate(sessionId, "document.querySelector('#project-search').focus();");
  await executeFile(xdotoolExecutable, [
    "type",
    "--clearmodifiers",
    "--delay",
    "1",
    "native text edit",
  ]);
  await waitFor(
    sessionId,
    "return document.querySelector('#project-search').value === 'native text edit';",
  );
  // Preserve the editing event; GTK's accelerator bindings are not scenario authority.
  await evaluate(
    sessionId,
    `
    window.__editingKeys = [];
    const observe = (event) => {
      if (event.key.toLowerCase() !== "z" || !event.ctrlKey) return;
      queueMicrotask(() => window.__editingKeys.push({
        prevented: event.defaultPrevented, target: event.target.id, shift: event.shiftKey,
      }));
      if (event.shiftKey) document.removeEventListener("keydown", observe);
    };
    document.addEventListener("keydown", observe);
  `,
  );
  await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "ctrl+z"]);
  await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "ctrl+shift+z"]);
  await waitFor(sessionId, "return window.__editingKeys.length === 2;");
  assert.deepEqual(await evaluate(sessionId, "return window.__editingKeys;"), [
    { prevented: false, target: "project-search", shift: false },
    { prevented: false, target: "project-search", shift: true },
  ]);
  await idle(sessionId);
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    revision,
    "Text-field undo and redo must not invoke scenario history",
  );
  await setValue(sessionId, "#project-search", "");
  await evaluate(sessionId, "document.querySelector('#projects-heading').focus();");
  await keyboardChord(sessionId);
  await waitForOutcome(sessionId, "Scenario undo committed");
  const undone = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  assert(undone > revision);
  await keyboardChord(sessionId, true);
  await waitForOutcome(sessionId, "Scenario redo committed");
  assert(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision >
      undone,
  );
  await historyRouteAcceptance(sessionId, scenarioId, settingsCommandId, original.revision);
  await navigate(sessionId, `/projects?project=${copyId}`);
  await activateButton(sessionId, "Delete project");
  await activateButton(sessionId, "Delete permanently", '[role="dialog"]');
  await waitForOutcome(sessionId, "Deleted");
  assert.deepEqual(
    (await projects(sessionId)).map((project) => project.scenarioId),
    [scenarioId],
  );
  await screenshot(sessionId, "library.png");
}

async function choosePeopleReference(
  sessionId,
  label,
  name,
  root = 'section[aria-labelledby="people-editor-heading"]',
) {
  const input = `${root} input[aria-label=${JSON.stringify(`Search ${label}`)}]`;
  await setValue(sessionId, input, name);
  await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[role=\"option\"]')].some(option => option.textContent.includes(arguments[0]));",
    [name],
  );
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "people-reference",
        actions: [
          { type: "keyDown", value: "\uE015" },
          { type: "keyUp", value: "\uE015" },
          { type: "keyDown", value: "\uE007" },
          { type: "keyUp", value: "\uE007" },
        ],
      },
    ],
  });
}

async function peopleEditorAcceptance(sessionId, scenarioId) {
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  await navigate(sessionId, `/project/${scenarioId}/people`);
  await waitForElement(sessionId, "#people-list-heading");
  const editorRoot = 'section[aria-labelledby="people-editor-heading"]';
  const listRoot = 'section[aria-labelledby="people-list-heading"]';
  async function begin(kind, name) {
    await selectValue(sessionId, "#people-kind", kind);
    await activateButton(sessionId, "Create a record", listRoot);
    await setValue(sessionId, `${editorRoot} input[name="name"]`, name);
    return evaluate(
      sessionId,
      "return document.querySelector(arguments[0]).querySelector('p.break-all').textContent.match(/[a-f0-9-]{36}/u)[0];",
      [editorRoot],
    );
  }
  async function save() {
    await activateButton(sessionId, "Review changes", editorRoot);
    await waitForElement(sessionId, "#people-review-heading");
    await activateButton(sessionId, "Save this reviewed proposal");
    await idle(sessionId);
    await waitFor(sessionId, "return !document.querySelector('#people-editor-heading');");
  }
  const qualificationId = await begin("qualification", "Native training");
  await setValue(sessionId, `${editorRoot} textarea[name="description"]`, "Plain <training> notes");
  await save();
  const teamId = await begin("team", "Native ward");
  await save();
  const assignmentTypeId = await begin("assignmentType", "Native day work");
  await setValue(sessionId, `${editorRoot} input[name="category"]`, "clinical");
  await setValue(sessionId, `${editorRoot} input[id$="-duration"]`, "480");
  await selectValue(sessionId, `${editorRoot} select[name="qualificationMode"]`, "matches");
  await choosePeopleReference(sessionId, "All of these qualifications", "Native training");
  await selectValue(sessionId, `${editorRoot} select[name="locationMode"]`, "none");
  await selectValue(sessionId, `${editorRoot} select[name="timeBehavior"]`, "elapsed");
  await save();
  const locationId = requestId();
  const bucketId = requestId();
  const calendarId = requestId();
  const referenceRevision = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: referenceRevision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native E2E existing reference writer" },
    truncateRedo: false,
    command: {
      type: "applyBatch",
      payload: {
        label: "Existing person reference records",
        commands: [
          { id: locationId, kind: "location", name: "Existing home ward", transitions: [] },
          {
            id: bucketId,
            kind: "workloadBucket",
            name: "Existing scheduled minutes",
            measurement: "scheduledMinutes",
            overlappingContribution: "sum",
          },
          {
            id: calendarId,
            kind: "calendar",
            name: "Existing daily target",
            period: { kind: "day", startTime: "00:00:00" },
          },
        ].map((entity) => ({
          type: "applyDomainCommand",
          payload: { commandType: "official.workforce.add_entity", payload: { entity } },
        })),
      },
    },
  });
  await waitFor(
    sessionId,
    "return [...document.querySelectorAll('dt')].find(term => term.textContent.trim() === 'Revision')?.nextElementSibling?.textContent.trim() === arguments[0];",
    [String(referenceRevision + 1)],
  );
  const personId = await begin("person", "Native person");
  await choosePeopleReference(sessionId, "Teams", "Native ward");
  await choosePeopleReference(sessionId, "Eligible assignment types", "Native day work");
  await activateButton(sessionId, "Add qualification grant", editorRoot);
  await choosePeopleReference(sessionId, "Qualification", "Native training");
  await setValue(
    sessionId,
    `${editorRoot} input[id$="-effectiveFrom"]`,
    "2026-09-01T08:00:00+02:00",
  );
  await setValue(sessionId, `${editorRoot} input[id$="-expiresAt"]`, "2026-10-01T08:00:00+02:00");
  await activateButton(sessionId, "Add qualification grant", editorRoot);
  await evaluate(
    sessionId,
    "const row = [...document.querySelectorAll(arguments[0] + ' fieldset')].find(item => item.querySelector(':scope > legend')?.textContent.trim() === 'Grant 2'); row.id = 'native-second-grant';",
    [editorRoot],
  );
  await choosePeopleReference(
    sessionId,
    "Qualification",
    "Native training",
    "#native-second-grant",
  );
  await setValue(
    sessionId,
    "#native-second-grant input[id$='-effectiveFrom']",
    "2026-10-01T08:00:00+02:00",
  );
  await activateCheckbox(sessionId, "Limit active dates", editorRoot);
  await setValue(sessionId, `${editorRoot} input[id$="-startDate"]`, "2026-09-01");
  await setValue(sessionId, `${editorRoot} input[id$="-endDateExclusive"]`, "2026-11-01");
  await choosePeopleReference(sessionId, "Home location (optional)", "Existing home ward");
  await setValue(sessionId, `${editorRoot} input[id$="-weightNumerator"]`, "2");
  await setValue(sessionId, `${editorRoot} input[id$="-weightDenominator"]`, "3");
  await activateCheckbox(sessionId, "Store an optional workload target", editorRoot);
  await choosePeopleReference(sessionId, "Workload bucket", "Existing scheduled minutes");
  await choosePeopleReference(sessionId, "Workload calendar", "Existing daily target");
  await selectValue(sessionId, `${editorRoot} select[id$="-membership"]`, "intersection");
  await setValue(sessionId, `${editorRoot} input[id$="-target"]`, "480");
  await activateCheckbox(sessionId, "Store display metadata", editorRoot);
  await setValue(sessionId, `${editorRoot} input[id$="-color"]`, "#123456");
  await setValue(sessionId, `${editorRoot} input[id$="-initials"]`, "NP");
  // Intersection is deliberately invalid for this scheduled-minute bucket.
  // Native validation must refuse approval without discarding the editable target.
  const beforeInvalidTarget = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await activateButton(sessionId, "Review changes", editorRoot);
  await idle(sessionId);
  assert(
    await evaluate(
      sessionId,
      "return !document.querySelector('#people-review-heading') && !!document.querySelector('section[aria-labelledby=\"people-heading\"] > [role=\"alert\"]');",
    ),
    "An incompatible target membership must not receive native approval",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeInvalidTarget,
    "Rejected target preview must not mutate the scenario",
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      `${editorRoot} input[id$="-target"]`,
    ]),
    "480",
    "Native refusal must preserve the editable target",
  );
  await selectValue(sessionId, `${editorRoot} select[id$="-membership"]`, "startInstant");
  await save();
  await activateButton(sessionId, `Native person · ${personId}`, listRoot);
  await waitForElement(sessionId, `${editorRoot} input[id$="-target"]`);
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('Unit: scheduled minutes. Overlapping contributions: sum.') && document.querySelector(arguments[0]).textContent.includes('Existing daily target');",
    [editorRoot],
  );
  assert.deepEqual(
    await evaluate(
      sessionId,
      "const root = document.querySelector(arguments[0]); return { starts: [...root.querySelectorAll('input[id$=\"-effectiveFrom\"]')].map(input => Date.parse(input.value)), ends: [...root.querySelectorAll('input[id$=\"-expiresAt\"]')].map(input => input.value ? Date.parse(input.value) : null), activeStart: root.querySelector('input[id$=\"-startDate\"]').value, activeEnd: root.querySelector('input[id$=\"-endDateExclusive\"]').value, weight: ['weightNumerator', 'weightDenominator'].map(key => root.querySelector('input[id$=\"-' + key + '\"]').value), target: root.querySelector('input[id$=\"-target\"]').value, membership: root.querySelector('select[id$=\"-membership\"]').value, color: root.querySelector('input[id$=\"-color\"]').value, initials: root.querySelector('input[id$=\"-initials\"]').value };",
      [editorRoot],
    ),
    {
      starts: [Date.parse("2026-09-01T08:00:00+02:00"), Date.parse("2026-10-01T08:00:00+02:00")],
      ends: [Date.parse("2026-10-01T08:00:00+02:00"), null],
      activeStart: "2026-09-01",
      activeEnd: "2026-11-01",
      weight: ["2", "3"],
      target: "480",
      membership: "startInstant",
      color: "#123456",
      initials: "NP",
    },
    "Native saved detail must retain distinct grants and their exact instants, active dates and target/display semantics",
  );
  assert(
    await evaluate(
      sessionId,
      "return arguments[1].every(id => document.querySelector(arguments[0]).textContent.includes(id));",
      [editorRoot, [locationId, bucketId, calendarId]],
    ),
    "Native saved person references must retain their exact identities",
  );
  await screenshot(sessionId, "people-native-optional-fields.png");
  await activateButton(sessionId, "Edit draft", editorRoot);
  await waitFor(
    sessionId,
    "const input = document.querySelector(arguments[0]); return input && !input.readOnly && !input.disabled && document.activeElement === input;",
    [`${editorRoot} input[name="name"]`],
  );
  await setValue(sessionId, `${editorRoot} input[name="name"]`, "Local person name");
  await activateCheckbox(sessionId, "Use an external person ID", editorRoot);
  await setValue(sessionId, `${editorRoot} input[name="externalId"]`, "Retained inactive identity");
  await activateCheckbox(sessionId, "Use an external person ID", editorRoot);
  await activateButton(sessionId, "Add tag", editorRoot);
  await setValue(sessionId, `${editorRoot} input[id*="-tag-"]`, "local-tag");
  await activateButton(sessionId, "Review changes", editorRoot);
  await waitForElement(sessionId, "#people-review-heading");
  const beforeConcurrent = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  // A real second writer conflicts with local name/tags and removes untouched optional fields.
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: beforeConcurrent.revision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native E2E second writer" },
    truncateRedo: false,
    command: {
      type: "applyDomainCommand",
      payload: {
        commandType: "official.workforce.update_entity",
        payload: {
          entity: {
            id: personId,
            kind: "person",
            name: "Remote person name",
            activeRange: { kind: "always" },
            qualificationGrants: [{ qualificationId }],
            eligibleAssignmentTypeIds: [assignmentTypeId],
            workloadWeight: { numerator: 1, denominator: 1 },
            tags: ["concurrent-tag"],
            teamIds: [teamId],
          },
        },
      },
    },
  });
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('The saved context changed');",
    [editorRoot],
  );
  await waitFor(sessionId, "return document.activeElement?.id === 'people-editor-heading';");
  assert.equal(
    await evaluate(sessionId, "return !!document.querySelector('#people-review-heading');"),
    false,
    "A revision event must invalidate the previously approved command",
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      `${editorRoot} input[name="name"]`,
    ]),
    "Local person name",
    "A revision event must retain the unsaved raw draft",
  );
  // Referenced target metadata refreshes independently from the person after each revision.
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0])?.validity.valid === true;",
    [`${editorRoot} input[id$="-target"]`],
  );
  await activateButton(sessionId, "Review current changes", editorRoot);
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('Concurrent field changes');",
    [editorRoot],
  );
  await waitFor(
    sessionId,
    "return document.activeElement?.textContent.trim() === 'Concurrent field changes';",
  );
  await screenshot(sessionId, "people-native-rebase.png");
  await evaluate(
    sessionId,
    "const row = [...document.querySelectorAll(arguments[0] + ' section[aria-label=\"Concurrent field changes\"] li')].find(item => item.querySelector('h5')?.textContent === 'Tags'); row.id = 'manual-tag-conflict';",
    [editorRoot],
  );
  await activateButton(sessionId, "Use current field", "#manual-tag-conflict");
  const beforeNextRevision = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: beforeNextRevision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native E2E third writer" },
    truncateRedo: false,
    command: {
      type: "applyDomainCommand",
      payload: {
        commandType: "official.workforce.update_entity",
        payload: {
          entity: {
            id: qualificationId,
            kind: "qualification",
            name: "Native training",
            description: "Unrelated revision during explicit field choices",
          },
        },
      },
    },
  });
  await waitFor(
    sessionId,
    "return [...document.querySelectorAll('dt')].find(term => term.textContent.trim() === 'Revision')?.nextElementSibling?.textContent.trim() === arguments[0];",
    [String(beforeNextRevision + 1)],
  );
  assert.deepEqual(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll(arguments[0] + ' section[aria-label=\"Concurrent field changes\"] button')].filter(button => button.textContent.trim() !== 'Inspect the complete current record').map(button => button.disabled);",
      [editorRoot],
    ),
    [true, true],
    "Retained manual choices must stay disabled until the new revision is explicitly rebased",
  );
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0])?.validity.valid === true;",
    [`${editorRoot} input[id$="-target"]`],
  );
  await activateButton(sessionId, "Review current changes", editorRoot);
  await waitFor(
    sessionId,
    "return document.activeElement?.textContent.trim() === 'Concurrent field changes';",
  );
  assert.deepEqual(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll(arguments[0] + ' section[aria-label=\"Concurrent field changes\"] h5')].map(heading => heading.textContent);",
      [editorRoot],
    ),
    ["Name"],
    "A resolved current field must survive a later unrelated revision while the unresolved name remains",
  );
  await activateButton(sessionId, "Keep draft field", editorRoot);
  await waitFor(
    sessionId,
    "return document.activeElement?.textContent.trim() === 'Review changes';",
  );
  await activateCheckbox(sessionId, "Use an external person ID", editorRoot);
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      `${editorRoot} input[name="externalId"]`,
    ]),
    "Retained inactive identity",
    "Rebase must preserve inactive raw input when the resolved native field has not changed",
  );
  await activateCheckbox(sessionId, "Use an external person ID", editorRoot);
  await activateButton(sessionId, "Review changes", editorRoot);
  await waitForElement(sessionId, "#people-review-heading");
  const proposal = 'section[aria-label="Proposed complete record"]';
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      `${proposal} input[name="name"]`,
    ]),
    "Local person name",
  );
  assert(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll(arguments[0])].some(input => input.value === 'concurrent-tag');",
      [`${proposal} input`],
    ),
    "The native proposal must preserve the explicit current-tag choice instead of restoring stale raw fields",
  );
  await screenshot(sessionId, "people-native-proposal.png");
  await activateButton(sessionId, "Save this reviewed proposal");
  await idle(sessionId);
  await waitFor(sessionId, "return !document.querySelector('#people-editor-heading');");
  await selectValue(sessionId, "#people-kind", "qualification");
  await activateButton(sessionId, `Native training · ${qualificationId}`, listRoot);
  const beforeRejectedDelete = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  await activateButton(sessionId, "Review deletion", editorRoot);
  await idle(sessionId);
  assert.equal(
    await evaluate(sessionId, "return !!document.querySelector('#people-review-heading');"),
    false,
    "A referenced qualification must not receive deletion approval",
  );
  assert(
    await evaluate(
      sessionId,
      "return !!document.querySelector('section[aria-labelledby=\"people-heading\"] > [role=\"alert\"]') && !!document.querySelector('#people-editor-heading');",
    ),
    "A rejected deletion must leave a visible native error and the original editor",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeRejectedDelete.revision,
    "Rejected deletion preview must not mutate the scenario",
  );
  await activateButton(sessionId, "Close record", editorRoot);
  const removedTeamId = await begin("team", "Team to recover");
  await save();
  await activateButton(sessionId, `Team to recover · ${removedTeamId}`, listRoot);
  await activateButton(sessionId, "Edit draft", editorRoot);
  await setValue(sessionId, `${editorRoot} input[name="name"]`, "Recovered team");
  const beforeRemove = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: beforeRemove.revision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native E2E second writer" },
    truncateRedo: false,
    command: {
      type: "applyDomainCommand",
      payload: {
        commandType: "official.workforce.remove_entity",
        payload: { entityId: removedTeamId },
      },
    },
  });
  await activateButton(sessionId, "Keep these fields as a new record with a new ID", editorRoot);
  await waitFor(
    sessionId,
    "return document.activeElement === document.querySelector(arguments[0]);",
    [`${editorRoot} input[name="name"]`],
  );
  const recoveredTeamId = await evaluate(
    sessionId,
    "return document.querySelector(arguments[0]).querySelector('p.break-all').textContent.match(/[a-f0-9-]{36}/u)[0];",
    [editorRoot],
  );
  assert.notEqual(
    recoveredTeamId,
    removedTeamId,
    "Recovery must allocate a new identity, never replay Update as Add",
  );
  const beforeUnrelated = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: beforeUnrelated.revision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native E2E second writer" },
    truncateRedo: false,
    command: {
      type: "applyDomainCommand",
      payload: {
        commandType: "official.workforce.add_entity",
        payload: { entity: { id: requestId(), kind: "team", name: "Concurrent unrelated team" } },
      },
    },
  });
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('The saved context changed');",
    [editorRoot],
  );
  assert(
    await evaluate(
      sessionId,
      "return document.activeElement === document.querySelector(arguments[0]);",
      [`${editorRoot} input[name="name"]`],
    ),
    "A context event must not steal focus from an existing draft input",
  );
  await activateButton(sessionId, "Review current changes", editorRoot);
  await save();
  await activateButton(sessionId, `Recovered team · ${recoveredTeamId}`, listRoot);
  await activateButton(sessionId, "Close record", editorRoot);
  return { personId, qualificationId, teamId, assignmentTypeId };
}

async function peopleBulkAcceptance(sessionId, scenarioId, support) {
  const listRoot = 'section[aria-labelledby="people-list-heading"]';
  const bulkRoot = 'section[aria-labelledby="people-bulk-heading"]';
  const seedTeamId = requestId();
  const people = ["A", "B"].map((letter) => ({
    id: requestId(),
    kind: "person",
    name: `Bulk native ${letter}`,
    externalId: `bulk-${letter}`,
    activeRange: { kind: "always" },
    qualificationGrants: [
      { qualificationId: support.qualificationId },
      { qualificationId: support.qualificationId, effectiveFrom: "2026-01-01T00:00:00Z" },
    ],
    eligibleAssignmentTypeIds: [support.assignmentTypeId],
    workloadWeight: { numerator: 2, denominator: 3 },
    tags: ["bulk-preserved"],
    teamIds: letter === "A" ? [seedTeamId, support.teamId] : [seedTeamId],
    display: { color: "#334455", avatarInitials: letter },
  }));
  const revision = async () =>
    (await projects(sessionId)).find((item) => item.scenarioId === scenarioId).revision;
  async function writeEntities(entities, commandType) {
    return nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: await revision(),
      commandId: requestId(),
      actor: { actorId: null, displayName: "Native E2E bulk fixture writer" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: {
          label: "Native bulk fixture",
          commands: entities.map((entity) => ({
            type: "applyDomainCommand",
            payload: { commandType, payload: { entity } },
          })),
        },
      },
    });
  }
  await writeEntities(
    [{ id: seedTeamId, kind: "team", name: "Bulk seed team" }, ...people],
    "official.workforce.add_entity",
  );
  await navigate(sessionId, `/project/${scenarioId}/people`);
  await selectValue(sessionId, "#people-kind", "person");
  async function open() {
    for (const person of people)
      await activateCheckbox(
        sessionId,
        `Select for a bulk action · ${person.name} · ${person.id}`,
        listRoot,
      );
    await activateButton(sessionId, "Edit selected people", listRoot);
    await waitForElement(sessionId, "#bulk-action");
    assert.equal(
      await evaluate(sessionId, "return document.activeElement?.id;"),
      "people-bulk-heading",
    );
  }
  async function preview() {
    await activateButton(sessionId, "Review this one native batch", bulkRoot);
    await waitForElement(sessionId, "#bulk-review-heading");
    assert.equal(
      await evaluate(sessionId, "return document.activeElement?.id;"),
      "bulk-review-heading",
    );
  }
  async function save() {
    const before = await revision();
    await activateButton(sessionId, "Save this reviewed People batch", bulkRoot);
    await idle(sessionId);
    await waitFor(sessionId, "return !document.querySelector('#people-bulk-heading');");
    assert.equal(await revision(), before + 1, "A People bulk action commits exactly one revision");
    assert.equal(
      await evaluate(sessionId, "return document.activeElement?.id;"),
      "people-list-heading",
    );
  }
  await open();
  await selectValue(sessionId, "#bulk-action", "addTeam");
  await choosePeopleReference(sessionId, "Team for this bulk action", "Native ward", bulkRoot);
  await preview();
  // Add is initially a no-op for A, but still owns the explicitly selected team field.
  await writeEntities(
    [{ ...people[0], teamIds: [seedTeamId] }],
    "official.workforce.update_entity",
  );
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('The saved context changed');",
    [bulkRoot],
  );
  await activateButton(sessionId, "Read current people and review draft conflicts", bulkRoot);
  await waitFor(
    sessionId,
    "return document.activeElement?.textContent === 'Concurrent field changes';",
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll(arguments[0] + ' fieldset')].some(field => field.querySelector('legend')?.textContent.includes(arguments[1]));",
      [bulkRoot, people[0].id],
    ),
    true,
    "An explicitly targeted no-op field must require a choice after a concurrent change",
  );
  await activateButton(sessionId, "Keep draft field", bulkRoot);
  await activateButton(sessionId, "Use these resolved drafts", bulkRoot);
  await preview();
  await selectValue(sessionId, "#bulk-proposed-person", people[1].id);
  await waitFor(sessionId, "return document.querySelector(arguments[0])?.value === arguments[1];", [
    `${bulkRoot} section[aria-label="Proposed complete record"] input[name="name"]`,
    people[1].name,
  ]);
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes(arguments[1]);",
    [`${bulkRoot} section[aria-label="Proposed complete record"]`, support.teamId],
  );
  await evaluate(sessionId, "document.querySelector('#bulk-review-heading').scrollIntoView();");
  await screenshot(sessionId, "people-bulk-native-proposal.png");
  await save();
  await open();
  await waitFor(
    sessionId,
    "const text = document.querySelector(arguments[0]).textContent; return text.includes(arguments[1]) && text.includes(arguments[2]);",
    [bulkRoot, support.teamId, seedTeamId],
  );
  await selectValue(sessionId, "#bulk-action", "removeTeam");
  await choosePeopleReference(sessionId, "Team for this bulk action", "Native ward", bulkRoot);
  await preview();
  await save();
  await open();
  await selectValue(sessionId, "#bulk-action", "activeRange");
  await activateCheckbox(sessionId, "Limit active dates", `${bulkRoot} form`);
  await setValue(sessionId, "#bulk-start-date", "not-a-date");
  await setValue(sessionId, "#bulk-end-date", "2030-02-01");
  const beforeInvalid = await revision();
  await activateButton(sessionId, "Review this one native batch", bulkRoot);
  await waitFor(
    sessionId,
    "return document.activeElement?.textContent === 'Bulk action needs attention';",
  );
  assert.equal(
    await revision(),
    beforeInvalid,
    "Native rejection cannot partially apply a bulk proposal",
  );
  await setValue(sessionId, "#bulk-start-date", "2030-01-01");
  await preview();
  // Both active-range fields conflict, while an independent name/tag change must survive.
  const remoteRange = {
    kind: "dateRange",
    startDate: "2030-02-01",
    endDateExclusive: "2030-03-01",
  };
  const remote = people.map((person, index) => ({
    ...person,
    activeRange: remoteRange,
    teamIds: [seedTeamId],
    ...(index === 0
      ? { name: "Bulk native A updated", tags: ["bulk-preserved", "bulk-concurrent"] }
      : {}),
  }));
  await writeEntities(remote, "official.workforce.update_entity");
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('The saved context changed');",
    [bulkRoot],
  );
  assert.equal(
    await evaluate(sessionId, "return !!document.querySelector('#bulk-review-heading');"),
    false,
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector('#bulk-start-date').value;"),
    "2030-01-01",
  );
  await activateButton(sessionId, "Read current people and review draft conflicts", bulkRoot);
  await waitFor(
    sessionId,
    "return document.activeElement?.textContent === 'Concurrent field changes';",
  );
  await screenshot(sessionId, "people-bulk-native-rebase.png");
  // Scope each keyboard choice to its native stable identity, not a duplicate display name.
  for (const [index, person] of people.entries()) {
    await waitFor(
      sessionId,
      "return [...document.querySelectorAll(arguments[0] + ' fieldset')].some(field => field.querySelector('legend')?.textContent.includes(arguments[1]));",
      [bulkRoot, person.id],
    );
    await evaluate(
      sessionId,
      "const group = [...document.querySelectorAll(arguments[0] + ' fieldset')].find(field => field.querySelector('legend')?.textContent.includes(arguments[1])); group.id = arguments[2];",
      [bulkRoot, person.id, `bulk-conflict-${String(index)}`],
    );
    await activateButton(
      sessionId,
      index === 0 ? "Keep draft field" : "Use current field",
      `#bulk-conflict-${String(index)}`,
    );
    if (index === 0) {
      // Another revision must retain A's explicit choice and B's unresolved conflict.
      await writeEntities(
        [{ id: seedTeamId, kind: "team", name: "Bulk seed team revised" }],
        "official.workforce.update_entity",
      );
      await waitFor(
        sessionId,
        "return document.querySelector(arguments[0]).textContent.includes('The saved context changed');",
        [bulkRoot],
      );
      await activateButton(sessionId, "Read current people and review draft conflicts", bulkRoot);
      await waitFor(
        sessionId,
        "return document.activeElement?.textContent === 'Concurrent field changes';",
      );
      assert.equal(
        await evaluate(
          sessionId,
          "return [...document.querySelectorAll(arguments[0] + ' fieldset')].some(field => field.querySelector('legend')?.textContent.includes(arguments[1]));",
          [bulkRoot, person.id],
        ),
        false,
        "An already resolved draft choice survives an unrelated revision",
      );
    }
  }
  await activateButton(sessionId, "Use these resolved drafts", bulkRoot);
  await preview();
  await selectValue(sessionId, "#bulk-proposed-person", people[1].id);
  await waitFor(sessionId, "return document.querySelector(arguments[0])?.value === '2030-02-01';", [
    `${bulkRoot} section[aria-label="Proposed complete record"] input[id$="-startDate"]`,
  ]);
  await save();
  people[0].name = remote[0].name;
  await open();
  const preserved = await evaluate(
    sessionId,
    "const root = document.querySelector(arguments[0]); return {name: root.querySelector('input[name=\"name\"]').value, externalId: root.querySelector('input[name=\"externalId\"]').value, from: root.querySelector('input[id$=\"-startDate\"]').value, values: [...root.querySelectorAll('input')].map(input => input.value), text: root.textContent};",
    [bulkRoot],
  );
  assert.equal(preserved.name, "Bulk native A updated");
  assert.equal(preserved.externalId, "bulk-A");
  assert.equal(preserved.from, "2030-01-01");
  assert(
    preserved.values.includes("bulk-preserved") && preserved.values.includes("bulk-concurrent"),
  );
  await waitFor(
    sessionId,
    "const text = document.querySelector(arguments[0]).textContent; return arguments[1].every(id => text.includes(id));",
    [bulkRoot, [seedTeamId, support.qualificationId, support.assignmentTypeId]],
  );
  await selectValue(sessionId, "#bulk-person-inspection", people[1].id);
  await waitFor(sessionId, "return document.querySelector(arguments[0])?.value === '2030-02-01';", [
    `${bulkRoot} input[id$="-startDate"]`,
  ]);
  await selectValue(sessionId, "#bulk-action", "delete");
  const beforeConfirmation = await revision();
  await activateButton(sessionId, "Review this one native batch", bulkRoot);
  assert.equal(await revision(), beforeConfirmation);
  assert.equal(
    await evaluate(sessionId, "return !!document.querySelector('#bulk-review-heading');"),
    false,
  );
  await activateCheckbox(sessionId, "I intend to delete these selected people", bulkRoot);
  await preview();
  await save();
  const historyRoot = '[aria-labelledby="history-heading"]';
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Undo scenario change", historyRoot);
  await idle(sessionId);
  await navigate(sessionId, `/project/${scenarioId}/people`);
  for (const person of people)
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0]).textContent.includes(arguments[1]);",
      [listRoot, person.id],
    );
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Redo scenario change", historyRoot);
  await idle(sessionId);
  await navigate(sessionId, `/project/${scenarioId}/people`);
  await waitForElement(sessionId, "#people-list-heading");
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('Matching records:');",
    [listRoot],
  );
  for (const person of people)
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelector(arguments[0]).textContent.includes(arguments[1]);",
        [listRoot, person.id],
      ),
      false,
      "One redo removes both reviewed people",
    );
}

async function peopleRestartAcceptance(sessionId, scenarioId, personId) {
  await navigate(sessionId, `/project/${scenarioId}/people`);
  await activateButton(
    sessionId,
    `Local person name · ${personId}`,
    'section[aria-labelledby="people-list-heading"]',
  );
  const editorRoot = 'section[aria-labelledby="people-editor-heading"]';
  await waitForElement(sessionId, `${editorRoot} input[name="name"]`);
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      `${editorRoot} input[name="name"]`,
    ]),
    "Local person name",
  );
  assert(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll(arguments[0])].some(input => input.value === 'concurrent-tag');",
      [`${editorRoot} input`],
    ),
    "The rebased fields must survive a full native restart",
  );
  await activateButton(sessionId, "Close record", editorRoot);
}

async function peopleCsvAcceptance(sessionId, scenarioId, directory, people) {
  const root = 'section[aria-labelledby="csv-import-heading"]';
  const badPath = join(directory, "unsupported-people.csv");
  const sourcePath = join(directory, "reviewed-people.csv");
  await writeFile(
    badPath,
    Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from("name\r\nperson", "utf16le")]),
  );
  await writeFile(
    sourcePath,
    [
      "<svg onload=window.csvPwned=1>,Personnel key",
      "Local person name,csv-native-a",
      "CSV native second,csv-native-b",
      "Rejected row,bad,unexpected",
      "Duplicate external identity,csv-native-a",
      "Local person name,csv-updated",
      "",
    ].join("\r\n"),
  );
  await navigate(sessionId, `/project/${scenarioId}/people/import`);
  const beforeCancel = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await activateButton(sessionId, "Choose a CSV file", root);
  await nativeChooser("Choose people CSV snapshot");
  await waitForOutcome(sessionId, "Operation cancelled");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeCancel,
  );
  await activateButton(sessionId, "Choose a CSV file", root);
  await nativeChooser("Choose people CSV snapshot", badPath);
  await waitFor(sessionId, "return document.querySelector(arguments[0] + ' [role=\"alert\"]');", [
    root,
  ]);
  assert.equal(await evaluate(sessionId, "return document.querySelector('#csv-dialect');"), null);
  await activateButton(sessionId, "Discard this source and choose another", root);
  await nativeChooser("Choose people CSV snapshot", sourcePath);
  await waitForElement(sessionId, "#csv-dialect");
  await idle(sessionId);
  assert.equal(
    await evaluate(sessionId, "return document.querySelector('#csv-dialect').value;"),
    "",
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector('#csv-header').value;"),
    "",
  );
  await selectValue(sessionId, "#csv-dialect", "comma");
  await selectValue(sessionId, "#csv-header", "yes");
  await setValue(sessionId, "#csv-width", "2");
  await selectValue(sessionId, 'select[data-source-index="0"]', "name");
  await selectValue(sessionId, 'select[data-source-index="1"]', "externalId");
  assert.equal(await evaluate(sessionId, "return window.csvPwned ?? null;"), null);
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelector(arguments[0]).querySelector('svg,img');",
      [root],
    ),
    null,
  );
  await setValue(sessionId, `${root} input[id$="-record"]`, "2");
  await activateButton(sessionId, "Add", `${root} section[aria-labelledby$="-manual"]`);
  const occupiedId = await evaluate(
    sessionId,
    "return document.querySelector(arguments[0] + ' article[id$=\"-decision-2\"]').textContent.match(/Person ID: ([a-f0-9-]{36})/u)[1];",
    [root],
  );
  const beforeCollision = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: beforeCollision.revision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native CSV identity collision" },
    truncateRedo: false,
    command: {
      type: "applyDomainCommand",
      payload: {
        commandType: "official.workforce.add_entity",
        payload: {
          entity: {
            id: occupiedId,
            kind: "person",
            name: "Native occupied draft identity",
            activeRange: { kind: "always" },
            qualificationGrants: [],
            eligibleAssignmentTypeIds: [],
            workloadWeight: { numerator: 1, denominator: 1 },
            tags: [],
            teamIds: [],
          },
        },
      },
    },
  });
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('previous approval is invalid');",
    [root],
  );
  await activateButton(sessionId, "Preview the native import", root);
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0] + ' [role=\"alert\"]')?.textContent.includes('/peopleCsv/records/2');",
    [root],
  );
  assert.equal(
    await evaluate(sessionId, "return document.activeElement?.id.endsWith('-decision-2');"),
    true,
    "A native row error must focus its retained explicit decision without a successful preview",
  );
  await activateButton(sessionId, "Remove explicit decision", `${root} article[id$="-decision-2"]`);
  await activateButton(sessionId, "Preview the native import", root);
  await waitForElement(sessionId, "#csv-review-heading");
  assert((await getText(sessionId, root)).includes("Blocked"));
  assert.equal(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll(arguments[0] + ' button')].some(button => button.textContent.trim() === 'Apply exactly this reviewed import');",
      [root],
    ),
    false,
  );
  assert(
    !(await getText(sessionId, `${root} article[id$="-review-6"]`)).includes(people.personId),
    "An equal name with an unmatched external ID must remain unresolved until explicit Update selection",
  );
  const manualRoot = `${root} section[aria-labelledby$="-manual"]`;
  async function decide(record, kind) {
    await setValue(sessionId, `${manualRoot} input`, record.toString());
    await activateButton(sessionId, kind, manualRoot);
  }
  await decide(2, "Add");
  await decide(3, "Add");
  await decide(5, "Skip");
  await decide(6, "Update");
  await choosePeopleReference(
    sessionId,
    "Existing person for logical record 6",
    "Local person name",
    root,
  );
  const newIds = await evaluate(
    sessionId,
    "return [2,3].map(record => document.querySelector(arguments[0] + ' article[id$=\"-decision-' + record + '\"]').textContent.match(/Person ID: ([a-f0-9-]{36})/u)[1]);",
    [root],
  );
  assert(
    newIds.every((id) => id !== people.personId),
    "Equal display names must not choose an existing identity",
  );
  await setValue(sessionId, `${manualRoot} input`, "2");
  await activateButton(sessionId, "Inspect native record sample", manualRoot);
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('Native sample ready for logical record 2');",
    [root],
  );
  await activateButton(sessionId, "Preview the native import", root);
  await waitForElement(sessionId, "#csv-review-heading");
  await activate(sessionId, "#csv-review-heading");
  const beforeConcurrent = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: beforeConcurrent.revision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native CSV second writer" },
    truncateRedo: false,
    command: {
      type: "applyDomainCommand",
      payload: {
        commandType: "official.workforce.update_entity",
        payload: {
          entity: {
            id: people.qualificationId,
            kind: "qualification",
            name: "Native training",
            description: "Changed during CSV review",
          },
        },
      },
    },
  });
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('previous approval is invalid') && !document.querySelector('#csv-review-heading');",
    [root],
  );
  await waitFor(sessionId, "return document.activeElement?.id === 'csv-import-heading';");
  assert.deepEqual(
    await evaluate(
      sessionId,
      "return [2,3].map(record => document.querySelector(arguments[0] + ' article[id$=\"-decision-' + record + '\"]').textContent.match(/Person ID: ([a-f0-9-]{36})/u)[1]);",
      [root],
    ),
    newIds,
    "Revision-only review must retain explicit Add IDs",
  );
  await activateButton(sessionId, "Preview the native import", root);
  await waitForElement(sessionId, "#csv-review-heading");
  const commandId = await evaluate(
    sessionId,
    "return document.querySelector('[data-csv-review] p.break-all code').textContent;",
  );
  await screenshot(sessionId, "people-csv-native-review.png");
  await selectValue(sessionId, "[data-csv-review] select", newIds[0]);
  await waitForElement(sessionId, '[data-csv-review] input[name="externalId"]');
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelector('[data-csv-review] input[name=\"externalId\"]').value;",
    ),
    "csv-native-a",
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelector('[data-csv-review] input[name=\"name\"]').readOnly;",
    ),
    true,
  );
  await activate(sessionId, '[data-csv-review] input[name="name"]');
  await screenshot(sessionId, "people-csv-native-person.png");
  const beforeApply = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  await activateButton(sessionId, "Apply exactly this reviewed import", root);
  await waitForOutcome(sessionId, "committed as one command batch");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeApply.revision + 1,
  );
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('snapshot has been consumed');",
    [root],
  );
  const reportPath = join(directory, "people-import-rejected.json");
  await activateButton(sessionId, "Save the rejected-row report", root);
  await nativeChooser("Save rejected people rows", reportPath);
  await waitForOutcome(sessionId, "report was saved");
  const report = JSON.parse(await readFile(reportPath, "utf8"));
  assert.deepEqual(report.rejectedRows, [{ record: 4, code: "columnCount" }]);
  assert.equal(report.consumed, true);
  await activateButton(sessionId, "Save the rejected-row report", root);
  await nativeChooser("Save rejected people rows");
  await waitForOutcome(sessionId, "Operation cancelled");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeApply.revision + 1,
    "Report cancellation must not replay or undo the import",
  );
  assert(
    !(await getText(sessionId, root)).includes(sourcePath),
    "Native source paths must not enter the CSV route",
  );
  await screenshot(sessionId, "people-csv-native-report.png");
  await activateButton(sessionId, "Discard this source and choose another", root);
  await nativeChooser("Choose people CSV snapshot", sourcePath);
  await waitForElement(sessionId, "#csv-dialect");
  await idle(sessionId);
  await selectValue(sessionId, "#csv-dialect", "comma");
  await selectValue(sessionId, "#csv-header", "yes");
  await setValue(sessionId, "#csv-width", "2");
  await selectValue(sessionId, 'select[data-source-index="0"]', "name");
  await selectValue(sessionId, 'select[data-source-index="1"]', "externalId");
  await decide(5, "Skip");
  await activateButton(sessionId, "Preview the native import", root);
  await waitForElement(sessionId, "#csv-review-heading");
  await activateButton(sessionId, "Apply exactly this reviewed import", root);
  await waitForOutcome(sessionId, "made no changes");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeApply.revision + 1,
    "An exact no-change import must not create another history step",
  );
  await activate(sessionId, `a[href=${JSON.stringify(`#/project/${scenarioId}/people`)}]`);
  await activateButton(sessionId, "Discard and leave", '[role="dialog"]');
  await waitForElement(sessionId, "#people-list-heading");
  return { newIds, commandId };
}

async function peopleCsvRestartAcceptance(sessionId, scenarioId, people, imported) {
  const list = 'section[aria-labelledby="people-list-heading"]';
  const editor = 'section[aria-labelledby="people-editor-heading"]';
  async function observe(applied) {
    await navigate(sessionId, `/project/${scenarioId}/people`);
    await activateButton(sessionId, `Local person name · ${people.personId}`, list);
    await waitForElement(sessionId, `${editor} input[name="name"]`);
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelector(arguments[0] + ' input[name=\"externalId\"]')?.value ?? null;",
        [editor],
      ),
      applied ? "csv-updated" : null,
    );
    assert(
      await evaluate(
        sessionId,
        "return [...document.querySelectorAll(arguments[0] + ' input')].some(input => input.value === 'concurrent-tag');",
        [editor],
      ),
      "CSV updates must preserve unmapped current fields",
    );
    await activateButton(sessionId, "Close record", editor);
    for (const id of imported.newIds) {
      assert.equal(
        await evaluate(
          sessionId,
          "return [...document.querySelectorAll(arguments[0] + ' button')].some(button => button.textContent.includes(arguments[1]));",
          [list, id],
        ),
        applied,
        "All imported additions must follow the same history step",
      );
    }
    const project = (await projects(sessionId)).find((value) => value.scenarioId === scenarioId);
    const history = (
      await nativeRequest(sessionId, "scenario_get_history_page", {
        schemaVersion: 1,
        scenarioId,
        expectedRevision: project.revision,
        limit: 50,
        continuation: null,
      })
    ).result;
    assert.equal(history.entries[0].id, imported.commandId);
    assert.equal(history.entries[0].applied, applied);
  }
  await observe(true);
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(
    sessionId,
    "Undo scenario change",
    'section[aria-labelledby="history-heading"]',
  );
  await waitForOutcome(sessionId, "Scenario undo committed");
  await observe(false);
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(
    sessionId,
    "Redo scenario change",
    'section[aria-labelledby="history-heading"]',
  );
  await waitForOutcome(sessionId, "Scenario redo committed");
  await observe(true);
}

async function stopDriver() {
  const child = tauriDriver;
  tauriDriver = undefined;
  if (child === undefined || child.exitCode !== null || child.signalCode !== null) return;

  const exited = new Promise((resolve) => child.once("exit", resolve));
  child.kill("SIGTERM");
  await Promise.race([exited, sleep(5_000)]);
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGKILL");
    await exited;
  }
}

async function run() {
  const dataHome = process.env.XDG_DATA_HOME;
  assert.equal(typeof dataHome, "string");
  assert(isAbsolute(dataHome), "Native E2E requires its isolated XDG data directory");
  const directory = join(dirname(dataHome), "portable-files");
  await mkdir(directory, { recursive: true });
  const configuration = JSON.parse(
    await readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
  );
  const backupDirectory = join(dataHome, configuration.identifier, "backups");
  tauriDriver = spawn(
    tauriDriverExecutable,
    [
      "--port",
      driverPort.toString(),
      "--native-port",
      nativeDriverPort.toString(),
      "--native-driver",
      nativeDriverExecutable,
    ],
    { stdio: "inherit" },
  );
  try {
    await waitForDriver();
    const projectTitle = `E2E persistence ${Date.now().toString()}`;
    const firstSessionId = await createSession();
    await waitForElement(firstSessionId, "#welcome-heading");
    await navigate(firstSessionId, "/projects/new");
    await setValue(firstSessionId, "#create-title", projectTitle);
    await setValue(firstSessionId, "#create-time-zone", "UTC");
    // WebKit's native date editor accepts displayed month/day/year keystrokes, not ISO text.
    await setValue(firstSessionId, "#first-date", "01/01/2030");
    await setValue(firstSessionId, "#last-date", "01/31/2030");
    await evaluate(firstSessionId, "document.querySelector('form details summary').focus();");
    await command("POST", `/session/${encodeURIComponent(firstSessionId)}/actions`, {
      actions: [
        {
          type: "key",
          id: "details-keyboard",
          actions: [
            { type: "keyDown", value: " " },
            { type: "keyUp", value: " " },
          ],
        },
      ],
    });
    await waitFor(firstSessionId, "return document.querySelector('form details').open;");
    assert.deepEqual(
      await evaluate(
        firstSessionId,
        "return [document.querySelector('#first-date').value, document.querySelector('#last-date').value];",
      ),
      ["2030-01-01", "2030-01-31"],
    );
    await setValue(firstSessionId, "#create-locale", "en-US");
    await selectValue(firstSessionId, "#create-units", "metric");
    await submitForm(firstSessionId, "#create-title");
    await waitForElement(firstSessionId, "#setup-calendar");
    assert.equal(await getText(firstSessionId, "#project-heading"), projectTitle);
    const scenarioId = await evaluate(
      firstSessionId,
      "return window.location.hash.match(/^#\\/project\\/([^/]+)\\/setup$/)?.[1];",
    );
    assert.match(scenarioId, /^[a-f0-9-]{36}$/u);
    const saved = (await projects(firstSessionId)).find(
      (project) => project.scenarioId === scenarioId,
    );
    assert.equal(saved.domainPackId, "official.workforce");
    assert.equal(
      typeof saved.lastOpenedAt,
      "string",
      "The actual project route must pass project_open ACL and record its opening",
    );
    assert((await getText(firstSessionId, "#setup-results")).includes("Accepted results"));
    assert(
      (await getText(firstSessionId, '[aria-labelledby="setup-results"]')).includes(
        "No accepted result",
      ),
    );
    await previewSuccessiveBackups(firstSessionId);
    await settingsAcceptance(firstSessionId, directory);
    const copyId = await portableAcceptance(firstSessionId, scenarioId, directory, backupDirectory);
    await deletionAndHistoryAcceptance(
      firstSessionId,
      scenarioId,
      copyId,
      configuration.app.windows[0].title,
    );
    const people = await peopleEditorAcceptance(firstSessionId, scenarioId);
    await peopleBulkAcceptance(firstSessionId, scenarioId, people);
    const importedPeople = await peopleCsvAcceptance(firstSessionId, scenarioId, directory, people);
    await navigate(firstSessionId, "/about/licenses");
    await waitForElement(firstSessionId, "#about-inventory-title");
    await waitFor(
      firstSessionId,
      "return document.querySelector('[aria-labelledby=\"about-inventory-title\"] .preview-list > li');",
    );
    assert(
      !(await getText(firstSessionId, "main")).includes(dataHome),
      "About must not expose native directory paths",
    );

    await deleteSession();
    const secondSessionId = await createSession();
    assert.notStrictEqual(secondSessionId, firstSessionId);
    await waitForElement(secondSessionId, "#welcome-heading");
    await navigate(secondSessionId, "/projects");
    await waitForElement(
      secondSessionId,
      `[aria-label=${JSON.stringify(`Open project ${projectTitle}`)}]`,
    );
    assert.deepEqual(
      (await projects(secondSessionId)).map((project) => project.scenarioId),
      [scenarioId],
    );
    const persisted = await settingsSnapshot(secondSessionId);
    assert.equal(persisted.settings.appearance.value.theme, "dark");
    assert.equal(persisted.settings.locale.value, "fr-CA");
    await navigate(secondSessionId, `/project/${scenarioId}/setup`);
    await waitForElement(secondSessionId, "#setup-calendar");
    assert(
      (await getText(secondSessionId, '[aria-labelledby="setup-calendar"]')).includes("en-GB"),
      "Scenario settings must persist independently from the application's formatting locale",
    );
    await peopleRestartAcceptance(secondSessionId, scenarioId, people.personId);
    await peopleCsvRestartAcceptance(secondSessionId, scenarioId, people, importedPeople);
    await evaluate(secondSessionId, "window.location.hash = '#/unavailable-route';");
    await waitForElement(secondSessionId, "#route-recovery-heading");
    assert.deepEqual(
      (await projects(secondSessionId)).map((project) => project.scenarioId),
      [scenarioId],
    );
    console.log(
      "PASS: real Workforce creation/setup and one-open-per-entry; native settings CAS/draft/self-commit/import/export/reset; four bounded backup reviews; editable export/unopened exact re-export/import; additive restore, verified safety backup, real failure/bypass/recovery; cancelled export and changed-revision deletion review; unconsumed editing shortcuts and scoped scenario undo/redo; History route committed metadata, one-step undo/redo, authoritative revision refresh and persisted scenario settings; confirmed deletion; offline About; independent restart persistence and unknown-route recovery",
    );
    console.log(
      "PASS: native supporting-record/person creation with temporal grants, existing target/location references and display fields; manual and bulk choices across concurrent revisions; no-op bulk target conflicts; native batch preview/delete and one-step undo/redo; CSV identity/proposal/report/no-change flow and restart-safe history; referenced deletion refusal and fresh-identity recovery",
    );
  } catch (error) {
    if (activeSessionId !== undefined) {
      await screenshot(activeSessionId, "native-failure.png");
      console.error(
        await evaluate(
          activeSessionId,
          "return { hash: location.hash, text: document.querySelector('main')?.innerText, active: document.activeElement?.outerHTML, details: Array.from(document.querySelectorAll('details')).map(node => ({open: node.open, text: node.innerText})), inputs: Array.from(document.querySelectorAll('input')).map(node => ({id: node.id, type: node.type, value: node.value, disabled: node.disabled, readOnly: node.readOnly})) };",
        ),
      );
    }
    throw error;
  } finally {
    try {
      await deleteSession();
    } finally {
      await stopDriver();
    }
  }
}

await run();

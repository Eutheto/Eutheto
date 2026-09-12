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
  if (text.length === 0) {
    // WebKit's clear command changes the DOM without notifying bound input state.
    const keys = "\uE009a\uE000\uE003";
    await command("POST", `${elementPath}/value`, { text: keys, value: Array.from(keys) });
  } else {
    await command("POST", `${elementPath}/clear`, {});
    await command("POST", `${elementPath}/value`, { text, value: Array.from(text) });
  }
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
  await nativeRequest(sessionId, "scenario_apply_command", {
    commandId: requestId(),
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
    await evaluate(secondSessionId, "window.location.hash = '#/unavailable-route';");
    await waitForElement(secondSessionId, "#route-recovery-heading");
    assert.deepEqual(
      (await projects(secondSessionId)).map((project) => project.scenarioId),
      [scenarioId],
    );
    console.log(
      "PASS: real Workforce creation/setup and one-open-per-entry; native settings CAS/draft/self-commit/import/export/reset; four bounded backup reviews; editable export/unopened exact re-export/import; additive restore, verified safety backup, real failure/bypass/recovery; cancelled export and changed-revision deletion review; unconsumed editing shortcuts and scoped scenario undo/redo; confirmed deletion; offline About; independent restart persistence and unknown-route recovery",
    );
  } catch (error) {
    if (activeSessionId !== undefined) {
      await screenshot(activeSessionId, "native-failure.png");
      console.error(
        await evaluate(
          activeSessionId,
          "return { hash: location.hash, text: document.querySelector('main')?.innerText, details: Array.from(document.querySelectorAll('details')).map(node => ({open: node.open, text: node.innerText})), inputs: Array.from(document.querySelectorAll('input')).map(node => ({id: node.id, type: node.type, value: node.value, disabled: node.disabled})) };",
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

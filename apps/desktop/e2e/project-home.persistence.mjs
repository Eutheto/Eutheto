import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn, execFile } from "node:child_process";
import { mkdir, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { createConnection } from "node:net";
import { cpus, platform, release } from "node:os";
import { basename, dirname, isAbsolute, join } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { isWorkforceResultScheduleViewData } from "../src/api/generated-domain-pack-contracts.ts";

const driverHost = "127.0.0.1";
const driverPort = 4_444;
const nativeDriverPort = 4_445;
const timeout = 15_000;
const elementKey = "element-6066-11e4-a52e-4f735466cecf";
const application =
  process.env.EUTHETO_E2E_APPLICATION ??
  fileURLToPath(new URL("../../../.cache/cargo-target/debug/eutheto-desktop", import.meta.url));
assert(isAbsolute(application), "Native E2E requires an absolute application path");
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

async function command(method, path, body, waitMilliseconds = timeout) {
  const response = await fetch(`http://${driverHost}:${driverPort.toString()}${path}`, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(waitMilliseconds),
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
  await command("POST", `/session/${encodeURIComponent(value.sessionId)}/timeouts`, {
    script: 120_000,
  });
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

async function waitForElement(sessionId, selector, waitMilliseconds = timeout) {
  const deadline = Date.now() + waitMilliseconds;
  let lastError;
  while (Date.now() < deadline) {
    try {
      return await findElement(sessionId, selector);
    } catch (error) {
      lastError = error;
      await sleep(100);
    }
  }
  throw new Error(`Element ${selector} was not available within ${String(waitMilliseconds)} ms`, {
    cause: lastError,
  });
}

async function setValue(sessionId, selector, text) {
  const id = await waitForElement(sessionId, `${selector}:not(:disabled):not([readonly])`);
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

async function waitFor(sessionId, script, args = [], waitMilliseconds = timeout) {
  const deadline = Date.now() + waitMilliseconds;
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

async function openPersonOptions(sessionId, title, root) {
  const summary = await waitFor(
    sessionId,
    "return [...document.querySelectorAll(arguments[1] + ' details > summary')].find(item => item.textContent.trim().startsWith(arguments[0])) ?? null;",
    [title, root],
  );
  const open = await evaluate(sessionId, "return arguments[0].parentElement.open;", [summary]);
  if (!open) await activateElement(sessionId, summary);
  return summary;
}
async function navigate(sessionId, path) {
  const href = `#${path}`;
  const closed = await evaluate(
    sessionId,
    "return document.querySelector(arguments[0])?.closest('details:not([open])')?.querySelector('summary') ?? null;",
    [`a[href=${JSON.stringify(href)}]`],
  );
  if (closed) await activateElement(sessionId, closed);
  await activate(sessionId, `a[href=${JSON.stringify(href)}]`);
  await waitFor(sessionId, "return window.location.hash === arguments[0];", [href]);
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

async function idle(sessionId, waitMilliseconds = timeout) {
  await waitFor(
    sessionId,
    "return document.querySelector('main[data-operation-active=\"false\"]') !== null;",
    [],
    waitMilliseconds,
  );
}

async function startOperationTiming(sessionId) {
  await evaluate(
    sessionId,
    `const main = document.querySelector('main[data-operation-active]');
    if (!main) throw new Error('Native operation state marker is missing');
    const spans = [];
    let active = null;
    let wasActive = main.dataset.operationActive === 'true';
    let overflow = false;
    const observe = () => {
      const now = performance.now();
      const isActive = main.dataset.operationActive === 'true';
      if (isActive && !wasActive) active = { started: now, panelAfter: null, label: null };
      const panel = main.querySelector('.operation-panel');
      if (active && panel && panel.getBoundingClientRect().height > 0) {
        active.panelAfter ??= now - active.started;
        active.label ??= panel.querySelector('#operation-label')?.textContent ?? null;
      }
      if (!isActive && wasActive && active) {
        if (spans.length < 1024) spans.push({ ...active, elapsed: now - active.started });
        else overflow = true;
        active = null;
      }
      wasActive = isActive;
    };
    const observer = new MutationObserver(observe);
    observer.observe(main, {
      childList: true, subtree: true, attributes: true,
      attributeFilter: ['data-operation-active']
    });
    window.__euthetoOperationTiming = () => {
      observe();
      observer.disconnect();
      return { schemaVersion: 1, userAgent: navigator.userAgent, spans, overflow };
    };`,
  );
}

async function finishOperationTiming(sessionId) {
  await idle(sessionId);
  const measurements = await evaluate(sessionId, "return window.__euthetoOperationTiming();");
  assert.equal(measurements.overflow, false);
  const fast = measurements.spans.filter((span) => span.elapsed < 300);
  const displayed = measurements.spans.filter((span) => span.panelAfter !== null);
  assert(fast.length > 0, "The native run must exercise real subthreshold operations");
  assert(displayed.length > 0, "The native run must exercise real displayed long operations");
  for (const span of fast)
    assert.equal(span.panelAfter, null, "Subthreshold native operations must not flash progress");
  for (const span of displayed)
    assert(span.panelAfter >= 300, "Native progress must respect the perceptual display threshold");
  const path = fileURLToPath(
    new URL("../../../.cache/e2e/package9-progress-measurements.json", import.meta.url),
  );
  await writeFile(path, JSON.stringify(measurements, null, 2));
  console.log(
    "PASS: real native short operations did not flash progress; long-operation display was delayed",
    JSON.stringify({
      observed: measurements.spans.length,
      fast: fast.length,
      displayed: displayed.length,
    }),
  );
}

const package10FixtureIds = ["clinic-tiny", "clinic-initial", "large-supported"];

function package10Sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function package10Records(value) {
  return Array.isArray(value) ? value : Object.values(value ?? {});
}

function package10Counts(records, field) {
  const counts = new Map();
  for (const record of records) {
    const value = record?.[field];
    if (typeof value !== "string") continue;
    counts.set(value, (counts.get(value) ?? 0) + 1);
  }
  return Object.fromEntries([...counts].sort(([left], [right]) => left.localeCompare(right)));
}

function package10FixtureShape(fixture, expected) {
  const entities = package10Records(fixture.domain?.payload?.entities);
  const rules = package10Records(fixture.domain?.payload?.rules);
  return {
    people: expected?.people ?? package10Counts(entities, "kind").person ?? 0,
    resolvedShiftCount: expected?.resolvedShifts ?? null,
    selectedAssignments: expected?.selectedAssignments ?? null,
    entityCounts: package10Counts(entities, "kind"),
    ruleCounts: package10Counts(rules, "kind"),
    rules: { total: rules.length },
    settings: {
      timeZone: fixture.settings?.timeZone ?? null,
      locale: fixture.settings?.locale ?? null,
      units: fixture.settings?.units ?? null,
      horizon: fixture.settings?.horizon ?? null,
      gapPolicy: fixture.settings?.gapPolicy ?? null,
      overlapPolicy: fixture.settings?.overlapPolicy ?? null,
    },
  };
}

async function package10FixtureEvidence() {
  const manifestUrl = new URL("../../../benchmarks/corpus/workforce/v1.json", import.meta.url);
  const manifestBytes = await readFile(manifestUrl);
  const manifest = JSON.parse(manifestBytes.toString("utf8"));
  const expectedBytes = await readFile(
    new URL("../../../benchmarks/expected/workforce/v1.json", import.meta.url),
  );
  const expected = JSON.parse(expectedBytes.toString("utf8"));
  const cases = [];
  for (const id of package10FixtureIds) {
    const binding = manifest.cases.find((entry) => entry.id === id);
    assert(binding, `The corpus manifest must contain ${id}`);
    const fixtureUrl = new URL(`../../../${binding.input.path}`, import.meta.url);
    const bytes = await readFile(fixtureUrl);
    const sha256 = package10Sha256(bytes);
    assert.equal(
      sha256,
      binding.input.sha256,
      `The checked source fixture digest changed for ${id}`,
    );
    const fixture = JSON.parse(bytes.toString("utf8"));
    const expectedCase = expected.cases.find((entry) => entry.inputSha256 === sha256)?.expected;
    assert.equal(expectedCase?.id, id, `The expected corpus binding changed for ${id}`);
    cases.push({
      id,
      input: {
        path: binding.input.path,
        sha256,
        bytes: bytes.byteLength,
      },
      expected: {
        path: "benchmarks/expected/workforce/v1.json",
        sha256: package10Sha256(expectedBytes),
      },
      shape: package10FixtureShape(fixture, expectedCase),
      fixture,
    });
  }
  return {
    manifest: {
      path: "benchmarks/corpus/workforce/v1.json",
      sha256: package10Sha256(manifestBytes),
      corpusVersion: manifest.corpusVersion,
      schemaVersion: manifest.schemaVersion,
    },
    cases,
  };
}

async function package10NativeScenarioShape(sessionId, scenarioId, strict = false) {
  try {
    const current = (await projects(sessionId)).find(
      (project) => project.scenarioId === scenarioId,
    );
    assert(current, "The native performance scenario must exist");
    const operationStarted = Date.now();
    const overviewTiming = { backendOnlyMs: null };
    const overview = (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_get_view",
        "official.workforce.setup.overview",
        {
          source: { kind: "stored" },
          query: {
            schemaVersion: 1,
            viewId: "official.workforce.setup.overview",
            parameters: {},
          },
        },
        overviewTiming,
      )
    ).result.view.data.result.data;
    const operationRequestToSettlementMs = Date.now() - operationStarted;
    const workTiming = { backendOnlyMs: null };
    const work = (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_get_view",
        "official.workforce.setup.work_window",
        {
          source: { kind: "stored" },
          query: {
            schemaVersion: 1,
            viewId: "official.workforce.setup.work_window",
            parameters: { dates: overview.planningDates, limit: 128 },
          },
        },
        workTiming,
      )
    ).result.view.data.result.data;
    return {
      status: "measured",
      revision: current.revision,
      shape: {
        entityCounts: Object.fromEntries(
          [...(overview.entities ?? [])]
            .map((entry) => [entry.kind, entry.count])
            .sort(([left], [right]) => left.localeCompare(right)),
        ),
        resolvedShiftCount: work.totalItems,
        requiredRules: overview.requiredRules,
        activeRequiredRules: overview.activeRequiredRules,
        preferences: overview.preferences,
        activePreferences: overview.activePreferences,
        configuredTypeMemberships: overview.configuredTypeMemberships,
        planningDates: overview.planningDates,
        settings: {
          timeZone: overview.settings.timeZone,
          locale: overview.settings.locale,
          units: overview.settings.units,
          horizon: overview.settings.horizon,
          gapPolicy: overview.settings.gapPolicy,
          overlapPolicy: overview.settings.overlapPolicy,
        },
      },
      nativeOperation: {
        request: "scenario_get_view",
        view: "official.workforce.setup.overview",
        requestSubmitted: true,
        requestToSettlementMs: operationRequestToSettlementMs,
        settlementMs: operationRequestToSettlementMs,
        renderMs: null,
        backendOnlyMs: overviewTiming.backendOnlyMs,
        fullWindowBackendOnlyMs: workTiming.backendOnlyMs,
        backendOnlyBoundary:
          "Native buildingView to preparingResponse progress timestamps; excludes admission, response encoding, IPC and webview rendering.",
        backendOnlyUnavailableReason:
          overviewTiming.backendOnlyMs === null
            ? "The native buildingView/preparingResponse phase pair was not observed."
            : null,
      },
    };
  } catch (failure) {
    if (strict) throw failure;
    return {
      status: "unavailable",
      reason: "The native setup shape or operation settlement was not instrumented.",
      nativeOperation: {
        requestSubmitted: false,
        requestToSettlementMs: null,
        settlementMs: null,
        renderMs: null,
        backendOnlyMs: null,
        fullWindowBackendOnlyMs: null,
        backendOnlyUnavailableReason: "The native setup view and phase pair were not measured.",
      },
    };
  }
}

function package10LocalDate(instant, timeZone) {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone,
    calendar: "iso8601",
    numberingSystem: "latn",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(new Date(instant));
  const value = Object.fromEntries(parts.map((part) => [part.type, part.value]));
  return `${value.year}-${value.month}-${value.day}`;
}

async function package10CreateFixture(sessionId, source) {
  const { fixture } = source;
  const { settings } = fixture;
  const firstDate = package10LocalDate(settings.horizon.start, settings.timeZone);
  const lastExclusive = package10LocalDate(settings.horizon.end, settings.timeZone);
  const lastDate = new Date(`${lastExclusive}T00:00:00Z`);
  lastDate.setUTCDate(lastDate.getUTCDate() - 1);
  const created = await nativeRequest(sessionId, "project_create", {
    schemaVersion: 1,
    title: `Synthetic profile ${source.id}`,
    description: "Disposable versioned corpus UI profile",
    domainPack: { id: fixture.domain.packId, schemaVersion: fixture.domain.schemaVersion },
    settings: {
      timeZone: settings.timeZone,
      locale: settings.locale,
      units: settings.units,
      firstDate,
      lastDate: lastDate.toISOString().slice(0, 10),
      gapPolicy: settings.gapPolicy,
      overlapPolicy: settings.overlapPolicy,
    },
  });
  const scenarioId = created.result.scenarioId;
  const entities = package10Records(fixture.domain.payload.entities);
  const orderedKinds = [
    "qualification",
    "workloadBucket",
    "location",
    "calendar",
    "assignmentType",
    "person",
    "availability",
    "shiftTemplate",
    "shiftInstance",
    "baseSchedule",
    "scorePolicy",
  ];
  const commands = [
    ...entities
      .sort((left, right) => orderedKinds.indexOf(left.kind) - orderedKinds.indexOf(right.kind))
      .map((entity) => ({
        type: "applyDomainCommand",
        payload: { commandType: "official.workforce.add_entity", payload: { entity } },
      })),
    ...package10Records(fixture.domain.payload.rules).map((rule) => ({
      type: "applyDomainCommand",
      payload: { commandType: "official.workforce.add_rule", payload: { rule } },
    })),
  ];
  assert.equal(package10Records(fixture.domain.payload.preferences).length, 0);
  assert.equal(package10Records(fixture.domain.payload.lockedAssignments).length, 0);
  assert.equal(package10Records(fixture.domain.payload.extensions).length, 0);
  const started = Date.now();
  await nativeRequest(
    sessionId,
    "scenario_apply_command",
    {
      scenarioId,
      expectedRevision: created.result.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Disposable corpus UI profile" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: { label: `Profile ${source.id} source records`, commands },
      },
    },
    120_000,
  ).catch(async (failure) => {
    await package10DeleteFixture(sessionId, scenarioId);
    throw failure;
  });
  return { scenarioId, applyRequestToSettlementMs: Date.now() - started };
}

async function package10DeleteFixture(sessionId, scenarioId) {
  const current = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  if (current)
    await nativeRequest(sessionId, "project_delete", {
      scenarioId,
      expectedRevision: current.revision,
    });
}

async function package10MeasureWorkRender(sessionId, scenarioId, endDateExclusive, expectedRows) {
  const route = await command(
    "POST",
    `/session/${encodeURIComponent(sessionId)}/execute/async`,
    {
      script: `const done = arguments[arguments.length - 1];
        const started = performance.now();
        let finished = false;
        const section = '[aria-labelledby="work-window-heading"]';
        const ready = () => {
          const window = document.querySelector(section);
          return window !== null && window.offsetParent !== null && window.querySelector('nav.action-row') !== null;
        };
        const rows = () => document.querySelectorAll(section + ' tbody tr').length;
        let requested = false;
        const stop = (result) => {
          if (finished) return;
          finished = true;
          observer.disconnect();
          clearTimeout(timeout);
          done(result);
        };
        const check = () => {
          const switcher = [...document.querySelectorAll('[role="group"][aria-label="Work setup sections"] button')]
            .find(button => button.textContent.trim() === 'Shifts starting in these dates');
          if (!switcher) return;
          if (!requested && switcher.getAttribute('aria-pressed') !== 'true') {
            requested = true;
            switcher.click();
            requestAnimationFrame(check);
            return;
          }
          if (!ready()) return;
          requestAnimationFrame(() => requestAnimationFrame(() =>
            stop({
              status: 'measured',
              actionToDoubleAnimationFrameMs: Number((performance.now() - started).toFixed(3)),
              renderedRows: rows(),
              boundary: 'in-page Work navigation and section selection to visible native shift rows at a double-requestAnimationFrame boundary'
            })
          ));
        };
        const observer = new MutationObserver(check);
        observer.observe(document.body, {childList: true, subtree: true});
        const timeout = setTimeout(() => stop({status: 'unavailable', reason: 'initial native Work window did not render within 90 seconds'}), 90_000);
        window.location.hash = arguments[0];
        check();`,
      args: [`#/project/${scenarioId}/work`],
    },
    120_000,
  );
  if (route.status !== "measured") return { status: "unavailable", initialRoute: route };
  const initialEnd = await evaluate(
    sessionId,
    "return document.querySelector('#work-window-end')?.value;",
  );
  if (initialEnd === endDateExclusive) {
    assert.equal(route.renderedRows, expectedRows);
    return { status: "measured", initialRoute: route, fullWindow: route };
  }
  const selectedEnd = await evaluate(
    sessionId,
    "const field = document.querySelector('#work-window-end'); field.value = arguments[0]; field.dispatchEvent(new Event('input', {bubbles: true})); return field.value;",
    [endDateExclusive],
  );
  assert.equal(selectedEnd, endDateExclusive, "The full-horizon local end date must be selected");
  const settledEnd = await evaluate(
    sessionId,
    "return new Promise(resolve => requestAnimationFrame(() => resolve(document.querySelector('#work-window-end')?.value)));",
  );
  assert.equal(settledEnd, endDateExclusive, "Vue must retain the selected local end date");
  const fullWindow = await command(
    "POST",
    `/session/${encodeURIComponent(sessionId)}/execute/async`,
    {
      script: `const done = arguments[arguments.length - 1];
        const section = '[aria-labelledby="work-window-heading"]';
        const nav = () => document.querySelector(section + ' nav.action-row');
        const rows = () => document.querySelectorAll(section + ' tbody tr').length;
        const button = document.querySelector('#work-window-end')?.closest('form')?.querySelector('button[type="submit"]');
        if (!button || button.disabled) { done({status: 'unavailable', reason: 'full-window action unavailable'}); return; }
        let finished = false;
        let cleared = false;
        const started = performance.now();
        const stop = (result) => {
          if (finished) return;
          finished = true;
          observer.disconnect();
          clearTimeout(timeout);
          done(result);
        };
        const check = () => {
          if (!nav()) cleared = true;
          if (!cleared || rows() < arguments[0]) return;
          requestAnimationFrame(() => requestAnimationFrame(() =>
            stop({
              status: 'measured',
              actionToDoubleAnimationFrameMs: Number((performance.now() - started).toFixed(3)),
              renderedRows: rows(),
              boundary: 'in-page full-horizon Work query to native rows at a double-requestAnimationFrame boundary'
            })
          ));
        };
        const observer = new MutationObserver(check);
        observer.observe(document.body, {childList: true, subtree: true});
        const timeout = setTimeout(() => stop({status: 'unavailable', reason: 'full-horizon native Work rows did not render within 90 seconds'}), 90_000);
        button.click();
        check();`,
      args: [expectedRows],
    },
    120_000,
  );
  return { status: fullWindow.status, initialRoute: route, fullWindow };
}

async function package10StartPageInstrumentation(sessionId) {
  await evaluate(
    sessionId,
    `const maxSamples = 32;
    const maxLongTasks = 128;
    const maxLiveRegionMutations = 1024;
    const state = {
      samples: [],
      sampleOverflow: false,
      pending: Object.create(null),
      nextToken: 0,
      liveRegionMutationCount: 0,
      liveRegionMutationOverflow: false,
      longTaskEntries: [],
      longTaskEntryOverflow: false,
      longTaskSupported: false,
      longTaskUnsupportedReason: null,
      eventLoopLagMs: [],
      eventLoopLagOverflow: false
    };
    const isLiveRegion = (node) => {
      let current = node?.nodeType === 1 ? node : node?.parentElement;
      while (current) {
        if (current.matches?.('[aria-live], [role="alert"], [role="status"]')) return true;
        current = current.parentElement;
      }
      return false;
    };
    const liveObserver = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        if (!isLiveRegion(mutation.target)) continue;
        if (state.liveRegionMutationCount < maxLiveRegionMutations)
          state.liveRegionMutationCount += 1;
        else state.liveRegionMutationOverflow = true;
      }
    });
    liveObserver.observe(document.documentElement, {
      childList: true,
      subtree: true,
      characterData: true,
      attributes: true,
      attributeFilter: ['aria-live', 'role']
    });
    let longTaskObserver = null;
    try {
      if (typeof PerformanceObserver !== 'function')
        throw new Error('PerformanceObserver unavailable');
      longTaskObserver = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          if (state.longTaskEntries.length < maxLongTasks) {
            state.longTaskEntries.push({
              startTimeMs: Number(entry.startTime.toFixed(3)),
              durationMs: Number(entry.duration.toFixed(3))
            });
          } else state.longTaskEntryOverflow = true;
        }
      });
      longTaskObserver.observe({ entryTypes: ['longtask'], buffered: true });
      state.longTaskSupported = true;
    } catch {
      state.longTaskUnsupportedReason = 'WebKit did not expose PerformanceObserver longtask entries';
      longTaskObserver?.disconnect();
      longTaskObserver = null;
    }
    if (!state.longTaskSupported) {
      let expected = performance.now() + 100;
      const timer = setInterval(() => {
        const now = performance.now();
        const lag = Math.max(0, now - expected);
        if (state.eventLoopLagMs.length < maxSamples)
          state.eventLoopLagMs.push(Number(lag.toFixed(3)));
        else state.eventLoopLagOverflow = true;
        expected = now + 100;
      }, 100);
      state.eventLoopLagTimer = timer;
    }
    window.__euthetoPackage10Arm = (kind, selector, temperature) => {
      const target = document.querySelector(selector);
      if (!target) return { status: 'unavailable', reason: 'measurement target unavailable' };
      const token = String(++state.nextToken);
      const eventType = kind === 'keyboard' ? 'keydown' : kind === 'input' ? 'input' : 'scroll';
      const pending = { status: 'pending' };
      state.pending[token] = pending;
      const listener = () => {
        target.removeEventListener(eventType, listener, true);
        const eventAt = performance.now();
        requestAnimationFrame(() => requestAnimationFrame(() => {
          const sample = {
            kind,
            temperature,
            eventToDoubleAnimationFrameMs: Number((performance.now() - eventAt).toFixed(3)),
            samplingBoundary: 'double-requestAnimationFrame proxy, not a browser paint timestamp'
          };
          pending.status = 'done';
          pending.sample = sample;
          if (state.samples.length < maxSamples) state.samples.push(sample);
          else state.sampleOverflow = true;
        }));
      };
      target.addEventListener(eventType, listener, true);
      pending.cancel = () => target.removeEventListener(eventType, listener, true);
      return { status: 'armed', token };
    };
    window.__euthetoPackage10ReadSample = (token) => state.pending[token] ?? null;
    window.__euthetoPackage10CancelSample = (token) => {
      const pending = state.pending[token];
      pending?.cancel?.();
      if (pending?.status === 'pending') pending.status = 'unavailable';
    };
    window.__euthetoPackage10ArmResult = (selector, expected, metadata) => {
      const target = document.querySelector(selector);
      if (!target) throw new Error('Accepted-result keyboard target is missing');
      const token = String(++state.nextToken);
      const pending = { status: 'pending' };
      state.pending[token] = pending;
      let observer;
      let timer;
      let started;
      let framing = false;
      const ready = () => {
        const root = document.querySelector('[data-accepted-schedule]');
        const table = [...(root?.querySelectorAll('table') ?? [])].find(t =>
          t.caption?.textContent.trim() === expected.caption);
        const first = table?.tBodies[0]?.rows[0];
        const rows = table?.tBodies[0]?.rows.length ?? 0;
        const diagnostic = {
          scheduleErrorClass: !root?.querySelector(':scope > [role="alert"]')
            ? "none"
            : root.querySelector(':scope > [role="alert"]').textContent.includes(
                  "The accepted result or saved revision changed. Reopen the schedule.",
                )
              ? "accepted-result-source-guard"
              : root.querySelector(':scope > [role="alert"]').textContent.includes(
                    "The local project library could not complete that request.",
                  )
                ? "safe-message-fallback"
                : "other-application-error",
          scheduleErrorTextLength:
            root?.querySelector(':scope > [role="alert"]')?.textContent.length ?? 0,
          scheduleErrorMessage: root?.querySelector(':scope > [role="alert"]')?.textContent
            .trim().slice(0, 256) ?? null,
          eventObserved: started !== undefined,
          schedulePresent: !!root,
          scheduleBusy: !!root?.querySelector(':scope > [aria-busy="true"]'),
          scheduleError: !!root?.querySelector(':scope > [role="alert"]'),
          requestedViewChecked: !!root?.querySelector('input[value="' + expected.kind + '"]')?.checked,
          pageCountMatches: [...(root?.querySelectorAll(':scope > p[role="status"]') ?? [])].some(p =>
            p.textContent.trim() === expected.totalItems + ' saved rows · page ' + expected.pageNumber),
          expectedTablePresent: !!table,
          expectedFirstLabelMatches: !!first?.textContent.includes(expected.firstLabel),
          renderedRows: rows,
          expectedReturnedItems: expected.returnedItems,
          calendarSourceRowsMatch: !expected.calendarMode ||
            Number(table?.getAttribute('aria-rowcount')) === expected.returnedItems + 1,
          calendarDateMatches: !expected.calendarMode ||
            !!table?.querySelector('thead')?.textContent.includes(expected.startDate),
          calendarModeMatches: !expected.calendarMode ||
            table?.classList.contains('virtual') === (expected.calendarMode === 'virtual'),
          calendarFirstIndexMatches: !expected.calendarMode ||
            Number(first?.dataset.index) === expected.firstIndex
        };
        pending.readiness = diagnostic;
        if (!diagnostic.schedulePresent || diagnostic.scheduleBusy || diagnostic.scheduleError ||
            !diagnostic.requestedViewChecked || !diagnostic.pageCountMatches) return false;
        if (expected.returnedItems === 0) return !table;
        if (!diagnostic.expectedTablePresent || !diagnostic.expectedFirstLabelMatches || !rows)
          return false;
        if (expected.calendarMode) {
          if (!diagnostic.calendarSourceRowsMatch || !diagnostic.calendarDateMatches ||
              !diagnostic.calendarModeMatches || !diagnostic.calendarFirstIndexMatches) return false;
          return expected.calendarMode === 'virtual'
            ? rows <= expected.returnedItems && (expected.returnedItems <= 8 || rows < expected.returnedItems)
            : rows === Math.min(8, expected.returnedItems - expected.firstIndex);
        }
        return rows === expected.returnedItems;
      };
      const stop = () => {
        observer?.disconnect();
        clearTimeout(timer);
        target.removeEventListener('keydown', listener, true);
      };
      const check = () => {
        if (framing || !ready()) return;
        framing = true;
        requestAnimationFrame(() => requestAnimationFrame(() => {
          framing = false;
          if (pending.status !== 'pending') return;
          if (!ready()) { check(); return; }
          const root = document.querySelector('[data-accepted-schedule]');
          const sample = {
            kind: 'accepted-result-keyboard',
            ...metadata,
            eventToSettledContentDoubleAnimationFrameMs: Number((performance.now() - started).toFixed(3)),
            samplingBoundary: 'keyboard keydown to expected settled native content plus double-requestAnimationFrame; not INP or paint',
            renderedRows: root.querySelectorAll('table tbody tr').length,
            renderedGridCells: root.querySelectorAll('[role="gridcell"]').length,
            retainedScheduleElementNodes: root.querySelectorAll('*').length + 1
          };
          stop();
          pending.status = 'done';
          pending.sample = sample;
          if (state.samples.length < maxSamples) state.samples.push(sample);
          else state.sampleOverflow = true;
        }));
      };
      const listener = (event) => {
        if (!['Enter', 'ArrowUp', 'ArrowDown'].includes(event.key)) return;
        target.removeEventListener('keydown', listener, true);
        started = performance.now();
        observer = new MutationObserver(check);
        observer.observe(document.querySelector('[data-accepted-schedule]'), {
          childList: true, subtree: true, characterData: true, attributes: true
        });
        requestAnimationFrame(check);
      };
      target.addEventListener('keydown', listener, true);
      timer = setTimeout(() => {
        ready();
        stop();
        pending.status = 'unavailable';
      }, 15_000);
      pending.cancel = stop;
      return { status: 'armed', token };
    };
    window.__euthetoPackage10Finish = () => {
      liveObserver.disconnect();
      for (const token of Object.keys(state.pending)) window.__euthetoPackage10CancelSample(token);
      longTaskObserver?.disconnect();
      if (state.eventLoopLagTimer !== undefined) clearInterval(state.eventLoopLagTimer);
      return {
        schemaVersion: 1,
        samples: state.samples,
        sampleOverflow: state.sampleOverflow,
        liveRegionMutationCount: state.liveRegionMutationCount,
        liveRegionMutationOverflow: state.liveRegionMutationOverflow,
        longTask: {
          supported: state.longTaskSupported,
          unsupportedReason: state.longTaskUnsupportedReason,
          entryCount: state.longTaskEntries.length,
          entries: state.longTaskEntries,
          entryOverflow: state.longTaskEntryOverflow
        },
        eventLoopLag: state.longTaskSupported
          ? { equivalentToLongTask: false, status: 'not-collected' }
          : {
              equivalentToLongTask: false,
              status: 'NON-equivalent',
              samplesMs: state.eventLoopLagMs,
              sampleOverflow: state.eventLoopLagOverflow
            }
      };
    };`,
  );
}

async function package10AwaitSample(sessionId, token) {
  return waitFor(
    sessionId,
    `const value = window.__euthetoPackage10ReadSample(arguments[0]);
    return value?.status === 'done' || value?.status === 'unavailable' ? value : false;`,
    [token],
  );
}

async function package10InteractionSample(sessionId, kind, selector, temperature, trigger) {
  try {
    await waitForElement(sessionId, selector);
    const armed = await evaluate(
      sessionId,
      "return window.__euthetoPackage10Arm(arguments[0], arguments[1], arguments[2]);",
      [kind, selector, temperature],
    );
    if (armed?.status !== "armed")
      return { status: "unavailable", reason: armed?.reason ?? "measurement could not be armed" };
    await trigger();
    const result = await package10AwaitSample(sessionId, armed.token);
    return result.status === "done"
      ? { status: "measured", ...result.sample }
      : {
          status: "unavailable",
          reason: "event did not reach the double-animation-frame boundary",
        };
  } catch (failure) {
    return {
      status: "unavailable",
      reason: failure instanceof Error ? failure.message : "Event timing sample did not settle",
    };
  }
}

async function package10MatrixKeyboardSample(sessionId, temperature) {
  const selector = '[data-matrix-scroll] input[type="checkbox"]';
  return package10InteractionSample(sessionId, "keyboard", selector, temperature, async () => {
    const previous = await evaluate(
      sessionId,
      "const cell = document.querySelector(arguments[0]); cell.focus(); if (document.activeElement !== cell) throw new Error('Matrix cell did not receive focus'); return cell.id;",
      [selector],
    );
    await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
      actions: [
        {
          type: "key",
          id: `package10-${temperature}-matrix-keyboard`,
          actions: [
            { type: "keyDown", value: "\uE015" },
            { type: "keyUp", value: "\uE015" },
          ],
        },
      ],
    });
    await waitFor(
      sessionId,
      "return document.activeElement?.matches(arguments[0]) && document.activeElement.id !== arguments[1];",
      [selector, previous],
    );
  });
}

async function package10MeasureInteractions(sessionId, expected, temperature) {
  const peopleSearch = "#eligibility-people-search";
  await setValue(sessionId, peopleSearch, "");
  const input = await package10InteractionSample(
    sessionId,
    "input",
    peopleSearch,
    temperature,
    async () => {
      const id = await findElement(sessionId, peopleSearch);
      await command(
        "POST",
        `/session/${encodeURIComponent(sessionId)}/element/${encodeURIComponent(id)}/value`,
        { text: "p10", value: Array.from("p10") },
      );
    },
  );
  await setValue(sessionId, peopleSearch, "Matrix person");
  await setValue(sessionId, "#eligibility-type-search", "Matrix type");
  await activateButton(sessionId, "Find people and work types");
  await waitFor(
    sessionId,
    "const text = document.querySelector('[data-eligibility-matrix]')?.textContent ?? ''; return text.includes(arguments[0]) && text.includes(arguments[1]);",
    [
      `${String(expected.seededPeopleCount)} of ${String(expected.seededPeopleCount)} people`,
      `${String(expected.seededTypeCount)} of ${String(expected.seededTypeCount)} work types`,
    ],
  );
  const keyboard = await package10MatrixKeyboardSample(sessionId, temperature);
  const scroll = await package10InteractionSample(
    sessionId,
    "scroll",
    "[data-matrix-scroll]",
    temperature,
    async () => {
      const changed = await evaluate(
        sessionId,
        `const element = document.querySelector('[data-matrix-scroll]');
        if (!element) return { changed: false };
        const maximum = Math.max(0, element.scrollHeight - element.clientHeight);
        if (maximum === 0) return { changed: false };
        const before = element.scrollTop;
        const target = before > 0 ? 0 : Math.min(maximum, 240);
        element.scrollTop = target;
        return { changed: before !== target };`,
      );
      if (!changed?.changed) throw new Error("The matrix scroll axis did not move");
    },
  );
  await setValue(sessionId, peopleSearch, "");
  await setValue(sessionId, "#eligibility-type-search", "");
  return {
    axisShape: {
      people: expected.seededPeopleCount,
      assignmentTypes: expected.seededTypeCount,
      matrix: `${String(expected.seededPeopleCount)}x${String(expected.seededTypeCount)}`,
    },
    keyboard,
    input,
    scroll,
  };
}

async function package10NativeCancellation(sessionId, scenarioId) {
  let operationId;
  try {
    const current = (await projects(sessionId)).find(
      (project) => project.scenarioId === scenarioId,
    );
    assert(current, "The native cancellation scenario must exist");
    const prepared = await nativeRequest(sessionId, "operation_prepare", {
      schemaVersion: 1,
      purpose: { kind: "fullValidation" },
      context: { kind: "scenario", scenarioId, expectedRevision: current.revision },
    });
    operationId = prepared.result.operationId;
    const validationRequest = {
      schemaVersion: 3,
      requestId: requestId(),
      operationId,
      scenarioId,
      expectedRevision: current.revision,
    };
    const validationStarted = Date.now();
    await evaluate(
      sessionId,
      `const request = arguments[0];
      const ipc = window.__TAURI_INTERNALS__;
      const channel = ipc.transformCallback(() => {});
      const state = { status: 'pending' };
      window.__euthetoPackage10NativeValidation = state;
      ipc.invoke('scenario_validate', {
        request,
        onProgress: '__CHANNEL__:' + channel
      }).then(
        () => { state.status = 'fulfilled'; },
        () => { state.status = 'rejected'; }
      ).finally(() => ipc.unregisterCallback(channel));
      return true;`,
      [validationRequest],
    );
    await sleep(10);
    const cancelStarted = Date.now();
    const cancellation = await nativeRequest(sessionId, "operation_cancel", {
      schemaVersion: 1,
      operationId,
    });
    const cancelAcknowledged = Date.now();
    const settled = await waitFor(
      sessionId,
      "const value = window.__euthetoPackage10NativeValidation; return value && value.status !== 'pending' ? {status: value.status} : false;",
      [],
      120_000,
    );
    const operationSettled = Date.now();
    return {
      status: "measured",
      requestSubmitted: true,
      acknowledgement: cancellation.result.acknowledgement,
      requestToAcknowledgementMs: cancelAcknowledged - cancelStarted,
      acknowledgementToSettlementMs: operationSettled - cancelAcknowledged,
      requestToSettlementMs: operationSettled - validationStarted,
      settlementMs: operationSettled - cancelAcknowledged,
      renderMs: null,
      backendOnlyMs: null,
      backendOnlyUnavailableReason:
        "The native command contract exposes operation settlement, not backend-only timing.",
      settlement: settled.status,
    };
  } catch {
    return {
      status: "unavailable",
      requestSubmitted: false,
      reason: "Native cancellation request and settlement were not instrumented.",
      acknowledgement: null,
      requestToAcknowledgementMs: null,
      acknowledgementToSettlementMs: null,
      requestToSettlementMs: null,
      settlementMs: null,
      renderMs: null,
      backendOnlyMs: null,
      backendOnlyUnavailableReason:
        "The native command contract exposes operation settlement, not backend-only timing.",
      settlement: null,
    };
  } finally {
    if (operationId !== undefined) {
      try {
        await nativeRequest(sessionId, "operation_release", {
          schemaVersion: 1,
          operationId,
        });
      } catch {
        /* The native operation may already have retired after settlement. */
      }
    }
  }
}

async function package10OpenLibrary(sessionId) {
  await idle(sessionId, 120_000);
  await evaluate(sessionId, "window.location.hash = '#/projects';");
  const result = await waitFor(
    sessionId,
    "return document.getElementById('projects-heading') ? 'ready' : document.querySelector('[role=\"dialog\"] .dialog-actions button:last-child:not(:disabled)') ?? false;",
    [],
    30_000,
  );
  if (result !== "ready") await activateElement(sessionId, result);
  await waitForElement(sessionId, "#projects-heading", 120_000);
}

async function package10ProfileSource(sessionId, source) {
  let created;
  let profileFailure;
  try {
    created = await package10CreateFixture(sessionId, source);
    const native = await package10NativeScenarioShape(sessionId, created.scenarioId, true);
    assert.notEqual(
      native.nativeOperation.backendOnlyMs,
      null,
      `${source.id} native overview backend phase must be observed`,
    );
    assert.notEqual(
      native.nativeOperation.fullWindowBackendOnlyMs,
      null,
      `${source.id} native Work backend phase must be observed`,
    );
    const actualCounts = Object.fromEntries(
      Object.entries(native.shape.entityCounts).filter(([, count]) => count > 0),
    );
    assert.deepEqual(actualCounts, source.shape.entityCounts, `${source.id} native entity shape`);
    assert.deepEqual(native.shape.settings, source.shape.settings, `${source.id} native settings`);
    assert.equal(native.shape.requiredRules, source.shape.rules.total);
    assert.equal(
      native.shape.activeRequiredRules,
      package10Records(source.fixture.domain.payload.rules).filter((rule) => rule.active).length,
    );
    assert.equal(native.shape.preferences, 0);
    assert.equal(native.shape.resolvedShiftCount, source.shape.resolvedShiftCount);
    const nativeRules = [];
    for (const rule of package10Records(source.fixture.domain.payload.rules)) {
      const saved = (await package8Rule(sessionId, created.scenarioId, rule.id)).record;
      assert.deepEqual(saved, rule, `${source.id} authored rule ${rule.id}`);
      nativeRules.push(saved);
    }
    assert.deepEqual(package10Counts(nativeRules, "kind"), source.shape.ruleCounts);

    await package10OpenLibrary(sessionId);
    await waitFor(
      sessionId,
      "return [...document.querySelectorAll('.project-list a')].some(link => link.getAttribute('href')?.includes(arguments[0]));",
      [created.scenarioId],
      120_000,
    );
    await package10StartPageInstrumentation(sessionId);
    const render = await package10MeasureWorkRender(
      sessionId,
      created.scenarioId,
      package10LocalDate(source.fixture.settings.horizon.end, source.fixture.settings.timeZone),
      source.shape.resolvedShiftCount,
    );
    assert.equal(
      render.status,
      "measured",
      `${source.id} native Work route render: ${JSON.stringify(render)}`,
    );
    assert.equal(render.fullWindow.renderedRows, source.shape.resolvedShiftCount);
    await idle(sessionId);
    const inPage = await evaluate(sessionId, "return window.__euthetoPackage10Finish();");
    return {
      caseId: source.id,
      source: source.input,
      fixtureShape: source.shape,
      comparison: {
        kind: "sourcePayloadAppliedThroughNativeCommands",
        byteIdenticalScenario: false,
        semanticSettingsCountsAndResolvedShiftsMatch: true,
        authoredRequiredRulesMatch: true,
        nativeRuleCounts: package10Counts(nativeRules, "kind"),
        difference:
          "A disposable project has a new scenario ID, metadata, revision and command history.",
        nativeScenarioShape: native.shape,
      },
      inPage: {
        status: "measured",
        workActionToDoubleAnimationFrame: render,
        liveRegion: {
          mutationCount: inPage.liveRegionMutationCount,
          mutationOverflow: inPage.liveRegionMutationOverflow,
          interpretation: "announcement proxy; mutation count is not screen-reader output",
        },
        longTask: inPage.longTask,
        eventLoopLag: inPage.eventLoopLag,
      },
      nativeOperation: {
        status: "measured",
        batchApplyRequestToSettlementMs: created.applyRequestToSettlementMs,
        overviewRequestToSettlementMs: native.nativeOperation.requestToSettlementMs,
        backendOnlyMs: native.nativeOperation.backendOnlyMs,
        fullWindowBackendOnlyMs: native.nativeOperation.fullWindowBackendOnlyMs,
        backendOnlyBoundary: native.nativeOperation.backendOnlyBoundary,
        backendOnlyUnavailableReason: native.nativeOperation.backendOnlyUnavailableReason,
      },
    };
  } catch (failure) {
    profileFailure = failure;
    throw failure;
  } finally {
    if (created !== undefined) {
      try {
        await package10OpenLibrary(sessionId);
        await package10DeleteFixture(sessionId, created.scenarioId);
      } catch (cleanupFailure) {
        if (profileFailure === undefined) throw cleanupFailure;
        console.error(`Disposable ${source.id} cleanup failed:`, cleanupFailure);
      }
    }
  }
}

async function package10PerformanceAcceptance(sessionId, scenarioId, package7Expected) {
  const sources = await package10FixtureEvidence();
  const nativeScenario = await package10NativeScenarioShape(sessionId, scenarioId);
  const cancellation = await package10NativeCancellation(sessionId, scenarioId);
  await navigate(sessionId, `/project/${scenarioId}/eligibility`);
  await waitForElement(sessionId, "#eligibility-setup-heading");
  await package10StartPageInstrumentation(sessionId);
  const cold = await package10MeasureInteractions(sessionId, package7Expected, "cold");
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  await navigate(sessionId, `/project/${scenarioId}/eligibility`);
  await waitForElement(sessionId, "#eligibility-setup-heading");
  await idle(sessionId);
  const warm = await package10MeasureInteractions(sessionId, package7Expected, "warm");
  const inPage = await evaluate(sessionId, "return window.__euthetoPackage10Finish();");
  const interactionSamples = [cold, warm].flatMap((visit) =>
    ["keyboard", "input", "scroll"].map((kind) => visit[kind]),
  );
  const recordedSamples = interactionSamples.filter((sample) => sample.status === "measured");
  const measuredSamples = recordedSamples.length;
  const inPageStatus =
    measuredSamples === interactionSamples.length
      ? "measured"
      : measuredSamples === 0
        ? "unavailable"
        : "partial";
  const nativeShape =
    nativeScenario.status === "measured" ? nativeScenario.shape : { status: "unavailable" };
  const nativeBackend =
    typeof nativeScenario.nativeOperation?.backendOnlyMs === "number" &&
    typeof nativeScenario.nativeOperation.fullWindowBackendOnlyMs === "number"
      ? {
          status: "measured",
          overviewMs: nativeScenario.nativeOperation.backendOnlyMs,
          fullWindowMs: nativeScenario.nativeOperation.fullWindowBackendOnlyMs,
          boundary: nativeScenario.nativeOperation.backendOnlyBoundary,
        }
      : { status: "unavailable", reason: "The native setup phase pair was not observed." };
  const cases = [];
  for (const source of sources.cases) {
    cases.push(await package10ProfileSource(sessionId, source));
  }
  const matrixSource = sources.cases.find((source) => source.id === "large-supported");
  assert(matrixSource, "The large-supported source binding must exist");
  cases.push({
    caseId: "eligibility-matrix-100x64",
    source: matrixSource.input,
    fixtureShape: {
      sourceCaseId: "large-supported",
      people: package7Expected.seededPeopleCount,
      assignmentTypeCount: package7Expected.seededTypeCount,
      axisStress: "people-by-assignment-type",
    },
    comparison: {
      kind: "derivedAxisStress",
      exactFixtureClaim: false,
      nativeScenarioShape: nativeShape,
      gap: "This is the existing 100-person by 64-assignment-type eligibility matrix; it is not a 12-shift schedule claim.",
    },
    inPage: {
      status: inPageStatus,
      cold,
      warm,
      samples: recordedSamples,
      eventTimingMethod: {
        boundary: "double-requestAnimationFrame proxy, not a browser paint timestamp",
        inputAndScrollAreInPage: true,
        webDriverRoundTripExcluded: true,
      },
      liveRegion: {
        mutationCount: inPage.liveRegionMutationCount,
        mutationOverflow: inPage.liveRegionMutationOverflow,
        interpretation: "announcement proxy; mutation count is not screen-reader output",
      },
      longTask: inPage.longTask,
      eventLoopLag: inPage.eventLoopLag,
      sampleCount: recordedSamples.length,
      sampleOverflow: inPage.sampleOverflow,
    },
    nativeOperation: {
      status: nativeScenario.status,
      requestSubmitted: nativeScenario.nativeOperation?.requestSubmitted ?? false,
      requestToSettlementMs: nativeScenario.nativeOperation?.requestToSettlementMs ?? null,
      settlementMs: nativeScenario.nativeOperation?.settlementMs ?? null,
      cancel: cancellation,
      renderMs: null,
      backend: nativeBackend,
    },
  });
  await package10WritePerformanceEvidence(sessionId, sources, cases, {
    nativeScenario: { identity: "current native persistence scenario", ...nativeScenario },
    coldWarmMethod: {
      cold: "first measured eligibility visit in this native WebKit session",
      warm: "second eligibility route in the same native WebKit session after setup navigation and idle",
      cacheNote: "The method does not flush operating-system or WebKit caches.",
    },
  });
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  console.log(
    "PASS: bounded native Phase06 performance evidence recorded",
    JSON.stringify({
      cases: cases.length,
      measuredInPageSamples: recordedSamples.length,
      longTaskSupported: inPage.longTask.supported,
      liveRegionMutations: inPage.liveRegionMutationCount,
      nativeCancel: cancellation.status,
    }),
  );
}

async function package10WritePerformanceEvidence(sessionId, sources, cases, context = {}) {
  const path = fileURLToPath(
    new URL("../../../.cache/e2e/package10-performance-measurements.json", import.meta.url),
  );
  await mkdir(dirname(path), { recursive: true });
  const packaged = process.env.EUTHETO_E2E_PACKAGED_SOLVE === "1";
  const evidence = {
    schemaVersion: 1,
    measurement: {
      generatedAt: new Date().toISOString(),
      buildMode: packaged || application.includes("/release/") ? "release" : "debug",
      packaged,
      applicationSha256: package10Sha256(await readFile(application)),
      workerSha256: packaged
        ? package10Sha256(await readFile(join(dirname(application), "ortools-worker")))
        : null,
      host: {
        os: platform(),
        osRelease: release(),
        arch: process.arch,
        cpu: cpus()[0]?.model ?? "unknown",
        webKitUserAgent: await evaluate(sessionId, "return navigator.userAgent;"),
      },
      ...(context.coldWarmMethod ? { coldWarmMethod: context.coldWarmMethod } : {}),
      bounded: {
        maximumInteractionSamples: 32,
        maximumLongTaskEntries: 128,
        maximumLiveRegionMutations: 1024,
      },
    },
    source: sources.manifest,
    ...(context.nativeScenario ? { nativeScenario: context.nativeScenario } : {}),
    cases,
  };
  await writeFile(path, `${JSON.stringify(evidence, null, 2)}\n`);
}

async function packagedLargeResultPerformanceAcceptance(sessionId) {
  assert.equal(
    process.env.EUTHETO_E2E_PACKAGED_SOLVE,
    "1",
    "Large accepted-result evidence requires the actual bundled worker",
  );
  const sources = await package10FixtureEvidence();
  const source = sources.cases.find((entry) => entry.id === "large-supported");
  assert(source);
  assert.equal(source.shape.people, 101);
  assert.equal(source.shape.resolvedShiftCount, 12);
  assert.equal(source.shape.selectedAssignments, 12);
  const { scenarioId } = await package10CreateFixture(sessionId, source);
  let instrumentationStarted = false;
  try {
    const native = await package10NativeScenarioShape(sessionId, scenarioId, true);
    assert.deepEqual(native.shape.settings, source.shape.settings);
    assert.deepEqual(
      Object.fromEntries(
        Object.entries(native.shape.entityCounts).filter(([, count]) => count > 0),
      ),
      source.shape.entityCounts,
    );
    assert.equal(native.shape.resolvedShiftCount, 12);
    assert.equal(native.shape.requiredRules, source.shape.rules.total);
    for (const rule of package10Records(source.fixture.domain.payload.rules))
      assert.deepEqual((await package8Rule(sessionId, scenarioId, rule.id)).record, rule);
    const revision = native.revision;
    const prepared = await nativeRequest(sessionId, "operation_prepare", {
      schemaVersion: 1,
      purpose: { kind: "solveEstimate" },
      context: { kind: "scenario", scenarioId, expectedRevision: revision },
    });
    let estimate;
    try {
      estimate = (
        await nativeRequest(
          sessionId,
          "solve_estimate_model",
          {
            schemaVersion: 2,
            operationId: prepared.result.operationId,
            scenarioId,
            expectedRevision: revision,
          },
          120_000,
          true,
        )
      ).result;
    } finally {
      await nativeRequest(sessionId, "operation_release", {
        schemaVersion: 1,
        operationId: prepared.result.operationId,
      });
    }
    assert.equal(estimate.scenarioId, scenarioId);
    assert.equal(estimate.scenarioRevision, revision);
    assert.equal(estimate.currentRevision, revision);
    assert.equal(estimate.stale, false);
    assert.equal(estimate.compiled, true);
    assert.equal(estimate.solved, false);
    assert.equal(Number(estimate.candidateCount), 101 * 12);

    await package10OpenLibrary(sessionId);
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await waitForElement(sessionId, "#projects-heading");
    await activate(sessionId, '[aria-label="Open project Synthetic profile large-supported"]');
    await navigate(sessionId, `/project/${scenarioId}/validation`);
    await waitForElement(sessionId, "#validation-heading");
    await activateButton(sessionId, "Run full validation");
    await waitForElement(sessionId, '[data-full-validation-state="completed"]', 120_000);
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await waitForElement(sessionId, "#setup-calendar");
    if (
      !(await evaluate(sessionId, "return document.querySelector('#setup-results-details')?.open;"))
    )
      await activate(sessionId, "#setup-results-details > summary");
    await waitForElement(sessionId, '[data-optimize-state="ready"]');
    await activate(sessionId, '[data-optimize-panel] input[name="optimize-mode"][value="deep"]');
    await activateButton(sessionId, "Start optimization", "[data-optimize-panel]");
    await waitFor(
      sessionId,
      `return [...document.querySelectorAll('[data-optimize-panel] dt')].some(node =>
        node.textContent === 'Solve status' &&
        ['Optimal', 'Feasible', 'Infeasible', 'No solution within the mode limit',
         'Backend failed', 'Verification alarm'].includes(node.nextElementSibling?.textContent?.trim()));`,
      [],
      120_000,
    );
    const runs = (
      await nativeRequest(sessionId, "solve_list_runs", {
        schemaVersion: 2,
        scenarioId,
        revision,
        limit: 2,
      })
    ).result.runs;
    assert.equal(runs.length, 1);
    const [run] = runs;
    assert(["optimal", "feasible"].includes(run.status), `Large-supported solve: ${run.status}`);
    assert.equal(run.scenarioId, scenarioId);
    assert.equal(run.scenarioRevision, revision);
    assert.equal(run.stale, false);
    assert.match(run.solutionId, /^[a-f0-9-]{36}$/u);
    const diagnostics = (
      await nativeRequest(sessionId, "solve_get_diagnostics_summary", {
        schemaVersion: 2,
        scenarioId,
        runId: run.runId,
      })
    ).result;
    assert.equal(diagnostics.job.runId, run.runId);
    assert.equal(diagnostics.job.verificationChecksum, run.verificationChecksum);
    assert.match(diagnostics.manifestChecksum, /^[a-f0-9]{64}$/u);
    for (const value of [
      diagnostics.elapsedMilliseconds,
      diagnostics.firstIncumbentMilliseconds,
      diagnostics.firstVerifiedFeasibleMilliseconds,
    ])
      assert(Number.isSafeInteger(value) && value >= 0);
    assert(diagnostics.firstIncumbentMilliseconds <= diagnostics.firstVerifiedFeasibleMilliseconds);
    assert(diagnostics.firstVerifiedFeasibleMilliseconds <= diagnostics.elapsedMilliseconds);
    const verified = (
      await nativeRequest(sessionId, "solution_verify", {
        schemaVersion: 1,
        scenarioId,
        solutionId: run.solutionId,
      })
    ).result;
    assert.equal(verified.result.solutionId, run.solutionId);
    assert.equal(verified.scenarioRevision, revision);
    assert.equal(verified.verification.accepted, true);
    assert.equal(verified.verification.checksum, run.verificationChecksum);
    assert(verified.verification.requiredRuleResults.every((rule) => rule.satisfied));
    assert.match(verified.result.resultChecksum, /^[a-f0-9]{64}$/u);

    const getView = async (kind, parameters = {}, continuation = null) => {
      const result = (
        await nativeRequest(sessionId, "solution_get_view", {
          schemaVersion: 1,
          scenarioId,
          solutionId: run.solutionId,
          viewId: "official.workforce.result.schedule",
          parameters: { kind, limit: 50, ...parameters },
          continuation,
        })
      ).result;
      assert.equal(result.scenarioId, scenarioId);
      assert.equal(result.scenarioRevision, revision);
      assert.equal(result.currentRevision, revision);
      assert.deepEqual(result.result, verified.result);
      assert.equal(result.view.viewId, "official.workforce.result.schedule");
      if (!isWorkforceResultScheduleViewData(result.view.data)) {
        const shape = [];
        const describeShape = (value, path = "view.data", depth = 0) => {
          if (shape.length >= 128 || depth >= 8) return;
          if (Array.isArray(value)) {
            shape.push(`${path}:array(${value.length})`);
            if (value.length > 0) describeShape(value[0], `${path}[0]`, depth + 1);
          } else if (value !== null && typeof value === "object") {
            const keys = Object.keys(value);
            shape.push(`${path}:object{${keys.join(",")}}`);
            for (const key of keys) describeShape(value[key], `${path}.${key}`, depth + 1);
          } else {
            shape.push(`${path}:${value === null ? "null" : typeof value}`);
          }
        };
        describeShape(result.view.data);
        assert.fail(
          `Native ${kind} result failed the generated Workforce view guard; field-path shape: ${shape.join("; ")}`,
        );
      }
      const page = result.view.data.result;
      assert.equal(page.kind, kind);
      assert(page.items.length <= 50);
      return page;
    };
    const people = [await getView("people")];
    assert.equal(people[0].totalItems, 101);
    for (let index = 1; index < 3; index++) {
      assert(people[index - 1].continuation);
      people.push(await getView("people", {}, people[index - 1].continuation));
    }
    assert.deepEqual(
      people.map((page) => page.items.length),
      [50, 50, 1],
    );
    assert.equal(people[2].continuation, null);
    const timeline = await getView("timeline");
    const shifts = await getView("shifts");
    const health = await getView("health");
    const calendar = await getView("calendarPeople");
    const shiftCalendar = await getView("calendarShifts");
    assert.equal(timeline.totalItems, 12);
    assert.equal(timeline.items.length, 12);
    assert.equal(shifts.totalItems, 12);
    assert.equal(shifts.items.length, 12);
    assert.equal(
      shifts.items.reduce((sum, shift) => sum + shift.selectedCount, 0),
      12,
    );
    assert.equal(calendar.totalItems, 101);
    assert.equal(calendar.items.length, 50);
    assert(health.items.length > 0);
    const nextCalendar = await getView("calendarPeople", {
      startDate: calendar.window.endDateExclusive,
    });
    assert.equal(nextCalendar.window.startDate, calendar.window.endDateExclusive);

    await activateButton(sessionId, "Inspect schedule", "#setup-results-details");
    await waitForElement(sessionId, "[data-accepted-schedule] table tbody tr", 30_000);
    await waitFor(
      sessionId,
      `return document.querySelector(
      '[data-accepted-schedule] section[aria-label="Independent result check"]'
    )?.textContent.includes('Verified schedule ready. Every recorded Required rule passed.');`,
    );
    await package10StartPageInstrumentation(sessionId);
    instrumentationStarted = true;
    const root = "[data-accepted-schedule]";
    const captions = {
      people: "By person",
      shifts: "By shift",
      timeline: "Timeline by start time",
      health: "Plan health",
      calendarPeople: "Person calendar",
      calendarShifts: "Shift and location calendar",
    };
    const sample = async (
      page,
      temperature,
      action,
      target,
      pageNumber = 1,
      mode = null,
      firstIndex = 0,
    ) => {
      const row = page.items[firstIndex];
      // Labels are transient readiness assertions only; never copied into evidence.
      const firstLabel =
        row?.person?.name ??
        row?.name ??
        row?.personName ??
        row?.sourceRuleId ??
        row?.coverageOwnerId ??
        row?.assignmentTypeName ??
        row?.assignmentTypeId ??
        row?.shift?.assignmentTypeName ??
        row?.shift?.assignmentTypeId ??
        "";
      const expected = {
        kind: page.kind,
        caption: captions[page.kind],
        pageNumber,
        totalItems: page.totalItems,
        returnedItems: page.items.length,
        firstLabel,
        calendarMode: mode,
        startDate: page.window?.startDate,
        firstIndex,
      };
      const metadata = {
        temperature,
        action,
        viewKind: page.kind,
        pageNumber,
        pageLimit: 50,
        nativeTotalItems: page.totalItems,
        nativeReturnedItems: page.items.length,
        calendarMode: mode,
        visibleRowPage: mode === "paged" ? firstIndex / 8 + 1 : null,
        filters: { personId: null, shiftId: null, locationId: null, localDate: null },
        ...(page.window
          ? { startDate: page.window.startDate, endDateExclusive: page.window.endDateExclusive }
          : {}),
      };
      const isRadio = target.includes('input[type="radio"]');
      const currentRadio = isRadio
        ? await evaluate(
            sessionId,
            "return document.querySelector('[data-accepted-schedule] input[type=\"radio\"]:checked')?.value ?? null;",
          )
        : null;
      const eventTarget = isRadio ? radio(currentRadio) : target;
      assert(
        currentRadio !== null || !isRadio,
        "Accepted-result view radio has no keyboard selection",
      );
      const armed = await evaluate(
        sessionId,
        "return window.__euthetoPackage10ArmResult(arguments[0], arguments[1], arguments[2]);",
        [eventTarget, expected, metadata],
      );
      const beforeNavigation = await evaluate(
        sessionId,
        `return {activeTag: document.activeElement?.tagName ?? null,
          activeId: document.activeElement?.id ?? null,
          targetActive: document.activeElement === document.querySelector(arguments[0])};`,
        [target],
      );
      try {
        if (isRadio) {
          const views = [
            "people",
            "shifts",
            "timeline",
            "calendarPeople",
            "calendarShifts",
            "assignments",
            "fairness",
            "baseDiff",
            "health",
          ];
          const start = views.indexOf(currentRadio);
          const end = views.indexOf(page.kind);
          assert(start >= 0 && end >= 0, `Unsupported keyboard result view ${page.kind}`);
          await keyboardReach(sessionId, radio(currentRadio), 64);
          const key = end >= start ? "\uE015" : "\uE013";
          const count = Math.abs(end - start);
          if (count > 0) {
            const actions = Array.from({ length: count }, () => [
              { type: "keyDown", value: key },
              { type: "keyUp", value: key },
            ]).flat();
            await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
              actions: [{ type: "key", id: "bounded-large-result-view-keys", actions }],
            });
          }
        } else {
          // Reverse reaches the footer without traversing both buttons on every person row.
          const reverse = action === "source-page-next" || action === "calendar-next-dates";
          // WebKit blurs the paging button while native loading disables it,
          // but retains its tab-navigation anchor. Re-enter using real keys.
          if (action === "source-page-next" && beforeNavigation.activeTag === "BODY") {
            await keyboardKey(sessionId, "\uE004", true);
            await keyboardKey(sessionId, "\uE004");
          }
          await keyboardReach(sessionId, target, 64, reverse);
          await keyboardKey(sessionId, "\uE007");
        }
        let measured;
        try {
          measured = await package10AwaitSample(sessionId, armed.token);
        } catch (failure) {
          const diagnostic = await evaluate(
            sessionId,
            `const pending = window.__euthetoPackage10ReadSample(arguments[0]);
            const alert = document.querySelector('[data-accepted-schedule] > [role="alert"]');
            return {
              sampleStatus: pending?.status ?? 'missing',
              readiness: pending?.readiness ?? null,
              scheduleErrorMessage: alert?.textContent.trim().slice(0, 256) ?? null
            };`,
            [armed.token],
          ).catch(() => ({ sampleStatus: "diagnostic-unavailable" }));
          throw new Error(
            `${temperature} ${action} sample wait failed: ${JSON.stringify(diagnostic)}; ${failure instanceof Error ? failure.message : "unknown sample wait failure"}`,
            { cause: failure },
          );
        }
        assert.equal(
          measured.status,
          "done",
          `${temperature} ${action} must reach settled content: ${JSON.stringify(measured.readiness ?? {})}`,
        );
        assert(Number.isFinite(measured.sample.eventToSettledContentDoubleAnimationFrameMs));
        return measured.sample;
      } catch (failure) {
        const navigation = await evaluate(
          sessionId,
          `const target = document.querySelector(arguments[0]);
          const active = document.activeElement;
          const controls = [...document.querySelectorAll('a[href],button,input,select,textarea,summary,[tabindex]')]
            .filter(node => node.tabIndex >= 0 && !node.disabled &&
              (node.type !== 'radio' || node.checked) && node.getBoundingClientRect().height > 0);
          return {targetPresent: !!target, targetDisabled: target?.disabled ?? null,
            targetIndex: controls.indexOf(target), activeIndex: controls.indexOf(active),
            controlCount: controls.length, activeTag: active?.tagName ?? null,
            activeType: active?.type ?? null, targetTabIndex: target?.tabIndex ?? null};`,
          [target],
        );
        throw new Error(
          `${temperature} ${action} navigation failed: ${JSON.stringify({ beforeNavigation, navigation })}`,
          {
            cause: failure,
          },
        );
      } finally {
        await evaluate(sessionId, "window.__euthetoPackage10CancelSample(arguments[0]);", [
          armed.token,
        ]);
      }
    };
    const radio = (kind) => `${root} input[type="radio"][value="${kind}"]`;
    const button = `${root} button[id="package10-result-target"]`;
    const sampleButton = async (
      page,
      temperature,
      action,
      title,
      pageNumber = 1,
      mode = null,
      firstIndex = 0,
    ) => {
      await evaluate(
        sessionId,
        `document.getElementById('package10-result-target')?.removeAttribute('id');
        const target = [...document.querySelectorAll(arguments[1] + ' button')].find(node =>
          node.textContent.trim() === arguments[0] && !node.disabled);
        if (!target) throw new Error('Required result navigation control is unavailable');
        target.id = 'package10-result-target';`,
        [title, root],
      );
      return sample(page, temperature, action, button, pageNumber, mode, firstIndex);
    };
    for (const temperature of ["cold", "warm"]) {
      await sample(timeline, temperature, "view-timeline", radio("timeline"));
      await sample(people[0], temperature, "view-people", radio("people"));
      await sampleButton(people[1], temperature, "source-page-next", "Next page", 2);
      await sampleButton(people[2], temperature, "source-page-next", "Next page", 3);
      await sample(shifts, temperature, "view-shifts", radio("shifts"));
      await sample(health, temperature, "view-health", radio("health"));
      await sample(
        calendar,
        temperature,
        "view-calendar-people",
        radio("calendarPeople"),
        1,
        "virtual",
      );
      await sampleButton(
        calendar,
        temperature,
        "calendar-paged-mode",
        "Use keyboard-friendly pages",
        1,
        "paged",
      );
      await sampleButton(
        calendar,
        temperature,
        "calendar-visible-page-next",
        "Next visible rows",
        1,
        "paged",
        8,
      );
      await sampleButton(
        nextCalendar,
        temperature,
        "calendar-next-dates",
        "Next dates",
        1,
        "virtual",
      );
      await sample(
        shiftCalendar,
        temperature,
        "view-calendar-shifts",
        radio("calendarShifts"),
        1,
        "virtual",
      );
      await idle(sessionId);
    }
    await screenshot(sessionId, "packaged-large-supported-accepted-calendar-keyboard.png");
    await keyboardReach(
      sessionId,
      '[data-accepted-schedule] input[type="radio"][value="calendarShifts"]',
    );
    await keyboardKey(sessionId, "\uE013");
    await keyboardKey(sessionId, "\uE013");
    await waitFor(
      sessionId,
      `return document.querySelector('[data-accepted-schedule] input[value="timeline"]')?.checked &&
        [...document.querySelectorAll('[data-accepted-schedule] table')]
          .some(table => table.caption?.textContent.trim() === 'Timeline by start time' &&
            table.tBodies[0]?.rows.length === arguments[0]);`,
      [timeline.items.length],
    );
    const detailTarget = '[data-accepted-schedule] button[aria-label^="Inspect evidence for"]';
    await keyboardReach(sessionId, detailTarget);
    const detailLabel = await evaluate(
      sessionId,
      "return document.activeElement.getAttribute('aria-label');",
    );
    await keyboardKey(sessionId, "\uE007");
    await waitFor(
      sessionId,
      `const details = document.querySelector('[data-assignment-details]');
      return details?.textContent.includes('Recorded assignment evidence') &&
        details.textContent.includes('Pair-local');`,
    );
    await screenshot(sessionId, "packaged-large-supported-assignment-detail-keyboard.png");
    await keyboardActivateButton(
      sessionId,
      "Close assignment evidence",
      "[data-assignment-details]",
    );
    await waitFor(
      sessionId,
      "return document.activeElement?.getAttribute('aria-label') === arguments[0];",
      [detailLabel],
    );
    const inPage = await evaluate(sessionId, "return window.__euthetoPackage10Finish();");
    instrumentationStarted = false;
    assert.equal(inPage.samples.length, 22);
    assert.equal(inPage.sampleOverflow, false);
    assert.equal(
      (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
      revision,
    );
    await package10WritePerformanceEvidence(sessionId, sources, [
      {
        caseId: "large-supported-accepted-result",
        source: source.input,
        expectedCorpus: source.expected,
        fixtureShape: source.shape,
        comparison: {
          kind: "sourcePayloadAppliedThroughNativeCommands",
          byteIdenticalScenario: false,
          semanticSettingsCountsAndResolvedShiftsMatch: true,
          nativePeople: 101,
          nativeResolvedShifts: 12,
          nativeCandidatePairs: Number(estimate.candidateCount),
          nativeModelVariables: Number(estimate.variableCount),
          selectedAssignments: timeline.totalItems,
          denseSelectedAssignmentWorkload: false,
        },
        acceptedSource: {
          scenarioId,
          scenarioRevision: revision,
          runId: run.runId,
          terminalStatus: run.status,
          solutionId: run.solutionId,
          resultChecksum: verified.result.resultChecksum,
          verificationChecksum: verified.verification.checksum,
          runManifestChecksum: diagnostics.manifestChecksum,
          independentlyVerified: true,
        },
        nativeOperation: {
          elapsedMilliseconds: diagnostics.elapsedMilliseconds,
          firstIncumbentMilliseconds: diagnostics.firstIncumbentMilliseconds,
          firstVerifiedFeasibleMilliseconds: diagnostics.firstVerifiedFeasibleMilliseconds,
          boundary: "persisted native run diagnostics, separate from view interaction proxies",
        },
        inPage: {
          status: "measured",
          samples: inPage.samples,
          sampleOverflow: inPage.sampleOverflow,
          liveRegion: {
            mutationCount: inPage.liveRegionMutationCount,
            mutationOverflow: inPage.liveRegionMutationOverflow,
            interpretation: "announcement proxy, not screen-reader output",
          },
          longTask: inPage.longTask,
          eventLoopLag: inPage.eventLoopLag,
          method: {
            cold: "first measured transition to each result view in this native WebKit session",
            warm: "repeat of the same transitions on the same accepted source after idle",
            cacheNote:
              "No OS or WebKit caches are flushed; initial By person rendering precedes measurement.",
            webDriverRoundTripExcluded: true,
            domCount:
              "retained element nodes scoped to accepted schedule, not heap memory or viewport visibility",
            boundary:
              "keyboard keydown through expected native content settlement plus double-requestAnimationFrame; not INP, paint or backend-only time",
          },
        },
      },
    ]);
    console.log(
      "PASS: bounded native large-supported accepted-result performance evidence recorded",
    );
  } finally {
    if (instrumentationStarted)
      await evaluate(sessionId, "window.__euthetoPackage10Finish?.();").catch(() => undefined);
    await package10OpenLibrary(sessionId);
    await package10DeleteFixture(sessionId, scenarioId);
  }
}

let requestSequence = 100;
function requestId() {
  requestSequence += 1;
  return `01900000-0000-7000-8000-${requestSequence.toString(16).padStart(12, "0")}`;
}

async function nativeRequest(
  sessionId,
  name,
  request,
  waitMilliseconds = timeout,
  withProgressChannel = false,
) {
  const response = await command(
    "POST",
    `/session/${encodeURIComponent(sessionId)}/execute/async`,
    {
      script: `const done = arguments[arguments.length - 1];
        const ipc = window.__TAURI_INTERNALS__;
        const channel = arguments[2] ? ipc.transformCallback(() => {}) : null;
        const payload = {request: arguments[1]};
        if (channel !== null) payload.onProgress = '__CHANNEL__:' + channel;
        ipc.invoke(arguments[0], payload)
          .then(value => done({ok: value}), error => done({failure: typeof error === 'string' ? error : JSON.stringify(error)}))
          .finally(() => { if (channel !== null) ipc.unregisterCallback(channel); });`,
      args: [name, { requestId: requestId(), ...request }, withProgressChannel],
    },
    waitMilliseconds,
  );
  assert.equal(response.failure, undefined, `Native ${name} failed: ${response.failure ?? ""}`);
  return response.ok;
}

async function waitForSavedCounterfactual(sessionId, scenarioId, matches) {
  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    const jobs = (
      await nativeRequest(sessionId, "solution_list_counterfactuals", {
        schemaVersion: 1,
        scenarioId,
        before: null,
        limit: 8,
      })
    ).result.page.jobs;
    const job = jobs.find(matches);
    if (job && job.state !== "queued" && job.state !== "running") return job;
    await sleep(100);
  }
  throw new Error("The saved native counterfactual did not reach a terminal state");
}

async function nativeExplain(sessionId, scenarioId, sourceRevision, request) {
  const requestIdForExplain = requestId();
  const prepared = await nativeRequest(sessionId, "operation_prepare", {
    schemaVersion: 1,
    purpose: { kind: "solutionExplain" },
    context: { kind: "scenario", scenarioId, expectedRevision: sourceRevision },
  });
  const operationId = prepared.result.operationId;
  try {
    return await nativeRequest(
      sessionId,
      "solution_explain",
      {
        schemaVersion: 2,
        operationId,
        requestId: requestIdForExplain,
        scenarioId,
        sourceRevision,
        request,
      },
      timeout,
      true,
    );
  } finally {
    await nativeRequest(sessionId, "operation_release", { schemaVersion: 1, operationId });
  }
}

// Native setup reads require a reservation and a real IPC channel, just like the
// generated client. This test helper observes persisted data, not renderer caches.
async function nativeSetupRequest(sessionId, scenarioId, name, viewId, input, phaseTiming = null) {
  const current = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  assert(current, "The native setup fixture project must exist");
  const prepared = await nativeRequest(sessionId, "operation_prepare", {
    schemaVersion: 1,
    purpose: { kind: "setupView", viewId },
    context: { kind: "scenario", scenarioId, expectedRevision: current.revision },
  });
  const operationId = prepared.result.operationId;
  let succeeded = false;
  try {
    const reply = await command("POST", `/session/${encodeURIComponent(sessionId)}/execute/async`, {
      script: `const done = arguments[arguments.length - 1];
        const ipc = window.__TAURI_INTERNALS__;
        const progress = [];
        const tracked = arguments[2];
        const request = arguments[1];
        const channel = ipc.transformCallback(raw => {
          const value = raw?.message;
          if (tracked && progress.length < 16 &&
              value?.operationId === request.operationId &&
              value?.requestId === request.requestId &&
              typeof value.timestamp === 'string')
            progress.push({phase: value.phase, timestamp: value.timestamp});
        });
        ipc.invoke(arguments[0], {request: arguments[1], onProgress: '__CHANNEL__:' + channel})
          .then(value => done({ok: value, progress}), error => done({failure: typeof error === 'string' ? error : JSON.stringify(error)}))
          .finally(() => ipc.unregisterCallback(channel));`,
      args: [
        name,
        {
          ...input,
          schemaVersion: 2,
          requestId: requestId(),
          operationId,
          scenarioId,
          expectedRevision: current.revision,
        },
        phaseTiming !== null,
      ],
    });
    assert.equal(reply.failure, undefined, `Native ${name} failed: ${reply.failure ?? ""}`);
    assert.equal(reply.ok.currentRevision, current.revision);
    if (phaseTiming !== null) {
      const start = reply.progress.find((event) => event.phase === "buildingView");
      const end = reply.progress.find((event) => event.phase === "preparingResponse");
      const duration = start && end ? Date.parse(end.timestamp) - Date.parse(start.timestamp) : NaN;
      phaseTiming.backendOnlyMs = Number.isFinite(duration) && duration >= 0 ? duration : null;
    }
    succeeded = true;
    return reply.ok;
  } finally {
    if (!succeeded) {
      await nativeRequest(sessionId, "operation_release", { schemaVersion: 1, operationId });
    }
  }
}

async function projects(sessionId) {
  return (await nativeRequest(sessionId, "project_list", { schemaVersion: 1, scope: "all" }))
    .result;
}

async function appliedHistoryHead(sessionId, scenarioId) {
  const project = (await projects(sessionId)).find((item) => item.scenarioId === scenarioId);
  assert(project, "History head requires a saved project");
  let continuation = null;
  do {
    const page = (
      await nativeRequest(sessionId, "scenario_get_history_page", {
        schemaVersion: 1,
        scenarioId,
        expectedRevision: project.revision,
        limit: 100,
        continuation,
      })
    ).result;
    const head = page.entries.find((entry) => entry.applied);
    if (head) return head;
    continuation = page.continuation;
  } while (continuation !== null);
  return null;
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

async function nativeChooser(title, path = null, confirmOverwrite = false) {
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
  let existingWindows;
  let chooserPid;
  if (confirmOverwrite) {
    const { stdout } = await executeFile(xdotoolExecutable, ["getwindowpid", windowId]);
    chooserPid = stdout.trim();
    const visible = await executeFile(xdotoolExecutable, [
      "search",
      "--onlyvisible",
      "--pid",
      chooserPid,
      "--name",
      ".*",
    ]);
    existingWindows = new Set(visible.stdout.trim().split(/\s+/u));
  }
  await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "Return"]);
  if (confirmOverwrite) await nativeOverwriteConfirmation(chooserPid, existingWindows);
}

async function nativeOverwriteConfirmation(chooserPid, existingWindows) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      // GTK3 creates a new modal for overwrite confirmation without assigning a title.
      const { stdout } = await executeFile(xdotoolExecutable, [
        "search",
        "--onlyvisible",
        "--pid",
        chooserPid,
        "--name",
        ".*",
      ]);
      const windowId = stdout
        .trim()
        .split(/\s+/u)
        .find((id) => id && !existingWindows.has(id));
      if (windowId) {
        await executeFile(xdotoolExecutable, ["windowfocus", "--sync", windowId]);
        await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "Return"]);
        return;
      }
    } catch {
      /* The transient confirmation may not have been created yet. */
    }
    await sleep(100);
  }
  assert.fail("Native save chooser did not show its existing-file confirmation");
}

async function waitForOutcome(sessionId, text, waitMilliseconds = timeout) {
  await waitFor(
    sessionId,
    "return document.querySelector('.workspace-outcome')?.textContent.includes(arguments[0]) && document.querySelector('main[data-operation-active=\"false\"]') !== null;",
    [text],
    waitMilliseconds,
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
      "return document.querySelector('.portable-evidence > dl.preview-metadata code.identifier')?.textContent.trim();",
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
  await activate(sessionId, "#settings-motion");
  await submitForm(sessionId, "#settings-theme");
  await waitFor(
    sessionId,
    `return document.documentElement.dataset.theme === 'dark' &&
      document.documentElement.dataset.reducedMotion === 'true' &&
      getComputedStyle(document.querySelector('button')).transitionDuration
        .split(',').every(duration => Number.parseFloat(duration) <= 0.001) &&
      document.querySelector('main[data-operation-active="false"]') !== null;`,
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
  assert.equal(saved.settings.appearance.value.reducedMotion, true);
  assert.equal(saved.settings.locale.value, "fr-CA");
  assert.equal(saved.settings.units.value, "metric");
  const settingsPath = join(directory, "nonsecret-settings.json");
  await activateButton(sessionId, "Export saved display settings");
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
  assert.equal((await settingsSnapshot(sessionId)).settings.appearance.value.reducedMotion, true);
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
  await activateButton(sessionId, "Review editable project export");
  await waitForElement(sessionId, "#portable-review-heading");
  await idle(sessionId);
  await saveReviewed(sessionId, "Save Eutheto export", exportPath);

  await navigate(sessionId, "/projects");
  await navigate(sessionId, "/projects/import");
  await activateButton(sessionId, "Choose a file to check");
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

async function keyboardKey(sessionId, key, shift = false) {
  const actions = [];
  if (shift) actions.push({ type: "keyDown", value: "\uE008" });
  actions.push({ type: "keyDown", value: key }, { type: "keyUp", value: key });
  if (shift) actions.push({ type: "keyUp", value: "\uE008" });
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [{ type: "key", id: "bounded-native-keyboard-journey", actions }],
  });
  // Observe native focus/scroll after rendering, not just driver input dispatch.
  await command("POST", `/session/${encodeURIComponent(sessionId)}/execute/async`, {
    script:
      "const done = arguments[arguments.length - 1]; requestAnimationFrame(() => requestAnimationFrame(() => done(null)));",
    args: [],
  });
}

async function keyboardReach(sessionId, selector, maximumTabs = 32, reverse = false) {
  for (let index = 0; index <= maximumTabs; index += 1) {
    if (
      await evaluate(sessionId, "return document.activeElement?.matches(arguments[0]) ?? false;", [
        selector,
      ])
    )
      return index;
    assert(
      index < maximumTabs,
      `Keyboard Tab did not reach ${selector} within ${maximumTabs} steps`,
    );
    const previous = await evaluate(sessionId, "return document.activeElement;");
    await keyboardKey(sessionId, "\uE004", reverse);
    const focus = await evaluate(
      sessionId,
      `const node = document.activeElement;
      const rect = node?.getBoundingClientRect();
      return {matches: node?.matches(arguments[0]) ?? false, changed: node !== arguments[1],
        segmented: node instanceof HTMLInputElement && ['date', 'time', 'datetime-local'].includes(node.type),
        visible: !!rect && rect.width > 0 && rect.height > 0 &&
          rect.bottom > 0 && rect.top < innerHeight, tag: node?.tagName ?? null};`,
      [selector, previous],
    );
    // Native date/time segments can receive Tab without changing the enclosing input.
    assert(focus.changed || focus.segmented, `Tab did not move focus while seeking ${selector}`);
    assert(focus.visible, `Tab focus was not visible while seeking ${selector}`);
    if (focus.matches) return index + 1;
  }
  assert.fail(`Keyboard Tab did not reach ${selector}`);
}

async function keyboardActivate(sessionId, selector, key = "\uE007", maximumTabs = 32) {
  await keyboardReach(sessionId, selector, maximumTabs);
  await keyboardKey(sessionId, key);
}

async function keyboardReachButton(
  sessionId,
  text,
  root = "body",
  maximumTabs = 32,
  reverse = false,
) {
  for (let index = 0; index <= maximumTabs; index += 1) {
    const focused = await evaluate(
      sessionId,
      `const node = document.activeElement;
      return node instanceof HTMLButtonElement && node.textContent.trim() === arguments[0] &&
        node.closest(arguments[1]);`,
      [text, root],
    );
    if (focused) return index;
    assert(
      index < maximumTabs,
      `Keyboard Tab did not reach button "${text}" within ${maximumTabs} steps`,
    );
    const previous = await evaluate(sessionId, "return document.activeElement;");
    await keyboardKey(sessionId, "\uE004", reverse);
    const moved = await evaluate(
      sessionId,
      `const node = document.activeElement;
      const segmented = node instanceof HTMLInputElement && ['date', 'time', 'datetime-local'].includes(node.type);
      return (node !== arguments[0] || segmented) && node?.getBoundingClientRect().height > 0;`,
      [previous],
    );
    assert(moved, `Tab did not move to a visible control while seeking button "${text}"`);
  }
  assert.fail(`Keyboard Tab did not reach button "${text}"`);
}

async function keyboardActivateButton(sessionId, text, root = "body", maximumTabs = 32) {
  const tabs = await keyboardReachButton(sessionId, text, root, maximumTabs);
  await keyboardKey(sessionId, "\uE007");
  return tabs;
}

async function keyboardReplaceFocused(sessionId, value) {
  const keyActions = [
    { type: "keyDown", value: "\uE009" },
    { type: "keyDown", value: "a" },
    { type: "keyUp", value: "a" },
    { type: "keyUp", value: "\uE009" },
    { type: "keyDown", value: "\uE003" },
    { type: "keyUp", value: "\uE003" },
  ];
  for (const character of String(value)) {
    keyActions.push({ type: "keyDown", value: character }, { type: "keyUp", value: character });
  }
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [{ type: "key", id: "keyboard-workflow-input", actions: keyActions }],
  });
}

async function keyboardWorkflowType(sessionId, selector, value) {
  await keyboardReach(sessionId, selector, 64);
  await keyboardReplaceFocused(sessionId, value);
}

async function keyboardWorkflowSelect(sessionId, selector, value) {
  await keyboardReach(sessionId, selector, 64);
  const state = await evaluate(
    sessionId,
    `const select = document.querySelector(arguments[0]);
    return select ? {value: select.value, values: [...select.options].map(option => option.value)} : null;`,
    [selector],
  );
  assert(state, `Keyboard select is unavailable: ${selector}`);
  const start = state.values.indexOf(state.value);
  const end = state.values.indexOf(value);
  assert(start >= 0 && end >= 0, `Keyboard select option is unavailable: ${value}`);
  const key = end >= start ? "\uE015" : "\uE013";
  for (let index = 0; index < Math.abs(end - start); index += 1) await keyboardKey(sessionId, key);
  await keyboardKey(sessionId, "\uE007");
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0])?.value ?? null;", [
      selector,
    ]),
    value,
  );
}

async function keyboardWorkflowButton(sessionId, fragment, root = "body") {
  const text = await waitFor(
    sessionId,
    `const root = document.querySelector(arguments[1]);
    const button = [...(root?.querySelectorAll('button') ?? [])].find(node =>
      node.getBoundingClientRect().height > 0 &&
      node.textContent.trim().toLowerCase().includes(arguments[0].toLowerCase()));
    return button?.textContent.trim() ?? null;`,
    [fragment, root],
  );
  await keyboardReachButton(sessionId, text, root, 64);
  await keyboardKey(sessionId, "\uE007");
}

async function keyboardWorkflowRoute(sessionId, suffix) {
  const disclosureClosed = await evaluate(
    sessionId,
    `const link = document.querySelector('a[href*="' + arguments[0] + '"]');
    const details = link?.closest('details');
    return !!details && !details.open;`,
    [suffix],
  );
  if (disclosureClosed)
    await keyboardActivate(sessionId, "details.project-nav-more > summary", "\u0020", 64);
  await keyboardActivate(sessionId, `a[href*="${suffix}"]`, "\uE007", 64);
}
async function keyboardWorkflowView(sessionId, value) {
  const root = "[data-accepted-schedule]";
  await keyboardReach(sessionId, `${root} input[type=radio]:checked`, 64);
  const choices = await evaluate(
    sessionId,
    `return [...document.querySelectorAll(arguments[0] + ' input[type=radio]')]
      .map(node => ({value: node.value, checked: node.checked}));`,
    [root],
  );
  const current = choices.findIndex((choice) => choice.checked);
  const target = choices.findIndex((choice) => choice.value === value);
  assert(current >= 0 && target >= 0, `Accepted schedule view is unavailable: ${value}`);
  const key = target >= current ? "\uE015" : "\uE013";
  for (let index = 0; index < Math.abs(target - current); index += 1)
    await keyboardKey(sessionId, key);
  await waitFor(
    sessionId,
    `return document.querySelector(arguments[0] + ' input[type=radio]:checked')?.value === arguments[1];`,
    [root, value],
  );
  await waitForElement(sessionId, `${root} table tbody tr`);
}

async function packagedKeyboardWorkflowAcceptance(sessionId) {
  assert.equal(
    process.env.EUTHETO_E2E_PACKAGED_SOLVE,
    "1",
    "The keyboard workflow requires the actual bundled native worker",
  );
  const title = "Keyboard Workflow Sample";
  const personName = "Fictional Worker";
  const assignmentTypeName = "Fictional Shift";
  await keyboardActivate(sessionId, 'a[href*="/projects/new"]', "\uE007", 64);
  await waitForElement(sessionId, "#create-title");
  await keyboardWorkflowType(sessionId, "#create-title", title);
  await keyboardWorkflowType(sessionId, "#first-date", "01/01/2030");
  await keyboardWorkflowType(sessionId, "#last-date", "01/02/2030");
  await keyboardActivate(sessionId, "form button[type=submit]", "\uE007", 64);
  await waitForElement(sessionId, "#setup-calendar");
  const scenarioId = await evaluate(
    sessionId,
    "return window.location.hash.match(/^#\\/project\\/([^/]+)\\/setup$/)?.[1] ?? null;",
  );
  assert.match(scenarioId, /^[a-f0-9-]{36}$/u);
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId)?.domainPackId,
    "official.workforce",
  );

  await keyboardWorkflowRoute(sessionId, "/people");
  await waitForElement(sessionId, "#people-heading");
  await keyboardWorkflowButton(sessionId, "Create");
  await waitForElement(sessionId, 'input[name="name"]');
  await keyboardWorkflowType(sessionId, 'input[name="name"]', personName);
  await keyboardActivate(sessionId, "form button[type=submit]", "\uE007", 64);
  await waitForElement(sessionId, "#people-review-heading");
  await keyboardWorkflowButton(sessionId, "Save");
  await waitFor(sessionId, "return document.body.textContent.includes(arguments[0]);", [
    personName,
  ]);

  await keyboardWorkflowSelect(sessionId, "#people-kind", "assignmentType");
  await keyboardWorkflowButton(sessionId, "Create");
  await waitForElement(sessionId, 'input[name="category"]');
  await keyboardWorkflowType(sessionId, 'input[name="name"]', assignmentTypeName);
  await keyboardWorkflowType(sessionId, 'input[name="category"]', "fictional");
  await keyboardWorkflowType(sessionId, 'input[id$="-duration"]', "480");
  await keyboardWorkflowSelect(sessionId, 'select[name="qualificationMode"]', "unconstrained");
  await keyboardWorkflowSelect(sessionId, 'select[name="locationMode"]', "none");
  await keyboardWorkflowSelect(sessionId, 'select[name="timeBehavior"]', "localWallClock");
  await keyboardActivate(sessionId, "form button[type=submit]", "\uE007", 64);
  await waitForElement(sessionId, "#people-review-heading");
  await keyboardWorkflowButton(sessionId, "Save");
  await waitFor(sessionId, "return document.body.textContent.includes(arguments[0]);", [
    assignmentTypeName,
  ]);

  await keyboardWorkflowRoute(sessionId, "/work");
  await waitForElement(sessionId, "#work-heading");
  await keyboardWorkflowSelect(sessionId, "#work-record-kind", "shiftInstance");
  await keyboardWorkflowButton(sessionId, "Create a record:");
  await waitForElement(sessionId, 'input[role="combobox"]');
  await keyboardWorkflowType(sessionId, 'input[role="combobox"]', assignmentTypeName);
  await waitFor(sessionId, "return document.querySelector('[role=option]') !== null;");
  await keyboardKey(sessionId, "\uE015");
  await keyboardKey(sessionId, "\uE007");
  await keyboardWorkflowType(sessionId, 'input[id$="-interval-start"]', "2030-01-01T09:00");
  await keyboardWorkflowType(sessionId, 'input[id$="-interval-end"]', "2030-01-01T17:00");
  await keyboardWorkflowSelect(sessionId, 'select[name="reportingAttribution"]', "startLocalDate");
  await keyboardWorkflowSelect(sessionId, 'select[id$="-coverage.kind"]', "atLeast");
  await keyboardWorkflowType(sessionId, 'input[id$="-coverage.minimum"]', "1");
  await keyboardActivate(sessionId, "form button[type=submit]", "\uE007", 64);
  await waitForElement(sessionId, "#work-review-heading");
  await keyboardWorkflowButton(sessionId, "Apply");
  await waitForElement(sessionId, "#work-records-heading");

  await keyboardWorkflowRoute(sessionId, "/eligibility");
  await waitForElement(sessionId, "#eligibility-setup-heading");
  await waitForElement(sessionId, "[data-eligibility-matrix] input[type=checkbox]");
  const eligibilityCell = "[data-eligibility-matrix] input[type=checkbox]";
  if (
    !(await evaluate(sessionId, "return document.querySelector(arguments[0])?.checked;", [
      eligibilityCell,
    ]))
  )
    await keyboardActivate(sessionId, eligibilityCell, " ", 64);
  await keyboardWorkflowButton(sessionId, "Review");
  await waitForElement(sessionId, "#eligibility-review-heading");
  await keyboardWorkflowButton(sessionId, "Save reviewed work type choices");
  await waitFor(
    sessionId,
    "return document.querySelector('#eligibility-review-heading') === null;",
  );

  await keyboardWorkflowRoute(sessionId, "/rules");
  await waitForElement(sessionId, "#rules-heading");
  await keyboardWorkflowButton(sessionId, "Coverage");
  await waitForElement(sessionId, "#rule-editor-heading");
  await keyboardWorkflowButton(sessionId, "Review changes");
  await waitForElement(sessionId, "#rule-review-heading");
  await waitFor(sessionId, "return document.activeElement?.id === 'rule-review-heading';");
  await keyboardWorkflowButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-review-heading');");

  await keyboardWorkflowRoute(sessionId, "/score-policy");
  await waitForElement(sessionId, "#score-policy-list-heading");
  await keyboardWorkflowButton(sessionId, "Create score policy");
  await keyboardWorkflowType(sessionId, "#score-policy-profile-key", "keyboard-sample");
  await keyboardWorkflowButton(sessionId, "Review score policy changes");
  await waitForElement(sessionId, "#score-policy-review-heading");
  await waitFor(sessionId, "return document.activeElement?.id === 'score-policy-review-heading';");
  await keyboardWorkflowButton(sessionId, "Apply this reviewed score policy");
  await waitFor(sessionId, "return !document.querySelector('#score-policy-review-heading');");

  await keyboardWorkflowRoute(sessionId, "/validation");
  await waitForElement(sessionId, "#validation-heading");
  await keyboardWorkflowButton(sessionId, "Run full validation");
  await waitForElement(sessionId, '[data-full-validation-state="completed"]', 120_000);
  await keyboardWorkflowRoute(sessionId, "/setup");
  await waitForElement(sessionId, '[data-optimize-state="ready"]');
  await keyboardWorkflowButton(sessionId, "Start optimization", "[data-optimize-panel]");
  await waitForElement(sessionId, "#setup-results-details .result-summary-list > li", 120_000);
  await keyboardWorkflowButton(sessionId, "Inspect schedule", "#setup-results-details");
  await waitForElement(sessionId, "[data-accepted-schedule] table tbody tr");
  await waitFor(
    sessionId,
    `return document.querySelector(
      '[data-accepted-schedule] section[aria-label="Independent result check"]'
    )?.textContent.includes('Verified schedule ready. Every recorded Required rule passed.');`,
  );
  const accepted = (
    await nativeRequest(sessionId, "solution_list", { schemaVersion: 1, scenarioId })
  ).result.solutions[0];
  assert(accepted && ["optimal", "feasible"].includes(accepted.status));
  assert.match(accepted.result.resultChecksum, /^[a-f0-9]{64}$/u);
  const initialRevision = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  const firstResult = "#setup-results-details .result-summary-list > li:first-child";
  await keyboardWorkflowButton(sessionId, "Review as repair base", firstResult);
  await waitFor(
    sessionId,
    `return [...document.querySelectorAll(arguments[0] + ' button')]
      .some(button => button.textContent.trim() === 'Save this result as repair base');`,
    [firstResult],
  );
  await keyboardWorkflowButton(sessionId, "Save this result as repair base", firstResult);
  await waitFor(
    sessionId,
    "return document.body.textContent.includes('The accepted result is saved as the repair base.');",
  );
  const baseRevision = initialRevision + 1;
  const resultRoot = "[data-accepted-schedule]";
  await keyboardWorkflowButton(sessionId, "Inspect schedule", firstResult);
  await waitForElement(sessionId, `${resultRoot} table tbody tr`);
  await keyboardWorkflowView(sessionId, "calendarPeople");
  await screenshot(sessionId, "packaged-keyboard-workflow-accepted-calendar.png");
  await keyboardWorkflowView(sessionId, "timeline");
  await keyboardReach(sessionId, `${resultRoot} button[aria-label^="Inspect evidence for"]`, 64);
  const assignmentTrigger = await evaluate(sessionId, "return document.activeElement;");
  await keyboardKey(sessionId, "\uE007");
  await waitForElement(sessionId, "[data-assignment-details]");
  await screenshot(sessionId, "packaged-keyboard-workflow-assignment-evidence.png");
  await keyboardWorkflowButton(sessionId, "Close assignment evidence", "[data-assignment-details]");
  await waitFor(sessionId, "return document.activeElement === arguments[0];", [assignmentTrigger]);

  const repairPanel = "[data-manual-repair-panel]";
  await waitForElement(sessionId, repairPanel);
  await keyboardWorkflowButton(sessionId, "Refresh current selection", repairPanel);
  await waitForElement(sessionId, `${repairPanel} [data-native-pair]`);
  await keyboardWorkflowButton(sessionId, `Change protection for ${personName}`, repairPanel);
  await waitFor(sessionId, "return document.activeElement?.id === 'manual-repair-heading';");
  await keyboardWorkflowSelect(
    sessionId,
    `${repairPanel} form > fieldset > label:nth-of-type(2) select`,
    "hard",
  );
  await keyboardWorkflowButton(sessionId, "Review manual change", repairPanel);
  await waitForElement(sessionId, '[data-manual-review][data-disposition="valid"]');
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    baseRevision,
    "Hard-protection review must not mutate before explicit Apply",
  );
  await screenshot(sessionId, "packaged-keyboard-workflow-hard-review.png");
  await keyboardWorkflowButton(sessionId, "Apply reviewed manual change", repairPanel);
  await waitFor(
    sessionId,
    `return document.querySelector(arguments[0])?.textContent.toLowerCase().includes('hard') ?? false;`,
    [`${repairPanel} [data-native-pair]`],
  );
  await keyboardWorkflowRoute(sessionId, "/validation");
  await waitForElement(sessionId, "#validation-heading");
  await keyboardWorkflowButton(sessionId, "Run full validation");
  await waitForElement(sessionId, '[data-full-validation-state="completed"]', 120_000);
  await keyboardWorkflowRoute(sessionId, "/setup");
  await waitForElement(sessionId, '[data-optimize-state="ready"]');
  await keyboardWorkflowButton(sessionId, "Repair plan", "[data-optimize-panel]");
  await waitFor(
    sessionId,
    `return document.querySelectorAll('#setup-results-details .result-summary-list > li').length >= 2;`,
    [],
    120_000,
  );
  await keyboardWorkflowButton(sessionId, "Inspect schedule", "#setup-results-details");
  await waitForElement(sessionId, `${resultRoot} table tbody tr`);
  await keyboardWorkflowView(sessionId, "calendarPeople");
  await screenshot(sessionId, "packaged-keyboard-workflow-repaired-calendar.png");
  const comparedSolutions = (
    await nativeRequest(sessionId, "solution_list", { schemaVersion: 1, scenarioId })
  ).result.solutions;
  assert.equal(comparedSolutions.length, 2);
  const repaired = comparedSolutions.find(
    (solution) => solution.result.solutionId !== accepted.result.solutionId,
  );
  assert(repaired, "The Repair run must create a second accepted result for comparison");
  await keyboardReachButton(
    sessionId,
    "Compare latest two results",
    "#setup-results-details",
    32,
    true,
  );
  await keyboardKey(sessionId, "\uE007");
  await waitForElement(
    sessionId,
    '#setup-results-details section[aria-label="Compare saved results"]',
  );
  await waitFor(
    sessionId,
    `const panel = document.querySelector('#setup-results-details section[aria-label="Compare saved results"]');
    return !!panel && panel.textContent.includes(arguments[0]) && panel.textContent.includes(arguments[1]);`,
    [accepted.result.solutionId, repaired.result.solutionId],
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    baseRevision + 1,
    "Comparing accepted results must remain read-only",
  );
  await screenshot(sessionId, "packaged-keyboard-workflow-comparison.png");

  const dialog = '[role="dialog"]';
  await keyboardActivateButton(sessionId, "Export CSV/calendar", resultRoot);
  await waitForElement(sessionId, ".schedule-export-dialog");
  await keyboardReach(sessionId, `${dialog} input[type=radio][value="assignmentsCsv"]`, 64);
  await keyboardKey(sessionId, " ");
  await keyboardWorkflowButton(sessionId, "Prepare native review", dialog);
  await waitFor(
    sessionId,
    `const source = document.querySelector('.schedule-export-source');
    return source?.textContent.includes(arguments[0]) &&
      source.textContent.includes(arguments[1]);`,
    [repaired.result.solutionId, repaired.result.resultChecksum],
  );
  await screenshot(sessionId, "packaged-keyboard-workflow-csv-review.png");
  await keyboardWorkflowButton(sessionId, "Close and release review", dialog);
  await waitFor(sessionId, "return !document.querySelector('[role=dialog]');");
  await waitFor(
    sessionId,
    `return document.activeElement === [...document.querySelector(arguments[0]).querySelectorAll('button')]
      .find(button => button.textContent.trim() === 'Export CSV/calendar');`,
    [resultRoot],
  );
  await keyboardKey(sessionId, "\uE004", true);
  await keyboardActivateButton(sessionId, "Share result", resultRoot);
  await waitForElement(sessionId, ".recipient-share-dialog");
  await keyboardWorkflowButton(sessionId, "Prepare review", dialog);
  await waitFor(
    sessionId,
    `const source = document.querySelector('.recipient-share-source');
    return source?.textContent.includes(arguments[0]) &&
      source.textContent.includes(arguments[1]);`,
    [repaired.result.solutionId, repaired.result.resultChecksum],
  );
  await screenshot(sessionId, "packaged-keyboard-workflow-share-review.png");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    baseRevision + 1,
    "CSV and Share reviews must not mutate the scenario",
  );
  await keyboardWorkflowButton(sessionId, "Close and release review", dialog);
  await waitFor(sessionId, "return !document.querySelector('[role=dialog]');");
  await waitFor(
    sessionId,
    `return document.activeElement === [...document.querySelector(arguments[0]).querySelectorAll('button')]
      .find(button => button.textContent.trim() === 'Share result');`,
    [resultRoot],
  );
  console.log(
    "PASS: complete UI-keyboard Workforce setup, optimization, Hard review/apply, Repair, comparison, and export reviews",
  );
  // Fixture disposal is separate from the UI-keyboard acceptance proof.
  await package10OpenLibrary(sessionId);
  await package10DeleteFixture(sessionId, scenarioId);
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
    await activate(sessionId, "#setup-calendar-details > summary");
    assert(
      (await getText(sessionId, "#setup-calendar-details")).includes(expected),
      `A fresh native setup read must expose the persisted scenario locale ${expected}`,
    );
  }
  // The shortcut exercise already restored this command. Reuse its journal tail rather than
  // creating another change or treating an audit row as an individually replayable command.
  await observeSavedLocale("en-GB");
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
  await activateButton(sessionId, "Review editable project export");
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
  await evaluate(
    sessionId,
    `const dialog = document.querySelector('[role="dialog"]');
    const first = dialog.querySelector('button:not(:disabled)');
    first.focus();
    return document.activeElement === first;`,
  );
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "disabled-delete-reverse-tab",
        actions: [
          { type: "keyDown", value: "\uE008" },
          { type: "keyDown", value: "\uE004" },
          { type: "keyUp", value: "\uE004" },
          { type: "keyUp", value: "\uE008" },
        ],
      },
    ],
  });
  await waitFor(
    sessionId,
    `const buttons = [...document.querySelectorAll('[role="dialog"] button:not(:disabled)')];
    return buttons.length > 1 && document.activeElement === buttons.at(-1);`,
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
async function indexedElementId(sessionId, selector, index = 0) {
  return waitFor(
    sessionId,
    "return document.querySelectorAll(arguments[0])[arguments[1]]?.id ?? null;",
    [selector, index],
  );
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
    await activateButton(sessionId, "Save this change");
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
  assert.deepEqual(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll(arguments[0] + ' form details.setup-more > summary')].map(summary => [summary.textContent.trim().split('·')[0].trim(), summary.parentElement.open]);",
      [editorRoot],
    ),
    [
      ["Qualifications and work types", false],
      ["Dates, teams and home location", false],
      ["Workload and targets", false],
      ["Tags and appearance", false],
      ["ID from another system (optional)", false],
    ],
    "A new person exposes the name and summarized options without opening the full expert form",
  );
  assert(
    await evaluate(
      sessionId,
      "const panel = document.querySelector(arguments[0]); return panel.querySelector('form').getBoundingClientRect().width >= panel.getBoundingClientRect().width * 0.8;",
      [editorRoot],
    ),
    "The simplified person form must fill the editor instead of collapsing to its summary text width",
  );
  await screenshot(sessionId, "people-native-simple-person.png");
  await openPersonOptions(sessionId, "Qualifications and work types", editorRoot);
  await openPersonOptions(sessionId, "Dates, teams and home location", editorRoot);
  await openPersonOptions(sessionId, "Workload and targets", editorRoot);
  await openPersonOptions(sessionId, "Tags and appearance", editorRoot);
  await choosePeopleReference(sessionId, "Teams", "Native ward");
  await choosePeopleReference(sessionId, "Work types this person may do", "Native day work");
  await activateButton(sessionId, "Add qualification", editorRoot);
  await choosePeopleReference(sessionId, "Qualification", "Native training");
  await setValue(
    sessionId,
    `${editorRoot} input[id$="-effectiveFrom"]`,
    "2026-09-01T08:00:00+02:00",
  );
  await setValue(sessionId, `${editorRoot} input[id$="-expiresAt"]`, "2026-10-01T08:00:00+02:00");
  await activateButton(sessionId, "Add qualification", editorRoot);
  await evaluate(
    sessionId,
    "const row = [...document.querySelectorAll(arguments[0] + ' fieldset')].find(item => item.querySelector(':scope > legend')?.textContent.trim() === 'Qualification entry 2'); row.id = 'native-second-grant';",
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
  await activateCheckbox(sessionId, "Only active during these dates", editorRoot);
  await setValue(sessionId, `${editorRoot} input[id$="-startDate"]`, "2026-09-01");
  await setValue(sessionId, `${editorRoot} input[id$="-endDateExclusive"]`, "2026-11-01");
  await choosePeopleReference(sessionId, "Home location (optional)", "Existing home ward");
  await setValue(sessionId, `${editorRoot} input[id$="-weightNumerator"]`, "2");
  await setValue(sessionId, `${editorRoot} input[id$="-weightDenominator"]`, "3");
  await activateCheckbox(sessionId, "Set a workload target", editorRoot);
  await choosePeopleReference(sessionId, "Workload measure", "Existing scheduled minutes");
  await choosePeopleReference(sessionId, "Workload calendar", "Existing daily target");
  await selectValue(sessionId, `${editorRoot} select[id$="-membership"]`, "intersection");
  await setValue(sessionId, `${editorRoot} input[id$="-target"]`, "480");
  await activateCheckbox(sessionId, "Add appearance details", editorRoot);
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
    "return document.querySelector(arguments[0]).textContent.includes('Measured in scheduled minutes. Overlapping work: sum.') && document.querySelector(arguments[0]).textContent.includes('Existing daily target');",
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
  await activateButton(sessionId, "Edit person", editorRoot);
  await waitFor(
    sessionId,
    "const input = document.querySelector(arguments[0]); return input && !input.readOnly && !input.disabled && document.activeElement === input;",
    [`${editorRoot} input[name="name"]`],
  );
  const workloadSummary = await openPersonOptions(sessionId, "Workload and targets", editorRoot);
  await setValue(sessionId, `${editorRoot} input[id$="-weightNumerator"]`, "0");
  await activateElement(sessionId, workloadSummary);
  await activateButton(sessionId, "Review changes", editorRoot);
  await waitFor(
    sessionId,
    "const field = document.activeElement; return field?.id?.endsWith('-weightNumerator') && field.closest('details')?.open && !document.querySelector('#people-review-heading');",
  );
  await setValue(sessionId, `${editorRoot} input[id$="-weightNumerator"]`, "");
  await activateElement(sessionId, workloadSummary);
  await activateButton(sessionId, "Review changes", editorRoot);
  await waitFor(
    sessionId,
    "const field = document.activeElement; return field?.id?.endsWith('-weightNumerator') && field.closest('details')?.open && !document.querySelector('#people-review-heading');",
  );
  await setValue(sessionId, `${editorRoot} input[id$="-weightNumerator"]`, "2");
  await setValue(sessionId, `${editorRoot} input[name="name"]`, "Local person name");
  await openPersonOptions(sessionId, "ID from another system (optional)", editorRoot);
  await activateCheckbox(sessionId, "Use an ID from another system", editorRoot);
  await setValue(sessionId, `${editorRoot} input[name="externalId"]`, "Retained inactive identity");
  await activateCheckbox(sessionId, "Use an ID from another system", editorRoot);
  await openPersonOptions(sessionId, "Tags and appearance", editorRoot);
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
    "return document.querySelector(arguments[0]).textContent.includes('The saved project or library changed');",
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
    "return document.querySelector(arguments[0]).textContent.includes('Changes to the same fields');",
    [editorRoot],
  );
  await waitFor(
    sessionId,
    "return document.activeElement?.textContent.trim() === 'Changes to the same fields';",
  );
  await screenshot(sessionId, "people-native-rebase.png");
  await evaluate(
    sessionId,
    "const row = [...document.querySelectorAll(arguments[0] + ' section[aria-label=\"Changes to the same fields\"] li')].find(item => item.querySelector('h5')?.textContent === 'Tags'); row.id = 'manual-tag-conflict';",
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
      "return [...document.querySelectorAll(arguments[0] + ' section[aria-label=\"Changes to the same fields\"] button')].filter(button => button.textContent.trim() !== 'Inspect the complete current record').map(button => button.disabled);",
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
    "return document.activeElement?.textContent.trim() === 'Changes to the same fields';",
  );
  assert.deepEqual(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll(arguments[0] + ' section[aria-label=\"Changes to the same fields\"] h5')].map(heading => heading.textContent);",
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
  await openPersonOptions(sessionId, "ID from another system (optional)", editorRoot);
  await activateCheckbox(sessionId, "Use an ID from another system", editorRoot);
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      `${editorRoot} input[name="externalId"]`,
    ]),
    "Retained inactive identity",
    "Rebase must preserve inactive raw input when the resolved native field has not changed",
  );
  await activateCheckbox(sessionId, "Use an ID from another system", editorRoot);
  await activateButton(sessionId, "Review changes", editorRoot);
  await waitForElement(sessionId, "#people-review-heading");
  const proposal = 'section[aria-label="Complete record after this change"]';
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
  await activateButton(sessionId, "Save this change");
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
  await activateButton(sessionId, "Edit record", editorRoot);
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
  await activateButton(
    sessionId,
    "Create a new record from these entries with a new ID",
    editorRoot,
  );
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
    "return document.querySelector(arguments[0]).textContent.includes('The saved project or library changed');",
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
  return { personId, qualificationId, teamId, assignmentTypeId, bucketId, calendarId, locationId };
}
async function scorePolicyDetail(sessionId, scenarioId, policyId) {
  const viewId = "eutheto.setup.entity_detail";
  return (
    await nativeSetupRequest(sessionId, scenarioId, "scenario_get_view", viewId, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId,
        parameters: { entityId: policyId, entityKind: "scorePolicy" },
      },
    })
  ).result.view.data.result.data;
}

function assertScorePolicyDetail(value, expected) {
  assert.equal(value.kind, "scorePolicy");
  assert.equal(value.id, expected.policyId);
  assert.equal(value.profileKey, expected.profileKey);
  assert.equal(value.tieBreak, "stableAssignmentRank");
  assert(
    value.levels.some(
      (level) => level.levelKey === expected.levelKey && level.label === expected.levelLabel,
    ),
    "The saved score policy must retain the authored mapped level",
  );
  assert(
    value.priorityMapping.some(
      (mapping) =>
        mapping.levelKey === expected.levelKey &&
        mapping.priority === "veryHigh" &&
        mapping.scale === expected.levelScale,
    ),
    "The saved score policy must map a priority to the authored level",
  );
  const workload = value.workloadPolicies[expected.workloadPolicyId];
  assert(workload, "The saved score policy must retain its workload policy identity");
  assert.equal(workload.bucketId, expected.bucketId);
  assert.equal(workload.calendarId, expected.calendarId);
  assert.equal(workload.membership, "reportingDate");
  assert.deepEqual(workload.peerGroup.people, {
    kind: "selected",
    personIds: [expected.personId],
  });
  assert.deepEqual(workload.targetMode, { kind: "equalShare" });
  assert.deepEqual(workload.penalty, { kind: "absolute" });
}

async function scorePolicyReopenAcceptance(sessionId, scenarioId, expected) {
  await navigate(sessionId, `/project/${scenarioId}/score-policy`);
  const listRoot = 'section[aria-labelledby="score-policy-list-heading"]';
  const editorRoot = 'section[aria-labelledby="score-policy-editor-heading"]';
  const policyRoot = `${editorRoot} article`;
  await waitForElement(sessionId, "#score-policy-list-heading");
  await waitFor(
    sessionId,
    "return !!document.querySelector(arguments[0] + ' ul li button:not(:disabled)');",
    [listRoot],
  );
  await activate(sessionId, `${listRoot} ul li button`);
  await waitForElement(sessionId, "#score-policy-editor-heading");
  await activateButton(sessionId, "Edit score policy", editorRoot);
  await waitForElement(sessionId, "#score-policy-profile-key");
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      "#score-policy-profile-key",
    ]),
    expected.profileKey,
    "Reopening the route must read the committed profile key",
  );
  const levelKeySelector = `${editorRoot} input[id^="score-level-"][id$="-key"]`;
  const levelLabelSelector = `${editorRoot} input[id^="score-level-"][id$="-label"]`;
  assert(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll(arguments[0])].some(input => input.value === arguments[1]);",
      [levelKeySelector, expected.levelKey],
    ),
    "Reopening the route must read the committed level key",
  );
  assert(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll(arguments[0])].some(input => input.value === arguments[1]);",
      [levelLabelSelector, expected.levelLabel],
    ),
    "Reopening the route must read the committed level label",
  );
  const policyText = await getText(sessionId, policyRoot);
  assert(
    policyText.includes(expected.bucketId) &&
      policyText.includes(expected.calendarId) &&
      policyText.includes(expected.personId),
    "Reopening the route must retain native workload reference identities",
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelector(arguments[0] + ' select[id$=\"-target-mode\"]').value;",
      [policyRoot],
    ),
    "equalShare",
    "Reopening the route must retain the native EqualShare target mode",
  );
  const persisted = await scorePolicyDetail(sessionId, scenarioId, expected.policyId);
  assertScorePolicyDetail(persisted, expected);
  await screenshot(sessionId, "score-policy-native.png");
  await activateButton(sessionId, "Close editor", editorRoot);
  await waitForElement(sessionId, "#score-policy-list-heading");
}

async function scorePolicyAcceptance(sessionId, scenarioId, people) {
  await navigate(sessionId, `/project/${scenarioId}/score-policy`);
  const listRoot = 'section[aria-labelledby="score-policy-list-heading"]';
  const editorRoot = 'section[aria-labelledby="score-policy-editor-heading"]';
  const reviewRoot = 'div[aria-live="polite"]';
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0])?.textContent.includes('No score policy has been saved.') ?? false;",
    [listRoot],
  );
  await activateButton(sessionId, "Create score policy", listRoot);
  await waitForElement(sessionId, "#score-policy-profile-key");
  await setValue(sessionId, "#score-policy-profile-key", "initial-preferences");
  const beforeDraftChange = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: beforeDraftChange.revision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native E2E independent revision" },
    truncateRedo: false,
    command: {
      type: "applyDomainCommand",
      payload: {
        commandType: "official.workforce.add_entity",
        payload: {
          entity: { id: requestId(), kind: "team", name: "Unrelated score-policy revision" },
        },
      },
    },
  });
  await waitFor(
    sessionId,
    "return document.querySelector('details.setup-more dl.metadata-list div:nth-child(2) dd')?.textContent.trim() === arguments[0] && !document.querySelector(arguments[1] + ' button[type=\"submit\"]')?.disabled;",
    [String(beforeDraftChange.revision + 1), editorRoot],
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      "#score-policy-profile-key",
    ]),
    "initial-preferences",
    "An unrelated native revision must retain the unsaved score policy draft",
  );
  await submitForm(sessionId, "#score-policy-profile-key");
  await waitForElement(sessionId, "#score-policy-review-heading");
  await activateButton(sessionId, "Return to unsaved score policy", reviewRoot);
  await waitForElement(sessionId, "#score-policy-profile-key");
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      "#score-policy-profile-key",
    ]),
    "initial-preferences",
    "Returning from native review must retain unsaved editor input",
  );
  await setValue(sessionId, "#score-policy-profile-key", "initial-preferences-reviewed");
  await submitForm(sessionId, "#score-policy-profile-key");
  await waitForElement(sessionId, "#score-policy-review-heading");
  await activateButton(sessionId, "Apply this reviewed score policy", reviewRoot);
  await idle(sessionId);
  await waitForElement(sessionId, "#score-policy-list-heading");
  await waitFor(
    sessionId,
    "return !!document.querySelector(arguments[0] + ' ul li button:not(:disabled)');",
    [listRoot],
  );
  const policyId = await evaluate(
    sessionId,
    "return document.querySelector(arguments[0] + ' ul li code')?.textContent.match(/[a-f0-9-]{36}/u)?.[0] ?? null;",
    [listRoot],
  );
  assert(policyId, "The score policy list must expose the native singleton identity");
  const original = await scorePolicyDetail(sessionId, scenarioId, policyId);
  assert.equal(original.kind, "scorePolicy");
  assert.equal(original.profileKey, "initial-preferences-reviewed");
  await activate(sessionId, `${listRoot} ul li button`);
  await waitForElement(sessionId, "#score-policy-editor-heading");
  await activateButton(sessionId, "Edit score policy", editorRoot);
  await waitForElement(sessionId, "#score-policy-profile-key");
  await setValue(sessionId, "#score-policy-profile-key", "e2e-score-policy");

  const levelKeySelector = `${editorRoot} input[id^="score-level-"][id$="-key"]`;
  const levelLabelSelector = `${editorRoot} input[id^="score-level-"][id$="-label"]`;
  await activateButton(sessionId, "Add score level", editorRoot);
  await waitFor(sessionId, "return document.querySelectorAll(arguments[0]).length === 2;", [
    levelKeySelector,
  ]);
  const authoredLevelKeyId = await indexedElementId(sessionId, levelKeySelector, 1);
  const authoredLevelLabelId = await indexedElementId(sessionId, levelLabelSelector, 1);
  await setValue(sessionId, `#${authoredLevelKeyId}`, "workload");
  await setValue(sessionId, `#${authoredLevelLabelId}`, "Workload target");
  const mappingLevelSelector = `${editorRoot} select[id^="score-mapping-"][id$="-level"]`;
  const mappingScaleSelector = `${editorRoot} input[id^="score-mapping-"][id$="-scale"]`;
  const authoredMappingLevelId = await indexedElementId(sessionId, mappingLevelSelector, 3);
  const authoredMappingScaleId = await indexedElementId(sessionId, mappingScaleSelector, 3);
  await selectValue(sessionId, `#${authoredMappingLevelId}`, "workload");
  await setValue(sessionId, `#${authoredMappingScaleId}`, "2500");

  await activateButton(sessionId, "Add workload policy", editorRoot);
  const policyRoot = `${editorRoot} article`;
  await waitFor(sessionId, "return document.querySelectorAll(arguments[0]).length === 1;", [
    policyRoot,
  ]);
  const workloadPolicyId = await evaluate(
    sessionId,
    "return document.querySelector(arguments[0] + ' p.break-all')?.textContent.match(/[a-f0-9-]{36}/u)?.[0] ?? null;",
    [policyRoot],
  );
  assert(workloadPolicyId, "An added workload policy must have a native stable identity");
  await choosePeopleReference(
    sessionId,
    "Workload bucket",
    "Existing scheduled minutes",
    policyRoot,
  );
  await choosePeopleReference(sessionId, "Calendar", "Existing daily target", policyRoot);
  await selectValue(sessionId, `${policyRoot} select[id$="-membership"]`, "reportingDate");
  await choosePeopleReference(sessionId, "People", "Local person name", policyRoot);
  await selectValue(sessionId, `${policyRoot} select[id$="-target-mode"]`, "equalShare");
  await selectValue(sessionId, `${policyRoot} select[id$="-penalty"]`, "absolute");

  const expected = {
    policyId,
    profileKey: "e2e-score-policy",
    levelKey: "workload",
    levelLabel: "Workload target",
    levelScale: 2500,
    workloadPolicyId,
    bucketId: people.bucketId,
    calendarId: people.calendarId,
    personId: people.personId,
  };
  const beforeInvalid = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await setValue(sessionId, `#${authoredMappingScaleId}`, "0");
  await submitForm(sessionId, "#score-policy-profile-key");
  await waitFor(
    sessionId,
    "return !document.querySelector('#score-policy-review-heading') && [...document.querySelectorAll('code')].some(code => code.textContent.trim() === 'priorityMapping.3.scale');",
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      `#${authoredMappingScaleId}`,
    ]),
    "0",
    "Rejected raw numeric input must remain visible in the editor",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeInvalid,
    "Rejected raw score policy input must not mutate the native scenario",
  );
  await setValue(sessionId, `#${authoredMappingScaleId}`, "2500");
  const beforeNativeInvalid = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await selectValue(sessionId, `${policyRoot} select[id$="-membership"]`, "intersection");
  await submitForm(sessionId, "#score-policy-profile-key");
  await waitFor(
    sessionId,
    "return !document.querySelector('#score-policy-review-heading') && !!document.querySelector('section[aria-labelledby=\"score-policy-editor-heading\"] [role=\"alert\"]');",
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      `${policyRoot} select[id$="-membership"]`,
    ]),
    "intersection",
    "Native rejection must preserve the authored invalid membership for correction",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeNativeInvalid,
    "Native score policy rejection must not mutate the scenario",
  );
  await selectValue(sessionId, `${policyRoot} select[id$="-membership"]`, "reportingDate");
  await submitForm(sessionId, "#score-policy-profile-key");
  await waitForElement(sessionId, "#score-policy-review-heading");
  const reviewText = await getText(sessionId, reviewRoot);
  assert(reviewText.includes("levels") && reviewText.includes("workloadPolicies"));
  await activateButton(sessionId, "Apply this reviewed score policy", reviewRoot);
  await idle(sessionId);
  await waitForElement(sessionId, "#score-policy-list-heading");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeInvalid + 1,
    "A reviewed score policy commit must advance the native revision once",
  );
  const saved = await scorePolicyDetail(sessionId, scenarioId, policyId);
  assertScorePolicyDetail(saved, expected);
  await scorePolicyReopenAcceptance(sessionId, scenarioId, expected);

  const historyRoot = '[aria-labelledby="history-heading"]';
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await waitForElement(sessionId, "#history-heading");
  await activateButton(sessionId, "Undo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario undo committed");
  const undone = await scorePolicyDetail(sessionId, scenarioId, policyId);
  assert.deepEqual(
    undone,
    original,
    "Native undo must restore the score policy detail without renderer authority",
  );
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Redo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario redo committed");
  const redone = await scorePolicyDetail(sessionId, scenarioId, policyId);
  assert.deepEqual(redone, saved, "Native redo must restore the committed score policy detail");
  await scorePolicyReopenAcceptance(sessionId, scenarioId, expected);
  return expected;
}

async function workloadBalanceAcceptance(sessionId, scenarioId, scorePolicy) {
  const preferenceEditor = '[aria-labelledby="rule-editor-heading"]';
  const workloadPolicyId = scorePolicy.workloadPolicyId;
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await waitForElement(sessionId, "#rules-heading");
  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Workload balance");
  const workloadBalanceId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(workloadBalanceId, "A new WorkloadBalance preference needs a stable UUIDv7");
  await waitForElement(sessionId, "#rule-workload-policy");
  await waitFor(
    sessionId,
    "const select = document.querySelector('#rule-workload-policy'); return !!select && !select.disabled && [...select.options].some(option => option.value === arguments[0]);",
    [workloadPolicyId],
  );
  await selectValue(sessionId, "#rule-workload-policy", workloadPolicyId);
  await selectValue(sessionId, "#rule-preference-priority", "high");
  await setValue(sessionId, "#rule-preference-weight", "17");
  const beforeReview = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return !!review && review.textContent.includes(arguments[0]) && review.textContent.includes('EqualShare') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
    [workloadPolicyId],
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const saved = await package8Rule(sessionId, scenarioId, workloadBalanceId, "preference");
  assert.equal(saved.class, "preference");
  assert.equal(saved.record.kind, "workloadBalance");
  assert.equal(saved.record.workloadPolicyId, workloadPolicyId);
  assert.equal(saved.record.priority, "high");
  assert.equal(saved.record.weight, 17);
  assert.equal(saved.record.active, true);
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeReview + 1,
    "A reviewed WorkloadBalance preference commit must advance the native revision once",
  );

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await waitForElement(sessionId, "#rules-heading");
  await selectValue(sessionId, "#rule-class-filter", "preference");
  const row = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[aria-labelledby=\"rule-list-heading\"] ul li button')].find(button => button.textContent.includes(arguments[0])) ?? null;",
    [workloadBalanceId],
  );
  await activateElement(sessionId, row);
  await waitForElement(sessionId, "#rule-editor-heading");
  await activateButton(sessionId, "Edit", preferenceEditor);
  await waitForElement(sessionId, "#rule-workload-policy");
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      "#rule-workload-policy",
    ]),
    workloadPolicyId,
    "Reopening the rule editor must retain the persisted workload-policy identity",
  );
  await activateButton(sessionId, "Discard");
  await waitForElement(sessionId, "#rule-list-heading");

  await navigate(sessionId, `/project/${scenarioId}/history`);
  await waitForElement(sessionId, "#history-heading");
  await activateButton(sessionId, "Undo scenario change", '[aria-labelledby="history-heading"]');
  await waitForOutcome(sessionId, "Scenario undo committed");
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await waitForElement(sessionId, "#rules-heading");
  await selectValue(sessionId, "#rule-class-filter", "preference");
  await waitFor(
    sessionId,
    "return ![...document.querySelectorAll('[aria-labelledby=\"rule-list-heading\"] ul li button')].some(button => button.textContent.includes(arguments[0]));",
    [workloadBalanceId],
  );
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Redo scenario change", '[aria-labelledby="history-heading"]');
  await waitForOutcome(sessionId, "Scenario redo committed");
  const redone = await package8Rule(sessionId, scenarioId, workloadBalanceId, "preference");
  assert.equal(redone.record.workloadPolicyId, workloadPolicyId);
  assert.equal(redone.record.priority, "high");
  assert.equal(redone.record.weight, 17);
  return { workloadBalanceId, workloadPolicyId };
}

async function workloadBalanceReopenAcceptance(sessionId, scenarioId, expected) {
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await waitForElement(sessionId, "#rules-heading");
  await selectValue(sessionId, "#rule-class-filter", "preference");
  const row = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[aria-labelledby=\"rule-list-heading\"] ul li button')].find(button => button.textContent.includes(arguments[0])) ?? null;",
    [expected.workloadBalanceId],
  );
  await activateElement(sessionId, row);
  await waitForElement(sessionId, "#rule-editor-heading");
  await activateButton(sessionId, "Edit", '[aria-labelledby="rule-editor-heading"]');
  await waitFor(
    sessionId,
    "const select = document.querySelector('#rule-workload-policy'); return !!select && !select.disabled && [...select.options].some(option => option.value === arguments[0]);",
    [expected.workloadPolicyId],
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
      "#rule-workload-policy",
    ]),
    expected.workloadPolicyId,
    "Restarting and reopening the route must read the persisted workload-policy identity",
  );
  const detail = await package8Rule(
    sessionId,
    scenarioId,
    expected.workloadBalanceId,
    "preference",
  );
  assert.equal(detail.record.kind, "workloadBalance");
  assert.equal(detail.record.workloadPolicyId, expected.workloadPolicyId);
  await activateButton(sessionId, "Discard", '[aria-labelledby="rule-editor-heading"]');
  await waitForElement(sessionId, "#rule-list-heading");
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
        `Select to edit with others · ${person.name} · ${person.id}`,
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
    await activateButton(sessionId, "Review changes to selected people", bulkRoot);
    await waitForElement(sessionId, "#bulk-review-heading");
    assert.equal(
      await evaluate(sessionId, "return document.activeElement?.id;"),
      "bulk-review-heading",
    );
  }
  async function save() {
    const before = await revision();
    await activateButton(sessionId, "Save changes to selected people", bulkRoot);
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
  await choosePeopleReference(sessionId, "Team to add or remove", "Native ward", bulkRoot);
  await preview();
  // Add is initially a no-op for A, but still owns the explicitly selected team field.
  await writeEntities(
    [{ ...people[0], teamIds: [seedTeamId] }],
    "official.workforce.update_entity",
  );
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes('The saved project or library changed');",
    [bulkRoot],
  );
  await activateButton(sessionId, "Review changes against the latest saved people", bulkRoot);
  await waitFor(
    sessionId,
    "return document.activeElement?.textContent === 'Changes to the same fields';",
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
    `${bulkRoot} section[aria-label="Complete record after this change"] input[name="name"]`,
    people[1].name,
  ]);
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0]).textContent.includes(arguments[1]);",
    [`${bulkRoot} section[aria-label="Complete record after this change"]`, support.teamId],
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
  await choosePeopleReference(sessionId, "Team to add or remove", "Native ward", bulkRoot);
  await preview();
  await save();
  await open();
  await selectValue(sessionId, "#bulk-action", "activeRange");
  await activateCheckbox(sessionId, "Only active during these dates", `${bulkRoot} form`);
  await setValue(sessionId, "#bulk-start-date", "not-a-date");
  await setValue(sessionId, "#bulk-end-date", "2030-02-01");
  const beforeInvalid = await revision();
  await activateButton(sessionId, "Review changes to selected people", bulkRoot);
  await waitFor(
    sessionId,
    "return document.activeElement?.textContent === 'Changes to selected people need attention';",
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
    "return document.querySelector(arguments[0]).textContent.includes('The saved project or library changed');",
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
  await activateButton(sessionId, "Review changes against the latest saved people", bulkRoot);
  await waitFor(
    sessionId,
    "return document.activeElement?.textContent === 'Changes to the same fields';",
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
        "return document.querySelector(arguments[0]).textContent.includes('The saved project or library changed');",
        [bulkRoot],
      );
      await activateButton(sessionId, "Review changes against the latest saved people", bulkRoot);
      await waitFor(
        sessionId,
        "return document.activeElement?.textContent === 'Changes to the same fields';",
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
    `${bulkRoot} section[aria-label="Complete record after this change"] input[id$="-startDate"]`,
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
  await activateButton(sessionId, "Review changes to selected people", bulkRoot);
  assert.equal(await revision(), beforeConfirmation);
  assert.equal(
    await evaluate(sessionId, "return !!document.querySelector('#bulk-review-heading');"),
    false,
  );
  await activateCheckbox(sessionId, "I want to delete these selected people", bulkRoot);
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
  await nativeChooser("Choose CSV snapshot");
  await waitForOutcome(sessionId, "Operation cancelled");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeCancel,
  );
  await activateButton(sessionId, "Choose a CSV file", root);
  await nativeChooser("Choose CSV snapshot", badPath);
  await waitFor(sessionId, "return document.querySelector(arguments[0] + ' [role=\"alert\"]');", [
    root,
  ]);
  assert.equal(await evaluate(sessionId, "return document.querySelector('#csv-dialect');"), null);
  await activateButton(sessionId, "Discard this source and choose another", root);
  await nativeChooser("Choose CSV snapshot", sourcePath);
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
  await activateButton(sessionId, "Review import changes", root);
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
  await activateButton(sessionId, "Review import changes", root);
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
    "Person to update from CSV record 6",
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
  await activateButton(sessionId, "Show a sample of this CSV record", manualRoot);
  await waitFor(
    sessionId,
    "return !!document.querySelector(arguments[0] + ' section[aria-labelledby$=\"-sample\"] dl dt');",
    [root],
  );
  await activateButton(sessionId, "Review import changes", root);
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
  await activateButton(sessionId, "Review import changes", root);
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
  assert.deepEqual(report.rows, {
    family: "people",
    rejectedRows: [{ record: 4, code: "columnCount" }],
  });
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
  await nativeChooser("Choose CSV snapshot", sourcePath);
  await waitForElement(sessionId, "#csv-dialect");
  await idle(sessionId);
  await selectValue(sessionId, "#csv-dialect", "comma");
  await selectValue(sessionId, "#csv-header", "yes");
  await setValue(sessionId, "#csv-width", "2");
  await selectValue(sessionId, 'select[data-source-index="0"]', "name");
  await selectValue(sessionId, 'select[data-source-index="1"]', "externalId");
  await decide(5, "Skip");
  await activateButton(sessionId, "Review import changes", root);
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
    // Search by command id; later undone entries remain in history so entries[0] is unreliable.
    const csvEntry = history.entries.find((e) => e.id === imported.commandId);
    assert(csvEntry, `CSV import entry ${imported.commandId} must exist in history`);
    assert.equal(csvEntry.applied, applied);
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

async function seedEligibilityWindow(sessionId, scenarioId) {
  // Extend the existing synthetic corpus through real native commands; never a renderer mock.
  const corpus = JSON.parse(
    await readFile(
      new URL("../../../domains/workforce/fixtures/v1/large-supported.json", import.meta.url),
      "utf8",
    ),
  );
  const entities = Object.values(corpus.domain.payload.entities);
  const template = entities.find((entity) => entity.kind === "assignmentType");
  assert(template, "The authoritative synthetic corpus must supply its assignment type");
  const types = Array.from({ length: 64 }, (_, index) => ({
    ...template,
    id: requestId(),
    name: `Matrix type ${(index + 1).toString().padStart(2, "0")}`,
  }));
  // Keep the measured 100×64 eligibility contract independent of the
  // 101-person solve corpus; only this bounded synthetic subset is seeded.
  const people = entities
    .filter((entity) => entity.kind === "person")
    .slice(0, 100)
    .map((entity, index) => ({
      ...entity,
      // The independently profiled corpus project must retain the source identities.
      id: requestId(),
      name: `Matrix person ${(index + 1).toString().padStart(3, "0")}`,
      eligibleAssignmentTypeIds: types.map((type) => type.id),
    }));
  assert.equal(people.length, 100);
  const current = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  assert(current);
  await nativeRequest(
    sessionId,
    "scenario_apply_command",
    {
      scenarioId,
      expectedRevision: current.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Native matrix fixture" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: {
          label: "Synthetic 100-person, 64-type matrix",
          commands: [...types, ...people].map((entity) => ({
            type: "applyDomainCommand",
            payload: { commandType: "official.workforce.add_entity", payload: { entity } },
          })),
        },
      },
    },
    90_000,
  );
  return { people, types };
}

async function package7SelectPerson(sessionId, personName) {
  // The person picker input IS #availability-person directly.
  await activate(sessionId, "#availability-person");
  await setValue(sessionId, "#availability-person", personName);
  await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[role=\"option\"]')].some(o => o.textContent.includes(arguments[0]));",
    [personName],
  );
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "pkg7-pick",
        actions: [
          { type: "keyDown", value: "\uE015" },
          { type: "keyUp", value: "\uE015" },
          { type: "keyDown", value: "\uE007" },
          { type: "keyUp", value: "\uE007" },
        ],
      },
    ],
  });
  await waitFor(
    sessionId,
    "return document.querySelector('[aria-labelledby=\"availability-records-heading\"]')?.textContent.includes(' saved records.');",
  );
}

async function package7WeeklyRowKey(sessionId, legendText) {
  const legendId = await evaluate(
    sessionId,
    "const l = [...document.querySelectorAll('legend')].find(el => el.textContent.trim() === arguments[0]); return l?.id ?? null;",
    [legendText],
  );
  assert(legendId, `Legend "${legendText}" must exist`);
  return legendId.replace("-heading", "");
}

async function package7FillExistingWeekly(sessionId, rowKey, startTime, endTime, weekdays) {
  await setValue(sessionId, `#${rowKey}-start`, startTime);
  await setValue(sessionId, `#${rowKey}-end`, endTime);
  for (const day of weekdays) {
    const cb = await waitFor(
      sessionId,
      "const legend = document.getElementById(arguments[0]); const outer = legend?.closest('fieldset'); const wk = [...(outer?.querySelectorAll('fieldset') ?? [])].find(fs => fs.querySelector(':scope > legend')?.textContent.trim() === 'Weekdays'); if (!wk) return null; for (const l of wk.querySelectorAll('label')) { if (l.textContent.trim().toLowerCase() === arguments[1]) { const c = l.querySelector('input[type=\"checkbox\"]'); if (c && !c.disabled) return c; } } return null;",
      [`${rowKey}-heading`, day],
    );
    await activateElement(sessionId, cb);
  }
}

async function assertPackage7Memberships(sessionId, scenarioId, seeded, applied) {
  const viewId = "official.workforce.setup.eligibility_matrix";
  const personIds = seeded.people.map((person) => person.id);
  const assignmentTypeIds = seeded.types.map((type) => type.id);
  const matrix = (
    await nativeSetupRequest(sessionId, scenarioId, "scenario_get_view", viewId, {
      source: { kind: "stored" },
      query: { schemaVersion: 1, viewId, parameters: { personIds, assignmentTypeIds } },
    })
  ).result.view.data.result.data;
  assert.deepEqual(matrix.personIds, personIds);
  assert.deepEqual(matrix.assignmentTypeIds, assignmentTypeIds);
  assert.deepEqual(
    matrix.configuredMemberships,
    personIds.map((personId) =>
      assignmentTypeIds.map(
        (typeId) =>
          !applied || (personId !== personIds.at(-1) && typeId !== assignmentTypeIds.at(-1)),
      ),
    ),
    "The entire row and column must change together; every other membership must survive",
  );
}

async function package7Acceptance(sessionId, scenarioId, directory, importedPeople) {
  const historyRoot = '[aria-labelledby="history-heading"]';

  // ── Phase 1: seed 100 people + 64 types ───────────────────────────────────
  console.log("Seeding the native 100-person, 64-type matrix");
  const seeded = await seedEligibilityWindow(sessionId, scenarioId);
  console.log("Native matrix seed accepted");

  // ── Phase 2: eligibility matrix ────────────────────────────────────────────
  await navigate(sessionId, `/project/${scenarioId}/eligibility`);
  await waitForElement(sessionId, "#eligibility-setup-heading");
  await waitFor(sessionId, "return !!document.querySelector('[data-eligibility-matrix]');");
  // Filter axes to isolate seeded entities from pre-existing people/work data.
  await setValue(sessionId, "#eligibility-people-search", "Matrix person");
  await setValue(sessionId, "#eligibility-type-search", "Matrix type");
  const renderStarted = Date.now();
  await activateButton(sessionId, "Find people and work types");
  await waitFor(
    sessionId,
    "const text = document.querySelector('[data-eligibility-matrix]')?.textContent; return text?.includes('100 of 100 people') && text.includes('64 of 64 work types');",
  );
  const matrixText = await getText(sessionId, "[data-eligibility-matrix]");
  assert(
    matrixText.includes("100 of 100 people"),
    "Filtered matrix must show all 100 seeded people",
  );
  assert(
    matrixText.includes("64 of 64 work types"),
    "Filtered matrix must show all 64 seeded types",
  );
  // Persist measured scroll dimensions.
  const matrixDimensions = await evaluate(
    sessionId,
    "const el = document.querySelector('[data-matrix-scroll]'); return el ? { scrollWidth: el.scrollWidth, scrollHeight: el.scrollHeight, viewportWidth: el.clientWidth, viewportHeight: el.clientHeight, renderedCells: el.querySelectorAll('input[type=\"checkbox\"]').length, renderedHtmlBytes: new TextEncoder().encode(el.outerHTML).byteLength } : null;",
  );
  assert(matrixDimensions, "Virtual matrix scroll container must exist");
  matrixDimensions.loadWebDriverRoundTripMs = Date.now() - renderStarted;
  await evaluate(
    sessionId,
    "document.querySelector('[data-matrix-scroll]').scrollIntoView({block:'center', behavior:'instant'});",
  );
  await screenshot(sessionId, "package7-matrix.png");

  // ── keyboard navigation: exercise arrow keys and offscreen Ctrl+End ────────
  const firstCellId = await evaluate(
    sessionId,
    "const cb = document.querySelector('[data-matrix-scroll] input[type=\"checkbox\"]'); if (cb) { cb.focus(); return cb.id; } return null;",
  );
  assert(firstCellId, "Matrix must have at least one cell checkbox");
  // ArrowRight → same row, next column.
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "pkg7-nav-right",
        actions: [
          { type: "keyDown", value: "\uE014" },
          { type: "keyUp", value: "\uE014" },
        ],
      },
    ],
  });
  const afterRight = await evaluate(
    sessionId,
    "const el = document.activeElement; return el ? { id: el.id, label: el.getAttribute('aria-label') } : null;",
  );
  assert(afterRight, "ArrowRight must keep focus in grid");
  assert(
    afterRight.label?.includes(seeded.people[0].name) &&
      afterRight.label?.includes(seeded.types[1].name),
    `ArrowRight must focus ${seeded.people[0].name} × ${seeded.types[1].name}`,
  );
  // ArrowDown → next row, same column.
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "pkg7-nav-down",
        actions: [
          { type: "keyDown", value: "\uE015" },
          { type: "keyUp", value: "\uE015" },
        ],
      },
    ],
  });
  const afterDown = await evaluate(
    sessionId,
    "const el = document.activeElement; return el ? { id: el.id, label: el.getAttribute('aria-label') } : null;",
  );
  assert(
    afterDown?.label?.includes(seeded.people[1].name) &&
      afterDown?.label?.includes(seeded.types[1].name),
    `ArrowDown must focus ${seeded.people[1].name} × ${seeded.types[1].name}`,
  );
  // Ctrl+End → last cell (last person × last type), likely offscreen.
  // End = \uE010, Home = \uE011; \uE023 is NUMPAD9, NOT End.
  const cornerStarted = Date.now();
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "pkg7-ctrl-end",
        actions: [
          { type: "keyDown", value: "\uE009" },
          { type: "keyDown", value: "\uE010" },
          { type: "keyUp", value: "\uE010" },
          { type: "keyUp", value: "\uE009" },
        ],
      },
    ],
  });
  const lastCell = await evaluate(
    sessionId,
    "const el = document.activeElement; return el ? { id: el.id, label: el.getAttribute('aria-label') } : null;",
  );
  assert(lastCell, "Ctrl+End must focus a cell");
  const lastPerson = seeded.people[seeded.people.length - 1].name;
  const lastType = seeded.types[seeded.types.length - 1].name;
  assert(
    lastCell.label?.includes(lastPerson) && lastCell.label?.includes(lastType),
    `Ctrl+End must focus ${lastPerson} × ${lastType}, got "${lastCell.label}"`,
  );
  matrixDimensions.cornerWebDriverRoundTripMs = Date.now() - cornerStarted;
  matrixDimensions.cornerRenderedCells = await evaluate(
    sessionId,
    "return document.querySelectorAll('[data-matrix-scroll] input[type=\"checkbox\"]').length;",
  );
  console.log("Native 100 × 64 matrix DOM measurements:", JSON.stringify(matrixDimensions));
  await writeFile(
    new URL("../../../.cache/e2e/package7-matrix-measurements.json", import.meta.url),
    `${JSON.stringify(matrixDimensions, null, 2)}\n`,
  );

  // Enter opens the selected person's inspector without changing membership.
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "pkg7-inspector",
        actions: [
          { type: "keyDown", value: "\uE007" },
          { type: "keyUp", value: "\uE007" },
        ],
      },
    ],
  });
  await waitFor(
    sessionId,
    "return document.activeElement?.id === 'assignment-inspection-heading';",
  );
  assert(
    (await getText(sessionId, "[data-eligibility-matrix]")).includes(lastPerson),
    "Opening the inspector must preserve the selected person",
  );
  await evaluate(sessionId, "document.getElementById(arguments[0]).focus();", [lastCell.id]);
  assert.equal(
    await evaluate(sessionId, "return document.activeElement.checked;"),
    true,
    "Opening the inspector must not toggle the configured membership",
  );

  // Toggle the focused cell with Space.
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "pkg7-toggle",
        actions: [
          { type: "keyDown", value: " " },
          { type: "keyUp", value: " " },
        ],
      },
    ],
  });
  const pendingText = await evaluate(
    sessionId,
    "return document.querySelector('[data-eligibility-matrix]').textContent;",
  );
  assert(pendingText.includes("Pending"), "Toggled cell must show Pending marker");

  // ── equivalent paged table ─────────────────────────────────────────────────
  await activateButton(sessionId, "Use equivalent paged table");
  const pagedTable = await waitFor(
    sessionId,
    "return document.querySelector('[aria-label=\"Paged configured membership table\"]');",
  );
  assert(pagedTable, "Paged table must render after mode switch");
  const pagedCell = await evaluate(
    sessionId,
    'const cb = document.activeElement; return cb?.closest(\'[aria-label="Paged configured membership table"]\') ? { id: cb.id, label: cb.getAttribute("aria-label"), checked: cb.checked } : null;',
  );
  assert(pagedCell, "Paged table must have at least one cell");
  assert.equal(
    pagedCell.label,
    lastCell.label,
    "Mode switch must preserve the far-corner active cell",
  );
  assert.equal(pagedCell.checked, false, "Pending membership must survive the mode switch");
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "pkg7-paged-toggle",
        actions: [
          { type: "keyDown", value: " " },
          { type: "keyUp", value: " " },
        ],
      },
    ],
  });
  await activateButton(sessionId, "Use virtual matrix");

  // Disallow one complete column and one complete row in a single 100-person batch.
  const bulkTypeName = lastType;
  await activateButton(sessionId, `Disallow ${bulkTypeName} for all loaded people`);
  await activateButton(sessionId, `Disallow all loaded types for ${lastPerson}`);

  // ── stale-before-apply: concurrent person edit and SELECTED type rename ────
  const beforeStale = (await projects(sessionId)).find((p) => p.scenarioId === scenarioId);
  const stalePersonId = seeded.people.at(-1).id;
  const stalePerson = {
    ...seeded.people.at(-1),
    tags: [...seeded.people.at(-1).tags, "stale-test"],
  };
  const renamedType = { ...seeded.types.at(-1), name: `${lastType} (updated)` };
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: beforeStale.revision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Package7 stale-mutation" },
    truncateRedo: false,
    command: {
      type: "applyBatch",
      payload: {
        label: "Concurrent selected membership records",
        commands: [stalePerson, renamedType].map((entity) => ({
          type: "applyDomainCommand",
          payload: { commandType: "official.workforce.update_entity", payload: { entity } },
        })),
      },
    },
  });
  // Stale warning is a sibling of the heading, not inside [data-eligibility-matrix].
  await waitFor(
    sessionId,
    "return document.querySelector('[aria-labelledby=\"eligibility-setup-heading\"]').parentElement.textContent.includes('saved revision changed');",
  );

  // ── preview (re-review against current revision) ───────────────────────────
  const previewStarted = Date.now();
  await activateButton(sessionId, "Review choices against the latest saved version");
  await waitForElement(sessionId, "#eligibility-review-heading", 120_000);
  console.log(`Native 100-person membership preview: ${String(Date.now() - previewStarted)} ms`);
  const reviewInfo = await getText(sessionId, '[aria-labelledby="eligibility-review-heading"]');
  assert.match(reviewInfo, /\b[1-9][\d,]* changes\./u, "Review must report a nonzero change count");
  const reviewedType = await evaluate(
    sessionId,
    "return document.querySelector('[aria-label=\"Work type choices before and after\"] tbody tr td')?.textContent?.trim() ?? '';",
  );
  assert(
    reviewedType.startsWith(renamedType.name),
    `Current-revision membership approval must name the renamed assignment type, got ${reviewedType}`,
  );
  const beforeApply = (await projects(sessionId)).find((p) => p.scenarioId === scenarioId);

  // ── apply ──────────────────────────────────────────────────────────────────
  const applyStarted = Date.now();
  await activateButton(sessionId, "Save reviewed work type choices");
  await idle(sessionId, 120_000);
  console.log(`Native 100-person membership apply: ${String(Date.now() - applyStarted)} ms`);
  const afterEligibility = (await projects(sessionId)).find((p) => p.scenarioId === scenarioId);
  assert.equal(
    afterEligibility.revision,
    beforeApply.revision + 1,
    "One batch must commit exactly one revision",
  );

  await assertPackage7Memberships(sessionId, scenarioId, seeded, true);
  // Verify tags survived on the selected person.
  const stalePersonVerify = (
    await nativeSetupRequest(
      sessionId,
      scenarioId,
      "scenario_get_entity",
      "eutheto.setup.entity_detail",
      {
        kind: "person",
        entityId: stalePersonId,
      },
    )
  ).result.view.data.result.data;
  assert(
    stalePersonVerify.tags?.includes("stale-test"),
    "Concurrent tag mutation must survive alongside membership edits",
  );
  await waitForElement(sessionId, "[data-matrix-scroll]");
  await evaluate(
    sessionId,
    "document.querySelector('[data-matrix-scroll]').scrollIntoView({block:'center', behavior:'instant'});",
  );
  await screenshot(sessionId, "package7-matrix-applied.png");

  // ── single-step undo (revision always increases monotonically) ─────────────
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Undo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
  const afterUndoElig = (await projects(sessionId)).find((p) => p.scenarioId === scenarioId);
  assert.equal(afterUndoElig.revision, afterEligibility.revision + 1);
  await assertPackage7Memberships(sessionId, scenarioId, seeded, false);
  const historyAfterUndo = (
    await nativeRequest(sessionId, "scenario_get_history_page", {
      schemaVersion: 1,
      scenarioId,
      expectedRevision: afterUndoElig.revision,
      limit: 10,
      continuation: null,
    })
  ).result;
  const undoneEntry = historyAfterUndo.entries.find(
    (entry) =>
      entry.revisionBefore === beforeApply.revision &&
      entry.revisionAfter === afterEligibility.revision,
  );
  assert(undoneEntry, "The membership batch must be present in history");
  assert.equal(undoneEntry.applied, false);

  // ── single-step redo ───────────────────────────────────────────────────────
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Redo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario redo committed", 120_000);
  const afterRedoElig = (await projects(sessionId)).find((p) => p.scenarioId === scenarioId);
  assert.equal(afterRedoElig.revision, afterUndoElig.revision + 1);
  await assertPackage7Memberships(sessionId, scenarioId, seeded, true);
  const historyAfterRedo = (
    await nativeRequest(sessionId, "scenario_get_history_page", {
      schemaVersion: 1,
      scenarioId,
      expectedRevision: afterRedoElig.revision,
      limit: 10,
      continuation: null,
    })
  ).result;
  const redoneEntry = historyAfterRedo.entries.find((entry) => entry.id === undoneEntry.id);
  assert(redoneEntry, "The membership batch must remain present in history");
  assert.equal(redoneEntry.applied, true);

  // ── Phase 3: availability authoring (all 3 kinds, instant+weekly) ─────────
  // A real manual shift makes the person–assignment rejection observable.
  const shiftInstanceId = requestId();
  const shiftTypeId = seeded.types[0].id;
  const shiftRevision = (await projects(sessionId)).find(
    (p) => p.scenarioId === scenarioId,
  ).revision;
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: shiftRevision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Package7 shift fixture" },
    truncateRedo: false,
    command: {
      type: "applyBatch",
      payload: {
        label: "Package7 shift fixtures",
        commands: [
          {
            type: "applyDomainCommand",
            payload: {
              commandType: "official.workforce.add_entity",
              payload: {
                entity: {
                  id: shiftInstanceId,
                  kind: "shiftInstance",
                  assignmentTypeId: shiftTypeId,
                  coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
                  startsAt: {
                    instant: "2030-01-16T08:00:00Z",
                    local: "2030-01-16T08:00:00",
                    offsetSeconds: 0,
                  },
                  endsAt: {
                    instant: "2030-01-16T17:00:00Z",
                    local: "2030-01-16T17:00:00",
                    offsetSeconds: 0,
                  },
                  origin: { kind: "manual" },
                  reportingAttribution: "startLocalDate",
                  tags: [],
                },
              },
            },
          },
        ],
      },
    },
  });

  // Navigate to availability and select a seeded person.
  await navigate(sessionId, `/project/${scenarioId}/availability`);
  await waitForElement(sessionId, "#availability-heading");
  await package7SelectPerson(sessionId, "Matrix person 001");
  const initialRecords = await getText(
    sessionId,
    '[aria-labelledby="availability-records-heading"]',
  );
  assert(
    initialRecords.includes("No availability records"),
    "Seeded person starts with no records",
  );

  // ── Record 1: unavailable + weekly + type restriction ──────────────────────
  // Form starts with windowKind=weekly and ONE empty weekly row (no "Add weekly entry" click).
  await activateButton(sessionId, "Add availability record");
  await waitForElement(sessionId, "#availability-editor-heading");
  await selectValue(sessionId, "#availability-kind", "unavailable");
  await setValue(sessionId, "#availability-start-date", "2030-01-01");
  await setValue(sessionId, "#availability-end-date", "2030-02-01");
  // Weekly is default; fill the existing "Weekly schedule 1" row via its legend id.
  const rowKey1 = await package7WeeklyRowKey(sessionId, "Weekly schedule 1");
  await package7FillExistingWeekly(sessionId, rowKey1, "08:00", "17:00", ["monday"]);
  // Limit to specific work types.
  await activateCheckbox(sessionId, "Limit to certain work types");
  await waitFor(sessionId, "return !!document.querySelector('#availability-types');");
  // #availability-types IS the combobox input directly.
  await activate(sessionId, "#availability-types");
  await setValue(sessionId, "#availability-types", seeded.types[0].name);
  await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[role=\"option\"]')].some(o => o.textContent.includes(arguments[0]));",
    [seeded.types[0].name],
  );
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "pkg7-type-pick",
        actions: [
          { type: "keyDown", value: "\uE015" },
          { type: "keyUp", value: "\uE015" },
          { type: "keyDown", value: "\uE007" },
          { type: "keyUp", value: "\uE007" },
        ],
      },
    ],
  });
  // Inert source/note with HTML-like literal text.
  await setValue(sessionId, "#availability-source", "e2e-source-unavailable");
  const beforeRejectedReview = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await setValue(sessionId, "#availability-note", "<script>alert(1)</script>");
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "return document.activeElement?.id === 'availability-errors-heading' && document.querySelector('[aria-labelledby=\"availability-errors-heading\"]')?.textContent.includes('command.prohibited_data');",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeRejectedReview,
    "A rejected preview must not mutate the project",
  );
  assert.equal(
    await evaluate(sessionId, "return document.querySelector('#availability-note').value;"),
    "<script>alert(1)</script>",
    "Rejected input remains available for explicit repair",
  );
  await setValue(
    sessionId,
    "#availability-note",
    "literal <b data-package7-note>bold</b> & plain text",
  );
  await activateButton(sessionId, "Review changes");
  await waitForElement(sessionId, "#availability-review-heading");
  // Assert literal note text is present and no DOM injection.
  const noteCheck1 = await evaluate(
    sessionId,
    "const main = document.querySelector('main'); return { literal: main.textContent.includes('literal <b data-package7-note>bold</b> & plain text'), injected: !!main.querySelector('[data-package7-note]') };",
  );
  assert(noteCheck1.literal, "Note must retain its literal markup as text");
  assert(!noteCheck1.injected, "Note markup must not become a DOM element");
  await screenshot(sessionId, "package7-review-unavailable.png");
  await activateButton(sessionId, "Apply changes");
  await idle(sessionId);

  // ── Record 2: availableOnly + instant + location restriction ───────────────
  await activateButton(sessionId, "Add availability record");
  await waitForElement(sessionId, "#availability-editor-heading");
  await selectValue(sessionId, "#availability-kind", "availableOnly");
  await setValue(sessionId, "#availability-start-date", "2030-01-10");
  await setValue(sessionId, "#availability-end-date", "2030-01-20");
  // Switch to instant window.
  await selectValue(sessionId, "#availability-window-kind", "instant");
  // Fill the instant DateTimeRangeField (#availability-local-interval).
  await setValue(sessionId, "#availability-local-interval-start", "2030-01-15T09:00:00.000000001");
  await setValue(sessionId, "#availability-local-interval-end", "2030-01-15T17:00:00.000000001");
  // Restrict to locations.
  await activateCheckbox(sessionId, "Restrict to locations");
  await waitFor(sessionId, "return !!document.querySelector('#availability-locations');");
  await activate(sessionId, "#availability-locations");
  await setValue(sessionId, "#availability-locations", "Existing home ward");
  await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[role=\"option\"]')].some(o => o.textContent.includes('Existing home ward'));",
  );
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "pkg7-loc-pick",
        actions: [
          { type: "keyDown", value: "\uE015" },
          { type: "keyUp", value: "\uE015" },
          { type: "keyDown", value: "\uE007" },
          { type: "keyUp", value: "\uE007" },
        ],
      },
    ],
  });
  await setValue(sessionId, "#availability-source", "e2e-source-availableOnly");
  await setValue(sessionId, "#availability-note", "availableOnly inert note");
  await activateButton(sessionId, "Review changes");
  await waitForElement(sessionId, "#availability-review-heading");
  await activateButton(sessionId, "Apply changes");
  await idle(sessionId);

  // ── Record 3: approvedTimeOff + weekly (overlaps the shift instance) ───────
  await activateButton(sessionId, "Add availability record");
  await waitForElement(sessionId, "#availability-editor-heading");
  await selectValue(sessionId, "#availability-kind", "approvedTimeOff");
  await setValue(sessionId, "#availability-start-date", "2030-01-13");
  await setValue(sessionId, "#availability-end-date", "2030-01-20");
  // Weekly is default; fill the existing row.
  const rowKey3 = await package7WeeklyRowKey(sessionId, "Weekly schedule 1");
  await package7FillExistingWeekly(sessionId, rowKey3, "08:00", "17:00", ["wednesday"]);
  await setValue(sessionId, "#availability-source", "e2e-source-approvedTimeOff");
  await setValue(sessionId, "#availability-note", "approvedTimeOff inert");
  await activateButton(sessionId, "Review changes");
  await waitForElement(sessionId, "#availability-review-heading");
  await activateButton(sessionId, "Apply changes");
  await idle(sessionId);

  // ── verify persisted records ───────────────────────────────────────────────
  await waitFor(
    sessionId,
    "return document.querySelector('[aria-labelledby=\"availability-records-heading\"]')?.textContent.includes('3 saved records.');",
  );
  const recordsAfter = await getText(sessionId, '[aria-labelledby="availability-records-heading"]');
  assert(recordsAfter.includes("3 saved records"), "Three availability records must persist");
  assert(recordsAfter.includes("Unavailable"), "Record kind Unavailable must be visible");
  assert(recordsAfter.includes("Available only"), "Record kind Available only must be visible");
  assert(
    recordsAfter.includes("Approved time off"),
    "Record kind Approved time off must be visible",
  );

  // ── calendar occurrences ───────────────────────────────────────────────────
  await waitFor(
    sessionId,
    "return !!document.querySelector('[aria-labelledby=\"availability-calendar-heading\"]');",
  );
  await evaluate(
    sessionId,
    "const s = document.querySelector('#availability-calendar-start'); const e = document.querySelector('#availability-calendar-end'); if (s) { s.value = '2030-01-01'; s.dispatchEvent(new Event('input', {bubbles:true})); } if (e) { e.value = '2030-02-01'; e.dispatchEvent(new Event('input', {bubbles:true})); }",
  );
  await activateButton(sessionId, "Show range");
  await waitFor(
    sessionId,
    "const list = document.querySelector('[aria-label=\"Availability intervals on this page\"]'); return list?.textContent.includes('2030-01-07T08:00:00Z') && list.textContent.includes('2030-01-16T08:00:00Z') && list.textContent.includes('2030-01-15T09:00:00.000000001Z');",
  );
  await screenshot(sessionId, "package7-calendar.png");

  // ── inspector: approvedTimeOff blocks the shift ────────────────────────────
  await setValue(sessionId, "#inspection-start-date", "01/16/2030");
  await setValue(sessionId, "#inspection-end-date", "01/17/2030");
  await activateButton(
    sessionId,
    "Show shifts",
    '[aria-labelledby="assignment-inspection-heading"]',
  );
  await activateButton(
    sessionId,
    "Inspect this shift",
    '[aria-labelledby="assignment-inspection-heading"]',
  );
  await waitFor(
    sessionId,
    "const result = document.querySelector('[aria-labelledby=\"assignment-inspection-result\"]'); return result?.textContent.includes('Approved time off overlaps this shift.');",
  );
  const inspection = await getText(sessionId, '[aria-labelledby="assignment-inspection-result"]');
  assert(
    inspection.includes(shiftInstanceId),
    "The rejection must belong to the selected native shift",
  );
  await screenshot(sessionId, "package7-availability.png");
  const historyHead = await appliedHistoryHead(sessionId, scenarioId);
  assert(historyHead, "Package 7 must finish with an applied history command");

  // Return expected state for restart verification.
  return {
    historyHead,
    seeded,
    matrixDimensions,
    csvCommandId: importedPeople.commandId,
    seededPersonId: seeded.people[0].id,
    seededPersonName: seeded.people[0].name,
    seededTypeCount: seeded.types.length,
    seededPeopleCount: seeded.people.length,
    shiftInstanceId,
    shiftTypeId,
    expectedAvailabilityKinds: ["Unavailable", "Available only", "Approved time off"],
    bulkTypeName,
    disallowedPersonId: stalePersonId,
  };
}

async function package7RestartAcceptance(sessionId, scenarioId, expected) {
  const historyRoot = '[aria-labelledby="history-heading"]';

  await assertPackage7Memberships(sessionId, scenarioId, expected.seeded, true);
  await navigate(sessionId, `/project/${scenarioId}/eligibility`);
  await waitForElement(sessionId, "#eligibility-setup-heading");
  await waitFor(sessionId, "return !!document.querySelector('[data-eligibility-matrix]');");
  await setValue(sessionId, "#eligibility-people-search", "Matrix person");
  await setValue(sessionId, "#eligibility-type-search", "Matrix type");
  await activateButton(sessionId, "Find people and work types");
  await waitFor(
    sessionId,
    "const text = document.querySelector('[data-eligibility-matrix]')?.textContent; return text?.includes('100 of 100 people') && text.includes('64 of 64 work types');",
  );
  const restartMatrixText = await getText(sessionId, "[data-eligibility-matrix]");
  assert(
    restartMatrixText.includes(
      `${String(expected.seededPeopleCount)} of ${String(expected.seededPeopleCount)}`,
    ),
    "Seeded people count must survive restart",
  );
  assert(
    restartMatrixText.includes(
      `${String(expected.seededTypeCount)} of ${String(expected.seededTypeCount)}`,
    ),
    "Seeded type count must survive restart",
  );
  await screenshot(sessionId, "package7-restart-matrix.png");

  // ── verify availability records persist ────────────────────────────────────
  await navigate(sessionId, `/project/${scenarioId}/availability`);
  await waitForElement(sessionId, "#availability-heading");
  await package7SelectPerson(sessionId, expected.seededPersonName);
  const recordsText = await getText(sessionId, '[aria-labelledby="availability-records-heading"]');
  assert(recordsText.includes("3 saved records"), "Three records must survive restart");
  for (const kind of expected.expectedAvailabilityKinds)
    assert(recordsText.includes(kind), `Record kind ${kind} must survive restart`);
  await screenshot(sessionId, "package7-restart-records.png");

  // ── verify calendar occurrences persist ────────────────────────────────────
  await waitFor(
    sessionId,
    "return !!document.querySelector('[aria-labelledby=\"availability-calendar-heading\"]');",
  );
  await evaluate(
    sessionId,
    "const s = document.querySelector('#availability-calendar-start'); const e = document.querySelector('#availability-calendar-end'); if (s) { s.value = '2030-01-01'; s.dispatchEvent(new Event('input', {bubbles:true})); } if (e) { e.value = '2030-02-01'; e.dispatchEvent(new Event('input', {bubbles:true})); }",
  );
  await activateButton(sessionId, "Show range");
  await waitFor(
    sessionId,
    "const list = document.querySelector('[aria-label=\"Availability intervals on this page\"]'); return list?.textContent.includes('2030-01-07T08:00:00Z') && list.textContent.includes('2030-01-16T08:00:00Z') && list.textContent.includes('2030-01-15T09:00:00.000000001Z');",
  );

  // ── restart undo/redo: undo availability record, then redo ─────────────────
  await navigate(sessionId, `/project/${scenarioId}/history`);
  const beforeRestartUndo = (await projects(sessionId)).find(
    (p) => p.scenarioId === scenarioId,
  ).revision;
  await activateButton(sessionId, "Undo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario undo committed");
  const afterRestartUndo = (await projects(sessionId)).find(
    (p) => p.scenarioId === scenarioId,
  ).revision;
  assert(afterRestartUndo > beforeRestartUndo, "Restart undo must increase revision");
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Redo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario redo committed");
  const afterRestartRedo = (await projects(sessionId)).find(
    (p) => p.scenarioId === scenarioId,
  ).revision;
  assert(afterRestartRedo > afterRestartUndo, "Restart redo must increase revision");

  // ── undo all package7 additions back to CSV command head ───────────────────
  const csvCommandId = expected.csvCommandId;
  const maxUndos = 32;
  for (let i = 0; i < maxUndos; i++) {
    const proj = (await projects(sessionId)).find((p) => p.scenarioId === scenarioId);
    const page = (
      await nativeRequest(sessionId, "scenario_get_history_page", {
        schemaVersion: 1,
        scenarioId,
        expectedRevision: proj.revision,
        limit: 50,
        continuation: null,
      })
    ).result;
    const csvEntry = page.entries.find((e) => e.id === csvCommandId);
    assert(csvEntry?.applied, "The CSV boundary must remain applied in the bounded history page");
    const mostRecentApplied = page.entries.find((e) => e.applied);
    if (mostRecentApplied?.id === csvCommandId) break;
    await navigate(sessionId, `/project/${scenarioId}/history`);
    await activateButton(sessionId, "Undo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
  }
  // Verify CSV is now the most recent applied entry.
  const finalProj = (await projects(sessionId)).find((p) => p.scenarioId === scenarioId);
  const finalPage = (
    await nativeRequest(sessionId, "scenario_get_history_page", {
      schemaVersion: 1,
      scenarioId,
      expectedRevision: finalProj.revision,
      limit: 50,
      continuation: null,
    })
  ).result;
  const finalCsv = finalPage.entries.find((e) => e.id === csvCommandId);
  assert(finalCsv?.applied, "CSV import must remain applied after package7 undo cycle");
  const mostRecentFinal = finalPage.entries.find((e) => e.applied);
  assert.equal(
    mostRecentFinal?.id,
    csvCommandId,
    "CSV import must be at history head after restore",
  );
  await screenshot(sessionId, "package7-restart-restored.png");
}

async function package8Rule(sessionId, scenarioId, ruleId, recordClass = "required") {
  const viewId = "official.workforce.setup.rule_detail";
  return (
    await nativeSetupRequest(sessionId, scenarioId, "scenario_get_view", viewId, {
      source: { kind: "stored" },
      query: { schemaVersion: 1, viewId, parameters: { rule: { class: recordClass, ruleId } } },
    })
  ).result.view.data.result.data;
}

async function optimizeHandoffAcceptance(sessionId, scenarioId, ready, stateName) {
  const before = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  await selectValue(sessionId, 'select:has(option[value="optimize"])', "optimize");
  await activate(sessionId, "#setup-results-details > summary");
  const capabilities = (await nativeRequest(sessionId, "app_get_capabilities", {})).result;
  const nativeSolve = capabilities.availableCommands.includes("solve_start");
  assert(capabilities.availableCommands.includes("solve_cancel"));
  assert(
    nativeSolve || capabilities.unavailableCommands.includes("solve_start"),
    "Native capability discovery must explicitly classify solve_start",
  );
  await waitForElement(
    sessionId,
    `[data-optimize-state="${ready ? (nativeSolve ? "ready" : "validation-ready") : "validation-not-ready"}"]`,
  );
  const visible = await evaluate(
    sessionId,
    `const panel = document.querySelector('[data-optimize-panel]');
    return {
      executionReady: panel.getAttribute('data-optimize-ready'),
      solveControls: panel.querySelectorAll('input[name="optimize-mode"], #optimize-maximum-time').length +
        [...panel.querySelectorAll('button')].filter(node => node.textContent.trim() === 'Start optimization').length,
      estimateDisabled: panel.querySelector('[data-optimize-estimate] button')?.disabled,
      commandStates: [...panel.querySelectorAll('dd')].slice(0, 3).map(node => node.textContent)
    };`,
  );
  assert.match(visible.commandStates[0] ?? "", nativeSolve ? /\bavailable\b/i : /\bunavailable\b/i);
  assert.match(visible.commandStates[1] ?? "", /\bavailable\b/i);
  const estimateAvailable = capabilities.availableCommands.includes("solve_estimate_model");
  assert.match(
    visible.commandStates[2] ?? "",
    estimateAvailable ? /\bavailable\b/i : /\bunavailable\b/i,
  );
  assert.equal(visible.estimateDisabled, !ready || !estimateAvailable);
  if (ready && nativeSolve) {
    assert(visible.solveControls > 0, "A validated native solve offers mode and action controls");
  } else {
    assert.equal(visible.executionReady, "false");
    assert.equal(visible.solveControls, 0, "An unavailable or not-ready solve cannot start");
    if (ready) await waitForElement(sessionId, '[data-optimize-state="unsupported"]');
  }
  if (ready && estimateAvailable) {
    await activateButton(sessionId, "Estimate model", "[data-optimize-estimate]");
    await waitForElement(sessionId, '[data-optimize-state="estimate-compiled"]');
    const evidence = await evaluate(
      sessionId,
      `const result = document.querySelector('[data-optimize-estimate-result]');
       return {
         message: result.querySelector('[data-optimize-state="estimate-compiled"]').textContent,
         labels: [...result.querySelectorAll('dt')].map(node => node.textContent.trim()),
         values: [...result.querySelectorAll('dd')].map(node => node.textContent.trim()),
         stale: !!result.querySelector('[data-optimize-state="estimate-stale"]')
       };`,
    );
    assert.match(evidence.message, /compiled, not solved; feasibility unknown/i);
    assert.equal(evidence.labels.length, 5);
    assert.equal(evidence.values.length, 5);
    assert(
      evidence.values.every((value) => /^\d+$/.test(value)),
      "Workforce exact compiled counts must be numeric",
    );
    assert.equal(evidence.stale, false);
  }
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    before.revision,
    "Inspecting the Optimize handoff must not mutate the scenario",
  );
  await evaluate(
    sessionId,
    "document.querySelector('[data-optimize-panel]').scrollIntoView({block:'start'});",
  );
  await screenshot(sessionId, `package9-optimize-${stateName}.png`);
}
async function packagedAssignmentPreferenceAcceptance(sessionId, scenarioId, expected) {
  const workType = expected.seeded.types.find((entry) => entry.id === expected.shiftTypeId);
  assert(workType, "Packaged accepted shift needs an existing native work type");
  // A bounded source population leaves a modeled, unselected candidate. Preferring
  // this work for everyone would select every eligible person under AtLeast coverage.
  let preferredRuleId = null;
  for (const [person, direction] of [
    [expected.seeded.people[1], "prefer"],
    [expected.seeded.people[2], "avoid"],
    [expected.seeded.people[3], "prefer"],
  ]) {
    await navigate(sessionId, `/project/${scenarioId}/rules`);
    await openPersonOptions(sessionId, "Preferences", "body");
    await activateButton(sessionId, "Assignment type preferences");
    const id = await waitFor(
      sessionId,
      "return document.querySelector('#rule-identity')?.textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0] ?? null;",
    );
    const editor = '[aria-labelledby="rule-editor-heading"]';
    await selectValue(sessionId, `#rule-${id}-main-people-mode`, "selected");
    await choosePeopleReference(sessionId, "Selected people", person.name, editor);
    await choosePeopleReference(sessionId, "Preferred work types", workType.name, editor);
    await selectValue(sessionId, "#rule-preference-direction", direction);
    await selectValue(sessionId, "#rule-preference-priority", "high");
    await activateButton(sessionId, "Review changes", editor);
    await activateButton(sessionId, "Save reviewed change");
    await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
    const saved = await package8Rule(sessionId, scenarioId, id, "preference");
    assert.equal(saved.record.active, true);
    assert.equal(saved.record.kind, "assignmentType");
    assert.equal(saved.record.direction, direction);
    assert.deepEqual(saved.record.assignmentTypeIds, [expected.shiftTypeId]);
    assert.deepEqual(saved.record.scope.people, { kind: "selected", personIds: [person.id] });
    if (direction === "prefer" && preferredRuleId === null) preferredRuleId = id;
  }
  assert(preferredRuleId, "The selected source pair needs its native preference identity");
  return preferredRuleId;
}

async function packagedSolveAcceptance(
  sessionId,
  scenarioId,
  preferenceId,
  preferredShiftId,
  replacementPersonId,
  excludedPersonId,
  alreadyAssignedPersonId,
) {
  const current = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  assert(current, "The packaged solve project must exist");
  await activate(sessionId, '[data-optimize-panel] input[name="optimize-mode"][value="deep"]');
  await activateButton(sessionId, "Start optimization", "[data-optimize-panel]");
  await waitFor(
    sessionId,
    `return [...document.querySelectorAll('[data-optimize-panel] dt')].some(
      node => node.textContent === 'Solve status' &&
        ['Optimal', 'Feasible', 'Infeasible', 'No solution within the mode limit',
         'Backend failed', 'Verification alarm'].includes(node.nextElementSibling?.textContent?.trim())
    );`,
    [],
    60_000,
  );
  const retained = (
    await nativeRequest(sessionId, "solve_list_runs", {
      schemaVersion: 2,
      scenarioId,
      revision: current.revision,
      limit: 2,
    })
  ).result.runs;
  assert.equal(retained.length, 1, "Exactly one real bundled-worker solve was requested");
  const [job] = retained;
  assert(["optimal", "feasible"].includes(job.status), `Packaged solve: ${job.status}`);
  assert.equal(job.scenarioRevision, current.revision);
  assert.match(job.solutionId, /^[a-f0-9-]{36}$/u);
  const diagnostics = (
    await nativeRequest(sessionId, "solve_get_diagnostics_summary", {
      schemaVersion: 2,
      scenarioId,
      runId: job.runId,
    })
  ).result;
  assert.equal(diagnostics.job.runId, job.runId);
  assert.equal(diagnostics.job.verificationChecksum, job.verificationChecksum);
  assert.match(diagnostics.manifestChecksum, /^[a-f0-9]{64}$/u);
  assert(Number.isSafeInteger(diagnostics.elapsedMilliseconds));
  assert(Number.isSafeInteger(diagnostics.firstIncumbentMilliseconds));
  assert(Number.isSafeInteger(diagnostics.firstVerifiedFeasibleMilliseconds));
  assert(
    diagnostics.firstIncumbentMilliseconds <= diagnostics.firstVerifiedFeasibleMilliseconds &&
      diagnostics.firstVerifiedFeasibleMilliseconds <= diagnostics.elapsedMilliseconds,
    "The independently verified plan must follow the unverified backend incumbent",
  );
  await evaluate(
    sessionId,
    `const summary = [...document.querySelectorAll('[data-optimize-panel] summary')]
      .find(node => node.textContent === 'Saved run diagnostics');
     if (!summary) throw new Error('Native diagnostics disclosure is missing');
     summary.id = 'packaged-solve-diagnostics';`,
  );
  await activate(sessionId, "#packaged-solve-diagnostics");
  await waitFor(
    sessionId,
    `return [...document.querySelectorAll('[data-optimize-panel] dt')].some(
      node => node.textContent === 'First independently verified feasible plan, milliseconds' &&
        node.nextElementSibling?.textContent?.trim() === arguments[0]
    );`,
    [String(diagnostics.firstVerifiedFeasibleMilliseconds)],
  );
  await activateButton(sessionId, "Explain terminal status", "[data-optimize-panel]");
  await waitFor(
    sessionId,
    `const panel = document.querySelector('[data-optimize-terminal-explanation]');
     const evidence = panel?.querySelector('ol li');
     return panel?.querySelector('[data-optimize-state="terminal-explanation-accepted"]') &&
       panel.textContent.includes(arguments[0]) &&
       panel.textContent.includes(arguments[1]) &&
       evidence?.textContent.includes(arguments[0]) &&
       evidence.textContent.includes(arguments[1]);`,
    [job.runId, diagnostics.manifestChecksum],
    30_000,
  );
  await evaluate(
    sessionId,
    "document.querySelector('[data-optimize-terminal-explanation]').scrollIntoView({block:'start'});",
  );
  await screenshot(sessionId, "packaged-optimize-recorded-status.png");
  await evaluate(
    sessionId,
    "document.querySelector('[data-optimize-terminal-explanation] ol li').scrollIntoView({block:'center'});",
  );
  await screenshot(sessionId, "packaged-optimize-status-evidence.png");
  await waitForElement(sessionId, "#setup-results-details button", 30_000);
  await activateButton(sessionId, "Inspect schedule", "#setup-results-details");
  await waitForElement(sessionId, "[data-accepted-schedule] table tbody tr", 30_000);
  const verified = (
    await nativeRequest(sessionId, "solution_verify", {
      schemaVersion: 1,
      scenarioId,
      solutionId: job.solutionId,
    })
  ).result;
  assert.equal(verified.result.solutionId, job.solutionId);
  assert.equal(verified.scenarioRevision, current.revision);
  assert.equal(verified.verification.accepted, true);
  assert.equal(verified.verification.checksum, job.verificationChecksum);
  assert(verified.verification.requiredRuleResults.every((rule) => rule.satisfied));
  await waitFor(
    sessionId,
    `const check = document.querySelector('[data-accepted-schedule] section[aria-label="Independent result check"]');
     return check?.textContent.includes('Verified schedule ready. Every recorded Required rule passed.') &&
       check.textContent.includes('Required rule checks passed in this result.');`,
  );
  const spacing = await evaluate(
    sessionId,
    `const details = document.querySelector('#setup-results-details');
     const button = [...details.querySelectorAll('button')]
       .find(item => item.textContent.trim() === 'Close schedule');
     const heading = details.querySelector('[data-accepted-schedule] h4');
     return {buttonBottom: button.getBoundingClientRect().bottom,
       headingTop: heading.getBoundingClientRect().top};`,
  );
  assert(
    spacing.headingTop >= spacing.buttonBottom + 8,
    "The accepted schedule heading must not overlap its disclosure button",
  );
  await screenshot(sessionId, "packaged-optimize-verified.png");
  const timeline = (
    await nativeRequest(sessionId, "solution_get_view", {
      schemaVersion: 1,
      scenarioId,
      solutionId: job.solutionId,
      viewId: "official.workforce.result.schedule",
      parameters: { kind: "timeline", limit: 50 },
      continuation: null,
    })
  ).result.view.data.result;
  assert.equal(timeline.kind, "timeline");
  await activate(sessionId, '[data-accepted-schedule] input[type="radio"][value="timeline"]');
  await waitFor(
    sessionId,
    `const schedule = document.querySelector('[data-accepted-schedule]');
     if (arguments[0] === 0)
       return schedule?.textContent.includes('No selected assignments match this timeline.') &&
         !schedule.querySelector('table');
     return [...schedule.querySelectorAll('table')].some(
       table => table.caption?.textContent.trim() === 'Timeline by start time' &&
         table.tBodies[0]?.rows.length > 0
     );`,
    [timeline.totalItems],
  );
  await screenshot(sessionId, "packaged-optimize-timeline.png");
  const preferredRow = timeline.items.findIndex(
    (assignment) => assignment.shiftId === preferredShiftId,
  );
  assert(preferredRow >= 0, "The required shift must have a real accepted assignment on this page");
  const inspect = await waitFor(
    sessionId,
    `const table = [...document.querySelectorAll('[data-accepted-schedule] table')]
      .find(entry => entry.caption?.textContent.trim() === 'Timeline by start time');
     return table?.tBodies[0]?.rows[arguments[0]]?.querySelector('button') ?? null;`,
    [preferredRow],
  );
  const inspectLabel = await evaluate(
    sessionId,
    "return arguments[0].getAttribute('aria-label');",
    [inspect],
  );
  await keyboardActivate(
    sessionId,
    `[data-accepted-schedule] button[aria-label=${JSON.stringify(inspectLabel)}]`,
    "\uE007",
    32,
  );
  await waitFor(
    sessionId,
    `const panel = document.querySelector('[data-assignment-details]');
     return [...(panel?.querySelectorAll('ol li') ?? [])].some(message => {
       const facts = new Map([...message.querySelectorAll('dt')].map(term => [
         term.textContent.trim(), term.nextElementSibling?.textContent.trim()
       ]));
       const unselected = facts.get('unselected pair penalty');
       return message.textContent.includes('Matching source preference term and pair-local penalty for this selected pair.') &&
         message.textContent.includes(arguments[0]) &&
         message.textContent.includes('Assignment type') &&
         message.textContent.includes('matches an active Prefer condition') &&
         message.textContent.includes('not a feasible alternative') &&
         facts.get('selected pair penalty') === '0' &&
         /^[0-9]+$/.test(unselected ?? '') && BigInt(unselected) > 0n &&
         facts.get('objective level')?.startsWith('official.workforce.objective.preference.');
     });`,
    [preferenceId],
    60_000,
  );
  await evaluate(
    sessionId,
    "document.querySelector('[data-assignment-details]')?.scrollIntoView({block:'start'});",
  );
  await screenshot(sessionId, "packaged-accepted-assignment-preference-evidence.png");
  const peoplePage = (
    await nativeRequest(sessionId, "solution_get_view", {
      schemaVersion: 1,
      scenarioId,
      solutionId: job.solutionId,
      viewId: "official.workforce.result.schedule",
      parameters: { kind: "people", limit: 50 },
      continuation: null,
    })
  ).result.view.data.result;
  assert.equal(peoplePage.kind, "people");
  const incumbent = timeline.items[preferredRow];
  const otherPerson = peoplePage.items.find(
    (person) => person.personId === replacementPersonId && person.personId !== incumbent.personId,
  );
  assert(otherPerson, "A distinct person is needed for a real replacement question");
  const replacementDecision = (
    await nativeRequest(sessionId, "solution_get_view", {
      schemaVersion: 1,
      scenarioId,
      solutionId: job.solutionId,
      viewId: "official.workforce.result.schedule",
      parameters: { kind: "decision", personId: otherPerson.personId, shiftId: incumbent.shiftId },
      continuation: null,
    })
  ).result.view.data.result;
  assert.equal(replacementDecision.kind, "decision");
  assert.equal(
    replacementDecision.selected,
    false,
    "The replacement must be unselected, not absent",
  );
  assert.equal(typeof replacementDecision.assignmentId, "string");
  const beforeQuestion = (
    await nativeRequest(sessionId, "solution_list_counterfactuals", {
      schemaVersion: 1,
      scenarioId,
      before: null,
      limit: 8,
    })
  ).result.page.jobs.length;
  await waitForElement(
    sessionId,
    '[data-accepted-counterfactual] select[name="counterfactual-person"]',
    30_000,
  );
  const excludedPerson = peoplePage.items.find((person) => person.personId === excludedPersonId);
  assert(excludedPerson && excludedPerson.personId !== incumbent.personId);
  await selectValue(
    sessionId,
    '[data-accepted-counterfactual] select[name="counterfactual-person"]',
    excludedPerson.personId,
  );
  await activateButton(
    sessionId,
    "Check replacement (up to 30 seconds)",
    "[data-accepted-counterfactual]",
  );
  await waitFor(
    sessionId,
    `return document.querySelector('[data-accepted-counterfactual]')?.textContent.includes(
      'No question was saved.'
    );`,
    [],
    90_000,
  );
  const alreadyAssignedDecision = (
    await nativeRequest(sessionId, "solution_get_view", {
      schemaVersion: 1,
      scenarioId,
      solutionId: job.solutionId,
      viewId: "official.workforce.result.schedule",
      parameters: {
        kind: "decision",
        personId: alreadyAssignedPersonId,
        shiftId: incumbent.shiftId,
      },
      continuation: null,
    })
  ).result.view.data.result;
  assert.equal(alreadyAssignedDecision.kind, "decision");
  assert.equal(alreadyAssignedDecision.selected, true);
  await selectValue(
    sessionId,
    '[data-accepted-counterfactual] select[name="counterfactual-person"]',
    alreadyAssignedPersonId,
  );
  await activateButton(
    sessionId,
    "Check replacement (up to 30 seconds)",
    "[data-accepted-counterfactual]",
  );
  await waitFor(
    sessionId,
    `return document.querySelector('[data-accepted-counterfactual] [role="alert"]')?.textContent.includes(
      'already assigned to this shift'
    );`,
  );
  await selectValue(
    sessionId,
    '[data-accepted-counterfactual] select[name="counterfactual-person"]',
    otherPerson.personId,
  );
  await activateButton(
    sessionId,
    "Check replacement (up to 30 seconds)",
    "[data-accepted-counterfactual]",
  );
  const diagnostic = await waitForSavedCounterfactual(
    sessionId,
    scenarioId,
    (entry) =>
      entry.baseSolutionId === job.solutionId &&
      entry.condition.condition.type === "replaceAssignment" &&
      entry.condition.condition.replacedAssignmentId === incumbent.assignmentId,
  );
  const persisted = (
    await nativeRequest(sessionId, "solution_list_counterfactuals", {
      schemaVersion: 1,
      scenarioId,
      before: null,
      limit: 8,
    })
  ).result.page.jobs;
  assert.equal(persisted.length, beforeQuestion + 1, "Only the modeled replacement is persisted");
  assert(diagnostic, "The real replacement question must survive outside the Vue cache");
  assert.equal(diagnostic.state, "completed");
  assert.equal(diagnostic.condition.condition.targetAssignmentId, replacementDecision.assignmentId);
  const retainedQuestion = (
    await nativeRequest(sessionId, "solution_get_counterfactual", {
      schemaVersion: 1,
      scenarioId,
      jobId: diagnostic.jobId,
    })
  ).result.job;
  assert.equal(retainedQuestion.request.semantics.base.solutionId, job.solutionId);
  assert.equal(retainedQuestion.result.request.condition.checksum, diagnostic.condition.checksum);
  assert.equal(retainedQuestion.result.conclusion.type, "verifiedAlternative");
  const worseCategory = retainedQuestion.result.conclusion.comparison.scoreLevels
    .flatMap((level) => level.categories)
    .find((category) => category.delta !== null && BigInt(category.delta) > 0n);
  assert(
    worseCategory,
    "The forced replacement must retain an explicit increased preference penalty",
  );
  await waitFor(
    sessionId,
    `return [...document.querySelectorAll(
      '[data-accepted-counterfactual] [aria-label="Preference category differences"] tbody tr'
    )].some(row => row.querySelector('th')?.textContent.trim() === arguments[0] &&
      row.cells[4]?.textContent.trim() === arguments[1]);`,
    [worseCategory.categoryId, String(worseCategory.delta)],
  );
  const explanation = (
    await nativeExplain(sessionId, scenarioId, current.revision, {
      schemaVersion: 1,
      subject: {
        kind: "counterfactual",
        jobId: diagnostic.jobId,
        base: retainedQuestion.request.semantics.base,
      },
    })
  ).result.explanation;
  assert.equal(explanation.evidence.evidence.kind, "counterfactual");
  assert.deepEqual(
    explanation.rendered.messages[0].assignments,
    [
      diagnostic.condition.condition.targetAssignmentId,
      diagnostic.condition.condition.replacedAssignmentId,
    ].sort(),
  );
  if (retainedQuestion.result.conclusion.type === "verifiedAlternative") {
    assert.equal(
      retainedQuestion.result.conclusion.comparison.base.scenarioRevision,
      current.revision,
    );
    assert.equal(
      retainedQuestion.result.conclusion.comparison.candidate.scenarioRevision,
      current.revision,
    );
  }
  await evaluate(
    sessionId,
    `const heading = [...document.querySelectorAll('[data-accepted-counterfactual] h6')]
      .find(node => node.textContent.trim() === 'Recorded result');
     heading.scrollIntoView({block:'start'});`,
  );
  await screenshot(sessionId, "packaged-accepted-counterfactual.png");
  const counterfactualLayout = await evaluate(
    sessionId,
    `const panel = document.querySelector('[data-accepted-counterfactual]');
     const bounds = panel.getBoundingClientRect();
     return {right: bounds.right, viewport: document.documentElement.clientWidth,
       width: panel.clientWidth, content: panel.scrollWidth};`,
  );
  assert(counterfactualLayout.right <= counterfactualLayout.viewport + 1);
  assert(counterfactualLayout.content <= counterfactualLayout.width + 1);
  await evaluate(
    sessionId,
    `document.querySelector('[data-accepted-counterfactual] [aria-label="Preference category differences"]')
      .scrollIntoView({block:'center'});`,
  );
  await screenshot(sessionId, "packaged-accepted-counterfactual-categories.png");
  await keyboardReachButton(
    sessionId,
    "Close assignment evidence",
    "[data-assignment-details]",
    32,
    true,
  );
  await keyboardKey(sessionId, "\uE007");
  await waitFor(sessionId, "return document.activeElement?.matches(arguments[0]);", [
    `[data-accepted-schedule] button[aria-label=${JSON.stringify(inspectLabel)}]`,
  ]);
  const reopenEvidence = await waitFor(
    sessionId,
    `const table = [...document.querySelectorAll('[data-accepted-schedule] table')]
      .find(entry => entry.caption?.textContent.trim() === 'Timeline by start time');
     return table?.tBodies[0]?.rows[arguments[0]]?.querySelector('button') ?? null;`,
    [preferredRow],
  );
  await activateElement(sessionId, reopenEvidence);
  const savedQuestion = await waitFor(
    sessionId,
    `return [...document.querySelectorAll('[data-accepted-counterfactual] ul li button')]
      .find(button => button.textContent.includes(arguments[0])) ?? null;`,
    [replacementDecision.assignmentId],
  );
  await activateElement(sessionId, savedQuestion);
  await waitFor(
    sessionId,
    `return [...document.querySelectorAll('[data-accepted-counterfactual] code')]
      .some(node => node.textContent.trim() === arguments[0]);`,
    [retainedQuestion.result.conclusion.alternative.solutionId],
  );
  for (const [mode, title, conditionType, assignmentId, personId, selected] of [
    [
      "require",
      "Check required person (up to 30 seconds)",
      "forceAssignmentValue",
      replacementDecision.assignmentId,
      otherPerson.personId,
      true,
    ],
    [
      "exclude",
      "Check excluded person (up to 30 seconds)",
      "forbidAssignmentValue",
      incumbent.assignmentId,
      incumbent.personId,
      false,
    ],
  ]) {
    await selectValue(
      sessionId,
      '[data-accepted-counterfactual] select[name="counterfactual-question"]',
      mode,
    );
    if (mode === "require")
      await selectValue(
        sessionId,
        '[data-accepted-counterfactual] select[name="counterfactual-person"]',
        personId,
      );
    await activateButton(sessionId, title, "[data-accepted-counterfactual]");
    const single = await waitForSavedCounterfactual(
      sessionId,
      scenarioId,
      (entry) =>
        entry.baseSolutionId === job.solutionId &&
        entry.condition.condition.type === conditionType &&
        entry.condition.condition.assignmentId === assignmentId,
    );
    assert(single, "The single-assignment condition must be saved with its native source identity");
    assert.equal(single.state, "completed");
    assert.deepEqual(single.condition.condition.value, { type: "boolean", value: true });
    const recorded = (
      await nativeRequest(sessionId, "solution_get_counterfactual", {
        schemaVersion: 1,
        scenarioId,
        jobId: single.jobId,
      })
    ).result.job;
    assert.equal(recorded.result.conclusion.type, "verifiedAlternative");
    await waitFor(
      sessionId,
      `return [...document.querySelectorAll('[data-accepted-counterfactual] code')]
        .some(node => node.textContent.trim() === arguments[0]);`,
      [recorded.result.conclusion.alternative.solutionId],
    );
    const alternative = (
      await nativeRequest(sessionId, "solution_get_view", {
        schemaVersion: 1,
        scenarioId,
        solutionId: recorded.result.conclusion.alternative.solutionId,
        viewId: "official.workforce.result.schedule",
        parameters: { kind: "decision", personId, shiftId: incumbent.shiftId },
        continuation: null,
      })
    ).result.view.data.result;
    assert.equal(alternative.assignmentId, assignmentId);
    assert.equal(alternative.selected, selected);
    const rendered = (
      await nativeExplain(sessionId, scenarioId, current.revision, {
        schemaVersion: 1,
        subject: {
          kind: "counterfactual",
          jobId: single.jobId,
          base: recorded.request.semantics.base,
        },
      })
    ).result.explanation.rendered;
    assert.deepEqual(rendered.messages[0].assignments, [assignmentId]);
    await evaluate(
      sessionId,
      `const heading = [...document.querySelectorAll('[data-accepted-counterfactual] h6')]
        .find(node => node.textContent.trim() === 'Recorded result');
       heading.scrollIntoView({block:'start'});`,
    );
    await screenshot(sessionId, `packaged-accepted-counterfactual-${mode}.png`);
  }
  await activateButton(sessionId, "Close assignment evidence", "[data-assignment-details]");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    current.revision,
    "Native assignment inspection cannot mutate the accepted source scenario",
  );
  const health = (
    await nativeRequest(sessionId, "solution_get_view", {
      schemaVersion: 1,
      scenarioId,
      solutionId: job.solutionId,
      viewId: "official.workforce.result.schedule",
      parameters: { kind: "health", limit: 50 },
      continuation: null,
    })
  ).result.view.data.result;
  assert.equal(health.kind, "health");
  assert(health.items.length > 0, "The saved Required rules must yield Plan Health observations");
  const firstHealthSource = health.items[0].sourceRuleId ?? health.items[0].coverageOwnerId;
  assert(firstHealthSource, "The first native Plan Health observation must identify its source");
  await activate(sessionId, '[data-accepted-schedule] input[type="radio"][value="health"]');
  await waitFor(
    sessionId,
    `const table = [...document.querySelectorAll('[data-accepted-schedule] table')]
       .find(item => item.caption?.textContent.trim() === 'Plan health');
     return table?.tBodies[0]?.rows.length === arguments[0] &&
       table.tBodies[0].textContent.includes(arguments[1]);`,
    [health.items.length, firstHealthSource],
  );
  await screenshot(sessionId, "packaged-optimize-health.png");
  const personCalendar = (
    await nativeRequest(sessionId, "solution_get_view", {
      schemaVersion: 1,
      scenarioId,
      solutionId: job.solutionId,
      viewId: "official.workforce.result.schedule",
      parameters: { kind: "calendarPeople", limit: 50 },
      continuation: null,
    })
  ).result.view.data.result;
  assert.equal(personCalendar.kind, "calendarPeople");
  assert(personCalendar.totalItems > 0);
  assert(personCalendar.items.length > 0);
  assert(personCalendar.items[0].days.length > 0 && personCalendar.items[0].days.length <= 7);
  assert.equal(personCalendar.items[0].days[0].date, personCalendar.window.startDate);
  await activate(sessionId, '[data-accepted-schedule] input[type="radio"][value="calendarPeople"]');
  await waitFor(
    sessionId,
    `const grid = document.querySelector('[data-accepted-calendar] [role="grid"]');
     const first = grid?.querySelector('tbody tr[data-index="0"]');
     return grid?.querySelectorAll('thead th').length === arguments[0] &&
       first?.querySelector('th')?.textContent.includes(arguments[1]) &&
       first?.querySelector('td')?.textContent.includes(arguments[2]) &&
       grid.querySelectorAll('tbody tr').length < arguments[3];`,
    [
      personCalendar.items[0].days.length + 1,
      personCalendar.items[0].person.name,
      `${personCalendar.items[0].days[0].selectedCount} assigned`,
      personCalendar.items.length,
    ],
  );
  await evaluate(
    sessionId,
    `document.querySelector('[data-accepted-calendar]')?.scrollIntoView({block:'start'});
     return true;`,
  );
  await screenshot(sessionId, "packaged-optimize-person-calendar.png");
  const originalShifts = (
    await nativeRequest(sessionId, "solution_get_view", {
      schemaVersion: 1,
      scenarioId,
      solutionId: job.solutionId,
      viewId: "official.workforce.result.schedule",
      parameters: { kind: "shifts", limit: 50 },
      continuation: null,
    })
  ).result.view.data.result;
  const firstShift = originalShifts.items.find((shift) => {
    const date = shift.startsAt.local.slice(0, 10);
    return (
      date >= personCalendar.planningDates.startDate &&
      date < personCalendar.planningDates.endDateExclusive
    );
  });
  assert(firstShift, "The packaged source must include a shift inside its original planning dates");
  const firstDay = Date.parse(`${personCalendar.window.startDate}T00:00:00Z`);
  const shiftDay = Date.parse(`${firstShift.startsAt.local.slice(0, 10)}T00:00:00Z`);
  const shiftWeek = Math.floor((shiftDay - firstDay) / (7 * 86_400_000));
  const weekStart = new Date(firstDay + shiftWeek * 7 * 86_400_000).toISOString().slice(0, 10);
  const shiftCalendar = (
    await nativeRequest(sessionId, "solution_get_view", {
      schemaVersion: 1,
      scenarioId,
      solutionId: job.solutionId,
      viewId: "official.workforce.result.schedule",
      parameters: { kind: "calendarShifts", startDate: weekStart, limit: 50 },
      continuation: null,
    })
  ).result.view.data.result;
  assert.equal(shiftCalendar.kind, "calendarShifts");
  assert(shiftCalendar.totalItems > 0);
  assert(shiftCalendar.items.length > 0);
  assert.deepEqual(shiftCalendar.planningDates, personCalendar.planningDates);
  const firstShiftColumn = Math.round(
    (Date.parse(`${shiftCalendar.items[0].date}T00:00:00Z`) -
      Date.parse(`${shiftCalendar.window.startDate}T00:00:00Z`)) /
      86_400_000,
  );
  assert(
    firstShiftColumn >= 0 && firstShiftColumn < 7,
    "The original shift must start in the displayed native window",
  );
  await activate(sessionId, '[data-accepted-schedule] input[type="radio"][value="calendarShifts"]');
  if (shiftWeek > 0) {
    const firstWindow = (
      await nativeRequest(sessionId, "solution_get_view", {
        schemaVersion: 1,
        scenarioId,
        solutionId: job.solutionId,
        viewId: "official.workforce.result.schedule",
        parameters: { kind: "calendarShifts", limit: 50 },
        continuation: null,
      })
    ).result.view.data.result;
    if (firstWindow.totalItems === 0)
      await waitFor(
        sessionId,
        `return document.querySelector('[data-accepted-calendar]')?.textContent.includes(arguments[0]);`,
        ["No shifts start in these original-scenario dates."],
      );
  }
  for (let week = 1; week <= shiftWeek; week++) {
    await activateButton(sessionId, "Next dates", "[data-accepted-calendar]");
    const shown = new Date(firstDay + week * 7 * 86_400_000).toISOString().slice(0, 10);
    await waitFor(
      sessionId,
      `return document.querySelector('[data-accepted-calendar]')?.textContent.includes(arguments[0]);`,
      [`Shown dates: ${shown}`],
    );
  }
  await waitFor(
    sessionId,
    `const grid = document.querySelector('[data-accepted-calendar] [role="grid"]');
     const first = grid?.querySelector('tbody tr[data-index="0"]');
     return first?.querySelector('th')?.textContent.includes(arguments[0]) &&
       first?.querySelectorAll('td')[arguments[1]]?.textContent.includes(arguments[2]);`,
    [
      shiftCalendar.items[0].shift.shiftId,
      firstShiftColumn,
      `${shiftCalendar.items[0].shift.selectedCount} assigned`,
    ],
  );
  await evaluate(
    sessionId,
    `document.querySelector('[data-accepted-calendar]')?.scrollIntoView({block:'start'});
     return true;`,
  );
  await screenshot(sessionId, "packaged-optimize-shift-calendar.png");
  await activate(sessionId, '[data-accepted-schedule] input[type="radio"][value="timeline"]');
  await waitFor(
    sessionId,
    `return document.querySelector('[data-accepted-schedule] input[value="timeline"]')?.checked;`,
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    current.revision,
    "Inspecting Health, accepted calendars and Timeline cannot mutate the saved scenario",
  );
  await activateButton(sessionId, "Select this result for viewing", "#setup-results-details");
  await waitFor(
    sessionId,
    `const entry = [...document.querySelectorAll('#setup-results-details .result-summary-list > li')]
       .find(item => item.textContent.includes(arguments[0]));
     return entry?.textContent.includes('Selected for viewing') &&
       ![...entry.querySelectorAll('button')].some(
         button => button.textContent.trim() === 'Select this result for viewing'
       );`,
    [job.solutionId],
  );
  const selected = (
    await nativeRequest(sessionId, "solution_list", { schemaVersion: 1, scenarioId })
  ).result.solutions;
  assert.equal(selected.filter((result) => result.selected).length, 1);
  const selectedResult = selected.find((result) => result.result.solutionId === job.solutionId);
  assert.equal(selectedResult?.selected, true);
  assert.equal(selectedResult?.result.resultChecksum, verified.result.resultChecksum);
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    current.revision,
    "Selecting a local accepted result for viewing must not revise the scenario",
  );
  await screenshot(sessionId, "packaged-optimize-selected.png");
  // A second independently accepted run exercises the current comparison query,
  // without treating the selected-for-viewing result as the current/previous pair.
  await activateButton(sessionId, "Start optimization", "[data-optimize-panel]");
  await waitFor(
    sessionId,
    `return document.querySelectorAll('#setup-results-details .result-summary-list > li').length === arguments[0];`,
    [selected.length + 1],
    90_000,
  );
  const comparedResults = (
    await nativeRequest(sessionId, "solution_list", { schemaVersion: 1, scenarioId })
  ).result.solutions;
  assert.equal(
    comparedResults.length,
    selected.length + 1,
    "One additional normal solve is retained",
  );
  const [latest, previous] = comparedResults;
  assert.equal(previous.result.solutionId, selected[0].result.solutionId);
  assert.notEqual(
    previous.result.solutionId,
    job.solutionId,
    "The diagnostic, not the viewing selection, precedes the latest run",
  );
  assert.notEqual(latest.result.solutionId, previous.result.solutionId);
  const compared = (
    await nativeRequest(sessionId, "solution_compare", {
      schemaVersion: 3,
      scenarioId,
      baseSolutionId: previous.result.solutionId,
      candidateSolutionId: latest.result.solutionId,
    })
  ).result;
  assert.equal(compared.schemaVersion, 3);
  assert.equal(compared.currentRevision, current.revision);
  assert.equal(compared.comparison.score.type, "comparable");
  assert.equal(
    compared.comparison.base.acceptedResult.resultChecksum,
    previous.result.resultChecksum,
  );
  assert.equal(
    compared.comparison.candidate.acceptedResult.resultChecksum,
    latest.result.resultChecksum,
  );
  assert.equal(compared.comparison.runs.base.runId, previous.runId);
  assert.equal(compared.comparison.runs.candidate.runId, latest.runId);
  await keyboardActivateButton(sessionId, "Compare latest two results", "#setup-results-details");
  await waitFor(
    sessionId,
    `const panel = document.querySelector('#setup-results-details section[aria-label="Compare saved results"]');
     return panel?.textContent.includes(arguments[0]) &&
       panel.textContent.includes(arguments[1]);`,
    [previous.result.solutionId, latest.result.solutionId],
    30_000,
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    current.revision,
    "Comparing independently verified accepted results must not revise the saved scenario",
  );
  const comparisonLayout = await evaluate(
    sessionId,
    `const panel = document.querySelector('#setup-results-details section[aria-label="Compare saved results"]');
     const card = panel.querySelector('section[aria-labelledby]');
     const outer = panel.getBoundingClientRect();
     const inner = card.getBoundingClientRect();
     return { outerLeft: outer.left, outerRight: outer.right,
       innerLeft: inner.left, innerRight: inner.right,
       clientWidth: panel.clientWidth, scrollWidth: panel.scrollWidth };`,
  );
  assert(
    comparisonLayout.innerLeft >= comparisonLayout.outerLeft &&
      comparisonLayout.innerRight <= comparisonLayout.outerRight + 1 &&
      comparisonLayout.scrollWidth <= comparisonLayout.clientWidth + 1,
    `The native comparison must fit its panel without horizontal clipping: ${JSON.stringify(comparisonLayout)}`,
  );
  await screenshot(sessionId, "packaged-optimize-comparison.png");
  await activateButton(sessionId, "Explain saved-change facts", "#setup-results-details");
  await waitFor(
    sessionId,
    `const dialog = document.querySelector('[role="dialog"]');
     return dialog?.textContent.includes('Repair source bindings') &&
       dialog.textContent.includes(arguments[0]) &&
       dialog.textContent.includes(arguments[1]) &&
       dialog.textContent.includes('official.workforce.fact.repair_frame') &&
       dialog.textContent.includes('acceptedBaseToCandidate') &&
       dialog.textContent.includes('official.workforce.fact.baseline_admission') &&
       dialog.textContent.includes('notEstablished') &&
       dialog.textContent.includes(arguments[2]);`,
    [
      previous.result.solutionId,
      latest.result.solutionId,
      compared.comparison.candidate.documentHash,
    ],
    30_000,
  );
  await screenshot(sessionId, "packaged-optimize-repair-source-facts.png");
  await activateButton(sessionId, "Close", '[role="dialog"]');
  await waitFor(
    sessionId,
    `return !document.querySelector('[role="dialog"]') &&
       document.activeElement?.textContent.trim() === 'Explain saved-change facts';`,
    [],
    10_000,
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    current.revision,
    "Reading source-bound saved-change facts must not mutate the scenario",
  );
  // Base capture is an explicit mutation, not the earlier viewing selection.
  // Review and commit through the packaged WebKit UI, then solve against that
  // persisted base to exercise the pack-owned original-source base diff.
  await activateButton(
    sessionId,
    "Review as repair base",
    "#setup-results-details .result-summary-list > li:first-child",
  );
  await waitFor(
    sessionId,
    `return document.querySelector('#setup-results-details .result-summary-list > li:first-child')
      ?.textContent.includes(arguments[0]);`,
    [latest.result.solutionId],
  );
  await activateButton(
    sessionId,
    "Save this result as repair base",
    "#setup-results-details .result-summary-list > li:first-child",
  );
  await waitFor(
    sessionId,
    `return document.body.textContent.includes('The accepted result is saved as the repair base.');`,
    [],
    60_000,
  );
  const adoptedRevision = current.revision + 1;
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    adoptedRevision,
    "Explicit base capture must make exactly one scenario revision",
  );
  // Base capture changes the saved scenario; do not reuse full validation from
  // its prior revision to authorize a repair solve.
  await waitForElement(sessionId, '[data-optimize-state="validation-not-ready"]');
  await navigate(sessionId, `/project/${scenarioId}/validation`);
  await waitForElement(sessionId, "#validation-heading");
  await activateButton(sessionId, "Run full validation");
  await waitForElement(sessionId, '[data-full-validation-state="completed"]');
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, '[data-optimize-state="ready"]');
  await activateButton(sessionId, "Start optimization", "[data-optimize-panel]");
  await waitFor(
    sessionId,
    `return document.querySelectorAll('#setup-results-details .result-summary-list > li').length === arguments[0];`,
    [comparedResults.length + 1],
    90_000,
  );
  const basedResults = (
    await nativeRequest(sessionId, "solution_list", { schemaVersion: 1, scenarioId })
  ).result.solutions;
  const based = basedResults[0];
  assert.equal(based.scenarioRevision, adoptedRevision);
  const baseDiff = (
    await nativeRequest(sessionId, "solution_get_view", {
      schemaVersion: 1,
      scenarioId,
      solutionId: based.result.solutionId,
      viewId: "official.workforce.result.schedule",
      parameters: { kind: "baseDiff", limit: 50 },
      continuation: null,
    })
  ).result.view.data.result;
  assert.equal(baseDiff.kind, "baseDiff");
  assert.equal(baseDiff.baseSchedule.sourceSolutionId, latest.result.solutionId);
  assert.equal(baseDiff.baseSchedule.sourceRevision, current.revision);
  assert.equal(baseDiff.items.length, Math.min(50, baseDiff.totalItems));
  await activateButton(
    sessionId,
    "Inspect schedule",
    "#setup-results-details .result-summary-list > li:first-child",
  );
  await activate(
    sessionId,
    '#setup-results-details .result-summary-list > li:first-child [data-accepted-schedule] input[type="radio"][value="baseDiff"]',
  );
  await waitFor(
    sessionId,
    `const panel = document.querySelector(
       '#setup-results-details .result-summary-list > li:first-child [data-accepted-schedule]'
     );
     return panel?.textContent.includes(arguments[0]) &&
       panel.textContent.includes(arguments[1]);`,
    ["Changes from base", latest.result.solutionId],
  );
  await screenshot(sessionId, "packaged-optimize-base-difference.png");
  const undone = (
    await nativeRequest(sessionId, "scenario_undo", {
      scenarioId,
      expectedRevision: adoptedRevision,
    })
  ).result;
  assert.equal(undone.newRevision, adoptedRevision + 1);
  const redone = (
    await nativeRequest(sessionId, "scenario_redo", {
      scenarioId,
      expectedRevision: undone.newRevision,
    })
  ).result;
  assert.equal(redone.newRevision, undone.newRevision + 1);
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    redone.newRevision,
    "Base capture must undo and redo as one durable scenario command",
  );
  async function solveManualSelection(revision) {
    await navigate(sessionId, `/project/${scenarioId}/validation`);
    await activateButton(sessionId, "Run full validation");
    await waitForElement(sessionId, '[data-full-validation-state="completed"]', 120_000);
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await activateButton(sessionId, "Repair plan", "[data-optimize-panel]");
    await waitFor(
      sessionId,
      `return [...document.querySelectorAll('[data-optimize-panel] dt')].some(
        node => node.textContent === 'Solve status' &&
          ['Optimal', 'Feasible', 'Infeasible', 'No solution within the mode limit',
           'Backend failed', 'Verification alarm'].includes(node.nextElementSibling?.textContent?.trim())
      );`,
      [],
      60_000,
    );
    const runs = (
      await nativeRequest(sessionId, "solve_list_runs", {
        schemaVersion: 2,
        scenarioId,
        revision,
        limit: 2,
      })
    ).result.runs;
    assert.equal(runs.length, 1);
    const run = runs[0];
    assert(["optimal", "feasible"].includes(run.status));
    if (
      !(await evaluate(sessionId, "return document.querySelector('#setup-results-details')?.open;"))
    )
      await activate(sessionId, "#setup-results-details > summary");
    const inspect = await waitFor(
      sessionId,
      `const item = [...document.querySelectorAll('#setup-results-details .result-summary-list > li')]
        .find(entry => entry.textContent.includes(arguments[0]));
       return [...(item?.querySelectorAll('button') ?? [])]
         .find(button => button.textContent.trim() === 'Inspect schedule') ?? null;`,
      [run.solutionId],
    );
    await activateElement(sessionId, inspect);
    return run;
  }

  // Exercise the real reviewed manual B, not an imported or fabricated baseline.
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  const repairPanel = "[data-manual-repair-panel]";
  await waitForElement(sessionId, repairPanel);
  await activateButton(sessionId, "Refresh current selection", repairPanel);
  const incumbentRow = `${repairPanel} [data-native-pair][data-person-id="${incumbent.personId}"][data-shift-id="${incumbent.shiftId}"]`;
  await waitForElement(sessionId, incumbentRow);
  await keyboardActivateButton(
    sessionId,
    `Change protection for ${incumbent.personName}`,
    incumbentRow,
  );
  const protectionSelect = `${repairPanel} form > fieldset > label:nth-of-type(2) select`;
  await waitFor(sessionId, "return document.activeElement?.id === 'manual-repair-heading';");
  await keyboardReach(sessionId, protectionSelect);
  await keyboardKey(sessionId, "\uE015");
  await keyboardKey(sessionId, "\uE015");
  assert.equal(
    await evaluate(sessionId, "return document.activeElement.value;"),
    "hard",
    "Arrow keys must select Hard protection through the native select",
  );
  await keyboardActivateButton(sessionId, "Review manual change", repairPanel);
  await waitForElement(sessionId, '[data-manual-review][data-disposition="valid"]');
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    redone.newRevision,
    "Reviewing Hard protection must not mutate the source",
  );
  const manualReviewState = await evaluate(
    sessionId,
    `const review = document.querySelector('[data-manual-review]');
    return {visible: !!review && review.getBoundingClientRect().height > 0,
      textStatus: review?.querySelector('h4')?.textContent.trim() ?? null,
      explanation: review?.textContent.includes('Nothing changes until explicit Apply.') ?? false,
      hardVisible: review?.textContent.toLowerCase().includes('hard') ?? false};`,
  );
  assert.equal(manualReviewState.textStatus, "Native manual review — valid");
  assert.equal(manualReviewState.explanation, true);
  assert.equal(manualReviewState.hardVisible, true);
  await screenshot(sessionId, "packaged-keyboard-manual-protection-review.png");
  await activateButton(sessionId, "Apply reviewed manual change", repairPanel);
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0])?.textContent.includes('hard');",
    [incumbentRow],
  );
  const protectedRevision = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  assert.equal(protectedRevision, redone.newRevision + 1);
  const protectedRepairRun = await solveManualSelection(protectedRevision);
  await waitForElement(sessionId, incumbentRow);
  await activateButton(sessionId, `Replace ${incumbent.personName} on this shift`, incumbentRow);
  await choosePeopleReference(sessionId, "Replacement person", otherPerson.name, repairPanel);
  await activateButton(sessionId, "Review manual change", repairPanel);
  await waitForElement(sessionId, '[data-manual-review][data-disposition="invalid"]');
  assert.equal(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll('[data-manual-review] button')].some(button => button.textContent.trim() === 'Apply reviewed manual change');",
    ),
    false,
    "A Hard incumbent cannot be silently removed by a replacement",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    protectedRevision,
  );
  await activateButton(
    sessionId,
    "Check chosen current-result replacement diagnostic",
    repairPanel,
  );
  const currentDiagnostic = `${repairPanel} [aria-label="Current accepted-source global diagnostic"]`;
  await waitFor(
    sessionId,
    "const panel = document.querySelector(arguments[0]); return panel?.textContent.includes(arguments[1]) && panel.textContent.includes(`immutable revision ${arguments[2]}`);",
    [currentDiagnostic, protectedRepairRun.solutionId, protectedRevision],
  );
  await selectValue(
    sessionId,
    `${currentDiagnostic} select[name="counterfactual-person"]`,
    otherPerson.personId,
  );
  await activateButton(sessionId, "Check replacement (up to 30 seconds)", currentDiagnostic);
  const protectedQuestion = await waitForSavedCounterfactual(
    sessionId,
    scenarioId,
    (entry) =>
      entry.baseSolutionId === protectedRepairRun.solutionId &&
      entry.condition.condition.type === "replaceAssignment",
  );
  assert(protectedQuestion);
  assert.equal(protectedQuestion.state, "completed");
  const protectedEvidence = (
    await nativeRequest(sessionId, "solution_get_counterfactual", {
      schemaVersion: 1,
      scenarioId,
      jobId: protectedQuestion.jobId,
    })
  ).result.job;
  assert.equal(protectedEvidence.request.semantics.base.solutionId, protectedRepairRun.solutionId);
  assert(
    ["provenImpossible", "notDistinguishedWithinBudget"].includes(
      protectedEvidence.result.conclusion.type,
    ),
    "The diagnostic must not publish an alternative that removes Hard protection",
  );
  const protectedDiagnosticMessage =
    protectedEvidence.result.conclusion.type === "provenImpossible"
      ? "The saved diagnostic proved the requested temporary condition impossible"
      : "No conclusive answer within the budget. This does not prove the temporary condition";
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0])?.textContent.includes(arguments[1]);",
    [currentDiagnostic, protectedDiagnosticMessage],
    90_000,
  );
  const diagnosticLayout = await evaluate(
    sessionId,
    `const panel = document.querySelector(arguments[0]);
     [...panel.querySelectorAll('h6')].find(node => node.textContent === 'Recorded result')
       .scrollIntoView({block:'center'});
     return {width: panel.scrollWidth, available: panel.clientWidth};`,
    [currentDiagnostic],
  );
  assert(diagnosticLayout.width <= diagnosticLayout.available + 1);
  await screenshot(sessionId, "packaged-manual-current-result-diagnostic.png");
  await activateButton(
    sessionId,
    "Close current accepted-source diagnostic; keep manual draft",
    currentDiagnostic,
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    protectedRevision,
    "The diagnostic uses current accepted C, preserves Hard protection, and does not apply a change",
  );
  await activate(sessionId, `${repairPanel} form > fieldset > label input[type="checkbox"]`);
  await selectValue(sessionId, protectionSelect, "soft");
  await setValue(
    sessionId,
    `${repairPanel} form > fieldset > label input[inputmode="numeric"]`,
    "3",
  );
  await activateButton(sessionId, "Review manual change", repairPanel);
  await waitForElement(sessionId, '[data-manual-review][data-disposition="valid"]');
  await activateButton(sessionId, "Apply reviewed manual change", repairPanel);
  const replacementRow = `${repairPanel} [data-native-pair][data-person-id="${otherPerson.personId}"][data-shift-id="${incumbent.shiftId}"]`;
  await waitFor(
    sessionId,
    "const row = document.querySelector(arguments[0]); return row?.textContent.includes('soft') && row.textContent.includes('weight 3');",
    [replacementRow],
  );
  const manualRevision = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  assert.equal(
    manualRevision,
    protectedRevision + 1,
    "Replacement and explicit protection changes form one command",
  );
  await evaluate(
    sessionId,
    "document.querySelector(arguments[0]).scrollIntoView({block:'start'});",
    [repairPanel],
  );
  await screenshot(sessionId, "packaged-guided-manual-replacement.png");

  const repairRun = await solveManualSelection(manualRevision);
  const recordedRepair = (
    await nativeRequest(sessionId, "solution_get_view", {
      schemaVersion: 1,
      scenarioId,
      solutionId: repairRun.solutionId,
      viewId: "official.workforce.result.schedule",
      parameters: { kind: "repairDiff", limit: 50 },
      continuation: null,
    })
  ).result.view.data.result;
  assert.equal(recordedRepair.kind, "repairDiff");
  assert.equal(recordedRepair.availability, "available");
  assert.equal(
    recordedRepair.baseline.original.acceptedResult.solutionId,
    latest.result.solutionId,
  );
  assert.equal(recordedRepair.baseline.selection.kind, "manualSelection");
  assert.equal(recordedRepair.baseline.verificationStatus, "notEstablished");
  assert.equal(recordedRepair.counts.removedPairs, 0);
  assert.equal(recordedRepair.counts.addedPairs, 0);
  assert(
    recordedRepair.items.some(
      (group) =>
        group.shiftId === incumbent.shiftId &&
        group.unchangedParticipants.some((member) => member.personId === otherPerson.personId) &&
        !group.unchangedParticipants.some((member) => member.personId === incumbent.personId),
    ),
    "Repair must retain the valid manually replaced pair before authored utility",
  );
  await waitFor(
    sessionId,
    "const panel = document.querySelector('[data-recorded-repair-diff]'); return panel?.textContent.includes(arguments[0]) && panel.textContent.includes('manualSelection') && panel.textContent.includes('notEstablished');",
    [repairRun.solutionId],
  );
  assert.deepEqual(
    await evaluate(
      sessionId,
      `return [...document.querySelectorAll('[data-recorded-repair-diff] article')].map(group => ({
        shiftId: group.querySelector(':scope > code').textContent,
        members: [...group.querySelectorAll(':scope > ul')].map(list =>
          [...list.querySelectorAll('li code')].map(code => code.textContent))
      }));`,
    ),
    recordedRepair.items.map((group) => ({
      shiftId: group.shiftId,
      members: [
        group.oldMembers.map((member) => member.personId),
        group.newMembers.map((member) => member.personId),
      ],
    })),
    "Each complete native before/after member must appear once, without adding the intersection again",
  );
  await activateButton(
    sessionId,
    "Explain recorded repair facts for chosen result",
    "#setup-results-details",
  );
  await waitFor(
    sessionId,
    `const dialog = document.querySelector('[role="dialog"]');
     return dialog?.textContent.includes(arguments[0]) &&
       dialog.textContent.includes(arguments[1]) &&
       dialog.textContent.includes(arguments[2]) &&
       dialog.textContent.includes('notEstablished');`,
    [
      latest.result.solutionId,
      repairRun.solutionId,
      recordedRepair.baseline.selection.recordChecksum,
    ],
    30_000,
  );
  await activateButton(
    sessionId,
    "Next",
    '[role="dialog"] nav[aria-label="Evidence parameter pages for official.workforce.explanation.repair"]',
  );
  await waitFor(
    sessionId,
    `const dialog = document.querySelector('[role="dialog"]');
     return dialog?.textContent.includes('official.workforce.fact.repair_frame') &&
       dialog.textContent.includes('acceptedBaseToCandidate') &&
       dialog.textContent.includes('notEstablished');`,
  );
  await evaluate(
    sessionId,
    `[...document.querySelectorAll('[role="dialog"] dt')]
       .find(node => node.textContent === 'official.workforce.fact.repair_frame')
       .scrollIntoView({block:'center'});`,
  );
  await screenshot(sessionId, "packaged-recorded-manual-repair-facts.png");
  await activateButton(sessionId, "Close", '[role="dialog"]');
  await waitFor(
    sessionId,
    "return !document.querySelector('[role=\"dialog\"]') && document.activeElement?.textContent.trim() === 'Explain recorded repair facts for chosen result';",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    manualRevision,
    "Repair and exact A/B/C explanation reads never apply a candidate or mutate the visible head",
  );
  const historyRoot = '[aria-labelledby="history-heading"]';
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Undo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario undo committed");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    manualRevision + 1,
  );
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0])?.textContent.includes('hard') && !document.querySelector(arguments[1]);",
    [incumbentRow, replacementRow],
  );
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Redo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario redo committed");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    manualRevision + 2,
  );
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitFor(
    sessionId,
    "const row = document.querySelector(arguments[0]); return row?.textContent.includes('soft') && row.textContent.includes('weight 3') && !document.querySelector(arguments[1]);",
    [replacementRow, incumbentRow],
  );
  await evaluate(
    sessionId,
    "document.querySelector(arguments[0]).scrollIntoView({block:'start'});",
    [repairPanel],
  );
  await screenshot(sessionId, "packaged-guided-manual-redo.png");
  console.log(
    `PASS: packaged native OR-Tools ${job.status}, accepted result detail/close keyboard focus restore, keyboard Hard-protection review, keyboard saved-result comparison, base capture, manual Repair and A/B/C verification`,
  );
}

async function packagedInfeasibilityAcceptance(sessionId, directory, dataHome) {
  const privateDescription = "EXPLAIN-001 private description MUST NOT enter diagnostic JSON";
  const privatePersonName = "EXPLAIN-001 constrained person";
  const fixtureTitle = "EXPLAIN-001 deterministic conflict fixture";
  const created = await nativeRequest(sessionId, "project_create", {
    schemaVersion: 1,
    title: fixtureTitle,
    description: privateDescription,
    domainPack: { id: "official.workforce", schemaVersion: 1 },
    settings: {
      timeZone: "UTC",
      locale: "en-US",
      units: "metric",
      firstDate: "2030-01-01",
      lastDate: "2030-01-01",
      gapPolicy: "reject",
      overlapPolicy: "reject",
    },
  });
  const scenarioId = created.result.scenarioId;
  const createdProject = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  assert(
    createdProject,
    "The native project-create identity must resolve in the saved project list",
  );
  assert.equal(createdProject.title, fixtureTitle);
  const assignmentTypeId = requestId();
  const assignedPersonId = requestId();
  const otherPersonId = requestId();
  const firstShiftId = requestId();
  const secondShiftId = requestId();
  const overlapRuleId = requestId();
  const firstFixedRuleId = requestId();
  const secondFixedRuleId = requestId();
  const entities = [
    {
      id: requestId(),
      kind: "scorePolicy",
      profileKey: "explain-conflict-fixture",
      levels: [{ levelKey: "preferences", label: "Fixture preferences" }],
      priorityMapping: ["low", "normal", "high", "veryHigh"].map((priority) => ({
        priority,
        levelKey: "preferences",
        scale: 1,
      })),
      workloadPolicies: {},
      tieBreak: "stableAssignmentRank",
    },
    {
      id: assignmentTypeId,
      kind: "assignmentType",
      name: "E2E clinic duty",
      category: "clinic",
      timeBehavior: "elapsed",
      defaultDurationMinutes: 60,
      qualifications: { kind: "unconstrained" },
      locationBehavior: { kind: "optional" },
      workloadBucketIds: [],
    },
    ...[assignedPersonId, otherPersonId].map((id, index) => ({
      id,
      kind: "person",
      name: index === 0 ? privatePersonName : "E2E eligible colleague",
      activeRange: { kind: "always" },
      qualificationGrants: [],
      eligibleAssignmentTypeIds: [assignmentTypeId],
      teamIds: [],
      tags: [],
      workloadWeight: { numerator: 1, denominator: 1 },
    })),
    ...[
      [firstShiftId, "09:00:00", "10:00:00"],
      [secondShiftId, "09:30:00", "10:30:00"],
    ].map(([id, start, end]) => ({
      id,
      kind: "shiftInstance",
      assignmentTypeId,
      startsAt: {
        instant: `2030-01-01T${start}Z`,
        local: `2030-01-01T${start}`,
        offsetSeconds: 0,
      },
      endsAt: {
        instant: `2030-01-01T${end}Z`,
        local: `2030-01-01T${end}`,
        offsetSeconds: 0,
      },
      coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
      tags: [],
      reportingAttribution: "startLocalDate",
      origin: { kind: "manual" },
    })),
  ];
  const allPeople = { people: { kind: "all" } };
  const rules = [
    {
      id: overlapRuleId,
      kind: "noOverlap",
      active: true,
      strength: "required",
      scope: allPeople,
      compatibleCategoryPairs: [],
    },
    ...[
      [firstFixedRuleId, firstShiftId],
      [secondFixedRuleId, secondShiftId],
    ].map(([id, shiftId]) => ({
      id,
      kind: "fixedAssignment",
      active: true,
      strength: "required",
      scope: allPeople,
      personId: assignedPersonId,
      shiftId,
      assigned: true,
    })),
  ];
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: created.result.revision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native EXPLAIN-001 conflict fixture" },
    truncateRedo: false,
    command: {
      type: "applyBatch",
      payload: {
        label: "Native EXPLAIN-001 overlapping fixed assignments",
        commands: [
          ...entities.map((entity) => ({
            type: "applyDomainCommand",
            payload: { commandType: "official.workforce.add_entity", payload: { entity } },
          })),
          ...rules.map((rule) => ({
            type: "applyDomainCommand",
            payload: { commandType: "official.workforce.add_rule", payload: { rule } },
          })),
        ],
      },
    },
  });
  const sourceRevision = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  const sourceHistoryHead = await appliedHistoryHead(sessionId, scenarioId);
  assert(sourceHistoryHead, "The authored infeasibility fixture must have a native journal head");

  await navigate(sessionId, "/projects");
  await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
  await waitForElement(sessionId, `[aria-label=${JSON.stringify(`Open project ${fixtureTitle}`)}]`);
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  assert.equal(await getText(sessionId, "#project-heading"), fixtureTitle);
  const openedProject = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  assert.equal(
    typeof openedProject?.lastOpenedAt,
    "string",
    "The fixture must pass the real native project-open route before validation",
  );
  await navigate(sessionId, `/project/${scenarioId}/validation`);
  await waitForElement(sessionId, "#validation-heading");
  await waitFor(
    sessionId,
    "return document.querySelector('[data-full-validation-state=\"notRun\"]');",
  );
  await activateButton(sessionId, "Run full validation");
  await waitFor(
    sessionId,
    "return document.querySelector('[data-full-validation-state=\"completed\"]');",
    [],
    120_000,
  );
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  if (
    !(await evaluate(sessionId, "return document.querySelector('#setup-results-details')?.open;"))
  )
    await activate(sessionId, "#setup-results-details > summary");
  await waitForElement(sessionId, '[data-optimize-state="ready"]');
  await activate(sessionId, '[data-optimize-panel] input[name="optimize-mode"][value="deep"]');
  await activateButton(sessionId, "Start optimization", "[data-optimize-panel]");
  await waitFor(
    sessionId,
    `return [...document.querySelectorAll('[data-optimize-panel] dt')].some(
      node => node.textContent === 'Solve status' &&
        node.nextElementSibling?.textContent?.trim() === 'Infeasible'
    );`,
    [],
    60_000,
  );
  const runs = (
    await nativeRequest(sessionId, "solve_list_runs", {
      schemaVersion: 2,
      scenarioId,
      revision: sourceRevision,
      limit: 2,
    })
  ).result.runs;
  assert.equal(runs.length, 1, "The infeasible fixture must come from one real worker run");
  const [run] = runs;
  assert.equal(run.status, "infeasible");
  assert.equal(run.solutionId, null);
  assert.equal(run.scenarioRevision, sourceRevision);
  const diagnostics = (
    await nativeRequest(sessionId, "solve_get_diagnostics_summary", {
      schemaVersion: 2,
      scenarioId,
      runId: run.runId,
    })
  ).result;
  assert.equal(diagnostics.job.runId, run.runId);
  assert.equal(diagnostics.job.scenarioRevision, sourceRevision);
  assert.equal(diagnostics.job.status, "infeasible");
  assert.match(diagnostics.manifestChecksum, /^[a-f0-9]{64}$/u);

  await activateButton(sessionId, "Inspect mapped conflict", "[data-optimize-panel]");
  await waitForElement(
    sessionId,
    '[data-infeasibility-inspection] ol[aria-label="Mapped Required-rule conflict groups"]',
  );
  const mapped = await evaluate(
    sessionId,
    `const group = document.querySelector(
      '[data-infeasibility-inspection] ol[aria-label="Mapped Required-rule conflict groups"] li'
    );
    const ruleId = group?.querySelector('ul[aria-label^="Required source rules"] code')
      ?.textContent.trim();
    return {
      ruleId,
      inspect: (ruleId && [...group.querySelectorAll('button')]
        .find(button => button.textContent.includes('Inspect') && button.textContent.includes(ruleId)))
        ?? null,
      groupText: group?.textContent ?? ''
    };`,
  );
  assert.match(mapped.ruleId ?? "", /^[a-f0-9-]{36}$/u);
  assert(
    mapped.groupText.includes(mapped.ruleId),
    "The recorded conflict must name its source rule",
  );
  await activateElement(sessionId, mapped.inspect);
  await waitForElement(sessionId, "#rule-editor-heading");
  assert((await getText(sessionId, "#rule-identity")).includes(mapped.ruleId));
  await activateButton(sessionId, "Discard", '[aria-labelledby="rule-editor-heading"]');
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  if (
    !(await evaluate(sessionId, "return document.querySelector('#setup-results-details')?.open;"))
  )
    await activate(sessionId, "#setup-results-details > summary");
  await waitForElement(sessionId, "[data-optimize-infeasibility-inspect]");
  await activateButton(sessionId, "Inspect mapped conflict", "[data-optimize-panel]");
  await waitForElement(
    sessionId,
    '[data-infeasibility-inspection] ol[aria-label="Mapped Required-rule conflict groups"]',
  );
  await screenshot(sessionId, "explain-001-native-mapped-conflict.png");
  const inspected = await getText(sessionId, "[data-infeasibility-inspection]");
  assert(inspected.includes(`Source revision ${sourceRevision}`));

  const libraryBeforeCopy = (await settingsSnapshot(sessionId)).libraryRevision;
  await activateButton(
    sessionId,
    "Review a new diagnostic copy",
    "[data-infeasibility-inspection]",
  );
  await waitForElement(sessionId, "[data-diagnostic-copy-review]");
  const review = await evaluate(
    sessionId,
    `const root = document.querySelector('[data-diagnostic-copy-review]');
    const rows = [...root.querySelectorAll('dl > div')];
    const value = label => rows.find(row => row.querySelector('dt')?.textContent.trim() === label)
      ?.querySelector('dd')?.innerText.trim() ?? null;
    const rulesRow = rows.find(row =>
      row.querySelector('dt')?.textContent.includes('Required rule UUIDs relaxed only in the copy')
    );
    return {
      scenarioId: value('Original scenario ID'),
      sourceRevision: Number(value('Recorded source revision')),
      currentRevision: Number(value('Observed current revision')),
      runId: value('Recorded run ID'),
      manifestChecksum: value('Run manifest checksum'),
      evidenceChecksum: value('Conflict evidence checksum'),
      conflictId: value('Conflict ID'),
      group: value('Selected mapped source group'),
      expectedLibraryRevision: Number(value('Current project-library revision reviewed')),
      ruleIds: [...(rulesRow?.querySelectorAll('code') ?? [])].map(node => node.textContent.trim())
    };`,
  );
  assert.equal(review.scenarioId, scenarioId);
  assert.equal(review.sourceRevision, sourceRevision);
  assert.equal(review.currentRevision, sourceRevision);
  assert.equal(review.runId, run.runId);
  assert.equal(review.manifestChecksum, diagnostics.manifestChecksum);
  assert.match(review.evidenceChecksum, /^[a-f0-9]{64}$/u);
  assert.equal(review.conflictId, `infeasibility.${review.evidenceChecksum}`);
  assert(review.group.includes("official.workforce.rule."));
  assert.deepEqual(review.ruleIds, [mapped.ruleId]);
  assert.equal(review.expectedLibraryRevision, libraryBeforeCopy);
  await activateButton(
    sessionId,
    "Create a new copy with this group relaxed",
    "[data-diagnostic-copy-review]",
  );
  await waitForElement(sessionId, "[data-diagnostic-copy-receipt]");
  await screenshot(sessionId, "explain-001-native-copy-receipt.png");
  const copyId = await evaluate(
    sessionId,
    `const receipt = document.querySelector('[data-diagnostic-copy-receipt]');
    const identities = receipt.querySelectorAll('code');
    return identities[identities.length - 1]?.textContent.trim() ?? null;`,
  );
  assert.match(copyId ?? "", /^[a-f0-9-]{36}$/u);
  assert.notEqual(copyId, scenarioId);
  const copyProject = (await projects(sessionId)).find((project) => project.scenarioId === copyId);
  assert(copyProject, "The reviewed native diagnostic copy must exist as a fresh project");
  assert.equal(
    (await settingsSnapshot(sessionId)).libraryRevision,
    libraryBeforeCopy + 1,
    "The copy receipt must advance the library from the exact reviewed revision",
  );
  const copyHistoryBefore = (
    await nativeRequest(sessionId, "scenario_get_history_page", {
      schemaVersion: 1,
      scenarioId: copyId,
      expectedRevision: copyProject.revision,
      limit: 50,
      continuation: null,
    })
  ).result;
  assert.equal(copyHistoryBefore.entries.length, 1);
  assert.equal(copyHistoryBefore.entries[0].applied, true);
  const sourceAfterCopy = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  assert.equal(sourceAfterCopy.revision, sourceRevision);
  assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), sourceHistoryHead);

  await navigate(sessionId, "/projects");
  await navigate(sessionId, `/project/${copyId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  await navigate(sessionId, `/project/${copyId}/history`);
  await waitForElement(sessionId, "#history-heading");
  await activateButton(sessionId, "Undo scenario change", '[aria-labelledby="history-heading"]');
  await waitForOutcome(sessionId, "Scenario undo committed");
  const copyAfterUndo = (await projects(sessionId)).find(
    (project) => project.scenarioId === copyId,
  );
  assert.equal(copyAfterUndo.revision, copyProject.revision + 1);
  const copyHistoryAfter = (
    await nativeRequest(sessionId, "scenario_get_history_page", {
      schemaVersion: 1,
      scenarioId: copyId,
      expectedRevision: copyAfterUndo.revision,
      limit: 50,
      continuation: null,
    })
  ).result;
  assert.equal(copyHistoryAfter.entries.length, 1);
  assert.equal(copyHistoryAfter.entries[0].id, copyHistoryBefore.entries[0].id);
  assert.equal(copyHistoryAfter.entries[0].applied, false);
  assert.equal(copyHistoryAfter.redoAvailable, true);
  assert.equal((await appliedHistoryHead(sessionId, scenarioId)).id, sourceHistoryHead.id);

  await navigate(sessionId, "/projects");
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  if (
    !(await evaluate(sessionId, "return document.querySelector('#setup-results-details')?.open;"))
  )
    await activate(sessionId, "#setup-results-details > summary");
  await waitForElement(sessionId, "[data-optimize-infeasibility-inspect]");
  await activateButton(sessionId, "Inspect mapped conflict", "[data-optimize-panel]");
  await waitForElement(
    sessionId,
    '[data-infeasibility-inspection] ol[aria-label="Mapped Required-rule conflict groups"]',
  );
  const outputBase = `eutheto-explain-001-${Date.now()}-${requestId()}`;
  const collisionPath = join(directory, `${outputBase}-collision.json`);
  const reportPath = join(directory, `${outputBase}-report.json`);
  await activateButton(
    sessionId,
    "Export deterministic diagnostic",
    "[data-infeasibility-inspection]",
  );
  await nativeChooser("Export diagnostic report");
  await waitFor(
    sessionId,
    "return document.querySelector('[data-infeasibility-inspection] [role=\"alert\"]') && document.querySelector('main[data-operation-active=\"false\"]') !== null;",
  );
  const cancellationMessage = await getText(
    sessionId,
    '[data-infeasibility-inspection] [role="alert"]',
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return !!document.querySelector('[data-diagnostic-export-receipt]');",
    ),
    false,
    "Picker cancellation must not publish a diagnostic receipt",
  );
  const sourceAfterCancellation = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  assert.equal(sourceAfterCancellation.revision, sourceRevision);
  assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), sourceHistoryHead);

  const sentinel = "EXPLAIN-001 existing destination must remain byte-for-byte unchanged";
  await writeFile(collisionPath, sentinel, { flag: "wx", mode: 0o600 });
  await activateButton(
    sessionId,
    "Export deterministic diagnostic",
    "[data-infeasibility-inspection]",
  );
  await nativeChooser("Export diagnostic report", collisionPath, true);
  await waitFor(
    sessionId,
    `const alert = document.querySelector('[data-infeasibility-inspection] [role="alert"]');
    return alert?.textContent.trim() &&
      !alert.textContent.includes(arguments[0]) &&
      document.querySelector('main[data-operation-active="false"]') !== null
        ? alert.textContent.trim() : null;`,
    [cancellationMessage],
  );
  assert.equal(await readFile(collisionPath, "utf8"), sentinel);
  await rm(collisionPath);
  assert.equal(
    await evaluate(
      sessionId,
      "return !!document.querySelector('[data-diagnostic-export-receipt]');",
    ),
    false,
    "A rejected no-clobber publication must not produce a success receipt",
  );

  await activateButton(
    sessionId,
    "Export deterministic diagnostic",
    "[data-infeasibility-inspection]",
  );
  await nativeChooser("Export diagnostic report", reportPath);
  await waitForElement(sessionId, "[data-diagnostic-export-receipt]");
  const receipt = await getText(sessionId, "[data-diagnostic-export-receipt]");
  assert(receipt.includes(scenarioId));
  assert(receipt.includes(run.runId));
  assert(receipt.includes(diagnostics.manifestChecksum));
  assert(receipt.includes(review.evidenceChecksum));
  assert(receipt.includes(`source revision ${sourceRevision}`));
  assert(receipt.includes(`observed current revision ${sourceRevision}`));
  const reportText = await readFile(reportPath, "utf8");
  const report = JSON.parse(reportText);
  assert.equal(report.format, "eutheto/infeasibility-report");
  assert.equal(report.schemaVersion, 2);
  assert.equal(report.scenarioId, scenarioId);
  assert.equal(report.sourceRevision, sourceRevision);
  assert.equal(report.runId, run.runId);
  assert.equal(report.runManifestChecksum, diagnostics.manifestChecksum);
  assert.equal(report.manifest.runId, run.runId);
  assert.equal(report.manifest.checksum, diagnostics.manifestChecksum);
  assert.equal(report.manifest.runInputChecksum, report.runInputChecksum);
  assert.equal(report.evidenceChecksum, review.evidenceChecksum);
  assert(!Object.hasOwn(report, "condition"), "An ordinary Optimize run has no condition binding");
  const reportKeys = [];
  const collectKeys = (value) => {
    if (Array.isArray(value)) {
      for (const child of value) collectKeys(child);
    } else if (value !== null && typeof value === "object") {
      for (const [key, child] of Object.entries(value)) {
        reportKeys.push(key);
        collectKeys(child);
      }
    }
  };
  collectKeys(report);
  assert(
    reportKeys.every((key) => !/path|assets?|description|solver.?logs?/iu.test(key)),
    "The whitelisted private report must omit raw paths, assets, project descriptions, and solver logs",
  );
  assert(!reportText.includes(privateDescription));
  assert(!reportText.includes(dataHome));
  assert(!reportText.includes(application));
  assert.equal((await stat(reportPath)).mode & 0o777, 0o600);
  await screenshot(sessionId, "explain-001-native-private-report-receipt.png");
  const sourceAfterExport = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  );
  assert.equal(sourceAfterExport.revision, sourceRevision);
  assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), sourceHistoryHead);

  await package10DeleteFixture(sessionId, copyId);
  await package10DeleteFixture(sessionId, scenarioId);
  console.log(
    "PASS: packaged real-worker infeasibility → current-source mapped rule routing → reviewed native diagnostic copy and one-step undo → actual picker cancellation, no-clobber, and private source-bound JSON publication",
  );
}

async function packagedAssignmentCsvAcceptance(sessionId, directory) {
  const root = "[data-assignment-csv-import]";
  const title = `Phase07 assignment CSV ${Date.now()}`;
  const created = await nativeRequest(sessionId, "project_create", {
    schemaVersion: 1,
    title,
    description: "Complete unverified CSV import",
    domainPack: { id: "official.workforce", schemaVersion: 1 },
    settings: {
      timeZone: "UTC",
      locale: "en-US",
      units: "metric",
      firstDate: "2030-01-01",
      lastDate: "2030-01-01",
      gapPolicy: "reject",
      overlapPolicy: "reject",
    },
  });
  const scenarioId = created.result.scenarioId;
  const personIds = [requestId(), requestId()],
    shiftIds = [requestId(), requestId()],
    typeId = requestId();
  const mappedPath = join(directory, "assignment-mapped.csv");
  const malformedPath = join(directory, "assignment-reserved-invalid.csv");
  const emptyPath = join(directory, "assignment-empty.csv");
  const reportPath = join(directory, "assignment-exclusions.json");
  await writeFile(
    mappedPath,
    `person,shift\nExact person,Exact shift\nExact person,Exact shift\n${personIds[1]},${shiftIds[1]}\nUnknown person,${shiftIds[1]}\n`,
  );
  await writeFile(malformedPath, "\ufeffeutheto/assignments;1\nassignment_id;person_id;shift_id\n");
  await writeFile(emptyPath, "eutheto/assignments,1\nassignment_id,person_id,shift_id\n");
  let selectedPlanId;
  let selector = 0;
  async function labelled(label, tag = "input") {
    const id = `assignment-csv-smoke-${++selector}`;
    await waitFor(
      sessionId,
      `return [...document.querySelectorAll(arguments[0] + ' label')].some(node => node.textContent.trim().startsWith(arguments[1]) && node.querySelector(arguments[2]));`,
      [root, label, tag],
    );
    await evaluate(
      sessionId,
      `const label = [...document.querySelectorAll(arguments[0] + ' label')].find(node => node.textContent.trim().startsWith(arguments[1]) && node.querySelector(arguments[2])); label.querySelector(arguments[2]).id = arguments[3];`,
      [root, label, tag, id],
    );
    return `#${id}`;
  }
  async function choose(path, first = false) {
    await activateButton(
      sessionId,
      first ? "Choose a CSV file" : "Discard this source and choose another",
      root,
    );
    await nativeChooser("Choose CSV snapshot", path);
    await idle(sessionId);
  }
  async function readPlan() {
    return (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_get_entity",
        "eutheto.setup.entity_detail",
        { kind: "externalStartingPlan", entityId: selectedPlanId },
      )
    ).result.view.data.result.data;
  }
  try {
    const instant = (hour) => ({
      instant: `2030-01-01T${hour}:00:00Z`,
      local: `2030-01-01T${hour}:00:00`,
      offsetSeconds: 0,
    });
    const entities = [
      {
        id: typeId,
        kind: "assignmentType",
        name: "CSV duty",
        category: "clinic",
        timeBehavior: "elapsed",
        defaultDurationMinutes: 60,
        qualifications: { kind: "unconstrained" },
        locationBehavior: { kind: "optional" },
        workloadBucketIds: [],
      },
      ...personIds.map((id, index) => ({
        id,
        kind: "person",
        name: `CSV person ${index + 1}`,
        activeRange: { kind: "always" },
        qualificationGrants: [],
        eligibleAssignmentTypeIds: [typeId],
        teamIds: [],
        tags: [],
        workloadWeight: { numerator: 1, denominator: 1 },
      })),
      ...shiftIds.map((id, index) => ({
        id,
        kind: "shiftInstance",
        assignmentTypeId: typeId,
        startsAt: instant(index === 0 ? "09" : "10"),
        endsAt: instant(index === 0 ? "10" : "11"),
        coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
        tags: [],
        reportingAttribution: "startLocalDate",
        origin: { kind: "manual" },
      })),
    ];
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: created.result.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Assignment CSV fixture" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: {
          label: "CSV endpoint fixture",
          commands: entities.map((entity) => ({
            type: "applyDomainCommand",
            payload: {
              commandType: "official.workforce.add_entity",
              payload: { entity },
            },
          })),
        },
      },
    });
    await navigate(sessionId, "/projects");
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await activate(sessionId, `[aria-label=${JSON.stringify(`Open project ${title}`)}]`);
    await waitForElement(sessionId, "#setup-calendar");
    await evaluate(
      sessionId,
      `const label = [...document.querySelectorAll('label')]
      .find(node => node.textContent.trim().startsWith('Starting source for Repair and Alternatives'));
      if (!label) throw new Error('Expected explicit starting-source selector');
      label.querySelector('select').id = 'assignment-csv-source-mode';`,
    );
    await selectValue(sessionId, "#assignment-csv-source-mode", "external");
    await navigate(sessionId, `/project/${scenarioId}/assignments/import`);
    await waitForElement(sessionId, "#assignment-csv-heading");
    await choose(malformedPath, true);
    await waitForElement(sessionId, `${root} [role="alert"]`);
    assert.equal(
      await evaluate(sessionId, "return !!document.querySelector('#assignment-csv-dialect');"),
      false,
      "Reserved malformed exports cannot expose mapped fallback",
    );
    await choose(mappedPath);
    await waitForElement(sessionId, "#assignment-csv-dialect");
    await selectValue(sessionId, "#assignment-csv-dialect", "comma");
    await selectValue(sessionId, "#assignment-csv-header", "yes");
    await selectValue(sessionId, "#assignment-csv-person-column", "0");
    await selectValue(sessionId, "#assignment-csv-shift-column", "1");
    await setValue(sessionId, await labelled("Exact CSV token"), "Exact person");
    await choosePeopleReference(sessionId, "Person for token", "CSV person 1", root);
    await activateButton(sessionId, "Add token mapping", root);
    await selectValue(sessionId, await labelled("Endpoint", "select"), "shift");
    await setValue(sessionId, await labelled("Exact CSV token"), "Exact shift");
    await activate(sessionId, `${root} details summary`);
    await setValue(sessionId, await labelled("Work starts on"), "01/01/2030");
    await setValue(sessionId, await labelled("Work ends before"), "01/02/2030");
    await activateButton(sessionId, "Read native work", root);
    await waitFor(
      sessionId,
      "return [...document.querySelectorAll(arguments[0] + ' button')].some(button => button.textContent.includes(arguments[1]));",
      [root, shiftIds[0]],
    );
    await evaluate(
      sessionId,
      "const button = [...document.querySelectorAll(arguments[0] + ' button')].find(button => button.textContent.includes(arguments[1])); button.id = 'assignment-native-work-choice';",
      [root, shiftIds[0]],
    );
    await activate(sessionId, "#assignment-native-work-choice");
    await activateButton(sessionId, "Add token mapping", root);
    await selectValue(sessionId, "#assignment-csv-target-kind", "create");
    await setValue(sessionId, "#assignment-csv-name", "Native CSV starting plan");
    await activateButton(sessionId, "Review import changes", root);
    await waitForElement(sessionId, "#assignment-csv-review");
    assert.match(await getText(sessionId, root), /duplicatePair/u);
    assert.match(await getText(sessionId, root), /unresolvedPerson/u);
    await activateCheckbox(sessionId, "Skip record 3", root);
    await activateCheckbox(sessionId, "Skip record 5", root);
    await activateButton(sessionId, "Review import changes", root);
    await waitForElement(sessionId, `${root} [data-assignment-csv-review]`);
    assert.match(await getText(sessionId, root), /0 existing pairs → 2 complete imported pairs/u);
    const reviewedRevision = (await projects(sessionId)).find(
      (item) => item.scenarioId === scenarioId,
    ).revision;
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: reviewedRevision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "CSV concurrent writer" },
      truncateRedo: false,
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: "official.workforce.add_entity",
          payload: {
            entity: {
              id: requestId(),
              kind: "qualification",
              name: "Concurrent CSV metadata",
              description: "",
            },
          },
        },
      },
    });
    await waitFor(
      sessionId,
      "return !document.querySelector('#assignment-csv-review') && document.querySelector(arguments[0]).textContent.includes('previous approval is invalid');",
      [root],
    );
    assert.equal(
      await evaluate(sessionId, "return document.querySelector('#assignment-csv-name').value;"),
      "Native CSV starting plan",
    );
    assert.equal(
      await evaluate(
        sessionId,
        "return [...document.querySelectorAll(arguments[0] + ' input[type=\"checkbox\"]')].filter(input => input.checked).length;",
        [root],
      ),
      2,
    );
    await activateButton(sessionId, "Review import changes", root);
    await waitForElement(sessionId, "#assignment-csv-review");
    await screenshot(sessionId, "assignment-csv-native-review.png");
    await activateButton(sessionId, "Apply exactly this reviewed import", root);
    await waitForOutcome(sessionId, "committed as one command batch");
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0]).textContent.includes('snapshot has been consumed');",
      [root],
    );
    const plans = (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_search_entities",
        "eutheto.setup.entity_page",
        {
          kind: "externalStartingPlan",
          search: "",
          limit: 50,
          cursor: null,
        },
      )
    ).result.view.data.result.data.items;
    assert.equal(plans.length, 1);
    selectedPlanId = plans[0].entityId;
    const imported = await readPlan();
    assert.deepEqual(
      imported.assignments,
      [
        { personId: personIds[0], shiftId: shiftIds[0] },
        { personId: personIds[1], shiftId: shiftIds[1] },
      ].sort(
        (left, right) =>
          left.personId.localeCompare(right.personId) || left.shiftId.localeCompare(right.shiftId),
      ),
    );
    assert.equal(imported.kind, "externalStartingPlan");
    await activateButton(sessionId, "Read exclusion report", root);
    await waitFor(
      sessionId,
      "return document.querySelector('#assignment-csv-report').parentElement.textContent.includes('duplicatePair');",
    );
    await activateButton(sessionId, "Save exclusion report", root);
    await nativeChooser("Save excluded assignment rows", reportPath);
    await waitForOutcome(sessionId, "report was saved");
    const report = JSON.parse(await readFile(reportPath, "utf8"));
    assert.deepEqual(report.rows, {
      family: "assignments",
      excludedRows: [
        { record: 3, skipped: true, issue: "duplicatePair" },
        { record: 5, skipped: true, issue: "unresolvedPerson" },
      ],
    });
    await activateButton(sessionId, "Save exclusion report", root);
    await nativeChooser("Save excluded assignment rows");
    await waitForOutcome(sessionId, "Operation cancelled");
    assert.equal(
      (await readPlan()).originalImport.sourceBlake3,
      imported.originalImport.sourceBlake3,
    );
    assert(!(await getText(sessionId, root)).includes(mappedPath));
    await screenshot(sessionId, "assignment-csv-native-report.png");
    await choose(emptyPath);
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0]).textContent.includes('Strict Eutheto assignments v1');",
      [root],
    );
    await selectValue(sessionId, "#assignment-csv-target-kind", "replace");
    await choosePeopleReference(sessionId, "Starting plan to replace", imported.name, root);
    await setValue(sessionId, "#assignment-csv-name", "Explicit empty CSV plan");
    await activateButton(sessionId, "Review import changes", root);
    await waitForElement(sessionId, "#assignment-csv-review");
    assert.equal(
      await evaluate(
        sessionId,
        "return [...document.querySelectorAll(arguments[0] + ' button')].some(button => button.textContent.trim() === 'Apply exactly this reviewed import');",
        [root],
      ),
      false,
    );
    await activate(sessionId, "#assignment-csv-allow-empty");
    await activateButton(sessionId, "Review import changes", root);
    await waitForElement(sessionId, "#assignment-csv-review");
    assert.match(await getText(sessionId, root), /2 existing pairs → 0 complete imported pairs/u);
    await evaluate(
      sessionId,
      `const callbacks = window.__TAURI_INTERNALS__.callbacks;
      const descriptor = Object.getOwnPropertyDescriptor(callbacks, 'get'), original = callbacks.get;
      const gate = { commandId: null };
      gate.restore = () => descriptor ? Object.defineProperty(callbacks, 'get', descriptor) : Reflect.deleteProperty(callbacks, 'get');
      window.__assignmentCsvGate = gate;
      Object.defineProperty(callbacks, 'get', { configurable: true, value: function(id) {
        const callback = original.call(this, id); if (!callback) return callback;
        return data => { if (gate.commandId === null && data?.result?.outcome?.kind === 'applied') {
          gate.commandId = data.result.outcome.commandId; callback({ ...data, result: null }); return;
        } callback(data); };
      }});`,
    );
    await activateButton(sessionId, "Apply exactly this reviewed import", root);
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0]).textContent.includes('apply outcome is unknown');",
      [root],
      60_000,
    );
    const committedId = await evaluate(
      sessionId,
      "window.__assignmentCsvGate.restore(); return window.__assignmentCsvGate.commandId;",
    );
    assert.equal(typeof committedId, "string");
    const empty = await readPlan();
    assert.deepEqual(empty.assignments, []);
    assert.notEqual(empty.originalImport.sourceBlake3, imported.originalImport.sourceBlake3);
    await activateButton(sessionId, "Check saved history");
    await waitFor(
      sessionId,
      "return document.querySelector('#mutation-recovery-title')?.textContent.trim() === 'Saved command confirmed';",
    );
    assert.equal((await appliedHistoryHead(sessionId, scenarioId))?.id, committedId);
    await activateButton(sessionId, "Acknowledge and dismiss");
    await activate(sessionId, `a[href=${JSON.stringify(`#/project/${scenarioId}/setup`)}]`);
    await activateButton(sessionId, "Discard and leave", '[role="dialog"]');
    await waitForElement(sessionId, "#setup-calendar");
    await navigate(sessionId, `/project/${scenarioId}/history`);
    await activateButton(sessionId, "Undo scenario change", '[aria-labelledby="history-heading"]');
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
    assert.deepEqual(await readPlan(), imported);
    await activateButton(sessionId, "Redo scenario change", '[aria-labelledby="history-heading"]');
    await waitForOutcome(sessionId, "Scenario redo committed", 120_000);
    assert.deepEqual(await readPlan(), empty);
    console.log(
      "PASS: packaged assignment CSV native picker, reserved-format refusal, exact endpoint mapping, duplicate/skip decisions, stale re-review, atomic complete plan, complete saved report/cancellation, canonical explicit-empty replacement, lost-receipt History reconciliation and exact Undo/Redo",
    );
  } finally {
    await evaluate(
      sessionId,
      "window.__assignmentCsvGate?.restore?.(); delete window.__assignmentCsvGate;",
    ).catch(() => undefined);
    await navigate(sessionId, "/projects").catch(() => undefined);
    await package10DeleteFixture(sessionId, scenarioId);
  }
}

async function packagedShiftCsvAcceptance(sessionId, directory) {
  const root = "[data-shift-csv-import]";
  const historyRoot = '[aria-labelledby="history-heading"]';
  const title = `Phase07 shift CSV ${Date.now()}`;
  const created = await nativeRequest(sessionId, "project_create", {
    schemaVersion: 1,
    title,
    description: "Native stored shift import with explicit local-time intent",
    domainPack: { id: "official.workforce", schemaVersion: 1 },
    settings: {
      timeZone: "America/New_York",
      locale: "en-US",
      units: "metric",
      firstDate: "2030-01-01",
      lastDate: "2030-11-04",
      gapPolicy: "reject",
      overlapPolicy: "reject",
    },
  });
  const scenarioId = created.result.scenarioId;
  const typeId = requestId(),
    locationId = requestId(),
    qualificationId = requestId();
  const templateId = requestId(),
    manualId = requestId(),
    detachedId = requestId();
  const suppliedId = requestId(),
    gapId = requestId(),
    foldId = requestId();
  const sourcePath = join(directory, "shift-reviewed.csv");
  const noOpPath = join(directory, "shift-no-change.csv");
  const clearPath = join(directory, "shift-clear.csv");
  const reportPath = join(directory, "shift-exclusions.json");
  const fields = [
    "assignmentTypeId",
    "locationId",
    "startsAt",
    "endsAt",
    "coverage",
    "tags",
    "reportingAttribution",
  ];
  const coverage = {
    kind: "exact",
    count: 2,
    qualificationMinimums: [
      {
        qualifications: {
          allQualificationIds: [qualificationId.toUpperCase()],
          anyQualificationIds: [],
        },
        minimum: 1,
      },
    ],
  };
  const importedCoverage = {
    kind: "atLeast",
    minimum: 1,
    preferredCount: 2,
    maximumCount: 3,
    qualificationMinimums: [
      {
        qualifications: { allQualificationIds: [qualificationId], anyQualificationIds: [] },
        minimum: 1,
      },
    ],
  };
  const csv = (rows) =>
    [["id", ...fields], ...rows]
      .map((row) => row.map((cell) => `"${String(cell).replaceAll('"', '""')}"`).join(","))
      .join("\n") + "\n";
  const aliasCoverage = {
    ...importedCoverage,
    qualificationMinimums: [
      {
        qualifications: { allQualificationIds: ["Skill"], anyQualificationIds: [] },
        minimum: 1,
      },
    ],
  };
  await writeFile(
    sourcePath,
    csv([
      [
        suppliedId.toUpperCase(),
        "Duty",
        "Site",
        "2030-01-01T23:30:00",
        "2030-01-02T01:00:00",
        JSON.stringify(aliasCoverage),
        '["imported"]',
        "endLocalDate",
      ],
      [detachedId, "", "", "", "2030-01-01T11:30:00", "", "", ""],
      [detachedId.toUpperCase(), "", "", "", "2030-01-01T12:00:00", "", "", ""],
      [manualId, "", "", "2030-01-01T09:00:00-05:00", "", "", "", ""],
      [
        gapId,
        "Duty",
        "",
        "2030-03-10T02:30:00",
        "2030-03-10T04:00:00",
        JSON.stringify(aliasCoverage),
        "",
        "startLocalDate",
      ],
      [
        foldId,
        "Duty",
        "",
        "2030-11-03T01:30:00",
        "2030-11-03T03:00:00",
        JSON.stringify(aliasCoverage),
        "",
        "startLocalDate",
      ],
    ]),
  );
  await writeFile(
    noOpPath,
    csv([
      [
        detachedId.toUpperCase(),
        typeId,
        locationId,
        "2030-01-01T09:00",
        "2030-01-01T11:30",
        JSON.stringify({
          ...coverage,
          qualificationMinimums: [
            {
              qualifications: { allQualificationIds: [qualificationId], anyQualificationIds: [] },
              minimum: 1,
            },
          ],
        }),
        "",
        "",
      ],
    ]),
  );
  await writeFile(clearPath, csv([[detachedId, "", "", "", "", "", "", ""]]));
  async function entity(id) {
    return (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_get_entity",
        "eutheto.setup.entity_detail",
        { kind: "shiftInstance", entityId: id },
      )
    ).result.view.data.result.data;
  }
  async function shifts() {
    const page = (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_search_entities",
        "eutheto.setup.entity_page",
        { kind: "shiftInstance", search: "", limit: 50, cursor: null },
      )
    ).result.view.data.result.data;
    assert.equal(page.continuation, null);
    return page.items.map((item) => item.entityId).sort();
  }
  async function choose(path, first = false) {
    await activateButton(
      sessionId,
      first ? "Choose a CSV file" : "Discard this source and choose another",
      root,
    );
    await nativeChooser("Choose CSV snapshot", path);
    await waitForElement(sessionId, "#shift-csv-dialect");
    await idle(sessionId);
    assert.equal(
      await evaluate(sessionId, "return document.querySelector('#shift-csv-dialect').value;"),
      "",
    );
    assert.equal(
      await evaluate(sessionId, "return document.querySelector('#shift-csv-header').value;"),
      "",
    );
    await selectValue(sessionId, "#shift-csv-dialect", "comma");
    await selectValue(sessionId, "#shift-csv-header", "yes");
    await setValue(sessionId, "#shift-csv-width", "8");
    await selectValue(sessionId, "#shift-csv-identity-column", "0");
    for (const [index, field] of fields.entries()) {
      await selectValue(sessionId, `#shift-csv-${field}-column`, String(index + 1));
      assert.equal(
        await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
          `#shift-csv-${field}-blank`,
        ]),
        "preserve",
      );
    }
  }
  async function review() {
    await activateButton(sessionId, "Review import changes", root);
    await waitForElement(sessionId, "#shift-csv-review");
  }
  async function approve() {
    await activate(sessionId, "#shift-csv-approve");
    await activateButton(sessionId, "Apply exactly this reviewed import", root);
  }
  async function history() {
    if (!(await evaluate(sessionId, "return document.querySelector('.project-nav-more')?.open;")))
      await activate(sessionId, ".project-nav-more > summary");
    await activate(sessionId, `a[href=${JSON.stringify(`#/project/${scenarioId}/history`)}]`);
    await activateButton(sessionId, "Discard and leave", '[role="dialog"]');
    await waitForElement(sessionId, "#history-heading");
  }
  async function openImporter() {
    await navigate(sessionId, `/project/${scenarioId}/work`);
    await waitFor(
      sessionId,
      "const work = document.querySelector('[aria-labelledby=\"work-heading\"]'); return work?.querySelector('#work-window-start') && !work.querySelector('[role=\"status\"]');",
    );
    await navigate(sessionId, `/project/${scenarioId}/work/import`);
    await waitForElement(sessionId, "#shift-csv-heading");
  }
  try {
    const resolved = (hour) => ({
      instant: `2030-01-01T${hour}:00:00-05:00`,
      local: `2030-01-01T${hour}:00:00`,
      offsetSeconds: -18000,
    });
    const entities = [
      { id: qualificationId, kind: "qualification", name: "Shift CSV skill", description: "" },
      { id: locationId, kind: "location", name: "Shift CSV site", transitions: [] },
      {
        id: typeId,
        kind: "assignmentType",
        name: "Shift CSV duty",
        category: "clinic",
        timeBehavior: "elapsed",
        defaultDurationMinutes: 60,
        qualifications: { kind: "unconstrained" },
        locationBehavior: { kind: "optional" },
        workloadBucketIds: [],
      },
      {
        id: templateId,
        kind: "shiftTemplate",
        name: "Shift CSV detached source",
        assignmentTypeId: typeId,
        locationId,
        recurrence: {
          weekdays: ["tuesday"],
          effectiveRange: { startDate: "2030-01-01", endDateExclusive: "2030-01-02" },
          excludedDates: [],
        },
        timing: { kind: "elapsedDuration", startTime: "09:00:00", durationMinutes: 60 },
        coverage,
        tags: ["template"],
        reportingAttribution: "startLocalDate",
        occurrenceIdentities: {},
      },
      ...[manualId, detachedId].map((id) => ({
        id,
        kind: "shiftInstance",
        assignmentTypeId: typeId.toUpperCase(),
        locationId: locationId.toUpperCase(),
        startsAt: resolved("09"),
        endsAt: resolved("10"),
        coverage,
        tags: ["retained", "ordered"],
        reportingAttribution: "startLocalDate",
        origin:
          id === manualId
            ? { kind: "manual" }
            : { kind: "detached", templateId, occurrenceDate: "2030-01-01" },
      })),
    ];
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: created.result.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Shift CSV fixture" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: {
          label: "Stored manual and detached shift fixture",
          commands: entities.map((record) => ({
            type: "applyDomainCommand",
            payload: { commandType: "official.workforce.add_entity", payload: { entity: record } },
          })),
        },
      },
    });
    const original = await entity(detachedId),
      manual = await entity(manualId);
    const originalIds = [manualId, detachedId].sort();
    assert.deepEqual(await shifts(), originalIds);
    await navigate(sessionId, "/projects");
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await activate(sessionId, `[aria-label=${JSON.stringify(`Open project ${title}`)}]`);
    await waitForElement(sessionId, "#setup-calendar");
    await openImporter();
    const beforeCancel = (await projects(sessionId)).find(
      (item) => item.scenarioId === scenarioId,
    ).revision;
    await activateButton(sessionId, "Choose a CSV file", root);
    await nativeChooser("Choose CSV snapshot");
    await waitForOutcome(sessionId, "Operation cancelled");
    assert.equal(
      (await projects(sessionId)).find((item) => item.scenarioId === scenarioId).revision,
      beforeCancel,
    );
    await choose(sourcePath, true);
    for (const [kind, token, label, name] of [
      ["assignmentType", "Duty", "Assignment type", "Shift CSV duty"],
      ["location", "Site", "Location", "Shift CSV site"],
      ["qualification", "Skill", "Qualification", "Shift CSV skill"],
    ]) {
      await selectValue(sessionId, "#shift-csv-mapping-endpoint", kind);
      await setValue(sessionId, "#shift-csv-token", token);
      await choosePeopleReference(sessionId, `Reference for exact token · ${label}`, name, root);
      await activate(sessionId, "#shift-csv-add-alias");
    }
    await setValue(sessionId, "#shift-csv-inspect-record", "2");
    await activateButton(sessionId, "Inspect selected record", root);
    await waitFor(
      sessionId,
      "return document.querySelector('[aria-label=\"Selected native CSV record\"]')?.textContent.includes('2030-01-01T23:30:00');",
    );
    await review();
    assert.match(await getText(sessionId, root), /unresolvedIdentity/u);
    assert.match(await getText(sessionId, root), /duplicateTarget/u);
    assert.match(await getText(sessionId, root), /invalidLocalTime/u);
    assert.equal(
      await evaluate(sessionId, "return !!document.querySelector('#shift-csv-approve');"),
      false,
    );
    assert.deepEqual(
      await shifts(),
      originalIds,
      "A blocked preview cannot add the unknown supplied ID",
    );
    await activate(sessionId, "#shift-csv-add-2");
    await review();
    assert.match(await getText(sessionId, '[data-shift-csv-record="6"]'), /unresolvableLocalTime/u);
    assert.match(await getText(sessionId, '[data-shift-csv-record="7"]'), /unresolvableLocalTime/u);
    assert.equal(
      await evaluate(sessionId, "return !!document.querySelector('#shift-csv-approve');"),
      false,
    );
    for (const record of [4, 5, 6, 7]) await activate(sessionId, `#shift-csv-skip-${record}`);
    await review();
    const addedText = await getText(sessionId, '[data-shift-csv-record="2"]');
    for (const meaning of [
      suppliedId,
      "2030-01-01T23:30:00",
      "2030-01-02T04:30:00",
      "-18000",
      "2030-01-02T01:00:00",
      "2030-01-02T06:00:00",
      "preferredCount",
      "maximumCount",
      qualificationId,
      "endLocalDate",
      "manual",
      "imported",
    ])
      assert(addedText.includes(meaning), `Native new-shift review must expose ${meaning}`);
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelector('[data-shift-csv-record=\"2\"]')?.textContent.includes(arguments[0]);",
        [suppliedId],
      ),
      true,
    );
    const reviewedRevision = (await projects(sessionId)).find(
      (item) => item.scenarioId === scenarioId,
    ).revision;
    const concurrent = {
      ...original,
      tags: ["independent", "ordered"],
      reportingAttribution: "endLocalDate",
    };
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: reviewedRevision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Shift CSV concurrent edit" },
      truncateRedo: false,
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: "official.workforce.update_entity",
          payload: { entity: concurrent },
        },
      },
    });
    await waitFor(
      sessionId,
      "return !document.querySelector('#shift-csv-review') && document.querySelector(arguments[0]).textContent.includes('previous approval is invalid');",
      [root],
    );
    assert.equal(
      await evaluate(sessionId, "return document.querySelector('#shift-csv-endsAt-column').value;"),
      "4",
    );
    assert.match(await getText(sessionId, root), /Skill/u);
    await review();
    const updateText = await getText(sessionId, '[data-shift-csv-record="3"]');
    for (const meaning of [
      "independent",
      "detached",
      templateId,
      "2030-01-01",
      "endLocalDate",
      "2030-01-01T11:30:00",
      "2030-01-01T16:30:00",
      "-18000",
    ])
      assert(updateText.includes(meaning), `Re-reviewed detached update must expose ${meaning}`);
    await screenshot(sessionId, "shift-csv-native-review.png");
    await approve();
    await waitForOutcome(sessionId, "committed as one command batch");
    const imported = {
      ...concurrent,
      endsAt: {
        local: "2030-01-01T11:30:00",
        instant: "2030-01-01T16:30:00Z",
        offsetSeconds: -18000,
      },
    };
    const added = {
      id: suppliedId,
      kind: "shiftInstance",
      assignmentTypeId: typeId,
      locationId,
      startsAt: {
        local: "2030-01-01T23:30:00",
        instant: "2030-01-02T04:30:00Z",
        offsetSeconds: -18000,
      },
      endsAt: {
        local: "2030-01-02T01:00:00",
        instant: "2030-01-02T06:00:00Z",
        offsetSeconds: -18000,
      },
      coverage: importedCoverage,
      tags: ["imported"],
      reportingAttribution: "endLocalDate",
      origin: { kind: "manual" },
    };
    assert.deepEqual(
      await entity(suppliedId),
      added,
      "Explicit Add must use exactly the native supplied ID",
    );
    assert.deepEqual(
      await entity(detachedId),
      imported,
      "Blank Preserve must retain every unrelated raw field and detached origin",
    );
    assert.deepEqual(await entity(manualId), manual);
    assert.deepEqual(
      await shifts(),
      [...originalIds, suppliedId].sort(),
      "All excluded rows must remain uncommitted",
    );
    const ordinaryHead = await appliedHistoryHead(sessionId, scenarioId);
    assert(ordinaryHead, "The ordinary batch must create one durable History entry");
    await activateButton(sessionId, "Read exclusion report", root);
    await activateButton(sessionId, "Save exclusion report", root);
    await nativeChooser("Save excluded shift rows", reportPath);
    await waitForOutcome(sessionId, "report was saved");
    assert.deepEqual(JSON.parse(await readFile(reportPath, "utf8")).rows, {
      family: "shifts",
      excludedRows: [
        { record: 4, skipped: true, issue: null },
        { record: 5, skipped: true, issue: null },
        { record: 6, skipped: true, issue: null },
        { record: 7, skipped: true, issue: null },
      ],
    });
    await activateButton(sessionId, "Save exclusion report", root);
    await nativeChooser("Save excluded shift rows");
    await waitForOutcome(sessionId, "Operation cancelled");
    assert.equal((await appliedHistoryHead(sessionId, scenarioId)).id, ordinaryHead.id);
    assert(!(await getText(sessionId, root)).includes(sourcePath));
    assert(!(await getText(sessionId, root)).includes(reportPath));
    await screenshot(sessionId, "shift-csv-native-report.png");
    await history();
    await activateButton(sessionId, "Undo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
    assert.deepEqual(await shifts(), originalIds);
    assert.deepEqual(await entity(detachedId), concurrent);
    assert.deepEqual(await entity(manualId), manual);
    await activateButton(sessionId, "Redo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario redo committed", 120_000);
    assert.deepEqual(await shifts(), [...originalIds, suppliedId].sort());
    assert.deepEqual(await entity(detachedId), imported);
    assert.deepEqual(await entity(suppliedId), added);
    await openImporter();
    const beforeNoOp = (await projects(sessionId)).find(
      (item) => item.scenarioId === scenarioId,
    ).revision;
    const beforeNoOpHead = await appliedHistoryHead(sessionId, scenarioId);
    await choose(noOpPath, true);
    await review();
    assert.match(await getText(sessionId, '[data-shift-csv-record="2"]'), /unchanged/u);
    await approve();
    await waitForOutcome(sessionId, "made no changes and created no history entry");
    assert.equal(
      (await projects(sessionId)).find((item) => item.scenarioId === scenarioId).revision,
      beforeNoOp,
    );
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), beforeNoOpHead);
    assert.deepEqual(
      await entity(detachedId),
      imported,
      "Equivalent typed reference/time/coverage meanings must preserve raw JSON",
    );
    await choose(clearPath);
    await selectValue(sessionId, "#shift-csv-locationId-blank", "clear");
    await selectValue(sessionId, "#shift-csv-tags-blank", "clear");
    await review();
    await evaluate(
      sessionId,
      `const callbacks = window.__TAURI_INTERNALS__.callbacks;
      const descriptor = Object.getOwnPropertyDescriptor(callbacks, 'get'), original = callbacks.get;
      const gate = { commandId: null };
      gate.restore = () => descriptor ? Object.defineProperty(callbacks, 'get', descriptor) : Reflect.deleteProperty(callbacks, 'get');
      window.__shiftCsvGate = gate;
      Object.defineProperty(callbacks, 'get', { configurable: true, value: function(id) {
        const callback = original.call(this, id); if (!callback) return callback;
        return data => { if (gate.commandId === null && data?.result?.outcome?.kind === 'applied') {
          gate.commandId = data.result.outcome.commandId; callback({ ...data, result: null }); return;
        } callback(data); };
      }});`,
    );
    await approve();
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0]).textContent.includes('apply outcome is unknown');",
      [root],
      60_000,
    );
    const committedId = await evaluate(
      sessionId,
      "window.__shiftCsvGate.restore(); return window.__shiftCsvGate.commandId;",
    );
    assert.equal(typeof committedId, "string");
    const cleared = { ...imported, tags: [] };
    delete cleared.locationId;
    assert.deepEqual(
      await entity(detachedId),
      cleared,
      "A lost receipt must not be mistaken for rollback",
    );
    assert.equal(
      await evaluate(sessionId, "return !!document.querySelector('#shift-csv-approve');"),
      false,
    );
    await activateButton(sessionId, "Check saved history");
    await waitFor(
      sessionId,
      "return document.querySelector('#mutation-recovery-title')?.textContent.trim() === 'Saved command confirmed';",
    );
    assert.equal((await appliedHistoryHead(sessionId, scenarioId)).id, committedId);
    await activateButton(sessionId, "Acknowledge and dismiss");
    await history();
    await activateButton(sessionId, "Undo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
    assert.deepEqual(await entity(detachedId), imported);
    assert.deepEqual(await entity(suppliedId), added);
    await activateButton(sessionId, "Redo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario redo committed", 120_000);
    assert.deepEqual(await entity(detachedId), cleared);
    assert.deepEqual(await entity(suppliedId), added);
    assert.deepEqual(await entity(manualId), manual);
    console.log(
      "PASS: packaged shift CSV native picker, exact reference mapping, supplied-ID Add, detached partial preservation, negative-offset/cross-midnight review, offset/gap/fold refusal and explicit skips, stale re-review, atomic apply, full saved report/cancellation, semantic no-op nohistory, lost-receipt History recovery and exact Undo/Redo",
    );
  } finally {
    await evaluate(
      sessionId,
      "window.__shiftCsvGate?.restore?.(); delete window.__shiftCsvGate;",
    ).catch(() => undefined);
    await navigate(sessionId, "/projects").catch(() => undefined);
    await package10DeleteFixture(sessionId, scenarioId);
  }
}

async function packagedAvailabilityCsvAcceptance(sessionId, directory) {
  const root = "[data-availability-csv-import]";
  const historyRoot = '[aria-labelledby="history-heading"]';
  const title = `Phase07 availability CSV ${Date.now()}`;
  const created = await nativeRequest(sessionId, "project_create", {
    schemaVersion: 1,
    title,
    description: "Native availability and time-off CSV acceptance",
    domainPack: { id: "official.workforce", schemaVersion: 1 },
    settings: {
      timeZone: "America/New_York",
      locale: "en-US",
      units: "metric",
      firstDate: "2030-01-01",
      lastDate: "2030-11-04",
      gapPolicy: "reject",
      overlapPolicy: "reject",
    },
  });
  const scenarioId = created.result.scenarioId;
  const personId = requestId();
  const typeIds = [requestId(), requestId()],
    locationIds = [requestId(), requestId()];
  const partialId = requestId(),
    weeklyId = requestId(),
    instantId = requestId();
  const convertibleId = requestId(),
    incompleteWeeklyId = requestId();
  const referencedIds = [requestId(), requestId()],
    preferenceIds = [requestId(), requestId()];
  const addedIds = [requestId(), requestId(), requestId(), requestId()];
  const fields = [
    "personId",
    "availabilityKind",
    "startsAt",
    "endsAt",
    "weeklyWindows",
    "effectiveStartDate",
    "effectiveEndDateExclusive",
    "assignmentTypeIds",
    "locationIds",
    "source",
    "note",
  ];
  const sourcePath = join(directory, "availability-reviewed.csv");
  const noOpPath = join(directory, "availability-no-change.csv");
  const clearPath = join(directory, "availability-clear.csv");
  const reportPath = join(directory, "availability-exclusions.json");
  const windows = [
    {
      weekdays: ["tuesday", "monday"],
      startTime: "23:30:00",
      endTime: "01:00:00",
      endDayOffset: 1,
    },
    { weekdays: ["friday"], startTime: "09:00:00", endTime: "12:00:00", endDayOffset: 0 },
    {
      weekdays: ["tuesday", "monday"],
      startTime: "23:30:00",
      endTime: "01:00:00",
      endDayOffset: 1,
    },
  ];
  const jsonWindows = JSON.stringify(windows);
  const csv = (rows) =>
    [["id", ...fields], ...rows.map((row) => [row.id, ...fields.map((field) => row[field] ?? "")])]
      .map((row) => row.map((cell) => `"${String(cell).replaceAll('"', '""')}"`).join(","))
      .join("\n") + "\n";
  const newInstant = (id, availabilityKind = "unavailable") => ({
    id,
    personId: "Shared",
    availabilityKind,
    startsAt: "2030-01-01T23:30:00",
    endsAt: "2030-01-02T01:00:00",
    effectiveStartDate: "2029-12-01",
    effectiveEndDateExclusive: "2032-02-01",
    assignmentTypeIds: '["Shared"]',
    locationIds: '["Shared"]',
    source: " authored source ",
    note: " \t ",
  });
  const newWeekly = (id, availabilityKind) => ({
    ...newInstant(id, availabilityKind),
    startsAt: "",
    endsAt: "",
    weeklyWindows: jsonWindows,
  });
  const rows = [
    newInstant(addedIds[0]),
    newWeekly(addedIds[1], "availableOnly"),
    newInstant(addedIds[2], "approvedTimeOff"),
    newWeekly(addedIds[3], "requestedTimeOff"),
    { id: partialId, endsAt: "2030-01-01T11:30:00", effectiveEndDateExclusive: "2032-03-01" },
    { id: weeklyId, startsAt: "2030-01-02T09:00:00", endsAt: "2030-01-02T10:00:00" },
    { id: instantId, weeklyWindows: jsonWindows },
    { id: convertibleId, availabilityKind: "approvedTimeOff" },
    { id: partialId.toUpperCase(), endsAt: "2030-01-01T12:00:00" },
    {
      ...newInstant(requestId()),
      assignmentTypeIds: JSON.stringify(["Shared", typeIds[0].toUpperCase()]),
    },
    { ...newInstant(requestId()), assignmentTypeIds: "[]" },
    { ...newInstant(requestId()), locationIds: "null" },
    {
      ...newWeekly(requestId(), "availableOnly"),
      weeklyWindows: JSON.stringify([{ ...windows[0], unknown: true }]),
    },
    {
      ...newWeekly(requestId(), "availableOnly"),
      weeklyWindows: JSON.stringify([{ ...windows[0], weekdays: ["monday", "monday"] }]),
    },
    { ...newInstant(requestId()), weeklyWindows: jsonWindows },
    { ...newInstant(requestId()), startsAt: "2030-01-01T23:30:00-05:00" },
    { ...newInstant(requestId()), startsAt: "2030-03-10T02:30:00", endsAt: "2030-03-10T04:00:00" },
    { ...newInstant(requestId()), startsAt: "2030-11-03T01:30:00", endsAt: "2030-11-03T03:00:00" },
    { ...newInstant(requestId()), endsAt: "2030-01-01T22:00:00" },
    { id: incompleteWeeklyId, startsAt: "2030-01-02T09:00:00" },
    ...referencedIds.map((id) => ({ id, availabilityKind: "approvedTimeOff" })),
    {
      ...newInstant(requestId()),
      locationIds: JSON.stringify(["Shared", locationIds[0].toUpperCase()]),
    },
    { ...newWeekly(requestId(), "availableOnly"), weeklyWindows: "[]" },
    { ...newWeekly(requestId(), "availableOnly"), weeklyWindows: "null" },
  ];
  const excludedRecords = rows.slice(8).map((_, index) => index + 10);
  await writeFile(sourcePath, csv(rows));
  await writeFile(clearPath, csv([{ id: partialId }]));
  async function entity(id) {
    return (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_get_entity",
        "eutheto.setup.entity_detail",
        { kind: "availability", entityId: id },
      )
    ).result.view.data.result.data;
  }
  async function records() {
    const page = (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_search_entities",
        "eutheto.setup.entity_page",
        { kind: "availability", search: "", limit: 50, cursor: null },
      )
    ).result.view.data.result.data;
    assert.equal(page.continuation, null);
    return page.items.map((item) => item.entityId).sort();
  }
  async function revision() {
    return (await projects(sessionId)).find((item) => item.scenarioId === scenarioId).revision;
  }
  async function verifyPreferences() {
    for (const [index, id] of preferenceIds.entries()) {
      assert.deepEqual((await package8Rule(sessionId, scenarioId, id, "preference")).record, {
        id,
        kind: "requestedTimeOff",
        active: index === 0,
        scope: { people: { kind: "all" } },
        priority: "normal",
        weight: 1,
        availabilityIds: [referencedIds[index]],
      });
    }
    const overview = (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_get_view",
        "official.workforce.setup.overview",
        {
          source: { kind: "stored" },
          query: {
            schemaVersion: 1,
            viewId: "official.workforce.setup.overview",
            parameters: {},
          },
        },
      )
    ).result.view.data.result.data;
    assert.equal(overview.requiredRules, 0, "Import must not create an availability rule");
    assert.equal(overview.preferences, 2, "Import must not create or remove preferences");
    assert.equal(overview.activePreferences, 1);
  }
  async function choose(path, first = false) {
    await activateButton(
      sessionId,
      first ? "Choose a CSV file" : "Discard this source and choose another",
      root,
    );
    await nativeChooser("Choose CSV snapshot", path);
    await waitForElement(sessionId, "#availability-csv-dialect");
    await idle(sessionId);
    for (const field of ["dialect", "header"])
      assert.equal(
        await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
          `#availability-csv-${field}`,
        ]),
        "",
      );
    await selectValue(sessionId, "#availability-csv-dialect", "comma");
    await selectValue(sessionId, "#availability-csv-header", "yes");
    await setValue(sessionId, "#availability-csv-width", "12");
    await selectValue(sessionId, "#availability-csv-identity-column", "0");
    for (const [index, field] of fields.entries()) {
      await selectValue(sessionId, `#availability-csv-${field}-column`, String(index + 1));
      assert.equal(
        await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
          `#availability-csv-${field}-blank`,
        ]),
        "preserve",
      );
    }
  }
  async function review() {
    await activateButton(sessionId, "Review import changes", root);
    await waitForElement(sessionId, "#availability-csv-review");
  }
  async function approve() {
    await activate(sessionId, "#availability-csv-approve");
    await activateButton(sessionId, "Apply exactly this reviewed import", root);
  }
  async function history() {
    if (!(await evaluate(sessionId, "return document.querySelector('.project-nav-more')?.open;")))
      await activate(sessionId, ".project-nav-more > summary");
    await activate(sessionId, `a[href=${JSON.stringify(`#/project/${scenarioId}/history`)}]`);
    await activateButton(sessionId, "Discard and leave", '[role="dialog"]');
    await waitForElement(sessionId, "#history-heading");
    await idle(sessionId);
  }
  async function openImporter() {
    await navigate(sessionId, `/project/${scenarioId}/availability`);
    // The nested picker's idle hint is a status, not a pending native read.
    await waitFor(
      sessionId,
      "const view = document.querySelector('[aria-labelledby=\"availability-heading\"]'); return view?.querySelector('#availability-person:not(:disabled)') && !view.querySelector(':scope > [role=\"status\"], [role=\"alert\"]');",
    );
    await idle(sessionId);
    await navigate(sessionId, `/project/${scenarioId}/availability/import`);
    await waitForElement(sessionId, "#availability-csv-heading");
  }
  const initialIds = [
    partialId,
    weeklyId,
    instantId,
    convertibleId,
    incompleteWeeklyId,
    ...referencedIds,
  ].sort();
  let failed = false;
  try {
    const availability = (id, availabilityKind = "unavailable") => ({
      id,
      kind: "availability",
      personId: personId.toUpperCase(),
      availabilityKind,
      timeWindow: {
        kind: "instant",
        startsAt: "2030-01-01T09:00:00.123456780-05:00",
        endsAt: "2030-01-01T10:00:00.123456780-05:00",
      },
      effectiveRange: { startDate: "2029-12-01", endDateExclusive: "2032-02-01" },
      assignmentTypeIds: [typeIds[1].toUpperCase(), typeIds[0].toUpperCase()],
      locationIds: [locationIds[1].toUpperCase(), locationIds[0].toUpperCase()],
      source: " raw source ",
      note: "raw <note> \t",
    });
    const entities = [
      ...locationIds.map((id, index) => ({
        id,
        kind: "location",
        name: `Availability CSV site ${index + 1}`,
        transitions: [],
      })),
      ...typeIds.map((id, index) => ({
        id,
        kind: "assignmentType",
        name: `Availability CSV duty ${index + 1}`,
        category: "clinic",
        timeBehavior: "elapsed",
        defaultDurationMinutes: 60,
        qualifications: { kind: "unconstrained" },
        locationBehavior: { kind: "optional" },
        workloadBucketIds: [],
      })),
      {
        id: personId,
        kind: "person",
        name: "Availability CSV person",
        externalId: "availability-csv-person",
        activeRange: { kind: "always" },
        qualificationGrants: [],
        eligibleAssignmentTypeIds: typeIds,
        teamIds: [],
        tags: [],
        workloadWeight: { numerator: 1, denominator: 1 },
        display: {},
      },
      availability(partialId),
      { ...availability(weeklyId, "availableOnly"), timeWindow: { kind: "weekly", windows } },
      availability(instantId, "approvedTimeOff"),
      availability(convertibleId, "requestedTimeOff"),
      {
        ...availability(incompleteWeeklyId, "availableOnly"),
        timeWindow: { kind: "weekly", windows },
      },
      ...referencedIds.map((id) => availability(id, "requestedTimeOff")),
    ];
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: created.result.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Availability CSV fixture" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: {
          label: "Availability windows and incoming requested-time-off preferences",
          commands: [
            ...entities.map((record) => ({
              type: "applyDomainCommand",
              payload: {
                commandType: "official.workforce.add_entity",
                payload: { entity: record },
              },
            })),
            ...preferenceIds.map((id, index) => ({
              type: "applyDomainCommand",
              payload: {
                commandType: "official.workforce.add_preference",
                payload: {
                  preference: {
                    id,
                    kind: "requestedTimeOff",
                    active: index === 0,
                    scope: { people: { kind: "all" } },
                    priority: "normal",
                    weight: 1,
                    availabilityIds: [referencedIds[index]],
                  },
                },
              },
            })),
          ],
        },
      },
    });
    const originals = new Map();
    for (const id of initialIds) originals.set(id, await entity(id));
    assert.deepEqual(await records(), initialIds);
    await verifyPreferences();
    await navigate(sessionId, "/projects");
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await activate(sessionId, `[aria-label=${JSON.stringify(`Open project ${title}`)}]`);
    await waitForElement(sessionId, "#setup-calendar");
    await openImporter();
    const beforeCancel = await revision(),
      beforeCancelHead = await appliedHistoryHead(sessionId, scenarioId);
    await activateButton(sessionId, "Choose a CSV file", root);
    await nativeChooser("Choose CSV snapshot");
    await waitForOutcome(sessionId, "Operation cancelled");
    assert.equal(await revision(), beforeCancel);
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), beforeCancelHead);
    await choose(sourcePath, true);
    for (const [kind, label, name] of [
      ["person", "Person", "Availability CSV person"],
      ["assignmentType", "Assignment type", "Availability CSV duty 1"],
      ["location", "Location", "Availability CSV site 1"],
    ]) {
      await selectValue(sessionId, "#availability-csv-mapping-endpoint", kind);
      await setValue(sessionId, "#availability-csv-token", "Shared");
      await choosePeopleReference(sessionId, `Reference for exact token · ${label}`, name, root);
      await activate(sessionId, "#availability-csv-add-alias");
    }
    await setValue(sessionId, "#availability-csv-inspect-record", "2");
    await activateButton(sessionId, "Inspect selected record", root);
    await waitFor(
      sessionId,
      "return document.querySelector('[aria-label=\"Selected native CSV record\"]')?.textContent.includes('2030-01-01T23:30:00');",
    );
    const blockedRevision = await revision(),
      blockedHead = await appliedHistoryHead(sessionId, scenarioId);
    await review();
    assert.match(await getText(sessionId, root), /unresolvedIdentity/u);
    assert.match(await getText(sessionId, root), /duplicateTarget/u);
    for (const [index, row] of rows.entries()) {
      if (initialIds.includes(row.id.toLowerCase())) continue;
      const selector = `#availability-csv-add-${index + 2}`;
      // Native mapped-cell failures are already blocked and deliberately offer no Add.
      if (await evaluate(sessionId, "return !!document.querySelector(arguments[0]);", [selector]))
        await activate(sessionId, selector);
    }
    await review();
    for (const [record, issue] of [
      [10, "duplicateTarget"],
      [16, "conflictingTimeWindow"],
      [17, "invalidLocalTime"],
      [18, "unresolvableLocalTime"],
      [19, "unresolvableLocalTime"],
      [20, "nonpositiveInterval"],
      [21, "missingRequiredField"],
      [22, "invalidAvailability"],
      [23, "invalidAvailability"],
    ])
      assert.match(
        await getText(sessionId, `[data-availability-csv-record="${record}"]`),
        new RegExp(issue, "u"),
      );
    for (const record of [11, 12, 13, 14, 15, 24, 25, 26]) {
      assert.match(
        await getText(sessionId, `[data-availability-csv-record="${record}"]`),
        /invalid(?:Cell|Reference|Availability)/u,
      );
    }
    assert.equal(
      await evaluate(sessionId, "return !!document.querySelector('#availability-csv-approve');"),
      false,
    );
    assert.equal(await revision(), blockedRevision, "Invalid preview must not mutate any row");
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), blockedHead);
    assert.deepEqual(await records(), initialIds);
    for (const id of initialIds) assert.deepEqual(await entity(id), originals.get(id));
    await verifyPreferences();
    for (const record of excludedRecords)
      await activate(sessionId, `#availability-csv-skip-${record}`);
    await review();
    const addedText = await getText(sessionId, '[data-availability-csv-record="2"]');
    for (const meaning of [
      addedIds[0],
      personId,
      typeIds[0],
      locationIds[0],
      "unavailable",
      "2030-01-01T23:30:00",
      "2030-01-02T04:30:00",
      "-18000",
      "2030-01-02T01:00:00",
      "2030-01-02T06:00:00",
      "2032-02-01",
    ])
      assert(addedText.includes(meaning), `Native instant review must expose ${meaning}`);
    for (const [record, kind] of [
      [3, "availableOnly"],
      [4, "approvedTimeOff"],
      [5, "requestedTimeOff"],
    ])
      assert(
        (await getText(sessionId, `[data-availability-csv-record="${record}"]`)).includes(kind),
      );
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelector('[data-availability-csv-record=\"3\"]').querySelector('[data-availability-csv-time]') === null;",
      ),
      true,
      "Weekly review must not invent resolved recurrence endpoints",
    );
    const concurrent = {
      ...originals.get(partialId),
      source: " concurrent source ",
      note: "concurrent <note> \t",
    };
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: await revision(),
      commandId: requestId(),
      actor: { actorId: null, displayName: "Availability CSV concurrent edit" },
      truncateRedo: false,
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: "official.workforce.update_entity",
          payload: { entity: concurrent },
        },
      },
    });
    await waitFor(
      sessionId,
      "return !document.querySelector('#availability-csv-review') && document.querySelector(arguments[0]).textContent.includes('previous approval is invalid');",
      [root],
    );
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelector('#availability-csv-endsAt-column').value;",
      ),
      "4",
    );
    assert.match(await getText(sessionId, root), /Shared/u);
    await review();
    const partialText = await getText(sessionId, '[data-availability-csv-record="6"]');
    for (const meaning of [
      "concurrent source",
      "concurrent <note>",
      "2030-01-01T11:30:00",
      "2030-01-01T16:30:00",
      "-18000",
    ])
      assert(partialText.includes(meaning), `Stale re-review must expose ${meaning}`);
    await screenshot(sessionId, "availability-csv-native-review.png");
    const beforeApply = await revision(),
      beforeApplyHead = await appliedHistoryHead(sessionId, scenarioId);
    await approve();
    await waitForOutcome(sessionId, "committed as one command batch");
    assert.equal(
      await revision(),
      beforeApply + 1,
      "Eight accepted changes must commit atomically in one revision",
    );
    const imported = new Map(originals);
    imported.set(partialId, {
      ...concurrent,
      timeWindow: { ...concurrent.timeWindow, endsAt: "2030-01-01T16:30:00Z" },
      effectiveRange: { ...concurrent.effectiveRange, endDateExclusive: "2032-03-01" },
    });
    imported.set(weeklyId, {
      ...originals.get(weeklyId),
      timeWindow: {
        kind: "instant",
        startsAt: "2030-01-02T14:00:00Z",
        endsAt: "2030-01-02T15:00:00Z",
      },
    });
    imported.set(instantId, {
      ...originals.get(instantId),
      timeWindow: { kind: "weekly", windows },
    });
    imported.set(convertibleId, {
      ...originals.get(convertibleId),
      availabilityKind: "approvedTimeOff",
    });
    for (const [index, id] of addedIds.entries())
      imported.set(id, {
        id,
        kind: "availability",
        personId,
        availabilityKind: ["unavailable", "availableOnly", "approvedTimeOff", "requestedTimeOff"][
          index
        ],
        timeWindow:
          index % 2 === 0
            ? { kind: "instant", startsAt: "2030-01-02T04:30:00Z", endsAt: "2030-01-02T06:00:00Z" }
            : { kind: "weekly", windows },
        effectiveRange: { startDate: "2029-12-01", endDateExclusive: "2032-02-01" },
        assignmentTypeIds: [typeIds[0]],
        locationIds: [locationIds[0]],
        source: " authored source ",
        note: " \t ",
      });
    const importedIds = [...imported.keys()].sort();
    assert.deepEqual(
      await records(),
      importedIds,
      "Only accepted rows may exist after atomic apply",
    );
    for (const [id, record] of imported)
      assert.deepEqual(
        await entity(id),
        record,
        `Exact native record ${id}, including raw unmapped siblings`,
      );
    await verifyPreferences();
    const ordinaryHead = await appliedHistoryHead(sessionId, scenarioId);
    assert.notEqual(ordinaryHead.id, beforeApplyHead.id);
    await activateButton(sessionId, "Read exclusion report", root);
    await activateButton(sessionId, "Save exclusion report", root);
    await nativeChooser("Save excluded availability rows", reportPath);
    await waitForOutcome(sessionId, "report was saved");
    assert.deepEqual(
      JSON.parse(await readFile(reportPath, "utf8")).rows,
      {
        family: "availability",
        excludedRows: excludedRecords.map((record) => ({ record, skipped: true, issue: null })),
      },
      "The saved report must contain every excluded native record, not only the displayed page",
    );
    await activateButton(sessionId, "Save exclusion report", root);
    await nativeChooser("Save excluded availability rows");
    await waitForOutcome(sessionId, "Operation cancelled");
    assert.equal(await revision(), beforeApply + 1);
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), ordinaryHead);
    assert(!(await getText(sessionId, root)).includes(sourcePath));
    assert(!(await getText(sessionId, root)).includes(reportPath));
    await screenshot(sessionId, "availability-csv-native-report.png");
    await history();
    await activateButton(sessionId, "Undo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
    assert.deepEqual(await records(), initialIds);
    for (const id of initialIds)
      assert.deepEqual(await entity(id), id === partialId ? concurrent : originals.get(id));
    await verifyPreferences();
    await activateButton(sessionId, "Redo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario redo committed", 120_000);
    assert.deepEqual(await records(), importedIds);
    for (const [id, record] of imported) assert.deepEqual(await entity(id), record);
    await openImporter();
    await writeFile(
      noOpPath,
      csv([
        {
          id: partialId.toUpperCase(),
          personId,
          availabilityKind: "unavailable",
          startsAt: "2030-01-01T09:00:00.12345678",
          endsAt: "2030-01-01T11:30",
          effectiveStartDate: "2029-12-01",
          effectiveEndDateExclusive: "2032-03-01",
          assignmentTypeIds: JSON.stringify(typeIds),
          locationIds: JSON.stringify(locationIds),
        },
        {
          id: instantId.toUpperCase(),
          weeklyWindows: JSON.stringify(
            windows.map((window) => ({
              ...window,
              weekdays: [...window.weekdays].reverse(),
            })),
          ),
          assignmentTypeIds: JSON.stringify(typeIds),
          locationIds: JSON.stringify(locationIds),
        },
      ]),
    );
    const beforeNoOp = await revision(),
      beforeNoOpHead = await appliedHistoryHead(sessionId, scenarioId);
    await choose(noOpPath, true);
    await review();
    for (const record of [2, 3])
      assert.match(
        await getText(sessionId, `[data-availability-csv-record="${record}"]`),
        /unchanged/u,
      );
    const equivalentText = await getText(sessionId, '[data-availability-csv-record="2"]');
    for (const meaning of [
      "2030-01-01T09:00:00.12345678",
      "2030-01-01T14:00:00.12345678",
      "-18000",
    ])
      assert(equivalentText.includes(meaning), `No-op retains native endpoint fact ${meaning}`);
    await approve();
    await waitForOutcome(sessionId, "made no changes and created no history entry");
    assert.equal(await revision(), beforeNoOp);
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), beforeNoOpHead);
    for (const [id, record] of imported)
      assert.deepEqual(
        await entity(id),
        record,
        "Equivalent reference sets, instants and weekday membership must preserve raw spelling and authored window order/multiplicity",
      );
    await choose(clearPath);
    for (const field of ["assignmentTypeIds", "locationIds", "source", "note"])
      await selectValue(sessionId, `#availability-csv-${field}-blank`, "clear");
    await review();
    await evaluate(
      sessionId,
      `const callbacks = window.__TAURI_INTERNALS__.callbacks;
      const descriptor = Object.getOwnPropertyDescriptor(callbacks, 'get'), original = callbacks.get;
      const gate = { commandId: null };
      gate.restore = () => descriptor ? Object.defineProperty(callbacks, 'get', descriptor) : Reflect.deleteProperty(callbacks, 'get');
      window.__availabilityCsvGate = gate;
      Object.defineProperty(callbacks, 'get', { configurable: true, value: function(id) {
        const callback = original.call(this, id); if (!callback) return callback;
        return data => { if (gate.commandId === null && data?.result?.outcome?.kind === 'applied') {
          gate.commandId = data.result.outcome.commandId; callback({ ...data, result: null }); return;
        } callback(data); };
      }});`,
    );
    const beforeLost = await revision();
    await approve();
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0]).textContent.includes('apply outcome is unknown');",
      [root],
      60_000,
    );
    const committedId = await evaluate(
      sessionId,
      "window.__availabilityCsvGate.restore(); return window.__availabilityCsvGate.commandId;",
    );
    assert.equal(
      typeof committedId,
      "string",
      "Recovery must observe the actual native committed command ID",
    );
    assert.equal(await revision(), beforeLost + 1);
    const cleared = { ...imported.get(partialId), source: "", note: "" };
    delete cleared.assignmentTypeIds;
    delete cleared.locationIds;
    assert.deepEqual(
      await entity(partialId),
      cleared,
      "Missing receipt is not rollback; Clear omits optional restrictions",
    );
    assert.equal(
      await evaluate(sessionId, "return !!document.querySelector('#availability-csv-approve');"),
      false,
    );
    await activateButton(sessionId, "Check saved history");
    await waitFor(
      sessionId,
      "return document.querySelector('#mutation-recovery-title')?.textContent.trim() === 'Saved command confirmed';",
    );
    assert.equal((await appliedHistoryHead(sessionId, scenarioId)).id, committedId);
    await activateButton(sessionId, "Acknowledge and dismiss");
    await history();
    await activateButton(sessionId, "Undo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
    for (const [id, record] of imported) assert.deepEqual(await entity(id), record);
    await activateButton(sessionId, "Redo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario redo committed", 120_000);
    assert.deepEqual(await records(), importedIds);
    for (const [id, record] of imported)
      assert.deepEqual(await entity(id), id === partialId ? cleared : record);
    await verifyPreferences();
    console.log(
      "PASS: packaged availability CSV native pickers, all four kinds, local/weekly transitions, same-ID Add, scoped alias collisions, raw partial preservation, duplicate/filter/window/DST skips, active/inactive incoming preference rejection, stale re-review, atomic apply/full report/cancellation, semantic no-op nohistory, actual lost-receipt reconciliation and exact Undo/Redo",
    );
  } catch (error) {
    failed = true;
    // Capture before fixture removal so cleanup cannot replace the useful failing view.
    await screenshot(sessionId, "availability-csv-native-failure.png").catch(() => undefined);
    const evidence = await evaluate(
      sessionId,
      "return {hash: location.hash, text: document.querySelector('main')?.innerText, active: document.activeElement?.outerHTML};",
    ).catch(() => null);
    await writeFile(
      new URL("../../../.cache/e2e/availability-csv-native-failure.json", import.meta.url),
      JSON.stringify({ error: String(error), evidence }, null, 2),
    ).catch(() => undefined);
    throw error;
  } finally {
    await evaluate(
      sessionId,
      "window.__availabilityCsvGate?.restore?.(); delete window.__availabilityCsvGate;",
    ).catch(() => undefined);
    await navigate(sessionId, "/projects").catch(() => undefined);
    await package10DeleteFixture(sessionId, scenarioId).catch((error) => {
      if (!failed) throw error;
      console.error("Availability CSV fixture cleanup failed:", error);
    });
  }
}

async function packagedEligibilityCsvAcceptance(sessionId, directory) {
  const root = "[data-eligibility-csv-import]";
  const title = `Phase07 eligibility CSV ${Date.now()}`;
  const created = await nativeRequest(sessionId, "project_create", {
    schemaVersion: 1,
    title,
    description: "Sparse native eligibility import",
    domainPack: { id: "official.workforce", schemaVersion: 1 },
    settings: {
      timeZone: "UTC",
      locale: "en-US",
      units: "metric",
      firstDate: "2030-01-01",
      lastDate: "2030-01-01",
      gapPolicy: "reject",
      overlapPolicy: "reject",
    },
  });
  const scenarioId = created.result.scenarioId;
  const personIds = [requestId(), requestId()],
    typeIds = [requestId(), requestId()];
  const sourcePath = join(directory, "eligibility-reviewed.csv");
  const noOpPath = join(directory, "eligibility-no-change.csv");
  const removalPath = join(directory, "eligibility-removal.csv");
  const reportPath = join(directory, "eligibility-exclusions.json");
  await writeFile(
    sourcePath,
    `person,type,eligible\nMapped person,Mapped duty,false\nMapped person,Mapped duty,false\n${personIds[0].toUpperCase()},${typeIds[1].toUpperCase()},true\n${personIds[1]},${typeIds[1]},true\nunknown,unknown,true\n${personIds[1]},${typeIds[0]},TRUE\n`,
  );
  await writeFile(noOpPath, `person,type,eligible\n${personIds[0]},${typeIds[1]},true\n`);
  await writeFile(removalPath, `person,type,eligible\n${personIds[0]},${typeIds[1]},false\n`);
  async function person(id) {
    return (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_get_entity",
        "eutheto.setup.entity_detail",
        { kind: "person", entityId: id },
      )
    ).result.view.data.result.data;
  }
  async function choose(path, first = false) {
    await activateButton(
      sessionId,
      first ? "Choose a CSV file" : "Discard this source and choose another",
      root,
    );
    await nativeChooser("Choose CSV snapshot", path);
    await idle(sessionId);
    await selectValue(sessionId, "#eligibility-csv-dialect", "comma");
    await selectValue(sessionId, "#eligibility-csv-header", "yes");
    await selectValue(sessionId, "#eligibility-csv-person-column", "0");
    await selectValue(sessionId, "#eligibility-csv-assignment-type-column", "1");
    await selectValue(sessionId, "#eligibility-csv-eligible-column", "2");
  }
  async function review() {
    await activateButton(sessionId, "Review import changes", root);
    await waitForElement(sessionId, "#eligibility-csv-review");
  }
  async function approve() {
    await activate(sessionId, "#eligibility-csv-approve");
    await activateButton(sessionId, "Apply exactly this reviewed import", root);
  }
  try {
    const entities = [
      ...typeIds.map((id, index) => ({
        id,
        kind: "assignmentType",
        name: `Eligibility duty ${index + 1}`,
        category: "clinic",
        timeBehavior: "elapsed",
        defaultDurationMinutes: 60,
        qualifications: { kind: "unconstrained" },
        locationBehavior: { kind: "optional" },
        workloadBucketIds: [],
      })),
      ...personIds.map((id, index) => ({
        id,
        kind: "person",
        name: `Eligibility person ${index + 1}`,
        externalId: `eligibility-${index + 1}`,
        activeRange: { kind: "always" },
        qualificationGrants: [],
        eligibleAssignmentTypeIds: [typeIds[index].toUpperCase()],
        teamIds: [],
        tags: ["retained"],
        workloadWeight: { numerator: 3, denominator: 2 },
        display: { color: "#102030", avatarInitials: "EP" },
      })),
    ];
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: created.result.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Eligibility CSV fixture" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: {
          label: "Eligibility endpoint fixture",
          commands: entities.map((entity) => ({
            type: "applyDomainCommand",
            payload: {
              commandType: "official.workforce.add_entity",
              payload: { entity },
            },
          })),
        },
      },
    });
    const original = await person(personIds[0]),
      untouched = await person(personIds[1]);
    await navigate(sessionId, "/projects");
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await activate(sessionId, `[aria-label=${JSON.stringify(`Open project ${title}`)}]`);
    await waitForElement(sessionId, "#setup-calendar");
    await navigate(sessionId, `/project/${scenarioId}/eligibility`);
    await activate(
      sessionId,
      `a[href=${JSON.stringify(`#/project/${scenarioId}/eligibility/import`)}]`,
    );
    await waitForElement(sessionId, "#eligibility-csv-heading");
    await choose(sourcePath, true);
    await setValue(sessionId, "#eligibility-csv-token", "Mapped person");
    await choosePeopleReference(sessionId, "Person for token", "Eligibility person 1", root);
    await activateButton(sessionId, "Add token mapping", root);
    await selectValue(sessionId, "#eligibility-csv-mapping-endpoint", "assignmentType");
    await setValue(sessionId, "#eligibility-csv-token", "Mapped duty");
    await choosePeopleReference(sessionId, "Assignment type for token", "Eligibility duty 1", root);
    await activateButton(sessionId, "Add token mapping", root);
    await setValue(sessionId, "#eligibility-csv-inspect-record", "2");
    await activateButton(sessionId, "Inspect selected record", root);
    await waitFor(
      sessionId,
      "return document.querySelector('[aria-label=\"Selected native CSV record\"]')?.textContent.includes('Mapped person');",
    );
    await review();
    assert.match(await getText(sessionId, root), /duplicatePair/u);
    assert.match(await getText(sessionId, root), /invalidMembership/u);
    for (const record of [3, 6, 7])
      await activateCheckbox(sessionId, `Skip record ${record}`, root);
    await review();
    assert.match(
      await getText(sessionId, root),
      /1 changed people in one ordinary atomic command batch/u,
    );
    const reviewedRevision = (await projects(sessionId)).find(
      (item) => item.scenarioId === scenarioId,
    ).revision;
    const concurrent = { ...original, name: "Eligibility person 1 revised independently" };
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: reviewedRevision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Eligibility concurrent edit" },
      truncateRedo: false,
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: "official.workforce.update_entity",
          payload: { entity: concurrent },
        },
      },
    });
    await waitFor(
      sessionId,
      "return !document.querySelector('#eligibility-csv-review') && document.querySelector(arguments[0]).textContent.includes('previous approval is invalid');",
      [root],
    );
    assert.match(await getText(sessionId, root), /Mapped duty/u);
    assert.equal(
      await evaluate(
        sessionId,
        "return [...document.querySelectorAll(arguments[0] + ' input[type=\"checkbox\"]')].filter(input => input.checked).length;",
        [root],
      ),
      3,
    );
    await review();
    await screenshot(sessionId, "eligibility-csv-native-review.png");
    await approve();
    await waitForOutcome(sessionId, "committed as one command batch");
    const imported = { ...concurrent, eligibleAssignmentTypeIds: [typeIds[1]] };
    assert.deepEqual(await person(personIds[0]), imported);
    assert.deepEqual(await person(personIds[1]), untouched);
    await activateButton(sessionId, "Read exclusion report", root);
    await activateButton(sessionId, "Save exclusion report", root);
    await nativeChooser("Save excluded eligibility rows", reportPath);
    await waitForOutcome(sessionId, "report was saved");
    assert.deepEqual(JSON.parse(await readFile(reportPath, "utf8")).rows, {
      family: "eligibility",
      excludedRows: [
        { record: 3, skipped: true, issue: "duplicatePair" },
        { record: 6, skipped: true, issue: "unresolvedPerson" },
        { record: 7, skipped: true, issue: "invalidMembership" },
      ],
    });
    await activateButton(sessionId, "Save exclusion report", root);
    await nativeChooser("Save excluded eligibility rows");
    await waitForOutcome(sessionId, "Operation cancelled");
    assert(!(await getText(sessionId, root)).includes(sourcePath));
    await screenshot(sessionId, "eligibility-csv-native-report.png");
    const beforeNoOp = await appliedHistoryHead(sessionId, scenarioId);
    await choose(noOpPath);
    await review();
    await approve();
    await waitForOutcome(sessionId, "made no changes and created no history entry");
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), beforeNoOp);
    assert.deepEqual(await person(personIds[0]), imported);
    await choose(removalPath);
    await review();
    await evaluate(
      sessionId,
      `const callbacks = window.__TAURI_INTERNALS__.callbacks;
      const descriptor = Object.getOwnPropertyDescriptor(callbacks, 'get'), original = callbacks.get;
      const gate = { commandId: null };
      gate.restore = () => descriptor ? Object.defineProperty(callbacks, 'get', descriptor) : Reflect.deleteProperty(callbacks, 'get');
      window.__eligibilityCsvGate = gate;
      Object.defineProperty(callbacks, 'get', { configurable: true, value: function(id) {
        const callback = original.call(this, id); if (!callback) return callback;
        return data => { if (gate.commandId === null && data?.result?.outcome?.kind === 'applied') {
          gate.commandId = data.result.outcome.commandId; callback({ ...data, result: null }); return;
        } callback(data); };
      }});`,
    );
    await approve();
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0]).textContent.includes('apply outcome is unknown');",
      [root],
      60_000,
    );
    const committedId = await evaluate(
      sessionId,
      "window.__eligibilityCsvGate.restore(); return window.__eligibilityCsvGate.commandId;",
    );
    const removed = { ...imported, eligibleAssignmentTypeIds: [] };
    assert.deepEqual(await person(personIds[0]), removed);
    await activateButton(sessionId, "Check saved history");
    await waitFor(
      sessionId,
      "return document.querySelector('#mutation-recovery-title')?.textContent.trim() === 'Saved command confirmed';",
    );
    assert.equal((await appliedHistoryHead(sessionId, scenarioId))?.id, committedId);
    await activateButton(sessionId, "Acknowledge and dismiss");
    await activate(sessionId, `a[href=${JSON.stringify(`#/project/${scenarioId}/history`)}]`);
    await activateButton(sessionId, "Discard and leave", '[role="dialog"]');
    const historyRoot = '[aria-labelledby="history-heading"]';
    await activateButton(sessionId, "Undo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
    assert.deepEqual(await person(personIds[0]), imported);
    await activateButton(sessionId, "Redo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario redo committed", 120_000);
    assert.deepEqual(await person(personIds[0]), removed);
    console.log(
      "PASS: packaged eligibility CSV native picker, exact mapping/sample, duplicate/boolean skips, stale re-review, sparse grouped preservation, complete saved report/cancellation, no-op history, lost-receipt reconciliation and Undo/Redo",
    );
  } finally {
    await evaluate(
      sessionId,
      "window.__eligibilityCsvGate?.restore?.(); delete window.__eligibilityCsvGate;",
    ).catch(() => undefined);
    await navigate(sessionId, "/projects").catch(() => undefined);
    await package10DeleteFixture(sessionId, scenarioId);
  }
}

async function packagedEligibilityMatrixAcceptance(sessionId, directory) {
  const root = "[data-eligibility-matrix-import]";
  const historyRoot = '[aria-labelledby="history-heading"]';
  const title = `Phase07 eligibility matrix ${Date.now()}`;
  const created = await nativeRequest(sessionId, "project_create", {
    schemaVersion: 1,
    title,
    description: "Real pasted-text native eligibility acceptance",
    domainPack: { id: "official.workforce", schemaVersion: 1 },
    settings: {
      timeZone: "UTC",
      locale: "en-US",
      units: "metric",
      firstDate: "2030-01-01",
      lastDate: "2030-01-01",
      gapPolicy: "reject",
      overlapPolicy: "reject",
    },
  });
  const scenarioId = created.result.scenarioId;
  const personIds = [requestId(), requestId(), requestId()];
  const typeIds = [requestId(), requestId(), requestId(), requestId(), requestId()];
  const reportPath = join(directory, "eligibility-matrix-exclusions.json");
  // Reverse the newly enabled type order, and include a blank duplicate: native
  // typed-pair authority must not pick a winner or sort the appended memberships.
  const text =
    [
      [
        "personId",
        "Shared",
        typeIds[1].toUpperCase(),
        typeIds[4].toUpperCase(),
        typeIds[3],
        typeIds[3].toUpperCase(),
      ],
      ["Shared", "false", "", "true", "true", ""],
      [personIds[1].toUpperCase(), "true", "", "TRUE", "true", "false"],
      ["unknown", "", "true", "false", "", "true"],
    ]
      .map((row) => row.join("\t"))
      .join("\n") + "\n";
  const skips = [
    { record: 2, column: 5, issue: "duplicatePair" },
    { record: 3, column: 3, issue: "invalidMembership" },
    { record: 3, column: 5, issue: "duplicatePair" },
    ...[1, 2, 3, 4, 5].map((column) => ({ record: 4, column, issue: "unresolvedPerson" })),
  ];
  const cell = (record, column) =>
    `[data-matrix-record="${record}"][data-matrix-column="${column}"]`;
  // EntityDetail returns typed UUIDs in canonical spelling. Raw spelling is
  // covered by the domain/Core/native snapshot regressions, not this projection.
  async function person(id) {
    return (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_get_entity",
        "eutheto.setup.entity_detail",
        { kind: "person", entityId: id },
      )
    ).result.view.data.result.data;
  }
  async function revision() {
    return (await projects(sessionId)).find((item) => item.scenarioId === scenarioId).revision;
  }
  async function verifyPeople(expected) {
    for (const [index, id] of personIds.entries())
      assert.deepEqual(await person(id), expected[index], `Exact native Person ${id}`);
  }
  async function openImporter() {
    await navigate(sessionId, `/project/${scenarioId}/eligibility`);
    // Wait for the real matrix read; nested entity-picker idle hints are not work.
    await waitFor(
      sessionId,
      "const view = document.querySelector('[aria-labelledby=\"eligibility-setup-heading\"]'); return view?.querySelector('#eligibility-people-search:not(:disabled)') && !view.querySelector(':scope > [role=\"status\"], [role=\"alert\"]');",
    );
    await idle(sessionId);
    await navigate(sessionId, `/project/${scenarioId}/eligibility/paste`);
    await waitForElement(sessionId, "#eligibility-matrix-heading");
  }
  async function replace() {
    await activate(sessionId, "#eligibility-matrix-replace");
    await waitForElement(sessionId, "#eligibility-matrix-text:not([readonly]):not(:disabled)");
    await waitForElement(sessionId, "#eligibility-matrix-capture:not(:disabled)");
  }
  async function paste(value) {
    await activate(sessionId, "#eligibility-matrix-text:not([readonly]):not(:disabled)");
    // WebDriver sendKeys treats tabs as focus traversal. Deliver a whole paste
    // through the actual textarea input handler; never inject native custody.
    await evaluate(
      sessionId,
      `const input = document.querySelector('#eligibility-matrix-text');
      input.value = arguments[0];
      input.dispatchEvent(new InputEvent('input', {bubbles:true, inputType:'insertFromPaste', data:arguments[0]}));`,
      [value],
    );
    assert.equal(
      await evaluate(sessionId, "return document.querySelector('#eligibility-matrix-text').value;"),
      value,
    );
  }
  async function capture(value) {
    if (
      await evaluate(sessionId, "return !!document.querySelector('#eligibility-matrix-replace');")
    )
      await replace();
    await paste(value);
    await activate(sessionId, "#eligibility-matrix-capture");
    await waitForElement(sessionId, "#eligibility-matrix-census");
    await idle(sessionId);
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelector('#eligibility-matrix-text').readOnly;",
      ),
      true,
      "Captured raw intent must be read-only until native custody is released",
    );
  }
  async function review() {
    await activate(sessionId, "#eligibility-matrix-preview");
    await waitForElement(sessionId, "#eligibility-matrix-review");
    await idle(sessionId);
  }
  async function approve() {
    await activate(sessionId, "#eligibility-matrix-approve");
    await activate(sessionId, "#eligibility-matrix-apply");
  }
  async function history(commandId) {
    if (!(await evaluate(sessionId, "return document.querySelector('.project-nav-more')?.open;")))
      await activate(sessionId, ".project-nav-more > summary");
    await activate(sessionId, `a[href=${JSON.stringify(`#/project/${scenarioId}/history`)}]`);
    await activateButton(sessionId, "Discard and leave", '[role="dialog"]');
    await waitForElement(sessionId, "#history-heading");
    await waitFor(
      sessionId,
      "const status = document.querySelector(arguments[0] + ' [role=\"status\"]'); return status?.getAttribute('aria-busy') === 'false' && status.textContent.includes('History captured at revision');",
      [historyRoot],
    );
    await idle(sessionId);
    const entry = `${historyRoot} ol[aria-label="Recorded changes"] > li:first-child`;
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0] + ' dd')?.textContent.trim() === arguments[1];",
      [entry, commandId],
    );
    await activate(sessionId, `${entry} details > summary`);
    assert((await getText(sessionId, `${entry} details`)).includes(commandId));
  }
  let failed = false;
  try {
    const entities = [
      ...typeIds.map((id, index) => ({
        id,
        kind: "assignmentType",
        name: `Matrix duty ${index + 1}`,
        category: "clinic",
        timeBehavior: "elapsed",
        defaultDurationMinutes: 60,
        qualifications: { kind: "unconstrained" },
        locationBehavior: { kind: "optional" },
        workloadBucketIds: [],
      })),
      ...personIds.map((id, index) => ({
        id,
        kind: "person",
        name: `Matrix person ${index + 1}`,
        externalId: `matrix-${index + 1}`,
        activeRange: { kind: "always" },
        qualificationGrants: [],
        eligibleAssignmentTypeIds: [
          [typeIds[0], typeIds[1]],
          [typeIds[1], typeIds[2]],
          [typeIds[2], typeIds[1]],
        ][index].map((value) => value.toUpperCase()),
        teamIds: [],
        tags: ["raw retained", "second tag"],
        workloadWeight: { numerator: 3, denominator: 2 },
        display: { color: "#102030", avatarInitials: "MP" },
      })),
    ];
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: created.result.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Eligibility matrix fixture" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: {
          label: "Eligibility matrix synthetic endpoints",
          commands: entities.map((entity) => ({
            type: "applyDomainCommand",
            payload: { commandType: "official.workforce.add_entity", payload: { entity } },
          })),
        },
      },
    });
    const originals = await Promise.all(personIds.map(person));
    await navigate(sessionId, "/projects");
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await activate(sessionId, `[aria-label=${JSON.stringify(`Open project ${title}`)}]`);
    await waitForElement(sessionId, "#setup-calendar");
    await openImporter();
    const beforeRefusal = await revision(),
      refusalHead = await appliedHistoryHead(sessionId, scenarioId);
    // Construct overflow in the real DOM so the WebDriver payload stays small.
    // This crosses UTF-8 bytes while staying below the code-unit cap, with a tail sentinel.
    await evaluate(
      sessionId,
      `const input = document.querySelector('#eligibility-matrix-text');
      input.value = '😀'.repeat(262144) + 'TAIL';
      input.dispatchEvent(new Event('input', {bubbles:true}));`,
    );
    await waitForElement(sessionId, "#eligibility-matrix-error");
    assert.match(
      await getText(sessionId, root),
      /All pasted text was cleared; no truncated prefix was accepted/u,
    );
    assert.equal(
      await evaluate(sessionId, "return document.querySelector('#eligibility-matrix-text').value;"),
      "",
    );
    await activate(sessionId, "#eligibility-matrix-capture");
    assert.match(await getText(sessionId, root), /Paste a complete matrix before capturing/u);
    assert.equal(
      await evaluate(sessionId, "return !!document.querySelector('#eligibility-matrix-replace');"),
      false,
    );
    await paste(`personId\t${typeIds[0]}\t${typeIds[1]}\n${personIds[0]}\ttrue\n`);
    await activate(sessionId, "#eligibility-matrix-capture");
    await waitForElement(sessionId, "#eligibility-matrix-error");
    await idle(sessionId);
    assert.match(await getText(sessionId, root), /matching data-row widths/u);
    assert.equal(
      await evaluate(
        sessionId,
        "return !!document.querySelector('#eligibility-matrix-census, #eligibility-matrix-preview, #eligibility-matrix-approve');",
      ),
      false,
    );
    await screenshot(sessionId, "eligibility-matrix-native-refusal.png");
    await replace();
    // A complete rectangle with 10,002 blank membership cells is refused natively,
    // without sending thousands of key actions or oversized execute arguments.
    await evaluate(
      sessionId,
      `const input = document.querySelector('#eligibility-matrix-text');
      input.value = 'personId\\t' + arguments[0] + '\\t' + arguments[1] + '\\n' + 'Shared\\t\\t\\n'.repeat(5001);
      input.dispatchEvent(new Event('input', {bubbles:true}));`,
      [typeIds[0], typeIds[1]],
    );
    await activate(sessionId, "#eligibility-matrix-capture");
    await waitForElement(sessionId, "#eligibility-matrix-error");
    await idle(sessionId);
    assert.match(await getText(sessionId, root), /complete membership-cell limit/u);
    assert.equal(
      await evaluate(
        sessionId,
        "return !!document.querySelector('#eligibility-matrix-census, #eligibility-matrix-preview, #eligibility-matrix-approve');",
      ),
      false,
    );
    assert.equal(await revision(), beforeRefusal);
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), refusalHead);
    await verifyPeople(originals);
    await capture(text);
    assert.deepEqual(
      (await getText(sessionId, "#eligibility-matrix-census")).match(/\d+/gu)?.map(Number),
      [6, 4, 15],
    );
    await review();
    assert.match(await getText(sessionId, cell(2, 1)), /unresolvedPerson/u);
    assert.equal(
      await evaluate(sessionId, "return !!document.querySelector('#eligibility-matrix-approve');"),
      false,
    );
    for (const [kind, label, name] of [
      ["person", "Person for token", "Matrix person 1"],
      ["assignmentType", "Assignment type for token", "Matrix duty 1"],
    ]) {
      await selectValue(sessionId, "#eligibility-csv-mapping-endpoint", kind);
      await setValue(sessionId, "#eligibility-csv-token", "Shared");
      await choosePeopleReference(sessionId, label, name, root);
      await activateButton(sessionId, "Add token mapping", root);
    }
    await setValue(sessionId, "#eligibility-matrix-inspect-record", "3");
    await activate(sessionId, "#eligibility-matrix-inspect");
    await waitFor(
      sessionId,
      "return document.querySelector('[aria-label=\"Selected native matrix record\"]')?.textContent.includes(arguments[0]);",
      [personIds[1].toUpperCase()],
    );
    assert.match(await getText(sessionId, '[aria-label="Selected native matrix record"]'), /TRUE/u);
    await review();
    for (const record of [2, 3])
      for (const column of [4, 5])
        assert.match(await getText(sessionId, cell(record, column)), /duplicatePair/u);
    assert.match(await getText(sessionId, cell(3, 3)), /invalidMembership/u);
    assert.equal(
      await evaluate(sessionId, "return !!document.querySelector('#eligibility-matrix-approve');"),
      false,
    );
    assert.equal(await revision(), beforeRefusal);
    await verifyPeople(originals);
    for (const { record, column } of skips)
      await activate(sessionId, `#eligibility-matrix-skip-${record}-${column}`);
    await review();
    assert.match(
      await getText(sessionId, root),
      /2 changed people in one ordinary atomic command batch/u,
    );
    assert.deepEqual(
      await evaluate(
        sessionId,
        "return [...document.querySelector(arguments[0]).cells].slice(0, 7).map(cell => cell.textContent.trim());",
        [cell(2, 2)],
      ),
      ["2", "3", "unchanged", personIds[0], typeIds[1], "true", "true"],
      "Blank preserves membership; native column two is displayed as column three",
    );
    const fingerprint = await evaluate(
      sessionId,
      "return document.querySelector('[data-eligibility-matrix-review] p.break-all code').textContent;",
    );
    const concurrent = {
      ...originals[0],
      name: "Matrix person 1 revised independently",
      tags: ["concurrent raw tag", "second tag"],
    };
    await activate(sessionId, "#eligibility-matrix-approve");
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: await revision(),
      commandId: requestId(),
      actor: { actorId: null, displayName: "Eligibility matrix concurrent edit" },
      truncateRedo: false,
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: "official.workforce.update_entity",
          payload: { entity: concurrent },
        },
      },
    });
    await waitFor(
      sessionId,
      "return !document.querySelector('#eligibility-matrix-review') && document.querySelector(arguments[0]).textContent.includes('previous approval is invalid');",
      [root],
    );
    assert.equal(
      await evaluate(sessionId, "return document.querySelector('#eligibility-matrix-text').value;"),
      text,
    );
    for (const { record, column } of skips)
      assert.equal(
        await evaluate(sessionId, "return document.querySelector(arguments[0]).checked;", [
          `#eligibility-matrix-skip-${record}-${column}`,
        ]),
        true,
      );
    assert.match(await getText(sessionId, "#eligibility-token-mappings"), /Exact token mappings/u);
    assert.match(await getText(sessionId, root), /Shared/u);
    await review();
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelector('[data-eligibility-matrix-review] p.break-all code').textContent;",
      ),
      fingerprint,
    );
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelector('#eligibility-matrix-approve').checked;",
      ),
      false,
    );
    await screenshot(sessionId, "eligibility-matrix-native-review.png");
    const approvedId = await evaluate(
      sessionId,
      "return [...document.querySelectorAll('[data-eligibility-matrix-review] p')].find(p => p.textContent.startsWith('Command identity:')).querySelector('code').textContent;",
    );
    const beforeApply = await revision();
    await approve();
    await waitForOutcome(sessionId, "committed as one command batch");
    assert.equal(
      await revision(),
      beforeApply + 1,
      "Two grouped Person updates commit in one revision",
    );
    const imported = [
      {
        ...concurrent,
        eligibleAssignmentTypeIds: [typeIds[1], typeIds[4], typeIds[3]],
      },
      {
        ...originals[1],
        eligibleAssignmentTypeIds: [
          ...originals[1].eligibleAssignmentTypeIds,
          typeIds[0],
          typeIds[3],
        ],
      },
      originals[2],
    ];
    await verifyPeople(imported);
    const ordinaryHead = await appliedHistoryHead(sessionId, scenarioId);
    assert.equal(ordinaryHead.id, approvedId);
    assert.equal(ordinaryHead.revisionBefore, beforeApply);
    assert.equal(ordinaryHead.revisionAfter, beforeApply + 1);
    await activate(sessionId, "#eligibility-matrix-read-report");
    await waitForElement(sessionId, "#eligibility-matrix-excluded");
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelectorAll('#eligibility-matrix-excluded tbody tr').length;",
      ),
      skips.length,
    );
    await activate(sessionId, "#eligibility-matrix-save-report");
    await nativeChooser("Save excluded eligibility matrix cells", reportPath);
    await waitForOutcome(sessionId, "report was saved");
    const savedReport = JSON.parse(await readFile(reportPath, "utf8"));
    assert.equal(savedReport.consumed, true);
    assert.deepEqual(
      savedReport.rows,
      {
        family: "eligibilityMatrix",
        excludedCells: skips.map(({ record, column, issue }) => ({
          cell: { record, column },
          skipped: true,
          issue,
        })),
      },
      "Native save must retain every excluded cell, including original duplicate/invalid reasons",
    );
    assert(
      !JSON.stringify(savedReport).includes("Shared"),
      "Exclusion report must not contain raw pasted tokens",
    );
    await activate(sessionId, "#eligibility-matrix-save-report");
    await nativeChooser("Save excluded eligibility matrix cells");
    await waitForOutcome(sessionId, "Operation cancelled");
    assert.equal(await revision(), beforeApply + 1);
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), ordinaryHead);
    assert(!(await getText(sessionId, root)).includes(reportPath));
    await screenshot(sessionId, "eligibility-matrix-native-report.png");
    await history(ordinaryHead.id);
    await activateButton(sessionId, "Undo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
    await verifyPeople([concurrent, originals[1], originals[2]]);
    await activateButton(sessionId, "Redo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario redo committed", 120_000);
    await verifyPeople(imported);
    await openImporter();
    await capture(`personId\t${typeIds[4].toUpperCase()}\n${personIds[0].toUpperCase()}\ttrue\n`);
    await review();
    assert.match(await getText(sessionId, cell(2, 1)), /unchanged/u);
    const beforeNoOp = await revision(),
      noOpHead = await appliedHistoryHead(sessionId, scenarioId);
    await approve();
    await waitForOutcome(sessionId, "made no changes and created no history entry");
    assert.equal(await revision(), beforeNoOp);
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), noOpHead);
    await verifyPeople(imported);
    assert.match(
      await getText(sessionId, root),
      /source has been consumed, including a no-change apply/u,
    );
    assert.equal(
      await evaluate(sessionId, "return document.querySelector('#eligibility-matrix-text').value;"),
      "",
    );
    assert.equal(
      await evaluate(
        sessionId,
        "return !!document.querySelector('#eligibility-matrix-preview, #eligibility-matrix-approve');",
      ),
      false,
    );
    await activate(sessionId, "#eligibility-matrix-read-report");
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0]).textContent.includes('No excluded cells.');",
      [root],
    );
    await screenshot(sessionId, "eligibility-matrix-native-noop.png");
    await capture(`personId\t${typeIds[4]}\n${personIds[0]}\tfalse\n`);
    await review();
    // Corrupt only the actual native commit receipt, never the mutation or authority.
    await evaluate(
      sessionId,
      `const callbacks = window.__TAURI_INTERNALS__.callbacks;
      const descriptor = Object.getOwnPropertyDescriptor(callbacks, 'get'), original = callbacks.get;
      const gate = { commandId: null };
      gate.restore = () => descriptor ? Object.defineProperty(callbacks, 'get', descriptor) : Reflect.deleteProperty(callbacks, 'get');
      window.__eligibilityMatrixGate = gate;
      Object.defineProperty(callbacks, 'get', { configurable: true, value: function(id) {
        const callback = original.call(this, id); if (!callback) return callback;
        return data => { if (gate.commandId === null && data?.result?.outcome?.kind === 'applied') {
          gate.commandId = data.result.outcome.commandId; callback({ ...data, result: null }); return;
        } callback(data); };
      }});`,
    );
    const beforeLost = await revision();
    await approve();
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0]).textContent.includes('apply outcome is unknown');",
      [root],
      60_000,
    );
    const committedId = await evaluate(
      sessionId,
      "window.__eligibilityMatrixGate.restore(); return window.__eligibilityMatrixGate.commandId;",
    );
    assert.equal(typeof committedId, "string", "Observe the actual native committed command ID");
    const removed = [
      { ...imported[0], eligibleAssignmentTypeIds: [typeIds[1], typeIds[3]] },
      imported[1],
      imported[2],
    ];
    assert.equal(await revision(), beforeLost + 1);
    await verifyPeople(removed);
    assert.equal(
      await evaluate(sessionId, "return !!document.querySelector('#eligibility-matrix-approve');"),
      false,
    );
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelector('#eligibility-matrix-replace').disabled && document.querySelector('#eligibility-matrix-text').disabled;",
      ),
      true,
    );
    await screenshot(sessionId, "eligibility-matrix-native-unknown.png");
    await activateButton(sessionId, "Check saved history");
    await waitFor(
      sessionId,
      "return document.querySelector('#mutation-recovery-title')?.textContent.trim() === 'Saved command confirmed';",
    );
    assert.equal((await appliedHistoryHead(sessionId, scenarioId)).id, committedId);
    assert.equal(
      await revision(),
      beforeLost + 1,
      "History reconciliation must not replay the import",
    );
    await verifyPeople(removed);
    await screenshot(sessionId, "eligibility-matrix-native-reconciled.png");
    await activateButton(sessionId, "Acknowledge and dismiss");
    await history(committedId);
    await activateButton(sessionId, "Undo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
    await verifyPeople(imported);
    await activateButton(sessionId, "Redo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario redo committed", 120_000);
    await verifyPeople(removed);
    await screenshot(sessionId, "eligibility-matrix-native-history.png");
    console.log(
      "PASS: packaged pasted eligibility matrix textarea/native capture/census/sample, independent role aliases, exact duplicate/invalid/unresolved cell skips, whole overflow/shape/cell-cap refusal, stale re-review, membership-order grouped apply/native report save/cancel, consumed no-op, committed lost-receipt History reconciliation without replay and exact Undo/Redo",
    );
  } catch (error) {
    failed = true;
    // Preserve the failing native view before cleanup can navigate or remove it.
    await screenshot(sessionId, "eligibility-matrix-native-failure.png").catch(() => undefined);
    const evidence = await evaluate(
      sessionId,
      "return {hash: location.hash, text: document.querySelector('main')?.innerText, active: document.activeElement?.outerHTML};",
    ).catch(() => null);
    await writeFile(
      new URL("../../../.cache/e2e/eligibility-matrix-native-failure.json", import.meta.url),
      JSON.stringify({ error: String(error), evidence }, null, 2),
    ).catch(() => undefined);
    throw error;
  } finally {
    await evaluate(
      sessionId,
      "window.__eligibilityMatrixGate?.restore?.(); delete window.__eligibilityMatrixGate;",
    ).catch(() => undefined);
    await navigate(sessionId, "/projects").catch(() => undefined);
    await package10DeleteFixture(sessionId, scenarioId).catch((error) => {
      if (!failed) throw error;
      console.error("Eligibility matrix fixture cleanup failed:", error);
    });
  }
}

async function packagedExternalPlanAcceptance(sessionId) {
  const historyRoot = '[aria-labelledby="history-heading"]';
  const title = `Phase07 external starting plan ${Date.now()}`;
  const created = await nativeRequest(sessionId, "project_create", {
    schemaVersion: 1,
    title,
    description: "Unverified editable source, native Repair and Alternatives",
    domainPack: { id: "official.workforce", schemaVersion: 1 },
    settings: {
      timeZone: "UTC",
      locale: "en-US",
      units: "metric",
      firstDate: "2030-01-01",
      lastDate: "2030-01-01",
      gapPolicy: "reject",
      overlapPolicy: "reject",
    },
  });
  const scenarioId = created.result.scenarioId;
  const assignmentTypeId = requestId(),
    planId = requestId();
  const people = [requestId(), requestId()];
  const shiftIds = Array.from({ length: 25 }, () => requestId());
  const panel = "[data-external-plan-editor]";
  const originalImport = { sourceBlake3: "a".repeat(64), pairContentHash: "b".repeat(64) };
  const initial = {
    kind: "externalStartingPlan",
    schemaVersion: 1,
    id: planId,
    name: "Unverified imported source",
    assignments: [
      { personId: people[1], shiftId: shiftIds[0] },
      ...shiftIds.map((shiftId) => ({ personId: people[0], shiftId })),
    ],
    originalImport,
  };
  const expectedPairs = initial.assignments.slice(1, -1);
  let controlIndex = 0;
  async function labelled(label, tag = "select") {
    const id = `external-smoke-control-${++controlIndex}`;
    await waitFor(
      sessionId,
      `return [...document.querySelectorAll('label')]
      .some(node => node.textContent.trim().startsWith(arguments[0]) && node.querySelector(arguments[1]));`,
      [label, tag],
    );
    await evaluate(
      sessionId,
      `const label = [...document.querySelectorAll('label')]
      .find(node => node.textContent.trim().startsWith(arguments[0]) && node.querySelector(arguments[1]));
      label.querySelector(arguments[1]).id = arguments[2];`,
      [label, tag, id],
    );
    return `#${id}`;
  }
  async function readPlan() {
    return (
      await nativeSetupRequest(
        sessionId,
        scenarioId,
        "scenario_get_entity",
        "eutheto.setup.entity_detail",
        { kind: "externalStartingPlan", entityId: planId },
      )
    ).result.view.data.result.data;
  }
  async function removePair(personId, shiftId) {
    await evaluate(
      sessionId,
      `const row = [...document.querySelectorAll(arguments[0] + ' li')]
      .find(node => [...node.querySelectorAll('code')].some(code => code.textContent === arguments[1]) &&
        [...node.querySelectorAll('code')].some(code => code.textContent === arguments[2]));
      if (!row) throw new Error('Expected complete pair is not visible');
      row.querySelector('button').id = 'external-smoke-remove';`,
      [panel, personId, shiftId],
    );
    await activate(sessionId, "#external-smoke-remove");
  }
  try {
    const instant = (index) => {
      const local = `2030-01-01T${String(9 + Math.floor(index / 2)).padStart(2, "0")}:${index % 2 ? "30" : "00"}:00`;
      return { instant: `${local}Z`, local, offsetSeconds: 0 };
    };
    const entities = [
      {
        id: requestId(),
        kind: "scorePolicy",
        profileKey: "external-smoke",
        levels: [{ levelKey: "preferences", label: "Authored preferences" }],
        priorityMapping: ["low", "normal", "high", "veryHigh"].map((priority) => ({
          priority,
          levelKey: "preferences",
          scale: 1,
        })),
        workloadPolicies: {},
        tieBreak: "stableAssignmentRank",
      },
      {
        id: assignmentTypeId,
        kind: "assignmentType",
        name: "External clinic duty",
        category: "clinic",
        timeBehavior: "elapsed",
        defaultDurationMinutes: 30,
        qualifications: { kind: "unconstrained" },
        locationBehavior: { kind: "optional" },
        workloadBucketIds: [],
      },
      ...people.map((id, index) => ({
        id,
        kind: "person",
        name: `External person ${index + 1}`,
        activeRange: { kind: "always" },
        qualificationGrants: [],
        eligibleAssignmentTypeIds: [assignmentTypeId],
        teamIds: [],
        tags: [],
        workloadWeight: { numerator: 1, denominator: 1 },
      })),
      ...shiftIds.map((id, index) => ({
        id,
        kind: "shiftInstance",
        assignmentTypeId,
        startsAt: instant(index),
        endsAt: instant(index + 1),
        coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
        tags: [],
        reportingAttribution: "startLocalDate",
        origin: { kind: "manual" },
      })),
      initial,
    ];
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: created.result.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "External source fixture" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: {
          label: "Unverified external source with excess coverage",
          commands: [
            ...entities.map((entity) => ({
              type: "applyDomainCommand",
              payload: { commandType: "official.workforce.add_entity", payload: { entity } },
            })),
            {
              type: "applyDomainCommand",
              payload: {
                commandType: "official.workforce.add_rule",
                payload: {
                  rule: {
                    id: requestId(),
                    kind: "coverage",
                    active: true,
                    strength: "required",
                    scope: { people: { kind: "all" } },
                  },
                },
              },
            },
          ],
        },
      },
    });
    await navigate(sessionId, "/projects");
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await activate(sessionId, `[aria-label=${JSON.stringify(`Open project ${title}`)}]`);
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await waitForElement(sessionId, "#setup-calendar");
    await selectValue(
      sessionId,
      await labelled("Starting source for Repair and Alternatives"),
      "external",
    );
    const picker = await indexedElementId(sessionId, `${panel} input[role="combobox"]`);
    await activate(sessionId, `#${picker}`);
    await setValue(sessionId, `#${picker}`, initial.name);
    await activate(sessionId, '[role="option"]');
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0])?.textContent.includes('26 total pairs.');",
      [panel],
    );
    await setValue(
      sessionId,
      await labelled("Starting-plan name", "input"),
      "Reviewed external source",
    );
    await removePair(people[1], shiftIds[0]);
    await activateButton(sessionId, "Next pairs", panel);
    await removePair(people[0], shiftIds[24]);
    assert.equal(
      (await readPlan()).assignments.length,
      26,
      "Local edits must not mutate the native source",
    );

    const beforeConcurrent = (await projects(sessionId)).find(
      (item) => item.scenarioId === scenarioId,
    );
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: beforeConcurrent.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Concurrent external source name" },
      truncateRedo: false,
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: "official.workforce.update_entity",
          payload: { entity: { ...initial, name: "Concurrent external source" } },
        },
      },
    });
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0])?.textContent.includes('Raw edits are retained');",
      [panel],
    );
    await activateButton(sessionId, "Read current record and rebase edits", panel);
    await activateButton(sessionId, "Keep draft name", panel);
    await activateButton(sessionId, "Review complete starting-plan update", panel);
    await waitForElement(sessionId, "#external-plan-review");
    assert.match(await getText(sessionId, panel), /26 saved pairs → 24 proposed pairs/u);
    assert.equal((await readPlan()).name, "Concurrent external source", "Review is not a write");
    await evaluate(
      sessionId,
      "document.querySelector('#external-plan-review').scrollIntoView({block:'start'});",
    );
    await screenshot(sessionId, "packaged-external-plan-review.png");
    await activateButton(sessionId, "Apply reviewed starting-plan changes", panel);
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0])?.textContent.includes('Committed command');",
      [panel],
      60_000,
    );
    await idle(sessionId);
    const saved = await readPlan();
    assert.equal(saved.name, "Reviewed external source");
    assert.deepEqual(
      saved.assignments,
      expectedPairs,
      "Paging must preserve every off-page pair in one complete update",
    );
    assert.deepEqual(
      saved.originalImport,
      originalImport,
      "Editing current pairs must preserve descriptive import fingerprints",
    );
    const afterEdit = (await projects(sessionId)).find((item) => item.scenarioId === scenarioId);
    assert.equal(afterEdit.revision, beforeConcurrent.revision + 2);

    await navigate(sessionId, `/project/${scenarioId}/history`);
    await activateButton(sessionId, "Undo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
    assert.deepEqual(await readPlan(), { ...initial, name: "Concurrent external source" });
    await activateButton(sessionId, "Redo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario redo committed", 120_000);
    assert.deepEqual(await readPlan(), saved);
    await navigate(sessionId, `/project/${scenarioId}/validation`);
    await activateButton(sessionId, "Run full validation");
    await waitForElement(sessionId, '[data-full-validation-state="completed"]', 60_000);
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await waitForElement(sessionId, "#setup-calendar");
    await selectValue(
      sessionId,
      await labelled("Starting source for Repair and Alternatives"),
      "external",
    );
    const nextPicker = await indexedElementId(sessionId, `${panel} input[role="combobox"]`);
    await activate(sessionId, `#${nextPicker}`);
    await setValue(sessionId, `#${nextPicker}`, saved.name);
    await activate(sessionId, '[role="option"]');
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0])?.textContent.includes('24 total pairs.');",
      [panel],
    );
    await selectValue(sessionId, await labelled("Next solve intent"), "repair");
    const head = await appliedHistoryHead(sessionId, scenarioId);
    await waitForElement(sessionId, '[data-optimize-state="ready"]');
    await activateButton(sessionId, "Repair plan", "[data-optimize-panel]");
    await waitForElement(sessionId, "#setup-results-details .result-summary-list > li", 60_000);
    const candidate = (
      await nativeRequest(sessionId, "solution_list", { schemaVersion: 1, scenarioId })
    ).result.solutions[0];
    assert(candidate && ["optimal", "feasible"].includes(candidate.status));
    const verified = await nativeRequest(sessionId, "solution_verify", {
      schemaVersion: 1,
      scenarioId,
      solutionId: candidate.result.solutionId,
    });
    assert.equal(verified.result.verification.accepted, true);
    const schedule = (
      await nativeRequest(sessionId, "solution_get_view", {
        schemaVersion: 1,
        scenarioId,
        solutionId: candidate.result.solutionId,
        viewId: "official.workforce.result.schedule",
        parameters: { kind: "timeline", limit: 50 },
        continuation: null,
      })
    ).result.view.data.result;
    assert.equal(
      schedule.items.length,
      25,
      "Required coverage repairs the unfilled shift without locking the external source",
    );
    for (const pair of expectedPairs)
      assert(
        schedule.items.some(
          (item) => item.personId === pair.personId && item.shiftId === pair.shiftId,
        ),
      );
    const difference = (
      await nativeRequest(sessionId, "solution_get_view", {
        schemaVersion: 1,
        scenarioId,
        solutionId: candidate.result.solutionId,
        viewId: "official.workforce.result.schedule",
        parameters: { kind: "repairDiff", limit: 50 },
        continuation: null,
      })
    ).result.view.data.result;
    assert.equal(difference.availability, "available");
    assert.equal(difference.baseline.kind, "external");
    assert.equal(difference.baseline.binding.entity.id, `official.workforce.${planId}`);
    assert.match(difference.baseline.binding.selectedPairContentHash, /^[a-f0-9]{64}$/u);
    assert.notEqual(
      difference.baseline.binding.selectedPairContentHash,
      originalImport.pairContentHash,
    );
    assert.equal(Object.hasOwn(difference.baseline, "original"), false);
    assert.deepEqual(
      await readPlan(),
      saved,
      "Accepted Repair never mutates its external starting plan",
    );
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), head);

    await selectValue(sessionId, await labelled("Next solve intent"), "alternatives");
    await waitForElement(sessionId, '[data-alternatives-ready="true"]', 60_000);
    await waitFor(
      sessionId,
      `const text = document.querySelector(arguments[0]).textContent;
      return !text.includes('Person label unavailable') && !text.includes('Work label unavailable');`,
      [panel],
    );
    // Hold a real prepare reply, not a fabricated group, while the next source changes.
    await evaluate(
      sessionId,
      `const callbacks = window.__TAURI_INTERNALS__.callbacks;
      const descriptor = Object.getOwnPropertyDescriptor(callbacks, 'get'), original = callbacks.get;
      const gate = { held: false };
      gate.restore = () => descriptor ? Object.defineProperty(callbacks, 'get', descriptor) : Reflect.deleteProperty(callbacks, 'get');
      window.__externalPlanGate = gate;
      Object.defineProperty(callbacks, 'get', { configurable: true, value: function(id) {
        const callback = original.call(this, id);
        if (!callback) return callback;
        return data => {
          if (!gate.held && data?.result?.schemaVersion === 1 && data.result.operationId &&
              Object.keys(data.result).length === 2) {
            gate.held = true; gate.release = () => callback(data); return;
          }
          callback(data);
        };
      }});`,
    );
    await activateButton(sessionId, "Run Alternatives", "[data-alternatives-panel]");
    await waitFor(sessionId, "return window.__externalPlanGate?.held === true;");
    await selectValue(
      sessionId,
      await labelled("Starting source for Repair and Alternatives"),
      "accepted",
    );
    await evaluate(
      sessionId,
      "window.__externalPlanGate.release(); window.__externalPlanGate.restore();",
    );
    await waitFor(
      sessionId,
      `const status = document.querySelector('[aria-label="Native Alternatives group status"]');
      return status?.textContent.includes('Settled') && status.textContent.includes('completed');`,
      [],
      60_000,
    );
    const revision = (await projects(sessionId)).find(
      (item) => item.scenarioId === scenarioId,
    ).revision;
    const group = (
      await nativeRequest(sessionId, "alternatives_get_group", {
        schemaVersion: 1,
        scenarioId,
        expectedRevision: revision,
      })
    ).result.group;
    assert.deepEqual(group.baselineSource, { kind: "external", entityId: planId });
    assert.deepEqual(
      group.snapshot.members.map((member) => member.profile),
      ["balanced", "preferenceFocused", "minimalChanges", "fairnessFirst"],
    );
    for (const member of group.snapshot.members) {
      assert.equal(member.outcome.type, "accepted");
      const check = await nativeRequest(sessionId, "solution_verify", {
        schemaVersion: 1,
        scenarioId,
        solutionId: member.outcome.solutionId,
      });
      assert.equal(check.result.verification.accepted, true);
    }
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), head);
    await evaluate(
      sessionId,
      "document.querySelector('[data-alternatives-panel]').scrollIntoView({block:'start'});",
    );
    await screenshot(sessionId, "packaged-external-alternatives.png");

    await selectValue(
      sessionId,
      await labelled("Starting source for Repair and Alternatives"),
      "external",
    );
    await waitForElement(sessionId, `${panel} li button:not(:disabled)`);
    for (let index = 0; index < expectedPairs.length; index += 1)
      await activate(sessionId, `${panel} li button:not(:disabled)`);
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0])?.textContent.includes('Explicit empty starting plan');",
      [panel],
    );
    await activateButton(sessionId, "Review complete starting-plan update", panel);
    await waitForElement(sessionId, "#external-plan-review");
    assert.match(await getText(sessionId, panel), /24 saved pairs → 0 proposed pairs/u);
    // The native command commits; invalidate only its returned transport envelope.
    await evaluate(
      sessionId,
      `const callbacks = window.__TAURI_INTERNALS__.callbacks;
      const descriptor = Object.getOwnPropertyDescriptor(callbacks, 'get'), original = callbacks.get;
      const gate = { revision: null };
      gate.restore = () => descriptor ? Object.defineProperty(callbacks, 'get', descriptor) : Reflect.deleteProperty(callbacks, 'get');
      window.__externalPlanGate = gate;
      Object.defineProperty(callbacks, 'get', { configurable: true, value: function(id) {
        const callback = original.call(this, id);
        if (!callback) return callback;
        return data => {
          if (gate.revision === null && Number.isInteger(data?.result?.newRevision)) {
            gate.revision = data.result.newRevision;
            callback({ ...data, result: null }); return;
          }
          callback(data);
        };
      }});`,
    );
    await activateButton(sessionId, "Apply reviewed starting-plan changes", panel);
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0])?.textContent.includes('The write outcome is unknown.');",
      [panel],
      60_000,
    );
    const uncertainRevision = await evaluate(
      sessionId,
      "window.__externalPlanGate.restore(); return window.__externalPlanGate.revision;",
    );
    const uncertainCommand = (await appliedHistoryHead(sessionId, scenarioId))?.id;
    assert.equal(typeof uncertainCommand, "string");
    assert.deepEqual(
      (await readPlan()).assignments,
      [],
      "The lost receipt must not be mistaken for rollback",
    );
    const emptyRevision = (await projects(sessionId)).find(
      (item) => item.scenarioId === scenarioId,
    ).revision;
    assert.equal(emptyRevision, revision + 1);
    assert.equal(uncertainRevision, emptyRevision);
    assert(
      await evaluate(
        sessionId,
        `const button = [...document.querySelectorAll(arguments[0] + ' button')]
      .find(button => button.textContent.trim() === 'Review complete starting-plan update');
      return button instanceof HTMLButtonElement && button.disabled;`,
        [panel],
      ),
    );
    await activateButton(sessionId, "Check saved history");
    await waitFor(
      sessionId,
      "return document.querySelector('#mutation-recovery-title')?.textContent.trim() === 'Saved command confirmed';",
    );
    assert(
      await evaluate(
        sessionId,
        "return document.querySelector('#mutation-recovery-title').parentElement.textContent.includes(arguments[0]);",
        [uncertainCommand],
      ),
    );
    await activateButton(sessionId, "Acknowledge and dismiss");
    await activateButton(sessionId, "Read current record and rebase edits", panel);
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0])?.textContent.includes('0 total pairs.');",
      [panel],
    );
    assert.equal(
      (await projects(sessionId)).find((item) => item.scenarioId === scenarioId).revision,
      emptyRevision,
      "History reconciliation must not replay the committed empty-plan update",
    );
    await waitFor(
      sessionId,
      `const field = document.querySelector(arguments[0]);
      return field && !field.disabled && [...field.options].some(option => option.value === 'repair' && !option.disabled);`,
      [await labelled("Next solve intent")],
    );
    await selectValue(sessionId, await labelled("Next solve intent"), "repair");
    assert.equal(
      await evaluate(sessionId, "return document.querySelector(arguments[0]).value;", [
        await labelled("Next solve intent"),
      ]),
      "repair",
      "An explicit empty source is still a selectable source",
    );
    await activateButton(sessionId, "Review starting-plan removal", panel);
    await waitForElement(sessionId, "#external-plan-review");
    await activateButton(sessionId, "Apply reviewed starting-plan changes", panel);
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0])?.textContent.includes('Choose an imported starting plan to inspect or edit it.');",
      [panel],
      60_000,
    );
    assert(
      await evaluate(
        sessionId,
        "return [...document.querySelector(arguments[0]).options].find(option => option.value === 'repair').disabled;",
        [await labelled("Next solve intent")],
      ),
      "Removal must not silently fall back to another source",
    );
    const retainedDifference = (
      await nativeRequest(sessionId, "solution_get_view", {
        schemaVersion: 1,
        scenarioId,
        solutionId: candidate.result.solutionId,
        viewId: "official.workforce.result.schedule",
        parameters: { kind: "repairDiff", limit: 50 },
        continuation: null,
      })
    ).result.view.data.result;
    assert.deepEqual(
      retainedDifference,
      difference,
      "Deleting the source must not rewrite the accepted run's captured baseline",
    );
    await navigate(sessionId, `/project/${scenarioId}/history`);
    await activateButton(sessionId, "Undo scenario change", historyRoot);
    await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
    assert.deepEqual(await readPlan(), { ...saved, assignments: [] });
    console.log(
      "PASS: native external-source paging/edit/rebase/review/apply/Undo/Redo, verified Repair, immutable four-profile source selection, explicit empty source, lost-receipt History reconciliation and reversible removal",
    );
  } finally {
    await evaluate(
      sessionId,
      `window.__externalPlanGate?.release?.();
      window.__externalPlanGate?.restore?.(); delete window.__externalPlanGate;`,
    ).catch(() => undefined);
    await navigate(sessionId, "/projects").catch(() => undefined);
    await package10DeleteFixture(sessionId, scenarioId);
  }
}

async function packagedRecipientExportAcceptance(sessionId, directory) {
  const source = (await package10FixtureEvidence()).cases.find(
    (entry) => entry.id === "clinic-tiny",
  );
  assert(source);
  const { scenarioId } = await package10CreateFixture(sessionId, source);
  const dialog = '[role="dialog"]';
  async function prepareRecipientReview() {
    await waitFor(
      sessionId,
      `return [...document.querySelectorAll('.recipient-share-dialog button')]
        .some(button => button.textContent.trim() === 'Prepare review' && !button.disabled);`,
    );
    await keyboardActivateButton(sessionId, "Prepare review", dialog);
    await waitForElement(sessionId, ".recipient-share-source", 60_000);
    return evaluate(sessionId, "return window.__recipientEvidence.preview;");
  }
  try {
    await navigate(sessionId, "/projects");
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await activate(sessionId, '[aria-label="Open project Synthetic profile clinic-tiny"]');
    await navigate(sessionId, `/project/${scenarioId}/validation`);
    await waitForElement(sessionId, "#validation-heading");
    await activateButton(sessionId, "Run full validation");
    await waitForElement(sessionId, '[data-full-validation-state="completed"]', 60_000);
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await waitForElement(sessionId, '[data-optimize-state="ready"]');
    await activateButton(sessionId, "Start optimization", "[data-optimize-panel]");
    await waitForElement(sessionId, "#setup-results-details .result-summary-list > li", 60_000);
    await activateButton(sessionId, "Inspect schedule", "#setup-results-details");
    await waitForElement(sessionId, "[data-accepted-schedule] table tbody tr");
    const [accepted] = (
      await nativeRequest(sessionId, "solution_list", { schemaVersion: 1, scenarioId })
    ).result.solutions;
    assert(accepted && ["optimal", "feasible"].includes(accepted.status));
    const originalRevision = (await projects(sessionId)).find(
      (project) => project.scenarioId === scenarioId,
    ).revision;
    const timeline = (
      await nativeRequest(sessionId, "solution_get_view", {
        schemaVersion: 1,
        scenarioId,
        solutionId: accepted.result.solutionId,
        viewId: "official.workforce.result.schedule",
        parameters: { kind: "timeline", limit: 50 },
        continuation: null,
      })
    ).result.view.data.result;
    const people = package10Records(source.fixture.domain.payload.entities).filter(
      (entity) => entity.kind === "person",
    );
    await keyboardActivateButton(sessionId, "Export CSV/calendar", "[data-accepted-schedule]");
    await waitForElement(sessionId, ".schedule-export-dialog");
    const assignmentFormat = `${dialog} input[type="radio"][value="assignmentsCsv"]`;
    await keyboardReach(sessionId, assignmentFormat);
    await keyboardKey(sessionId, " ");
    assert.equal(await evaluate(sessionId, "return document.activeElement.checked;"), true);
    await keyboardActivateButton(sessionId, "Prepare native review", dialog);
    await waitForElement(sessionId, ".schedule-export-source");
    await waitFor(
      sessionId,
      `const source = document.querySelector('.schedule-export-source');
      return source?.textContent.includes(arguments[0]) &&
        source.textContent.includes(arguments[1]);`,
      [accepted.result.solutionId, accepted.result.resultChecksum],
    );
    await screenshot(sessionId, "packaged-keyboard-accepted-export-review.png");
    assert.equal(
      (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
      originalRevision,
    );
    await keyboardActivateButton(sessionId, "Close and release review", dialog);
    await waitFor(sessionId, "return !document.querySelector('[role=dialog]');");
    await waitFor(
      sessionId,
      `const root = document.querySelector('[data-accepted-schedule]');
      return document.activeElement === [...(root?.querySelectorAll('button') ?? [])]
        .find(button => button.textContent.trim() === 'Export CSV/calendar');`,
    );

    await keyboardKey(sessionId, "\uE004", true);
    await keyboardActivateButton(sessionId, "Share result", "[data-accepted-schedule]");
    await waitForElement(sessionId, ".recipient-share-dialog");
    await keyboardActivateButton(sessionId, "Prepare review", dialog);
    await waitForElement(sessionId, ".recipient-share-source");
    await waitFor(
      sessionId,
      `const source = document.querySelector('.recipient-share-source');
      return source?.textContent.includes(arguments[0]) &&
        source.textContent.includes(arguments[1]);`,
      [accepted.result.solutionId, accepted.result.resultChecksum],
    );
    await screenshot(sessionId, "packaged-keyboard-recipient-share-review.png");
    assert.equal(
      (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
      originalRevision,
    );
    await keyboardActivateButton(sessionId, "Close and release review", dialog);
    await waitFor(sessionId, "return !document.querySelector('[role=dialog]');");
    await waitFor(
      sessionId,
      `const root = document.querySelector('[data-accepted-schedule]');
      return document.activeElement === [...(root?.querySelectorAll('button') ?? [])]
        .find(button => button.textContent.trim() === 'Share result');`,
    );
    for (const format of ["assignmentsCsv", "summaryCsv", "ics"]) {
      await activateButton(sessionId, "Export CSV/calendar");
      await waitForElement(sessionId, `${dialog} input[value="${format}"]`);
      await activate(sessionId, `${dialog} input[value="${format}"]`);
      await activateButton(sessionId, "Prepare native review", dialog);
      await waitForElement(sessionId, ".schedule-export-source");
      const preview = await evaluate(
        sessionId,
        `return Object.fromEntries([...document.querySelectorAll('.schedule-export-source dt')]
          .map(node => [node.textContent.trim(), node.nextElementSibling.textContent.trim()]));`,
      );
      assert.equal(preview["Solution ID"], accepted.result.solutionId);
      assert.equal(preview["Accepted result checksum"], accepted.result.resultChecksum);
      const output = join(directory, `accepted-${format}.${format === "ics" ? "ics" : "csv"}`);
      await activateButton(sessionId, "Save reviewed export…", dialog);
      await nativeChooser(format === "ics" ? "Save calendar" : "Save schedule CSV", output);
      await waitFor(
        sessionId,
        `return document.querySelector(arguments[0])?.textContent.includes('Native publication confirmed:');`,
        [dialog],
      );
      const bytes = await readFile(output);
      assert.equal(bytes.length, Number(preview["Complete artifact bytes"]));
      assert.equal(package10Sha256(bytes), preview["Complete artifact SHA-256"]);
      const text = bytes.toString("utf8");
      if (format === "assignmentsCsv") {
        for (const assignment of timeline.items) {
          assert(text.includes(assignment.personId) && text.includes(assignment.shiftId));
        }
        for (const person of people) assert(!text.includes(person.name));
      } else if (format === "summaryCsv") {
        for (const person of people) assert(text.includes(person.id) && text.includes(person.name));
      } else {
        assert.equal(text.match(/BEGIN:VEVENT/g)?.length ?? 0, timeline.items.length);
        assert(text.includes("BEGIN:VCALENDAR") && text.includes("END:VCALENDAR"));
      }
      await activateButton(sessionId, "Close and release review", dialog);
      await waitFor(sessionId, "return !document.querySelector('[role=dialog]');");
    }
    await activateButton(sessionId, "Share result");
    await waitForElement(sessionId, ".recipient-share-dialog");
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelectorAll('.recipient-share-dialog input:checked').length;",
      ),
      0,
    );
    await activateButton(sessionId, "Prepare review", dialog);
    await waitForElement(sessionId, ".recipient-share-source");
    await activateButton(sessionId, "Save reviewed HTML…", dialog);
    await nativeChooser("Save recipient HTML report");
    await waitFor(
      sessionId,
      `return !document.querySelector('.recipient-share-source') &&
        [...document.querySelectorAll('.recipient-share-dialog button')]
          .some(button => button.textContent.trim() === 'Prepare review' && !button.disabled);`,
    );
    const output = join(directory, "accepted-recipient.html");
    await activateButton(sessionId, "Prepare review", dialog);
    await waitForElement(sessionId, ".recipient-share-source");
    await activateButton(sessionId, "Save reviewed HTML…", dialog);
    await nativeChooser("Save recipient HTML report", output);
    await waitFor(
      sessionId,
      "return document.querySelector('.recipient-share-dialog')?.textContent.includes('Native publication confirmed:');",
    );
    const html = await readFile(output, "utf8");
    const payload = html
      .split('<script id="recipient-data" type="application/json">')[1]
      ?.split("</script>")[0];
    assert(payload, "Publication must contain the actual inert recipient payload");
    const report = JSON.parse(payload);
    assert.equal(report.source.solutionId, accepted.result.solutionId);
    assert.equal(report.source.acceptedResultChecksum, accepted.result.resultChecksum);
    assert.equal(report.source.scenarioRevision, originalRevision);
    assert.deepEqual(
      report.content.schedule.assignments
        .map(({ personId, shiftId }) => `${personId}/${shiftId}`)
        .sort(),
      timeline.items.map(({ personId, shiftId }) => `${personId}/${shiftId}`).sort(),
    );
    assert.deepEqual(
      report.content.schedule.people.map(({ personId }) => personId).sort(),
      [...new Set(timeline.items.map(({ personId }) => personId))].sort(),
      "Private defaults disclose only participants in the complete accepted assignment set",
    );
    for (const person of people) assert(!html.includes(person.name));
    assert.equal(report.content.schedule.staffing, null);
    assert.equal(report.content.schedule.changes, null);
    assert.equal(report.content.schedule.fairnessActuals, null);
    // Observe real native replies without replacing, holding, or altering their delivery.
    await evaluate(
      sessionId,
      `const callbacks = window.__TAURI_INTERNALS__.callbacks;
      const descriptor = Object.getOwnPropertyDescriptor(callbacks, 'get'), original = callbacks.get;
      const evidence = { preview: null, created: null };
      evidence.restore = () => descriptor ? Object.defineProperty(callbacks, 'get', descriptor) : Reflect.deleteProperty(callbacks, 'get');
      window.__recipientEvidence = evidence;
      Object.defineProperty(callbacks, 'get', { configurable: true, value: function(id) {
        const callback = original.call(this, id);
        if (!callback) return callback;
        return data => {
          if (data?.result?.previewId && data.result.report) evidence.preview = data.result;
          if (data?.result?.previewId && data.result.publication) evidence.created = data.result;
          callback(data);
        };
      }});`,
    );
    // An HTML review cannot authorize PDF: change format by keyboard and recapture.
    const htmlPreview = await prepareRecipientReview();
    assert.equal(htmlPreview.format, "html");
    await keyboardWorkflowSelect(sessionId, ".recipient-share-dialog select", "pdf");
    await waitFor(
      sessionId,
      `return !document.querySelector('.recipient-share-source') &&
        [...document.querySelectorAll('.recipient-share-dialog button')]
          .some(button => button.textContent.trim() === 'Prepare review' && !button.disabled);`,
    );
    assert.equal(
      await evaluate(
        sessionId,
        `return [...document.querySelectorAll('.recipient-share-dialog button')]
          .some(button => button.textContent.trim().startsWith('Save reviewed'));`,
      ),
      false,
      "Changing format retires the HTML save authority",
    );
    const firstPdfPreview = await prepareRecipientReview();
    assert.equal(firstPdfPreview.format, "pdf");
    assert.notEqual(firstPdfPreview.previewId, htmlPreview.previewId);
    await screenshot(sessionId, "packaged-keyboard-recipient-pdf-review.png");
    await keyboardActivateButton(sessionId, "Close and release review", dialog);
    await waitFor(sessionId, "return !document.querySelector('[role=dialog]');");
    await waitFor(
      sessionId,
      `const root = document.querySelector('[data-accepted-schedule]');
      return document.activeElement === [...(root?.querySelectorAll('button') ?? [])]
        .find(button => button.textContent.trim() === 'Share result');`,
    );
    await keyboardActivateButton(sessionId, "Share result", "[data-accepted-schedule]");
    await waitForElement(sessionId, ".recipient-share-dialog");
    assert.equal(
      await evaluate(
        sessionId,
        "return document.querySelector('.recipient-share-dialog select').value;",
      ),
      "pdf",
    );
    const cancelledPdfPreview = await prepareRecipientReview();
    assert.notEqual(cancelledPdfPreview.previewId, firstPdfPreview.previewId);
    await keyboardActivateButton(sessionId, "Save reviewed PDF…", dialog);
    await nativeChooser("Save recipient PDF report");
    await waitFor(
      sessionId,
      `return !document.querySelector('.recipient-share-source') &&
        [...document.querySelectorAll('.recipient-share-dialog button')]
          .some(button => button.textContent.trim() === 'Prepare review' && !button.disabled);`,
    );
    let retiredPreviewId = cancelledPdfPreview.previewId;
    for (const disclose of [false, true]) {
      if (disclose) {
        retiredPreviewId = (await prepareRecipientReview()).previewId;
        for (let index = 1; index <= 5; index += 1) {
          await keyboardActivate(
            sessionId,
            `.recipient-share-dialog fieldset label:nth-of-type(${index}) input`,
            " ",
          );
          await waitFor(
            sessionId,
            "return document.querySelector('.recipient-share-dialog fieldset')?.disabled === false;",
          );
        }
        await waitFor(sessionId, "return !document.querySelector('.recipient-share-source');");
      }
      const preview = await prepareRecipientReview();
      assert.equal(preview.format, "pdf");
      assert.notEqual(preview.previewId, retiredPreviewId);
      assert.match(preview.pdfSha256, /^[a-f0-9]{64}$/u);
      assert.equal(preview.pdfRenderer.typstVersion, "0.15.1");
      assert.equal(preview.report.source.scenarioId, scenarioId);
      assert.equal(preview.report.source.solutionId, accepted.result.solutionId);
      assert.equal(preview.report.source.acceptedResultChecksum, accepted.result.resultChecksum);
      assert.equal(preview.report.source.scenarioRevision, originalRevision);
      assert.equal(preview.report.source.observedCurrentRevision, originalRevision);
      assert.equal(preview.report.source.origin, "storedTerminal");
      assert.equal(
        preview.report.source.verificationReportChecksum,
        report.source.verificationReportChecksum,
      );
      assert.deepEqual(preview.report.privacy, {
        includePersonNames: disclose,
        includeAuthoredLabels: disclose,
        includeStaffing: disclose,
        includeChanges: disclose,
        includeFairnessActuals: disclose,
      });
      const schedule = preview.report.content.schedule;
      assert.deepEqual(
        schedule.assignments.map(({ personId, shiftId }) => `${personId}/${shiftId}`).sort(),
        timeline.items.map(({ personId, shiftId }) => `${personId}/${shiftId}`).sort(),
      );
      assert.deepEqual(
        schedule.people.map(({ personId }) => personId).sort(),
        report.content.schedule.people.map(({ personId }) => personId).sort(),
      );
      assert.deepEqual(
        schedule.shifts.map(({ shiftId }) => shiftId).sort(),
        report.content.schedule.shifts.map(({ shiftId }) => shiftId).sort(),
      );
      for (const person of schedule.people) {
        const authored = people.find((candidate) => candidate.id === person.personId);
        assert(authored);
        assert.equal(person.label.kind, disclose ? "authored" : "generated");
        assert.equal(person.label.text === authored.name, disclose);
      }
      for (const section of ["staffing", "changes", "fairnessActuals"])
        assert.equal(schedule[section] !== null, disclose);
      const pdfOutput = join(
        directory,
        `accepted-recipient-${disclose ? "all-fields" : "default"}.pdf`,
      );
      await keyboardActivateButton(sessionId, "Save reviewed PDF…", dialog);
      await nativeChooser("Save recipient PDF report", pdfOutput);
      await waitFor(
        sessionId,
        "return document.querySelector('.recipient-share-dialog')?.textContent.includes('Native publication confirmed:');",
      );
      const created = await waitFor(
        sessionId,
        "return window.__recipientEvidence.created?.previewId === arguments[0] && window.__recipientEvidence.created;",
        [preview.previewId],
      );
      const pdf = await readFile(pdfOutput);
      assert.equal(pdf.subarray(0, 5).toString("ascii"), "%PDF-");
      assert.equal(package10Sha256(pdf), preview.pdfSha256);
      assert.equal(created.format, "pdf");
      assert.equal(created.filename, basename(pdfOutput));
      assert.equal(created.publication.solutionId, accepted.result.solutionId);
      assert.equal(created.publication.sourceRevision, originalRevision);
      assert.equal(created.publication.currentRevision, originalRevision);
      assert.equal(created.publication.modelSha256, preview.modelSha256);
      assert.equal(created.publication.pdfSha256, preview.pdfSha256);
      retiredPreviewId = preview.previewId;
    }
    // Even a chooser overwrite confirmation cannot authorize replacing an existing file.
    const existingPdf = join(directory, "existing-recipient.pdf");
    const sentinel = "Disposable no-clobber sentinel";
    await writeFile(existingPdf, sentinel);
    await prepareRecipientReview();
    await keyboardActivateButton(sessionId, "Save reviewed PDF…", dialog);
    await nativeChooser("Save recipient PDF report", existingPdf, true);
    await waitFor(
      sessionId,
      `return !document.querySelector('.recipient-share-source') &&
        document.querySelector('.recipient-share-dialog [role=alert]')?.textContent.includes('already exists') &&
        [...document.querySelectorAll('.recipient-share-dialog button')]
          .some(button => button.textContent.trim() === 'Prepare review' && !button.disabled);`,
    );
    assert.equal(await readFile(existingPdf, "utf8"), sentinel);
    await keyboardActivateButton(sessionId, "Close and release review", dialog);
    await waitFor(sessionId, "return !document.querySelector('[role=dialog]');");
    await waitFor(
      sessionId,
      `const root = document.querySelector('[data-accepted-schedule]');
      return document.activeElement === [...(root?.querySelectorAll('button') ?? [])]
        .find(button => button.textContent.trim() === 'Share result');`,
    );
    assert.equal(
      (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
      originalRevision,
      "HTML/PDF review, format/disclosure retirement, picker cancellation, publication and no-clobber refusal cannot mutate the scenario",
    );
    await navigate(sessionId, "/projects");
    console.log(
      "PASS: real-worker accepted result → keyboard CSV/HTML/PDF review and focus restore → exact native CSV/ICS/HTML/PDF publication, private defaults/all disclosures, format retirement, picker cancellation and PDF no-clobber",
    );
  } finally {
    await evaluate(
      sessionId,
      "window.__recipientEvidence?.restore(); delete window.__recipientEvidence;",
    ).catch(() => undefined);
    await package10DeleteFixture(sessionId, scenarioId);
  }
}

async function packagedAlternativesAcceptance(sessionId) {
  const title = `Phase07 Alternatives ${Date.now()}`;
  const created = await nativeRequest(sessionId, "project_create", {
    schemaVersion: 1,
    title,
    description: "Real-worker sequential Alternatives acceptance",
    domainPack: { id: "official.workforce", schemaVersion: 1 },
    settings: {
      timeZone: "UTC",
      locale: "en-US",
      units: "metric",
      firstDate: "2030-01-01",
      lastDate: "2030-01-01",
      gapPolicy: "reject",
      overlapPolicy: "reject",
    },
  });
  const scenarioId = created.result.scenarioId;
  const assignmentTypeId = requestId(),
    shiftId = requestId();
  const people = [requestId(), requestId()];
  const panel = "[data-alternatives-panel]";
  let selector = 0;
  async function focusedControl(expectedName) {
    return evaluate(
      sessionId,
      `const element = document.activeElement;
      return { usable: element instanceof HTMLButtonElement && !element.disabled &&
        (element.getAttribute('aria-label') ?? element.textContent.trim()) === arguments[0],
        element: element?.outerHTML.slice(0, 512) };`,
      [expectedName],
    );
  }
  async function chooseLabel(label, value, root = "body") {
    const id = `alternatives-smoke-select-${++selector}`;
    await waitFor(
      sessionId,
      `return [...(document.querySelector(arguments[0])?.querySelectorAll('label') ?? [])]
        .some(node => node.textContent.trim().startsWith(arguments[1]) && node.querySelector('select'));`,
      [root, label],
    );
    await evaluate(
      sessionId,
      `const root = document.querySelector(arguments[0]);
      const label = [...root.querySelectorAll('label')].find(node => node.textContent.trim().startsWith(arguments[1]));
      const select = label?.querySelector('select');
      if (!select) throw new Error('Missing labelled select: ' + arguments[1]);
      select.id = arguments[2];`,
      [root, label, id],
    );
    await selectValue(sessionId, `#${id}`, value);
  }
  async function validateAndOpen() {
    await navigate(sessionId, `/project/${scenarioId}/validation`);
    await waitForElement(sessionId, "#validation-heading");
    await activateButton(sessionId, "Run full validation");
    await waitForElement(sessionId, '[data-full-validation-state="completed"]', 60_000);
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await waitForElement(sessionId, "#setup-calendar");
  }
  try {
    const entities = [
      {
        id: requestId(),
        kind: "scorePolicy",
        profileKey: "alternatives-smoke",
        levels: [{ levelKey: "preferences", label: "Authored preferences" }],
        priorityMapping: ["low", "normal", "high", "veryHigh"].map((priority) => ({
          priority,
          levelKey: "preferences",
          scale: 1,
        })),
        workloadPolicies: {},
        tieBreak: "stableAssignmentRank",
      },
      {
        id: assignmentTypeId,
        kind: "assignmentType",
        name: "Alternatives clinic duty",
        category: "clinic",
        timeBehavior: "elapsed",
        defaultDurationMinutes: 60,
        qualifications: { kind: "unconstrained" },
        locationBehavior: { kind: "optional" },
        workloadBucketIds: [],
      },
      ...people.map((id, index) => ({
        id,
        kind: "person",
        name: `Alternatives person ${index + 1}`,
        activeRange: { kind: "always" },
        qualificationGrants: [],
        eligibleAssignmentTypeIds: [assignmentTypeId],
        teamIds: [],
        tags: [],
        workloadWeight: { numerator: 1, denominator: 1 },
      })),
      {
        id: shiftId,
        kind: "shiftInstance",
        assignmentTypeId,
        startsAt: {
          instant: "2030-01-01T09:00:00Z",
          local: "2030-01-01T09:00:00",
          offsetSeconds: 0,
        },
        endsAt: { instant: "2030-01-01T10:00:00Z", local: "2030-01-01T10:00:00", offsetSeconds: 0 },
        coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
        tags: [],
        reportingAttribution: "startLocalDate",
        origin: { kind: "manual" },
      },
    ];
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: created.result.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Native Alternatives fixture" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: {
          label: "Two-person native Alternatives source",
          commands: [
            ...entities.map((entity) => ({
              type: "applyDomainCommand",
              payload: { commandType: "official.workforce.add_entity", payload: { entity } },
            })),
            {
              type: "applyDomainCommand",
              payload: {
                commandType: "official.workforce.add_rule",
                payload: {
                  rule: {
                    id: requestId(),
                    kind: "coverage",
                    active: true,
                    strength: "required",
                    scope: { people: { kind: "all" } },
                  },
                },
              },
            },
          ],
        },
      },
    });
    await navigate(sessionId, "/projects");
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await activate(sessionId, `[aria-label=${JSON.stringify(`Open project ${title}`)}]`);
    await validateAndOpen();
    await waitForElement(sessionId, '[data-optimize-state="ready"]');
    await activateButton(sessionId, "Start optimization", "[data-optimize-panel]");
    await waitForElement(sessionId, "#setup-results-details .result-summary-list > li", 60_000);
    const base = (await nativeRequest(sessionId, "solution_list", { schemaVersion: 1, scenarioId }))
      .result.solutions[0];
    assert(base && ["optimal", "feasible"].includes(base.status));
    const timeline = (
      await nativeRequest(sessionId, "solution_get_view", {
        schemaVersion: 1,
        scenarioId,
        solutionId: base.result.solutionId,
        viewId: "official.workforce.result.schedule",
        parameters: { kind: "timeline", limit: 50 },
        continuation: null,
      })
    ).result.view.data.result;
    assert.equal(timeline.items.length, 1);
    const incumbent = timeline.items[0].personId;
    assert(people.includes(incumbent));
    await chooseLabel("Next solve intent", "alternatives");
    await waitForElement(sessionId, panel);
    assert(
      await evaluate(
        sessionId,
        `return [...document.querySelector(arguments[0]).querySelectorAll('button')]
      .find(button => button.textContent.trim() === 'Run Alternatives').getAttribute('aria-disabled') === 'true';`,
        [panel],
      ),
      "A merely selected accepted result must not authorize Alternatives before explicit base adoption",
    );
    await activateButton(
      sessionId,
      "Review as repair base",
      "#setup-results-details .result-summary-list > li:first-child",
    );
    await activateButton(
      sessionId,
      "Save this result as repair base",
      "#setup-results-details .result-summary-list > li:first-child",
    );
    await waitFor(
      sessionId,
      `return document.body.textContent.includes('The accepted result is saved as the repair base.');`,
      [],
      60_000,
    );
    const adopted = (await projects(sessionId)).find(
      (project) => project.scenarioId === scenarioId,
    );
    assert.equal(adopted.revision, base.scenarioRevision + 1);
    const changed = await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: adopted.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Native Alternatives fixture" },
      truncateRedo: false,
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: "official.workforce.add_preference",
          payload: {
            preference: {
              id: requestId(),
              kind: "assignmentType",
              active: true,
              scope: { people: { kind: "selected", personIds: [incumbent] } },
              priority: "normal",
              weight: 1,
              direction: "avoid",
              assignmentTypeIds: [assignmentTypeId],
            },
          },
        },
      },
    });
    const revision = changed.result.newRevision;
    const head = await appliedHistoryHead(sessionId, scenarioId);
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await waitForElement(sessionId, "#setup-calendar");
    await validateAndOpen();
    await chooseLabel("Next solve intent", "alternatives");
    await waitForElement(sessionId, '[data-alternatives-ready="true"]', 60_000);
    await chooseLabel("Effort", "deep", panel);

    // Delay only real native responses: reservation first, then a pre-admission null discovery.
    // This exercises the start-result/discovery ordering race without inventing accepted data.
    await evaluate(
      sessionId,
      `const callbacks = window.__TAURI_INTERNALS__.callbacks;
      const scenario = arguments[0], revision = arguments[1];
      const descriptor = Object.getOwnPropertyDescriptor(callbacks, 'get');
      const originalGet = callbacks.get;
      const gate = { prepareHeld: false, nullHeld: false };
      gate.restore = () => descriptor
        ? Object.defineProperty(callbacks, 'get', descriptor) : Reflect.deleteProperty(callbacks, 'get');
      window.__alternativesSmokeGate = gate;
      Object.defineProperty(callbacks, 'get', { configurable: true, value: function(id) {
        const callback = originalGet.call(this, id);
        if (!callback) return callback;
        return data => {
          const result = data?.result;
          if (!gate.prepareHeld && result?.schemaVersion === 1 && result.operationId &&
            Object.keys(result).length === 2) {
            gate.prepareHeld = true; gate.releasePrepare = () => callback(data); return;
          }
          if (gate.prepareHeld && !gate.nullHeld && result?.group === null &&
            result.scenarioId === scenario && result.scenarioRevision === revision) {
            gate.nullHeld = true; gate.releaseNull = () => callback(data); return;
          }
          callback(data);
        };
      }});`,
      [scenarioId, revision],
    );
    await activateButton(sessionId, "Run Alternatives", panel);
    await waitFor(sessionId, "return window.__alternativesSmokeGate?.prepareHeld === true;");
    await waitFor(
      sessionId,
      "return window.__alternativesSmokeGate?.nullHeld === true;",
      [],
      10_000,
    );
    await evaluate(sessionId, "window.__alternativesSmokeGate.releasePrepare();");
    await waitFor(
      sessionId,
      `const status = document.querySelector('[aria-label="Native Alternatives group status"]');
      return status?.textContent.includes('Settled') && status.textContent.includes('completed');`,
      [],
      60_000,
    );
    const retained = (
      await nativeRequest(sessionId, "alternatives_get_group", {
        schemaVersion: 1,
        scenarioId,
        expectedRevision: revision,
      })
    ).result.group;
    assert(retained?.settled);
    assert.equal(retained.snapshot.status, "completed");
    assert.deepEqual(
      retained.snapshot.members.map((member) => member.profile),
      ["balanced", "preferenceFocused", "minimalChanges", "fairnessFirst"],
    );
    assert.equal(new Set(retained.snapshot.members.map((member) => member.runId)).size, 4);
    for (const member of retained.snapshot.members) {
      assert.equal(member.outcome.type, "accepted");
      const verified = (
        await nativeRequest(sessionId, "solution_verify", {
          schemaVersion: 1,
          scenarioId,
          solutionId: member.outcome.solutionId,
        })
      ).result;
      assert.equal(verified.verification.accepted, true);
      assert.equal(verified.result.resultChecksum, member.outcome.acceptedResultChecksum);
      assert.equal(verified.verification.checksum, member.outcome.verificationChecksum);
      const run = (
        await nativeRequest(sessionId, "solve_get_job", {
          schemaVersion: 2,
          scenarioId,
          runId: member.runId,
        })
      ).result;
      assert.equal(run.requestId, member.requestId);
      assert.equal(run.startedAt, retained.snapshot.startedAt);
      assert.equal(run.scenarioRevision, revision);
    }
    const suggestions = retained.snapshot.members.filter(
      (member) => member.equivalence.kind !== "equivalent",
    );
    assert.deepEqual(
      suggestions.map((member) => member.profile),
      ["balanced", "minimalChanges"],
    );
    await evaluate(
      sessionId,
      "window.__alternativesSmokeGate.releaseNull(); window.__alternativesSmokeGate.restore();",
    );
    await nativeRequest(sessionId, "alternatives_get_group", {
      schemaVersion: 1,
      scenarioId,
      expectedRevision: revision,
    });
    assert(
      await evaluate(
        sessionId,
        `return document.querySelector(arguments[0])?.textContent.includes(arguments[1]) &&
      document.querySelectorAll('[data-alternative-profile]').length === 4;`,
        [panel, retained.requestId],
      ),
      "A delayed native null discovery must not erase the completed accepted group",
    );
    await evaluate(
      sessionId,
      "document.querySelector(arguments[0]).scrollIntoView({block:'start'});",
      [panel],
    );
    await screenshot(sessionId, "packaged-alternatives-profiles.png");
    const focusAfterRun = await focusedControl("Run Alternatives");

    const beforeRemount = (
      await nativeRequest(sessionId, "solve_list_runs", {
        schemaVersion: 2,
        scenarioId,
        revision,
        limit: 8,
      })
    ).result.runs
      .map((run) => run.runId)
      .sort();
    assert.equal(beforeRemount.length, 4);
    await navigate(sessionId, "/projects");
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await chooseLabel("Next solve intent", "alternatives");
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0])?.textContent.includes(arguments[1]);",
      [panel, retained.requestId],
    );
    assert.deepEqual(
      (
        await nativeRequest(sessionId, "solve_list_runs", {
          schemaVersion: 2,
          scenarioId,
          revision,
          limit: 8,
        })
      ).result.runs
        .map((run) => run.runId)
        .sort(),
      beforeRemount,
      "Remount discovery must not replay or continue the group",
    );
    await chooseLabel("First accepted alternative", "balanced", panel);
    await chooseLabel("Second accepted alternative", "minimalChanges", panel);
    await activateButton(sessionId, "Compare selected Alternatives", panel);
    await waitFor(
      sessionId,
      `return [...document.querySelectorAll('h2')].some(node => node.textContent.trim() === 'Accepted result comparison') &&
      document.body.textContent.includes(arguments[0]) && document.body.textContent.includes(arguments[1]);`,
      suggestions.map((member) => member.outcome.solutionId),
      60_000,
    );
    await evaluate(
      sessionId,
      `const heading = [...document.querySelectorAll('h2')].find(node => node.textContent.trim() === 'Accepted result comparison');
      heading.scrollIntoView({block:'start'});`,
    );
    await screenshot(sessionId, "packaged-alternatives-exact-comparison.png");
    const focusAfterComparison = await focusedControl("Compare selected Alternatives");
    await activateButton(sessionId, "View Balanced accepted alternative", panel);
    await waitForElement(sessionId, "[data-accepted-schedule] table tbody tr", 60_000);
    const focusAfterView = await focusedControl("View Balanced accepted alternative");
    assert.equal(
      (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
      revision,
    );
    assert.deepEqual(
      await appliedHistoryHead(sessionId, scenarioId),
      head,
      "Alternatives, native verification, exact comparison and viewing must leave authored source/History unchanged",
    );
    const focus = { focusAfterRun, focusAfterComparison, focusAfterView };
    assert(
      Object.values(focus).every((observation) => observation.usable),
      `Keyboard activation must retain a usable focus target: ${JSON.stringify(focus)}`,
    );
    const lifecycleRequestId = requestId();
    const lifecycle = await command(
      "POST",
      `/session/${encodeURIComponent(sessionId)}/execute/async`,
      {
        script: `const [scenarioId, revision, requestId, controlId, cancelRequestId] = arguments;
          const done = arguments[arguments.length - 1];
          const invoke = (name, request) => window.__TAURI_INTERNALS__.invoke(name, {request});
          (async () => {
            const prepared = await invoke('operation_prepare', {
              schemaVersion: 1, requestId, purpose: {kind: 'solveAlternatives'},
              context: {kind: 'scenario', scenarioId, expectedRevision: revision}
            });
            const operationId = prepared.result.operationId;
            const running = invoke('alternatives_start', {
              schemaVersion: 1, requestId, operationId, scenarioId,
              expectedRevision: revision, mode: 'deep', maximumTimeMilliseconds: 30000
            }).then(result => ({result}), error => ({error}));
            const query = () => invoke('alternatives_get_group', {
              schemaVersion: 1, requestId: controlId, scenarioId,
              expectedRevision: revision, groupRequestId: requestId
            });
            let admitted;
            for (let attempt = 0; attempt < 100; attempt += 1) {
              admitted = (await query()).result.group;
              if (admitted) break;
              await new Promise(resolve => setTimeout(resolve, 1));
            }
            if (!admitted || admitted.settled) throw new Error('Expected a live admitted Alternatives owner');
            await invoke('operation_release', {schemaVersion: 1, requestId: controlId, operationId});
            const detached = (await query()).result.group;
            if (!detached || detached.settled || detached.cancelOperationId !== operationId)
              throw new Error('Release must retain the live native owner');
            await invoke('solve_cancel', {
              schemaVersion: 2, cancelRequestId, operationId, scenarioId, expectedRevision: revision
            });
            await running;
            return (await query()).result.group;
          })().then(result => done({result}), error => done({failure: String(error)}));`,
        args: [scenarioId, revision, lifecycleRequestId, requestId(), requestId()],
      },
      60_000,
    );
    assert.equal(lifecycle.failure, undefined);
    assert.equal(lifecycle.result.settled, true);
    assert.equal(lifecycle.result.snapshot.stopReason, "cancelled");
    assert(
      lifecycle.result.snapshot.members.some((member) => member.status === "skipped"),
      "Explicit cancellation must prevent later profile dispatch",
    );
    await navigate(sessionId, "/projects");
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await chooseLabel("Next solve intent", "alternatives");
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0])?.textContent.includes(arguments[1]);",
      [panel, lifecycleRequestId],
    );
    for (const member of retained.snapshot.members) {
      const verified = await nativeRequest(sessionId, "solution_verify", {
        schemaVersion: 1,
        scenarioId,
        solutionId: member.outcome.solutionId,
      });
      assert.equal(verified.result.verification.accepted, true);
      assert.equal(verified.result.result.resultChecksum, member.outcome.acceptedResultChecksum);
    }
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), head);
    console.log(
      "PASS: actual packaged four-profile Alternatives, explicit-base refusal, real stale-null response race, independent verification, exact deduplication, remount without replay, chosen-pair comparison and read-only viewing",
    );
  } finally {
    await evaluate(
      sessionId,
      `const gate = window.__alternativesSmokeGate;
      gate?.releasePrepare?.(); gate?.releaseNull?.(); gate?.restore?.();
      delete window.__alternativesSmokeGate;`,
    ).catch(() => undefined);
    await navigate(sessionId, "/projects").catch(() => undefined);
    await package10DeleteFixture(sessionId, scenarioId);
  }
}

async function packagedConditionalCopyAcceptance(sessionId) {
  const fixtureTitle = `EXPLAIN-001 conditional pin fixture ${Date.now()}`;
  const incumbentName = "EXPLAIN-001 conditional incumbent";
  const targetName = "EXPLAIN-001 conditional target";
  const existingProjectIds = new Set(
    (await projects(sessionId)).map((project) => project.scenarioId),
  );
  let scenarioId = null;
  let copyId = null;
  try {
    const created = await nativeRequest(sessionId, "project_create", {
      schemaVersion: 1,
      title: fixtureTitle,
      description: "Real-worker replacement-question diagnostic-copy fixture",
      domainPack: { id: "official.workforce", schemaVersion: 1 },
      settings: {
        timeZone: "UTC",
        locale: "en-US",
        units: "metric",
        firstDate: "2030-01-01",
        lastDate: "2030-01-01",
        gapPolicy: "reject",
        overlapPolicy: "reject",
      },
    });
    scenarioId = created.result.scenarioId;
    assert.match(scenarioId, /^[a-f0-9-]{36}$/u);
    assert(
      !existingProjectIds.has(scenarioId),
      "The accepted conditional source must be a fresh project",
    );

    const scorePolicyId = requestId();
    const assignmentTypeId = requestId();
    const incumbentPersonId = requestId();
    const targetPersonId = requestId();
    const shiftId = requestId();
    const targetMustRemainUnassignedRuleId = requestId();
    const coverageRuleId = requestId();
    const entities = [
      {
        id: scorePolicyId,
        kind: "scorePolicy",
        profileKey: "explain-conditional-fixture",
        levels: [{ levelKey: "preferences", label: "Conditional fixture preferences" }],
        priorityMapping: ["low", "normal", "high", "veryHigh"].map((priority) => ({
          priority,
          levelKey: "preferences",
          scale: 1,
        })),
        workloadPolicies: {},
        tieBreak: "stableAssignmentRank",
      },
      {
        id: assignmentTypeId,
        kind: "assignmentType",
        name: "EXPLAIN-001 clinic duty",
        category: "clinic",
        timeBehavior: "elapsed",
        defaultDurationMinutes: 60,
        qualifications: { kind: "unconstrained" },
        locationBehavior: { kind: "optional" },
        workloadBucketIds: [],
      },
      ...[
        [incumbentPersonId, incumbentName],
        [targetPersonId, targetName],
      ].map(([id, name]) => ({
        id,
        kind: "person",
        name,
        activeRange: { kind: "always" },
        qualificationGrants: [],
        eligibleAssignmentTypeIds: [assignmentTypeId],
        teamIds: [],
        tags: [],
        workloadWeight: { numerator: 1, denominator: 1 },
      })),
      {
        id: shiftId,
        kind: "shiftInstance",
        assignmentTypeId,
        startsAt: {
          instant: "2030-01-01T09:00:00Z",
          local: "2030-01-01T09:00:00",
          offsetSeconds: 0,
        },
        endsAt: {
          instant: "2030-01-01T10:00:00Z",
          local: "2030-01-01T10:00:00",
          offsetSeconds: 0,
        },
        coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
        tags: [],
        reportingAttribution: "startLocalDate",
        origin: { kind: "manual" },
      },
    ];
    const requiredRules = [
      {
        id: targetMustRemainUnassignedRuleId,
        kind: "fixedAssignment",
        active: true,
        strength: "required",
        scope: { people: { kind: "all" } },
        personId: targetPersonId,
        shiftId,
        assigned: false,
      },
      {
        id: coverageRuleId,
        kind: "coverage",
        active: true,
        strength: "required",
        scope: { people: { kind: "all" } },
      },
    ];
    const authored = await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: created.result.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Native EXPLAIN-001 conditional fixture" },
      truncateRedo: false,
      command: {
        type: "applyBatch",
        payload: {
          label: "Native EXPLAIN-001 feasible source with a conditional fixed-assignment conflict",
          commands: [
            ...entities.map((entity) => ({
              type: "applyDomainCommand",
              payload: { commandType: "official.workforce.add_entity", payload: { entity } },
            })),
            ...requiredRules.map((rule) => ({
              type: "applyDomainCommand",
              payload: { commandType: "official.workforce.add_rule", payload: { rule } },
            })),
          ],
        },
      },
    });
    const sourceRevision = authored.result.newRevision;
    const sourceHistoryHead = await appliedHistoryHead(sessionId, scenarioId);
    assert(sourceHistoryHead, "The authored conditional source must have a native journal head");

    await navigate(sessionId, "/projects");
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await waitForElement(
      sessionId,
      `[aria-label=${JSON.stringify(`Open project ${fixtureTitle}`)}]`,
    );
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await waitForElement(sessionId, "#setup-calendar");
    assert.equal(await getText(sessionId, "#project-heading"), fixtureTitle);
    const openedSource = (await projects(sessionId)).find(
      (project) => project.scenarioId === scenarioId,
    );
    assert.equal(openedSource.domainPackId, "official.workforce");
    assert.equal(typeof openedSource.lastOpenedAt, "string");

    const requiredViewId = "official.workforce.setup.rule_page";
    const readRequiredRules = async (id) => {
      const page = (
        await nativeSetupRequest(sessionId, id, "scenario_get_view", requiredViewId, {
          source: { kind: "stored" },
          query: {
            schemaVersion: 1,
            viewId: requiredViewId,
            parameters: { class: "required", limit: 200 },
          },
        })
      ).result.view.data.result.data;
      const records = [];
      for (const item of page.items) {
        const detail = (
          await nativeSetupRequest(
            sessionId,
            id,
            "scenario_get_view",
            "official.workforce.setup.rule_detail",
            {
              source: { kind: "stored" },
              query: {
                schemaVersion: 1,
                viewId: "official.workforce.setup.rule_detail",
                parameters: { rule: item.rule },
              },
            },
          )
        ).result.view.data.result.data;
        records.push(detail.record);
      }
      return records.sort((left, right) => left.id.localeCompare(right.id));
    };
    const readEntityPage = async (id, kind) =>
      (
        await nativeSetupRequest(
          sessionId,
          id,
          "scenario_search_entities",
          "eutheto.setup.entity_page",
          {
            kind,
            search: "",
            limit: 50,
            cursor: null,
          },
        )
      ).result.view.data.result.data;
    const sourceRulesBeforeCopy = await readRequiredRules(scenarioId);
    assert.deepEqual(
      sourceRulesBeforeCopy,
      requiredRules.toSorted((left, right) => left.id.localeCompare(right.id)),
    );
    const scorePolicies = await readEntityPage(scenarioId, "scorePolicy");
    assert.equal(scorePolicies.items.length, 1);
    assert.equal(scorePolicies.items[0].entityId, scorePolicyId);

    await navigate(sessionId, `/project/${scenarioId}/validation`);
    await waitForElement(sessionId, "#validation-heading");
    await waitFor(
      sessionId,
      "return document.querySelector('[data-full-validation-state=\"notRun\"]');",
    );
    await activateButton(sessionId, "Run full validation");
    await waitFor(
      sessionId,
      "return document.querySelector('[data-full-validation-state=\"completed\"]');",
      [],
      120_000,
    );
    await navigate(sessionId, `/project/${scenarioId}/setup`);
    await waitForElement(sessionId, "#setup-calendar");

    if (
      !(await evaluate(sessionId, "return document.querySelector('#setup-results-details')?.open;"))
    )
      await activate(sessionId, "#setup-results-details > summary");
    await waitForElement(sessionId, '[data-optimize-state="ready"]');
    await activate(sessionId, '[data-optimize-panel] input[name="optimize-mode"][value="deep"]');
    await activateButton(sessionId, "Start optimization", "[data-optimize-panel]");
    await waitFor(
      sessionId,
      `return [...document.querySelectorAll('[data-optimize-panel] dt')].some(
        node => node.textContent === 'Solve status' &&
          ['Optimal', 'Feasible', 'Infeasible', 'No solution within the mode limit',
           'Backend failed', 'Verification alarm'].includes(node.nextElementSibling?.textContent?.trim())
      );`,
      [],
      60_000,
    );
    const runs = (
      await nativeRequest(sessionId, "solve_list_runs", {
        schemaVersion: 2,
        scenarioId,
        revision: sourceRevision,
        limit: 2,
      })
    ).result.runs;
    assert.equal(runs.length, 1, "The source schedule must come from one real bundled-worker run");
    const [run] = runs;
    assert(["optimal", "feasible"].includes(run.status), `Conditional source solve: ${run.status}`);
    assert.equal(run.scenarioRevision, sourceRevision);
    assert.match(run.solutionId, /^[a-f0-9-]{36}$/u);
    const verified = (
      await nativeRequest(sessionId, "solution_verify", {
        schemaVersion: 1,
        scenarioId,
        solutionId: run.solutionId,
      })
    ).result;
    assert.equal(verified.verification.accepted, true);
    assert(
      verified.verification.requiredRuleResults.every((rule) => rule.satisfied),
      "The source solution must independently satisfy the authored Required group",
    );
    const timeline = (
      await nativeRequest(sessionId, "solution_get_view", {
        schemaVersion: 1,
        scenarioId,
        solutionId: run.solutionId,
        viewId: "official.workforce.result.schedule",
        parameters: { kind: "timeline", limit: 50 },
        continuation: null,
      })
    ).result.view.data.result;
    assert.equal(timeline.kind, "timeline");
    assert.equal(timeline.items.length, 1);
    const incumbent = timeline.items[0];
    assert.equal(incumbent.personId, incumbentPersonId);
    assert.equal(incumbent.shiftId, shiftId);
    const replacementDecision = (
      await nativeRequest(sessionId, "solution_get_view", {
        schemaVersion: 1,
        scenarioId,
        solutionId: run.solutionId,
        viewId: "official.workforce.result.schedule",
        parameters: {
          kind: "decision",
          personId: targetPersonId,
          shiftId: incumbent.shiftId,
        },
        continuation: null,
      })
    ).result.view.data.result;
    assert.equal(replacementDecision.kind, "decision");
    assert.equal(replacementDecision.personId, targetPersonId);
    assert.equal(replacementDecision.shiftId, incumbent.shiftId);
    assert.equal(replacementDecision.selected, false);
    assert.equal(typeof replacementDecision.assignmentId, "string");

    await waitForElement(sessionId, "#setup-results-details button", 30_000);
    await activateButton(sessionId, "Inspect schedule", "#setup-results-details");
    await waitForElement(sessionId, "[data-accepted-schedule]");
    await activate(sessionId, '[data-accepted-schedule] input[type="radio"][value="timeline"]');
    await waitFor(
      sessionId,
      `return [...document.querySelectorAll('[data-accepted-schedule] table')]
        .some(table => table.caption?.textContent.trim() === 'Timeline by start time' &&
          table.tBodies[0]?.rows.length === 1);`,
    );
    const assignmentInspect = await waitFor(
      sessionId,
      `return [...document.querySelectorAll('[data-accepted-schedule] table')]
        .find(table => table.caption?.textContent.trim() === 'Timeline by start time')
        ?.tBodies[0]?.rows[0]?.querySelector('button') ?? null;`,
    );
    await activateElement(sessionId, assignmentInspect);
    await waitForElement(
      sessionId,
      '[data-accepted-counterfactual] select[name="counterfactual-person"]',
      30_000,
    );
    const beforeQuestions = (
      await nativeRequest(sessionId, "solution_list_counterfactuals", {
        schemaVersion: 1,
        scenarioId,
        before: null,
        limit: 8,
      })
    ).result.page.jobs;
    assert.equal(beforeQuestions.length, 0);
    await selectValue(
      sessionId,
      '[data-accepted-counterfactual] select[name="counterfactual-person"]',
      targetPersonId,
    );
    await activateButton(
      sessionId,
      "Check replacement (up to 30 seconds)",
      "[data-accepted-counterfactual]",
    );
    const counterfactual = await waitForSavedCounterfactual(
      sessionId,
      scenarioId,
      (entry) =>
        entry.baseSolutionId === run.solutionId &&
        entry.condition.condition.type === "replaceAssignment" &&
        entry.condition.condition.targetAssignmentId === replacementDecision.assignmentId &&
        entry.condition.condition.replacedAssignmentId === incumbent.assignmentId,
    );
    assert(counterfactual);
    assert.equal(counterfactual.state, "completed");
    const savedQuestions = (
      await nativeRequest(sessionId, "solution_list_counterfactuals", {
        schemaVersion: 1,
        scenarioId,
        before: null,
        limit: 8,
      })
    ).result.page.jobs;
    assert.equal(savedQuestions.length, beforeQuestions.length + 1);
    const recorded = (
      await nativeRequest(sessionId, "solution_get_counterfactual", {
        schemaVersion: 1,
        scenarioId,
        jobId: counterfactual.jobId,
      })
    ).result.job;
    assert.equal(recorded.request.semantics.base.solutionId, run.solutionId);
    assert.equal(recorded.request.condition.condition.type, "replaceAssignment");
    assert.equal(
      recorded.request.condition.condition.targetAssignmentId,
      replacementDecision.assignmentId,
    );
    assert.equal(recorded.request.condition.condition.replacedAssignmentId, incumbent.assignmentId);
    assert.equal(recorded.result.request.condition.checksum, recorded.request.condition.checksum);
    assert.equal(recorded.result.conclusion.type, "provenImpossible");
    assert.equal(recorded.result.runManifest.outcome.type, "noResult");
    assert.equal(recorded.result.runManifest.outcome.status, "infeasible");
    assert.equal(
      recorded.result.runInput.temporaryConditionHash,
      recorded.request.condition.checksum,
    );
    await waitFor(
      sessionId,
      `const panel = document.querySelector('[data-accepted-counterfactual]');
       return panel?.textContent.includes(
         'The saved diagnostic proved the requested temporary condition impossible'
       ) && panel.textContent.includes(arguments[0]) && panel.textContent.includes(arguments[1]);`,
      [replacementDecision.assignmentId, incumbent.assignmentId],
      90_000,
    );
    await activateButton(
      sessionId,
      "Inspect saved conflict evidence",
      "[data-accepted-counterfactual]",
    );
    await waitForElement(
      sessionId,
      '[data-infeasibility-inspection] ol[aria-label="Mapped Required-rule conflict groups"]',
    );
    const conditionCaption = await getText(sessionId, "[data-infeasibility-condition]");
    assert(
      conditionCaption.includes(
        `Set target assignment ${replacementDecision.assignmentId} to Boolean true and replaced assignment ${incumbent.assignmentId} to Boolean false`,
      ),
      "The inspector must show both signed pins from the saved replacement question",
    );
    const mapped = await evaluate(
      sessionId,
      `const group = document.querySelector(
        '[data-infeasibility-inspection] ol[aria-label="Mapped Required-rule conflict groups"] li'
      );
      const caption = [...(group?.children ?? [])].find(node =>
        node.tagName === 'P' && node.getAttribute('role') !== 'heading'
      )?.textContent.trim();
      return {
        groupText: group?.textContent ?? '',
        caption,
        ruleIds: [...(group?.querySelectorAll('ul[aria-label^="Required source rules"] code') ?? [])]
          .map(node => node.textContent.trim()),
        action: [...(group?.querySelectorAll('button') ?? [])]
          .find(button => button.textContent.includes('Review a new diagnostic copy')) ?? null
      };`,
    );
    const inspectionText = await getText(sessionId, "[data-infeasibility-inspection]");
    assert(
      inspectionText.includes("Sufficient conflict") ||
        inspectionText.includes("Proven minimal conflict"),
      "The mapped Required group must come from sufficient infeasibility evidence",
    );
    await screenshot(sessionId, "explain-001-native-conditional-infeasibility.png");
    assert(mapped.groupText.includes(targetMustRemainUnassignedRuleId));
    assert.deepEqual(mapped.ruleIds, [targetMustRemainUnassignedRuleId]);
    assert.equal(typeof mapped.caption, "string");
    assert(mapped.caption.length > 0);
    assert(mapped.action);
    await evaluate(
      sessionId,
      `document.querySelector(
        '[data-infeasibility-inspection] ol[aria-label="Mapped Required-rule conflict groups"] li'
      ).id = 'conditional-copy-group';`,
    );
    const libraryBeforeCopy = (await settingsSnapshot(sessionId)).libraryRevision;
    const reviewSourceRunId = recorded.result.runInput.runId;
    const reviewManifestChecksum = recorded.result.runManifest.checksum;
    await activateButton(sessionId, "Review a new diagnostic copy", "#conditional-copy-group");
    await waitForElement(sessionId, "[data-diagnostic-copy-review]");
    const review = await evaluate(
      sessionId,
      `const root = document.querySelector('[data-diagnostic-copy-review]');
       const rows = [...root.querySelectorAll('dl > div')];
       const value = label => rows.find(row => row.querySelector('dt')?.textContent.trim() === label)
         ?.querySelector('dd')?.innerText.trim() ?? null;
       const rulesRow = rows.find(row =>
         row.querySelector('dt')?.textContent.includes('Required rule UUIDs relaxed only in the copy')
       );
       return {
         scenarioId: value('Original scenario ID'),
         sourceRevision: Number(value('Recorded source revision')),
         currentRevision: Number(value('Observed current revision')),
         runId: value('Recorded run ID'),
         manifestChecksum: value('Run manifest checksum'),
         evidenceChecksum: value('Conflict evidence checksum'),
         conflictId: value('Conflict ID'),
         group: value('Selected mapped source group'),
         sourceCaption: value('Exact source caption'),
         expectedLibraryRevision: Number(value('Current project-library revision reviewed')),
         ruleIds: [...(rulesRow?.querySelectorAll('code') ?? [])].map(node => node.textContent.trim())
       };`,
    );
    assert.equal(review.scenarioId, scenarioId);
    assert.equal(review.sourceRevision, sourceRevision);
    assert.equal(review.currentRevision, sourceRevision);
    assert.equal(review.runId, reviewSourceRunId);
    assert.equal(review.manifestChecksum, reviewManifestChecksum);
    assert.match(review.evidenceChecksum, /^[a-f0-9]{64}$/u);
    assert.equal(review.conflictId, `infeasibility.${review.evidenceChecksum}`);
    assert(review.group.includes("official.workforce.rule."));
    assert.equal(review.sourceCaption, mapped.caption);
    assert.deepEqual(review.ruleIds, [targetMustRemainUnassignedRuleId]);
    assert.equal(review.expectedLibraryRevision, libraryBeforeCopy);
    await activateButton(
      sessionId,
      "Create a new copy with this group relaxed",
      "[data-diagnostic-copy-review]",
    );
    await waitForElement(sessionId, "[data-diagnostic-copy-receipt]");
    await waitFor(
      sessionId,
      `const receipt = document.querySelector('[data-diagnostic-copy-receipt]');
       const heading = receipt?.querySelector('[role="heading"]');
       return heading !== null && document.activeElement === heading;`,
    );
    await screenshot(sessionId, "explain-001-native-conditional-copy-receipt.png");
    copyId = await evaluate(
      sessionId,
      `const identities = document.querySelector('[data-diagnostic-copy-receipt]')
         ?.querySelectorAll('code') ?? [];
       return identities[identities.length - 1]?.textContent.trim() ?? null;`,
    );
    assert.match(copyId ?? "", /^[a-f0-9-]{36}$/u);
    assert.notEqual(copyId, scenarioId);
    assert(
      !existingProjectIds.has(copyId),
      "The diagnostic copy must have a fresh project identity",
    );
    const copyProject = (await projects(sessionId)).find(
      (project) => project.scenarioId === copyId,
    );
    assert(copyProject, "The native conditional diagnostic copy must exist as a fresh project");
    assert.equal(
      (await settingsSnapshot(sessionId)).libraryRevision,
      libraryBeforeCopy + 1,
      "The conditional copy receipt must commit against the exact reviewed library revision",
    );
    const sourceAfterCopy = (await projects(sessionId)).find(
      (project) => project.scenarioId === scenarioId,
    );
    assert.equal(sourceAfterCopy.revision, sourceRevision);
    assert.deepEqual(await readRequiredRules(scenarioId), sourceRulesBeforeCopy);
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), sourceHistoryHead);

    await navigate(sessionId, "/projects");
    await command("POST", `/session/${encodeURIComponent(sessionId)}/refresh`, {});
    await waitForElement(
      sessionId,
      `[aria-label=${JSON.stringify(`Open project ${copyProject.title}`)}]`,
    );
    await navigate(sessionId, `/project/${copyId}/setup`);
    await waitForElement(sessionId, "#setup-calendar");
    assert.equal(await getText(sessionId, "#project-heading"), copyProject.title);
    const copyPersons = (await readEntityPage(copyId, "person")).items;
    const copyTarget = copyPersons.find((person) => person.name === targetName);
    const copyIncumbent = copyPersons.find((person) => person.name === incumbentName);
    assert(copyTarget && copyIncumbent);
    const copyShifts = (await readEntityPage(copyId, "shiftInstance")).items;
    assert.equal(copyShifts.length, 1);
    const copyShiftId = copyShifts[0].entityId;
    const copyRulesBeforeUndo = await readRequiredRules(copyId);
    assert.equal(copyRulesBeforeUndo.length, 4);
    const relaxedRule = copyRulesBeforeUndo.find(
      (rule) =>
        rule.kind === "fixedAssignment" &&
        rule.active === false &&
        rule.personId === copyTarget.entityId &&
        rule.shiftId === copyShiftId &&
        rule.assigned === false,
    );
    const targetPin = copyRulesBeforeUndo.find(
      (rule) =>
        rule.kind === "fixedAssignment" &&
        rule.active === true &&
        rule.personId === copyTarget.entityId &&
        rule.shiftId === copyShiftId &&
        rule.assigned === undefined,
    );
    const incumbentPin = copyRulesBeforeUndo.find(
      (rule) =>
        rule.kind === "fixedAssignment" &&
        rule.active === true &&
        rule.personId === copyIncumbent.entityId &&
        rule.shiftId === copyShiftId &&
        rule.assigned === false,
    );
    assert(relaxedRule, "Only the selected authored Required group must be relaxed in the copy");
    assert(targetPin, "The replacement target must be explicitly pinned true in the copy");
    assert(incumbentPin, "The replaced incumbent must be explicitly pinned false in the copy");
    const copiedCoverageRule = copyRulesBeforeUndo.find((rule) => rule.kind === "coverage");
    assert(copiedCoverageRule?.active, "Unselected Required coverage must stay active in the copy");
    assert.equal(targetPin.shiftId, incumbentPin.shiftId);
    assert.equal(targetPin.shiftId, copyShiftId);
    assert.equal(targetPin.strength, "required");
    assert.equal(incumbentPin.strength, "required");
    assert.equal(
      copyRulesBeforeUndo.filter((rule) => !rule.active).length,
      1,
      "The copy may relax only the one selected mapped authored Required rule",
    );
    assert.deepEqual(
      {
        ...relaxedRule,
        id: targetMustRemainUnassignedRuleId,
        personId: targetPersonId,
        shiftId,
        active: true,
      },
      sourceRulesBeforeCopy.find((rule) => rule.id === targetMustRemainUnassignedRuleId),
      "The relaxed rule must be the remapped source rule with no other semantic change",
    );
    const sourceBeforeUndo = (await projects(sessionId)).find(
      (project) => project.scenarioId === scenarioId,
    );
    assert.equal(sourceBeforeUndo.revision, sourceRevision);
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), sourceHistoryHead);

    const copyHistoryBefore = (
      await nativeRequest(sessionId, "scenario_get_history_page", {
        schemaVersion: 1,
        scenarioId: copyId,
        expectedRevision: copyProject.revision,
        limit: 50,
        continuation: null,
      })
    ).result;
    assert.equal(copyHistoryBefore.entries.length, 1);
    assert.equal(copyHistoryBefore.entries[0].applied, true);
    assert.equal(copyHistoryBefore.undoAvailable, true);
    await navigate(sessionId, `/project/${copyId}/history`);
    await waitForElement(sessionId, "#history-heading");
    await activateButton(sessionId, "Undo scenario change", '[aria-labelledby="history-heading"]');
    await waitForOutcome(sessionId, "Scenario undo committed");
    const copyAfterUndo = (await projects(sessionId)).find(
      (project) => project.scenarioId === copyId,
    );
    assert.equal(copyAfterUndo.revision, copyProject.revision + 1);
    const copyHistoryAfterUndo = (
      await nativeRequest(sessionId, "scenario_get_history_page", {
        schemaVersion: 1,
        scenarioId: copyId,
        expectedRevision: copyAfterUndo.revision,
        limit: 50,
        continuation: null,
      })
    ).result;
    assert.equal(copyHistoryAfterUndo.entries.length, 1);
    assert.equal(copyHistoryAfterUndo.entries[0].id, copyHistoryBefore.entries[0].id);
    assert.equal(copyHistoryAfterUndo.entries[0].applied, false);
    assert.equal(copyHistoryAfterUndo.redoAvailable, true);
    const copyRulesAfterUndo = await readRequiredRules(copyId);
    assert.deepEqual(
      copyRulesAfterUndo,
      [{ ...relaxedRule, active: true }, copiedCoverageRule].toSorted((left, right) =>
        left.id.localeCompare(right.id),
      ),
    );
    assert.equal(
      await evaluate(
        sessionId,
        `return [...document.querySelectorAll('[aria-labelledby="history-heading"] button')]
          .find(button => button.textContent.trim() === 'Redo scenario change')?.disabled;`,
      ),
      false,
      "One UI Undo must expose the matching UI Redo",
    );

    await activateButton(sessionId, "Redo scenario change", '[aria-labelledby="history-heading"]');
    await waitForOutcome(sessionId, "Scenario redo committed");
    const copyAfterRedo = (await projects(sessionId)).find(
      (project) => project.scenarioId === copyId,
    );
    assert.equal(copyAfterRedo.revision, copyAfterUndo.revision + 1);
    const copyHistoryAfterRedo = (
      await nativeRequest(sessionId, "scenario_get_history_page", {
        schemaVersion: 1,
        scenarioId: copyId,
        expectedRevision: copyAfterRedo.revision,
        limit: 50,
        continuation: null,
      })
    ).result;
    assert.equal(copyHistoryAfterRedo.entries.length, 1);
    assert.equal(copyHistoryAfterRedo.entries[0].id, copyHistoryBefore.entries[0].id);
    assert.equal(copyHistoryAfterRedo.entries[0].applied, true);
    assert.equal(copyHistoryAfterRedo.redoAvailable, false);
    assert.deepEqual(await readRequiredRules(copyId), copyRulesBeforeUndo);
    const sourceAfterRedo = (await projects(sessionId)).find(
      (project) => project.scenarioId === scenarioId,
    );
    assert.equal(sourceAfterRedo.revision, sourceRevision);
    assert.deepEqual(await readRequiredRules(scenarioId), sourceRulesBeforeCopy);
    assert.deepEqual(await appliedHistoryHead(sessionId, scenarioId), sourceHistoryHead);
    console.log(
      "PASS: real bundled-worker accepted source → impossible recorded replacement with exact target-true/incumbent-false pins → conditional mapped diagnostic copy → one journal command, UI Undo and Redo",
    );
  } finally {
    try {
      await navigate(sessionId, "/projects");
    } catch {}
    const copyCleanupIds = new Set(copyId === null ? [] : [copyId]);
    try {
      for (const project of await projects(sessionId)) {
        if (!existingProjectIds.has(project.scenarioId) && project.scenarioId !== scenarioId)
          copyCleanupIds.add(project.scenarioId);
      }
    } catch {}
    try {
      for (const id of copyCleanupIds) await package10DeleteFixture(sessionId, id);
    } finally {
      if (scenarioId !== null) await package10DeleteFixture(sessionId, scenarioId);
    }
  }
}

async function packagedBasePreferenceAcceptance(sessionId, scenarioId) {
  const page = (
    await nativeSetupRequest(
      sessionId,
      scenarioId,
      "scenario_search_entities",
      "eutheto.setup.entity_page",
      { kind: "baseSchedule", search: "", limit: 50, cursor: null },
    )
  ).result.view.data.result.data;
  assert.equal(page.items.length, 1, "The accepted result must have saved one real base schedule");
  const baseId = page.items[0].entityId;
  assert.equal(page.items[0].kind, "baseSchedule");
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  const editor = '[aria-labelledby="rule-editor-heading"]';
  for (const { title, kind, picker } of [
    {
      title: "Adjacency preferences",
      kind: "adjacency",
      picker: "#rule-adjacency-base-schedule",
    },
    {
      title: "Base-schedule stability",
      kind: "baseStability",
      picker: "#rule-base-stability-base-schedule",
    },
  ]) {
    await openPersonOptions(sessionId, "Preferences", "body");
    const create = await waitFor(
      sessionId,
      "return [...document.querySelectorAll('button')].find(button => button.textContent.trim() === arguments[0] && !button.disabled) ?? null;",
      [title],
    );
    await activateElement(sessionId, create);
    const id = await waitFor(
      sessionId,
      "return document.querySelector('#rule-identity')?.textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0] ?? null;",
    );
    await setValue(sessionId, picker, "");
    await waitFor(
      sessionId,
      "return [...document.querySelectorAll('[role=\"option\"]')].some(option => option.textContent.includes(arguments[0]));",
      [baseId],
    );
    await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
      actions: [
        {
          type: "key",
          id: "base-schedule-reference",
          actions: [
            { type: "keyDown", value: "\uE015" },
            { type: "keyUp", value: "\uE015" },
            { type: "keyDown", value: "\uE007" },
            { type: "keyUp", value: "\uE007" },
          ],
        },
      ],
    });
    await waitFor(
      sessionId,
      "return document.querySelector(arguments[0] + '-selection')?.textContent.includes(arguments[1]);",
      [picker, baseId],
    );
    if (kind === "adjacency") {
      await setValue(sessionId, "#rule-adjacency-gap", "60");
      await selectValue(sessionId, "#rule-adjacency-mode", "either");
    }
    await activateButton(sessionId, "Review changes", editor);
    await waitFor(
      sessionId,
      "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes(arguments[0]) && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
      [baseId],
    );
    await activateButton(sessionId, "Save reviewed change");
    await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
    const saved = await package8Rule(sessionId, scenarioId, id, "preference");
    assert.equal(saved.record.kind, kind);
    assert.equal(saved.record.baseScheduleId, baseId);
    assert.equal(saved.record.active, true);
    if (kind === "adjacency") {
      assert.equal(saved.record.maximumGapMinutes, 60);
      assert.equal(saved.record.mode, "either");
    }
  }
  console.log(
    "PASS: native-reviewed adjacency and base-stability preferences reference the independently verified accepted base in durable scenario state",
  );
}

async function package8Acceptance(sessionId, scenarioId, expected) {
  const anchoredWeekCalendarId = requestId();
  const calendarRevision = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: calendarRevision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native E2E consecutive-work calendar writer" },
    truncateRedo: false,
    command: {
      type: "applyDomainCommand",
      payload: {
        commandType: "official.workforce.add_entity",
        payload: {
          entity: {
            id: anchoredWeekCalendarId,
            kind: "calendar",
            name: "Existing anchored week",
            period: { kind: "week", anchorDate: "2030-01-01", startTime: "00:00:00" },
          },
        },
      },
    },
  });
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await waitForElement(sessionId, "#rules-heading");
  await activateButton(sessionId, "Minimum rest");
  const restId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(restId, "A new rule needs a real stable UUIDv7 before preview");
  await selectValue(sessionId, "#rule-minimum-rest-unit", "hours");
  await setValue(sessionId, "#rule-minimum-rest", "10");
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('600 minutes') && [...review.querySelectorAll('button')].some(b => b.textContent.trim() === 'Save reviewed change' && !b.disabled);",
  );
  const counts = await evaluate(
    sessionId,
    "return [...document.querySelectorAll('[aria-labelledby=\"rule-review-heading\"] dl > div')].map(row => ({label: row.querySelector('dt').textContent, count: Number(row.querySelector('dd').textContent.replaceAll(',', ''))}));",
  );
  assert.deepEqual(
    counts.map((row) => row.count),
    [104, 1, 1],
    "The native effective people, before and after populations must match the saved fixture",
  );
  await screenshot(sessionId, "package8-rest-review.png");
  const mainScope = `#rule-${restId}-main`;
  await activateButton(sessionId, "Show people and shifts covered", mainScope);
  await waitFor(
    sessionId,
    "return document.querySelectorAll(arguments[0] + '-population > ul > li').length === 50;",
    [mainScope],
  );
  const peopleScope = await getText(sessionId, `${mainScope}-population`);
  assert(peopleScope.includes(expected.seededPersonName));
  assert(peopleScope.includes(expected.seededPersonId));
  await screenshot(sessionId, "package8-native-people-scope.png");
  await selectValue(sessionId, `${mainScope}-axis`, "shifts");
  await waitFor(
    sessionId,
    "const population = document.querySelector(arguments[0] + '-population'); return population?.querySelectorAll('ul > li').length === 1 && population.textContent.includes(arguments[1]);",
    [mainScope, expected.shiftInstanceId],
  );
  await screenshot(sessionId, "package8-native-assignment-scope.png");
  await setValue(sessionId, "#rule-minimum-rest", "11");
  await waitFor(sessionId, "return !document.querySelector('#rule-review-heading');");
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0] + '-population > ul');", [
      mainScope,
    ]),
    null,
    "Editing must also invalidate the independently inspected native population",
  );
  assert(
    (await getText(sessionId, "#rule-identity")).includes(restId),
    "Editing must retain the rule identity while invalidating approval",
  );
  await setValue(sessionId, "#rule-minimum-rest", "10");
  await activateButton(sessionId, "Review changes");
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const rest = await package8Rule(sessionId, scenarioId, restId);
  assert.equal(rest.record.minimumMinutes, 600);
  assert.equal(rest.record.active, true);
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Maximum assignment count");
  const maximumId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(maximumId, "A new maximum-count rule needs a real stable UUIDv7");
  const maximumEditor = '[aria-labelledby="rule-editor-heading"]';
  await choosePeopleReference(
    sessionId,
    "Reporting-date calendar",
    "Existing daily target",
    maximumEditor,
  );
  const overlongMaximum = `${"0".repeat(32)}1`;
  await setValue(sessionId, "#rule-maximum", overlongMaximum);
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "return !document.querySelector('#rule-review-heading') && document.activeElement?.id === 'rule-maximum' && document.activeElement.value === arguments[0] && !!document.querySelector('#rule-maximum-error')?.textContent.trim();",
    [overlongMaximum],
  );
  await setValue(sessionId, "#rule-maximum", "2e");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "return !document.querySelector('#rule-review-heading') && document.activeElement?.id === 'rule-maximum' && document.activeElement.value === '2e';",
  );
  await setValue(sessionId, "#rule-maximum", "2");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('Allow at most 2 matching assignments') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  let maximum = await package8Rule(sessionId, scenarioId, maximumId);
  assert.equal(maximum.record.kind, "maximumAssignmentCount");
  assert.equal(maximum.record.calendarId, expected.calendarId);
  assert.equal(maximum.record.maximum, 2);

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  const maximumRow = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('button')].find(button => button.textContent.includes(arguments[0]) && button.closest('[aria-labelledby=\"rule-list-heading\"]')) ?? null;",
    [maximumId],
  );
  await activateElement(sessionId, maximumRow);
  await waitForElement(sessionId, "#rule-editor-heading");
  await activateButton(sessionId, "Edit");
  await setValue(sessionId, "#rule-maximum", "3");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('Allow at most 3 matching assignments') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  maximum = await package8Rule(sessionId, scenarioId, maximumId);
  assert.equal(maximum.record.calendarId, expected.calendarId);
  assert.equal(maximum.record.maximum, 3);
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Maximum worked weekends");
  const maximumWorkedWeekendsId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(maximumWorkedWeekendsId, "A new maximum-worked-weekends rule needs a stable UUIDv7");
  const maximumWorkedWeekendsScope = `#rule-${maximumWorkedWeekendsId}-main`;
  const weekdaysFilter = await waitFor(
    sessionId,
    "return document.querySelector(arguments[0] + '-weekdays-group button') ?? null;",
    [maximumWorkedWeekendsScope],
  );
  await activateElement(sessionId, weekdaysFilter);
  await activate(sessionId, `${maximumWorkedWeekendsScope}-sunday`);
  await setValue(sessionId, "#rule-maximum", "0");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('distinct local weekends') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  let maximumWorkedWeekends = await package8Rule(sessionId, scenarioId, maximumWorkedWeekendsId);
  assert.equal(maximumWorkedWeekends.record.kind, "maximumWorkedWeekends");
  assert.deepEqual(maximumWorkedWeekends.record.scope, {
    people: { kind: "all" },
    weekdays: ["sunday"],
  });
  assert.equal(maximumWorkedWeekends.record.maximum, 0);

  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Undo scenario change", '[aria-labelledby="history-heading"]');
  await waitForOutcome(sessionId, "Scenario undo committed");
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Redo scenario change", '[aria-labelledby="history-heading"]');
  await waitForOutcome(sessionId, "Scenario redo committed");
  maximumWorkedWeekends = await package8Rule(sessionId, scenarioId, maximumWorkedWeekendsId);
  assert.equal(maximumWorkedWeekends.record.kind, "maximumWorkedWeekends");
  assert.equal(maximumWorkedWeekends.record.maximum, 0);

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  const maximumWorkedWeekendsRow = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('button')].find(button => button.textContent.includes(arguments[0]) && button.closest('[aria-labelledby=\"rule-list-heading\"]')) ?? null;",
    [maximumWorkedWeekendsId],
  );
  await activateElement(sessionId, maximumWorkedWeekendsRow);
  await waitForElement(sessionId, "#rule-editor-heading");
  assert.equal(
    await evaluate(sessionId, "return document.querySelector('#rule-maximum').value;"),
    "0",
    "Reopening a saved maximum-worked-weekends rule must retain its native zero cap",
  );
  await activateButton(sessionId, "Edit");
  await setValue(sessionId, "#rule-maximum", "1");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('at most 1 distinct local weekends') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  maximumWorkedWeekends = await package8Rule(sessionId, scenarioId, maximumWorkedWeekendsId);
  assert.equal(maximumWorkedWeekends.record.kind, "maximumWorkedWeekends");
  assert.equal(maximumWorkedWeekends.record.maximum, 1);
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Required workload quota");
  const quotaId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(quotaId, "Required quota needs a stable native rule identity");
  await choosePeopleReference(sessionId, "Quota calendar", "Existing daily target", maximumEditor);
  await setValue(sessionId, "#rule-quota-minimum", "3");
  await setValue(sessionId, "#rule-quota-maximum", "2");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "return !document.querySelector('#rule-review-heading') && document.activeElement?.id === 'rule-quota-maximum' && !!document.querySelector('#rule-quota-maximum-error')?.textContent.trim();",
  );
  await setValue(sessionId, "#rule-quota-minimum", "1");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "return document.querySelector('[aria-labelledby=\"rule-review-heading\"]')?.textContent.includes('Require 1–2 matching assignments');",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const countQuota = await package8Rule(sessionId, scenarioId, quotaId);
  assert.equal(countQuota.record.kind, "workloadQuota");
  assert.equal(countQuota.record.calendarId, expected.calendarId);
  assert.deepEqual(countQuota.record.measure, { kind: "assignmentCount" });
  assert.equal(countQuota.record.minimum, 1);
  assert.equal(countQuota.record.maximum, 2);

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  const quotaRow = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('button')].find(button => button.textContent.includes(arguments[0]) && button.closest('[aria-labelledby=\"rule-list-heading\"]')) ?? null;",
    [quotaId],
  );
  await activateElement(sessionId, quotaRow);
  await waitForElement(sessionId, "#rule-editor-heading");
  await activateButton(sessionId, "Edit");
  await selectValue(sessionId, "#rule-quota-measure", "bucketMinutes");
  await choosePeopleReference(
    sessionId,
    "Workload bucket",
    "Existing scheduled minutes",
    maximumEditor,
  );
  await setValue(sessionId, "#rule-quota-minimum", "0");
  await setValue(sessionId, "#rule-quota-maximum", "480");
  await selectValue(sessionId, "#rule-quota-membership", "intersection");
  const beforeIncompatible = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "return !document.querySelector('#rule-review-heading') && [...document.querySelectorAll('section[aria-labelledby=\"rules-heading\"] > p[role=\"alert\"]')].some(node => node.textContent.includes('intersection requires an elapsed-minute bucket'));",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeIncompatible,
    "Native quota measurement rejection must not mutate the scenario",
  );
  await selectValue(sessionId, "#rule-quota-membership", "startInstant");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitForElement(sessionId, "#rule-review-heading");
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const minuteQuota = await package8Rule(sessionId, scenarioId, quotaId);
  assert.equal(minuteQuota.record.kind, "workloadQuota");
  assert.deepEqual(minuteQuota.record.measure, {
    kind: "bucketMinutes",
    bucketId: expected.bucketId,
  });
  assert.equal(minuteQuota.record.membership, "startInstant");
  assert.equal(minuteQuota.record.minimum, 0);
  assert.equal(minuteQuota.record.maximum, 480);
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Maximum consecutive work");
  const consecutiveWorkedDaysId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(consecutiveWorkedDaysId, "Worked-days maximum consecutive needs a stable rule identity");
  await setValue(sessionId, "#rule-maximum", "2");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('consecutive worked days') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const consecutiveWorkedDays = await package8Rule(sessionId, scenarioId, consecutiveWorkedDaysId);
  assert.equal(consecutiveWorkedDays.record.kind, "maximumConsecutive");
  assert.equal(consecutiveWorkedDays.record.maximum, 2);
  assert.deepEqual(consecutiveWorkedDays.record.mode, { kind: "workedDays" });

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Maximum consecutive work");
  const consecutiveAssignmentsId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(consecutiveAssignmentsId, "Assignment maximum consecutive needs a stable rule identity");
  await selectValue(sessionId, "#rule-consecutive-mode", "assignments");
  await setValue(sessionId, "#rule-maximum", "3");
  await setValue(sessionId, "#rule-consecutive-break-minutes", "1.5");
  await selectValue(sessionId, "#rule-consecutive-mode", "workedDays");
  await selectValue(sessionId, "#rule-consecutive-mode", "assignments");
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelector('#rule-consecutive-break-minutes')?.value;",
    ),
    "1.5",
    "Switching modes must retain invalid break input until it is repaired",
  );
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "return !document.querySelector('#rule-review-heading') && document.activeElement?.id === 'rule-consecutive-break-minutes' && document.activeElement.value === '1.5' && !!document.querySelector('#rule-consecutive-break-minutes-error')?.textContent.trim();",
  );
  await setValue(sessionId, "#rule-consecutive-break-minutes", "15");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('adjacent elapsed breaks are at most 15 minutes') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const consecutiveAssignments = await package8Rule(
    sessionId,
    scenarioId,
    consecutiveAssignmentsId,
  );
  assert.equal(consecutiveAssignments.record.kind, "maximumConsecutive");
  assert.equal(consecutiveAssignments.record.maximum, 3);
  assert.deepEqual(consecutiveAssignments.record.mode, { kind: "assignments", breakMinutes: 15 });

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Maximum hours");
  const calendarHoursId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(calendarHoursId, "Calendar maximum hours needs a stable native rule identity");
  await choosePeopleReference(
    sessionId,
    "Workload bucket",
    "Existing scheduled minutes",
    maximumEditor,
  );
  await choosePeopleReference(
    sessionId,
    "Reporting-date calendar",
    "Existing daily target",
    maximumEditor,
  );
  await setValue(sessionId, "#rule-maximum-minutes", "480");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const calendarHours = await package8Rule(sessionId, scenarioId, calendarHoursId);
  assert.equal(calendarHours.record.kind, "maximumHours");
  assert.equal(calendarHours.record.bucketId, expected.bucketId);
  assert.equal(calendarHours.record.window.kind, "calendar");
  assert.equal(calendarHours.record.window.calendarId, expected.calendarId);
  assert.equal(calendarHours.record.window.membership, "reportingDate");
  assert.equal(calendarHours.record.maximumMinutes, 480);

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Maximum hours");
  const rollingHoursId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(rollingHoursId, "Rolling maximum hours needs a stable native rule identity");
  await choosePeopleReference(
    sessionId,
    "Workload bucket",
    "Existing scheduled minutes",
    maximumEditor,
  );
  await selectValue(sessionId, "#rule-window-kind", "rolling");
  await selectValue(sessionId, "#rule-window-membership", "startInstant");
  await setValue(sessionId, "#rule-maximum-minutes", "960");
  await setValue(sessionId, "#rule-window-duration", "0");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await waitFor(
    sessionId,
    "return !document.querySelector('#rule-review-heading') && document.activeElement?.id === 'rule-window-duration' && document.activeElement.value === '0';",
  );
  await setValue(sessionId, "#rule-window-duration", "1440");
  await activateButton(sessionId, "Review changes", maximumEditor);
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const rollingHours = await package8Rule(sessionId, scenarioId, rollingHoursId);
  assert.equal(rollingHours.record.kind, "maximumHours");
  assert.equal(rollingHours.record.bucketId, expected.bucketId);
  assert.equal(rollingHours.record.window.kind, "rolling");
  assert.equal(rollingHours.record.window.membership, "startInstant");
  assert.equal(rollingHours.record.window.durationMinutes, 1440);
  assert.equal(rollingHours.record.maximumMinutes, 960);
  const packagedPreferenceId =
    process.env.EUTHETO_E2E_PACKAGED_SOLVE === "1"
      ? await packagedAssignmentPreferenceAcceptance(sessionId, scenarioId, expected)
      : null;

  await navigate(sessionId, `/project/${scenarioId}/validation`);
  await waitForElement(sessionId, "#validation-heading");
  await waitFor(sessionId, "return document.querySelector('[data-full-validation-state]');");
  await activateButton(sessionId, "Run full validation");
  await waitFor(
    sessionId,
    "return document.querySelector('[data-full-validation-state=\"completed\"]');",
    [],
    120_000,
  );
  await screenshot(sessionId, "package8-full-validation.png");
  await optimizeHandoffAcceptance(sessionId, scenarioId, true, "current");
  if (packagedPreferenceId !== null)
    await packagedSolveAcceptance(
      sessionId,
      scenarioId,
      packagedPreferenceId,
      expected.shiftInstanceId,
      expected.seeded.people[2].id,
      expected.seededPersonId,
      expected.seeded.people[3].id,
    );

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Eligibility");
  const inactiveId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(inactiveId);
  await selectValue(sessionId, `#rule-${inactiveId}-main-people-mode`, "filter");
  await activate(sessionId, '[data-scope-add="allTags"]');
  await setValue(sessionId, '[data-scope-text="allTags"]', "package8-no-person-has-this-tag");
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('An active rule needs a successful preview') && [...review.querySelectorAll('dl > div')].some(row => row.querySelector('dt').textContent === 'People covered' && row.querySelector('dd').textContent === '0');",
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Save reviewed change')?.disabled;",
    ),
    true,
  );
  await screenshot(sessionId, "package8-empty-active-scope.png");
  await activate(sessionId, "#rule-active");
  await waitFor(sessionId, "return !document.querySelector('#rule-review-heading');");
  await activateButton(sessionId, "Review changes");
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const inactive = await package8Rule(sessionId, scenarioId, inactiveId);
  assert.equal(inactive.record.active, false);
  assert.deepEqual(inactive.record.scope.people.allTags, ["package8-no-person-has-this-tag"]);
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Required qualification mix");
  const skillMixId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(skillMixId, "A new skill-mix rule needs a stable UUIDv7");
  const skillMixEditor = '[aria-labelledby="rule-editor-heading"]';
  const minimumInput = '#rule-qualification-minimums input[inputmode="numeric"]';
  await choosePeopleReference(
    sessionId,
    "Require all selected qualifications",
    "Native training",
    skillMixEditor,
  );
  await setValue(sessionId, minimumInput, "2e");
  await activateButton(sessionId, "Review changes", skillMixEditor);
  await waitFor(
    sessionId,
    "const input = document.querySelector(arguments[0]); const status = document.getElementById(input?.getAttribute('aria-describedby')); return !document.querySelector('#rule-review-heading') && document.activeElement === input && input?.value === '2e' && input.getAttribute('aria-invalid') === 'true' && Boolean(status?.textContent.trim());",
    [minimumInput],
  );
  await setValue(sessionId, minimumInput, "1");
  await activateButton(sessionId, "Review changes", skillMixEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes(arguments[0]) && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
    [expected.qualificationId],
  );
  await evaluate(
    sessionId,
    "document.querySelector('[aria-labelledby=\"rule-review-heading\"]').scrollIntoView({block:'start'}); return true;",
  );
  await screenshot(sessionId, "package8-required-skill-mix-review.png");
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const skillMix = await package8Rule(sessionId, scenarioId, skillMixId);
  assert.equal(skillMix.record.kind, "requiredSkillMix");
  assert.equal(skillMix.record.qualificationMinimums[0].minimum, 1);
  assert.deepEqual(skillMix.record.qualificationMinimums[0].qualifications.allQualificationIds, [
    expected.qualificationId,
  ]);
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Fixed assignments");
  const fixedId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(fixedId, "A new fixed assignment needs a stable UUIDv7");
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "return !document.querySelector('#rule-review-heading') && document.activeElement?.id === 'rule-fixed-person';",
  );
  const fixedPerson = expected.seeded.people[1];
  await choosePeopleReference(
    sessionId,
    "Named person",
    fixedPerson.name,
    '[aria-labelledby="rule-editor-heading"]',
  );
  await setValue(sessionId, "#rule-fixed-shift-start", "01/16/2030");
  await setValue(sessionId, "#rule-fixed-shift-end", "01/17/2030");
  for (const [selector, typed, expectedDate] of [
    ["#rule-fixed-shift-start", "01/16/2030", "2030-01-16"],
    ["#rule-fixed-shift-end", "01/17/2030", "2030-01-17"],
  ]) {
    if (
      !(await evaluate(
        sessionId,
        "return document.querySelector(arguments[0]).value === arguments[1];",
        [selector, expectedDate],
      ))
    )
      await setValue(sessionId, selector, typed);
  }
  assert.deepEqual(
    await evaluate(
      sessionId,
      "return [document.querySelector('#rule-fixed-shift-start').value, document.querySelector('#rule-fixed-shift-end').value];",
    ),
    ["2030-01-16", "2030-01-17"],
    "The native date controls must select the saved shift's local date",
  );
  const fixedShiftChoice = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('#rule-fixed-shift ul > li')].find(row => row.querySelector('code')?.textContent.trim() === arguments[0])?.querySelector('input[type=\"checkbox\"]:not(:disabled)') ?? null;",
    [expected.shiftInstanceId],
  );
  await activateElement(sessionId, fixedShiftChoice);
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes(arguments[0]) && review.textContent.includes(arguments[1]) && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
    [fixedPerson.id, expected.shiftInstanceId],
  );
  await screenshot(sessionId, "package8-fixed-assignment-review.png");
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const fixed = await package8Rule(sessionId, scenarioId, fixedId);
  assert.equal(fixed.record.kind, "fixedAssignment");
  assert.equal(fixed.record.personId, fixedPerson.id);
  assert.equal(fixed.record.shiftId, expected.shiftInstanceId);
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Assignment restrictions");
  const mutualId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(mutualId, "A new assignment restriction needs a stable native UUIDv7");
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return !!review?.querySelector('[role=\"alert\"]') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && button.disabled);",
  );
  const firstMutualPerson = expected.seeded.people[0];
  const secondMutualPerson = expected.seeded.people[1];
  await choosePeopleReference(
    sessionId,
    "People to keep apart",
    firstMutualPerson.name,
    '[aria-labelledby="rule-editor-heading"]',
  );
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return !!review?.querySelector('[role=\"alert\"]') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && button.disabled);",
  );
  await activate(sessionId, "#rule-active");
  await waitFor(sessionId, "return !document.querySelector('#rule-review-heading');");
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return !!review && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const inactiveMutual = await package8Rule(sessionId, scenarioId, mutualId);
  assert.equal(inactiveMutual.record.active, false);
  assert.deepEqual(inactiveMutual.record.personIds, [firstMutualPerson.id]);
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  const mutualRow = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('button')].find(button => button.textContent.includes(arguments[0]) && button.closest('[aria-labelledby=\"rule-list-heading\"]')) ?? null;",
    [mutualId],
  );
  await activateElement(sessionId, mutualRow);
  await waitForElement(sessionId, "#rule-editor-heading");
  const beforeBlockedActivation = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await activateButton(sessionId, "Review activation change");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return !!review?.querySelector('[role=\"alert\"]') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && button.disabled);",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeBlockedActivation,
    "Blocked singleton activation must not mutate the native scenario",
  );
  await activateButton(sessionId, "Edit");
  await waitForElement(sessionId, "#rule-mutual-people");
  await choosePeopleReference(
    sessionId,
    "People to keep apart",
    secondMutualPerson.name,
    '[aria-labelledby="rule-editor-heading"]',
  );
  await selectValue(sessionId, "#rule-mutual-mode", "overlappingShifts");
  await activate(sessionId, "#rule-active");
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return !!review && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await evaluate(
    sessionId,
    "document.querySelector('[aria-labelledby=\"rule-review-heading\"]').scrollIntoView({block:'start'}); return true;",
  );
  await screenshot(sessionId, "package8-mutual-restriction-review.png");
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const mutual = await package8Rule(sessionId, scenarioId, mutualId);
  assert.equal(mutual.record.kind, "mutualAssignmentRestriction");
  assert.equal(mutual.record.mode, "overlappingShifts");
  assert.deepEqual(
    [...mutual.record.personIds].sort(),
    [firstMutualPerson.id, secondMutualPerson.id].sort(),
  );
  const travelSourceId = requestId();
  const travelTargetId = requestId();
  const travelSourceShiftId = requestId();
  const travelTargetShiftId = requestId();
  const travelRevision = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  const travelShifts = [
    [travelSourceShiftId, travelSourceId, "18:00:00", "19:00:00"],
    [travelTargetShiftId, travelTargetId, "19:15:00", "20:15:00"],
  ].map(([id, locationId, start, end]) => ({
    kind: "shiftInstance",
    id,
    assignmentTypeId: expected.shiftTypeId,
    locationId,
    coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
    startsAt: {
      instant: `2030-01-16T${start}Z`,
      local: `2030-01-16T${start}`,
      offsetSeconds: 0,
    },
    endsAt: {
      instant: `2030-01-16T${end}Z`,
      local: `2030-01-16T${end}`,
      offsetSeconds: 0,
    },
    origin: { kind: "manual" },
    reportingAttribution: "startLocalDate",
    tags: [],
  }));
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: travelRevision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native travel-rule test fixture" },
    truncateRedo: false,
    command: {
      type: "applyBatch",
      payload: {
        label: "Two real located shifts and a directed travel matrix",
        commands: [
          { kind: "location", id: travelTargetId, name: "Travel target ward", transitions: [] },
          {
            kind: "location",
            id: travelSourceId,
            name: "Travel source ward",
            transitions: [{ locationId: travelTargetId, minutes: 30 }],
          },
          ...travelShifts,
        ].map((entity) => ({
          type: "applyDomainCommand",
          payload: { commandType: "official.workforce.add_entity", payload: { entity } },
        })),
      },
    },
  });
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await activateButton(sessionId, "Transition time");
  const transitionId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(transitionId, "A new directed transition rule needs a stable native UUIDv7");
  await choosePeopleReference(
    sessionId,
    "Locations for transition time",
    "Travel source ward",
    '[aria-labelledby="rule-editor-heading"]',
  );
  const beforeBlockedTravelReview = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return !!review?.querySelector('[role=\"alert\"]') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && button.disabled);",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeBlockedTravelReview,
    "Native review must refuse a newly active singleton travel rule without mutation",
  );
  await choosePeopleReference(
    sessionId,
    "Locations for transition time",
    "Travel target ward",
    '[aria-labelledby="rule-editor-heading"]',
  );
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return !!review && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled) && review.textContent.includes('2 selected locations');",
  );
  await screenshot(sessionId, "package8-transition-time-review.png");
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const transition = await package8Rule(sessionId, scenarioId, transitionId);
  assert.equal(transition.record.kind, "transitionTime");
  assert.equal(transition.record.active, true);
  assert.deepEqual(
    [...transition.record.locationIds].sort(),
    [travelSourceId, travelTargetId].sort(),
  );
  const requestedAvailabilityViewId = "official.workforce.setup.availability_records";
  const requestedAvailabilityPage = (
    await nativeSetupRequest(
      sessionId,
      scenarioId,
      "scenario_get_view",
      requestedAvailabilityViewId,
      {
        source: { kind: "stored" },
        query: {
          schemaVersion: 1,
          viewId: requestedAvailabilityViewId,
          parameters: {
            personId: expected.seededPersonId,
            limit: 100,
          },
          continuation: null,
        },
      },
    )
  ).result.view.data.result.data;
  const requestedTimeOffWindow = {
    kind: "instant",
    startsAt: "2030-01-16T09:30:00Z",
    endsAt: "2030-01-16T11:00:00Z",
  };
  let requestedTimeOffId = requestedAvailabilityPage.items.find(
    (item) => item.availabilityKind === "requestedTimeOff",
  )?.availabilityId;
  if (requestedTimeOffId === undefined) {
    requestedTimeOffId = requestId();
    const current = (await projects(sessionId)).find(
      (project) => project.scenarioId === scenarioId,
    );
    assert(current, "The native requested-time-off fixture project must exist");
    await nativeRequest(sessionId, "scenario_apply_command", {
      scenarioId,
      expectedRevision: current.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Package8 requested-time-off fixture" },
      truncateRedo: false,
      command: {
        type: "applyDomainCommand",
        payload: {
          commandType: "official.workforce.add_entity",
          payload: {
            entity: {
              kind: "availability",
              id: requestedTimeOffId,
              personId: expected.seededPersonId,
              availabilityKind: "requestedTimeOff",
              effectiveRange: { startDate: "2030-01-01", endDateExclusive: "2030-02-01" },
              timeWindow: requestedTimeOffWindow,
              source: "package8-requested-time-off",
              note: "Native requested-time-off preference fixture",
            },
          },
        },
      },
    });
  }
  assert(requestedTimeOffId, "A requested-time-off availability needs a stable native UUIDv7");

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Assignment-count targets");
  const assignmentTargetId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(assignmentTargetId, "A new AssignmentTarget preference needs a stable UUIDv7");
  await choosePeopleReference(
    sessionId,
    "Reporting-date calendar",
    "Existing daily target",
    '[aria-labelledby="rule-editor-heading"]',
  );
  await selectValue(sessionId, "#rule-preference-priority", "high");
  await setValue(sessionId, "#rule-preference-weight", "5");
  await setValue(sessionId, "#rule-assignment-target-count", "0");
  await activateButton(sessionId, "Review changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('Aim for 0 matching assignments per person') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const targetPreference = await package8Rule(
    sessionId,
    scenarioId,
    assignmentTargetId,
    "preference",
  );
  assert.equal(targetPreference.class, "preference");
  assert.equal(targetPreference.record.kind, "assignmentTarget");
  assert.equal(targetPreference.record.calendarId, expected.calendarId);
  assert.equal(targetPreference.record.targetCount, 0);
  assert.equal(targetPreference.record.priority, "high");
  assert.equal(targetPreference.record.weight, 5);
  assert.equal(targetPreference.record.active, true);

  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Worked-weekend target");
  const weekendTargetId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(weekendTargetId, "A new worked-weekend target needs a stable UUIDv7");
  const weekendEditor = '[aria-labelledby="rule-editor-heading"]';
  await selectValue(sessionId, `#rule-${weekendTargetId}-main-people-mode`, "selected");
  await choosePeopleReference(
    sessionId,
    "Selected people",
    expected.seeded.people[0].name,
    weekendEditor,
  );
  await activateButton(sessionId, "Enable Categories filter", weekendEditor);
  await activate(sessionId, `${weekendEditor} [data-scope-add="categories"]`);
  await setValue(
    sessionId,
    `${weekendEditor} [data-scope-text="categories"]`,
    "package8-no-matching-shift-category",
  );
  await setValue(sessionId, "#rule-weekend-target-count", "2e");
  await activateButton(sessionId, "Review changes", weekendEditor);
  await waitFor(
    sessionId,
    "return !document.querySelector('#rule-review-heading') && document.activeElement?.id === 'rule-weekend-target-count' && document.activeElement.value === '2e' && document.querySelector('#rule-weekend-target-count-error')?.textContent.trim();",
  );
  await setValue(sessionId, "#rule-weekend-target-count", "2");
  await selectValue(sessionId, "#rule-preference-priority", "high");
  await setValue(sessionId, "#rule-preference-weight", "11");
  await activateButton(sessionId, "Review changes", weekendEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); const rows = [...(review?.querySelectorAll('dl > div') ?? [])]; const count = label => rows.some(row => row.querySelector('dt')?.textContent === label && row.querySelector('dd')?.textContent === arguments[label === 'People covered' ? 0 : 1]); return review?.textContent.includes('Aim for 2 distinct local Saturday') && count('People covered') && count('Shifts covered') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
    ["1", "0"],
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  assert.deepEqual(
    (await package8Rule(sessionId, scenarioId, weekendTargetId, "preference")).record,
    {
      kind: "weekendTarget",
      id: weekendTargetId,
      active: true,
      scope: {
        people: { kind: "selected", personIds: [expected.seeded.people[0].id] },
        categories: ["package8-no-matching-shift-category"],
      },
      priority: "high",
      weight: 11,
      targetCount: 2,
    },
  );

  const preferenceEditor = '[aria-labelledby="rule-editor-heading"]';
  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Time preferences");
  const timePreferenceId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(timePreferenceId, "A new Time preference needs a stable UUIDv7");
  await selectValue(sessionId, "#rule-preference-priority", "veryHigh");
  await setValue(sessionId, "#rule-preference-weight", "7");
  await selectValue(sessionId, "#rule-preference-direction", "prefer");
  await selectValue(sessionId, "#rule-preference-time-window-kind", "instant");
  await setValue(sessionId, "#rule-preference-time-starts-at", requestedTimeOffWindow.startsAt);
  await setValue(sessionId, "#rule-preference-time-ends-at", requestedTimeOffWindow.endsAt);
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('resolved interval overlaps the selected') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const timePreference = await package8Rule(sessionId, scenarioId, timePreferenceId, "preference");
  assert.equal(timePreference.class, "preference");
  assert.deepEqual(timePreference.record, {
    kind: "time",
    id: timePreferenceId,
    active: true,
    scope: { people: { kind: "all" } },
    priority: "veryHigh",
    weight: 7,
    direction: "prefer",
    timeWindow: requestedTimeOffWindow,
  });

  const preferredAssignmentType = expected.seeded.types[1];
  assert(preferredAssignmentType, "The seeded assignment-type fixture must include a second type");
  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Assignment type preferences");
  const assignmentTypePreferenceId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(assignmentTypePreferenceId, "A new AssignmentType preference needs a stable UUIDv7");
  await choosePeopleReference(
    sessionId,
    "Preferred work types",
    preferredAssignmentType.name,
    preferenceEditor,
  );
  await selectValue(sessionId, "#rule-preference-priority", "high");
  await setValue(sessionId, "#rule-preference-weight", "9");
  await selectValue(sessionId, "#rule-preference-direction", "avoid");
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('1 selected work types') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const assignmentTypePreference = await package8Rule(
    sessionId,
    scenarioId,
    assignmentTypePreferenceId,
    "preference",
  );
  assert.equal(assignmentTypePreference.class, "preference");
  assert.deepEqual(assignmentTypePreference.record, {
    kind: "assignmentType",
    id: assignmentTypePreferenceId,
    active: true,
    scope: { people: { kind: "all" } },
    priority: "high",
    weight: 9,
    direction: "avoid",
    assignmentTypeIds: [preferredAssignmentType.id],
  });

  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Consecutive-work preferences");
  const consecutiveReportingDaysId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(
    consecutiveReportingDaysId,
    "A reporting-days ConsecutiveWork preference needs a stable UUIDv7",
  );
  await choosePeopleReference(
    sessionId,
    "Matching work types",
    preferredAssignmentType.name,
    preferenceEditor,
  );
  await selectValue(sessionId, "#rule-preference-priority", "high");
  await setValue(sessionId, "#rule-preference-weight", "13");
  await setValue(sessionId, "#rule-consecutive-work-threshold", "1");
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('threshold 1') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const consecutiveReportingDays = await package8Rule(
    sessionId,
    scenarioId,
    consecutiveReportingDaysId,
    "preference",
  );
  assert.deepEqual(consecutiveReportingDays.record, {
    kind: "consecutiveWork",
    id: consecutiveReportingDaysId,
    active: true,
    scope: { people: { kind: "all" } },
    priority: "high",
    weight: 13,
    mode: { kind: "reportingDays" },
    assignmentTypeIds: [preferredAssignmentType.id],
    threshold: 1,
  });

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await waitForElement(sessionId, "#rules-heading");
  await selectValue(sessionId, "#rule-class-filter", "preference");
  const reportingDaysRow = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[aria-labelledby=\"rule-list-heading\"] button')].find(button => button.textContent.includes(arguments[0]));",
    [consecutiveReportingDaysId],
  );
  await activateElement(sessionId, reportingDaysRow);
  await waitForElement(sessionId, "#rule-editor-heading");
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelector('#rule-consecutive-work-threshold').value;",
    ),
    "1",
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelector('#rule-consecutive-work-mode').value;",
    ),
    "reportingDays",
  );
  await activateButton(sessionId, "Close");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");

  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Consecutive-work preferences");
  const consecutiveWeekendsId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(consecutiveWeekendsId, "A weekends ConsecutiveWork preference needs a stable UUIDv7");
  await choosePeopleReference(
    sessionId,
    "Matching work types",
    preferredAssignmentType.name,
    preferenceEditor,
  );
  await selectValue(sessionId, "#rule-preference-priority", "normal");
  await setValue(sessionId, "#rule-preference-weight", "17");
  await setValue(sessionId, "#rule-consecutive-work-threshold", "2");
  await selectValue(sessionId, "#rule-consecutive-work-mode", "weekends");
  await choosePeopleReference(
    sessionId,
    "Anchored-week calendar",
    "Existing daily target",
    preferenceEditor,
  );
  await activate(sessionId, "#rule-consecutive-work-saturday");
  await activate(sessionId, "#rule-consecutive-work-sunday");
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[role=\"alert\"]')].some(alert => alert.textContent.includes('anchored-week calendar'));",
  );
  assert.equal(
    await evaluate(sessionId, "return !!document.querySelector('#rule-editor-heading');"),
    true,
    "A daily calendar must not pass native weekend validation",
  );
  await choosePeopleReference(
    sessionId,
    "Anchored-week calendar",
    "Existing anchored week",
    preferenceEditor,
  );
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('threshold 2') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const consecutiveWeekends = await package8Rule(
    sessionId,
    scenarioId,
    consecutiveWeekendsId,
    "preference",
  );
  assert.deepEqual(consecutiveWeekends.record, {
    kind: "consecutiveWork",
    id: consecutiveWeekendsId,
    active: true,
    scope: { people: { kind: "all" } },
    priority: "normal",
    weight: 17,
    mode: {
      kind: "weekends",
      calendarId: anchoredWeekCalendarId,
      weekdays: ["saturday", "sunday"],
    },
    assignmentTypeIds: [preferredAssignmentType.id],
    threshold: 2,
  });

  await navigate(sessionId, `/project/${scenarioId}/history`);
  await waitForElement(sessionId, "#history-heading");
  await activateButton(sessionId, "Undo scenario change", '[aria-labelledby="history-heading"]');
  await waitForOutcome(sessionId, "Scenario undo committed");
  await activateButton(sessionId, "Redo scenario change", '[aria-labelledby="history-heading"]');
  await waitForOutcome(sessionId, "Scenario redo committed");
  const consecutiveWeekendsAfterRedo = await package8Rule(
    sessionId,
    scenarioId,
    consecutiveWeekendsId,
    "preference",
  );
  assert.deepEqual(consecutiveWeekendsAfterRedo.record, consecutiveWeekends.record);

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await waitForElement(sessionId, "#rules-heading");
  await selectValue(sessionId, "#rule-class-filter", "preference");
  const weekendsRow = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[aria-labelledby=\"rule-list-heading\"] button')].find(button => button.textContent.includes(arguments[0]));",
    [consecutiveWeekendsId],
  );
  await activateElement(sessionId, weekendsRow);
  await waitForElement(sessionId, "#rule-editor-heading");
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelector('#rule-consecutive-work-mode').value;",
    ),
    "weekends",
  );
  await activateButton(sessionId, "Close");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Location preferences");
  const locationPreferenceId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(locationPreferenceId, "A new Location preference needs a stable UUIDv7");
  assert(expected.locationId, "The existing location fixture needs a stable native UUIDv7");
  await choosePeopleReference(
    sessionId,
    "Preferred locations",
    "Existing home ward",
    preferenceEditor,
  );
  await selectValue(sessionId, "#rule-preference-priority", "normal");
  await setValue(sessionId, "#rule-preference-weight", "11");
  await selectValue(sessionId, "#rule-preference-direction", "prefer");
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('1 selected locations') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const locationPreference = await package8Rule(
    sessionId,
    scenarioId,
    locationPreferenceId,
    "preference",
  );
  assert.equal(locationPreference.class, "preference");
  assert.deepEqual(locationPreference.record, {
    kind: "location",
    id: locationPreferenceId,
    active: true,
    scope: { people: { kind: "all" } },
    priority: "normal",
    weight: 11,
    direction: "prefer",
    locationIds: [expected.locationId],
  });

  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Requested time off");
  const requestedTimeOffPreferenceId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(requestedTimeOffPreferenceId, "A new RequestedTimeOff preference needs a stable UUIDv7");
  const requestedAvailabilityInput = `${preferenceEditor} input[aria-label=${JSON.stringify("Search Requested time-off records")}]`;
  await setValue(sessionId, requestedAvailabilityInput, "");
  await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[role=\"option\"]')].some(option => option.textContent.includes(arguments[0]));",
    [requestedTimeOffId],
  );
  const requestedAvailabilityOption = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[role=\"option\"]')].find(option => option.textContent.includes(arguments[0])) ?? null;",
    [requestedTimeOffId],
  );
  await activateElement(sessionId, requestedAvailabilityOption);
  await selectValue(sessionId, "#rule-preference-priority", "low");
  await setValue(sessionId, "#rule-preference-weight", "13");
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('1 selected requested-time-off records') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const requestedTimeOffPreference = await package8Rule(
    sessionId,
    scenarioId,
    requestedTimeOffPreferenceId,
    "preference",
  );
  assert.equal(requestedTimeOffPreference.class, "preference");
  assert.deepEqual(requestedTimeOffPreference.record, {
    kind: "requestedTimeOff",
    id: requestedTimeOffPreferenceId,
    active: true,
    scope: { people: { kind: "all" } },
    priority: "low",
    weight: 13,
    availabilityIds: [requestedTimeOffId],
  });
  const workloadBalance = await workloadBalanceAcceptance(sessionId, scenarioId, expected);

  await navigate(sessionId, `/project/${scenarioId}/validation`);
  await waitForElement(sessionId, "[data-full-validation-stale]");
  await screenshot(sessionId, "package8-stale-validation.png");
  await optimizeHandoffAcceptance(sessionId, scenarioId, false, "stale");
  return {
    restId,
    maximumId,
    maximumWorkedWeekendsId,
    quotaId,
    consecutiveWorkedDaysId,
    consecutiveAssignmentsId,
    consecutiveReportingDaysId,
    weekendTargetId,
    consecutiveWeekendsId,
    calendarHoursId,
    rollingHoursId,
    bucketId: expected.bucketId,
    inactiveId,
    skillMixId,
    fixedId,
    fixedPersonId: fixedPerson.id,
    fixedShiftId: expected.shiftInstanceId,
    mutualId,
    mutualPersonIds: [firstMutualPerson.id, secondMutualPerson.id],
    transitionId,
    transitionLocationIds: [travelSourceId, travelTargetId],
    counts,
    calendarId: expected.calendarId,
    anchoredWeekCalendarId,
    qualificationId: expected.qualificationId,
    preferredAssignmentTypeId: preferredAssignmentType.id,
    workloadBalanceId: workloadBalance.workloadBalanceId,
    workloadPolicyId: workloadBalance.workloadPolicyId,
  };
}

async function package8CoverageAcceptance(sessionId, scenarioId, shiftId) {
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  for (const [kind, title] of [
    ["availability", "Availability"],
    ["coverage", "Coverage"],
    ["noOverlap", "No overlap"],
  ]) {
    await activateButton(sessionId, title);
    const id = await evaluate(
      sessionId,
      "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
    );
    assert(id);
    if (kind === "noOverlap") {
      await activateButton(sessionId, "Add category pair");
      await setValue(sessionId, '#rule-category-pairs input[id$="-firstCategory"]', "call");
      await setValue(sessionId, '#rule-category-pairs input[id$="-secondCategory"]', "clinic");
      assert.equal(
        await evaluate(sessionId, "return document.activeElement?.value;"),
        "clinic",
        "Typing a category must retain the actual row control and keyboard focus",
      );
    }
    await activateButton(sessionId, "Review changes");
    await activateButton(sessionId, "Save reviewed change");
    await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
    const saved = await package8Rule(sessionId, scenarioId, id);
    assert.equal(saved.record.kind, kind);
    assert.equal(saved.record.active, true);
    if (kind === "noOverlap")
      assert.deepEqual(saved.record.compatibleCategoryPairs, [
        { firstCategory: "call", secondCategory: "clinic" },
      ]);
  }
  await selectValue(sessionId, "#rule-class-filter", "preference");
  await waitFor(
    sessionId,
    "const list = document.querySelector('[aria-labelledby=\"rule-list-heading\"]'); return list?.getAttribute('aria-busy') === 'false' && list.textContent.includes('Assignment-count targets') && list.textContent.includes('Worked-weekend target');",
  );
  await screenshot(sessionId, "package8-preferences-target.png");
  await selectValue(sessionId, "#rule-class-filter", "required");

  await navigate(sessionId, `/project/${scenarioId}/work`);
  await activateButton(sessionId, "Shifts starting in these dates");
  await setValue(sessionId, "#work-window-start", "01/15/2030");
  await setValue(sessionId, "#work-window-end", "01/18/2030");
  // WebKitWebDriver can lose a date-editor keystroke while Vue reflects the typed segment.
  // Check the actual control before using this range to select the saved shift.
  if (
    !(await evaluate(
      sessionId,
      "return document.querySelector('#work-window-end').value === '2030-01-18';",
    ))
  )
    await setValue(sessionId, "#work-window-end", "01/18/2030");
  assert.deepEqual(
    await evaluate(
      sessionId,
      "return [document.querySelector('#work-window-start').value, document.querySelector('#work-window-end').value];",
    ),
    ["2030-01-15", "2030-01-18"],
    "The WebDriver date editor must display the intended local-date window before native review",
  );
  await activateButton(sessionId, "Read this local-date window");
  // The completed native window read restores focus to its heading. Wait for
  // that handoff before another keyboard action, rather than letting it move
  // focus between activateElement's focus and Enter requests.
  await waitFor(sessionId, "return document.activeElement?.id === 'work-window-heading';");
  await activateButton(sessionId, "Saved work records");
  await selectValue(sessionId, "#work-record-kind", "coverageRequirement");
  await activateButton(sessionId, "Create a record: Separate coverage requirement");
  const editorRoot = '[aria-labelledby="work-editor-heading"]';
  const coverageId = await evaluate(
    sessionId,
    "return document.querySelector('[aria-labelledby=\"work-editor-heading\"] > p.break-all').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(coverageId);
  await activate(sessionId, `${editorRoot} input[id$="-active"]`);
  await selectValue(sessionId, `${editorRoot} select[id$="-scope.kind"]`, "selected");
  await activate(sessionId, `${editorRoot} input[id$="-choice-${shiftId}"]`);
  await selectValue(sessionId, `${editorRoot} select[id$="-coverage.kind"]`, "exact");
  await setValue(sessionId, `${editorRoot} input[id$="-coverage.count"]`, "1000");
  await activateButton(sessionId, "Review changes", editorRoot);
  await activateButton(sessionId, "Apply this exact reviewed proposal");
  await waitFor(sessionId, "return !document.querySelector('#work-editor-heading');");
  await waitFor(sessionId, "return document.activeElement?.id === 'work-window-heading';");
  await navigate(sessionId, `/project/${scenarioId}/validation`);
  await activateButton(sessionId, "Run full validation");
  await waitFor(
    sessionId,
    "return document.querySelector('[data-full-validation-state=\"completed\"]');",
    [],
    120_000,
  );
  const fieldPath = `domain.entities.${coverageId}.coverage.count`;
  const finding = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[aria-label=\"Validation issues\"] button')].find(button => button.textContent.includes('official.workforce.candidate_shortage') && button.textContent.includes(arguments[0]));",
    [fieldPath],
  );
  const beforeNavigation = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  await activateElement(sessionId, finding);
  const selectedFindingMessage = await getText(
    sessionId,
    '[aria-labelledby="validation-selected-heading"] > p',
  );
  await activateButton(sessionId, "Check this full validation finding");
  await waitForElement(sessionId, "[data-validation-source-conflict]");
  const explanation = await getText(sessionId, "[data-validation-explanation]");
  const proof = await getText(sessionId, "[data-validation-source-conflict]");
  assert(
    explanation.includes(selectedFindingMessage) &&
      explanation.includes(`Checked saved revision ${beforeNavigation}`) &&
      explanation.includes("source-level staffing contradiction") &&
      proof.includes("Required Coverage lower bound of 1000") &&
      /\b1[^\d]?000 are required\./u.test(proof) &&
      proof.includes("Required rules used in this count") &&
      (await evaluate(
        sessionId,
        "return document.querySelectorAll('[data-validation-source-conflict] li').length;",
      )) > 0,
    "Native coverage evidence must recheck the exact source and map a sufficient count contradiction without solver proof",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeNavigation,
    "Checking mapped count evidence must not mutate the saved scenario",
  );
  await screenshot(sessionId, "package8-coverage-validation-evidence.png");
  await activateButton(sessionId, "Open the exact editor field");
  await waitFor(
    sessionId,
    "return document.activeElement?.id.endsWith('-coverage.count') && document.activeElement.value === '1000';",
  );
  assert(
    (await getText(sessionId, editorRoot)).includes(coverageId),
    "The finding must open the standalone owner, not the shift's inline coverage",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeNavigation,
    "Finding navigation must not mutate the saved scenario",
  );
  await screenshot(sessionId, "package8-coverage-exact-field.png");
  await setValue(sessionId, `${editorRoot} input[id$="-coverage.count"]`, "1");
  await activateButton(sessionId, "Review changes", editorRoot);
  await activateButton(sessionId, "Apply this exact reviewed proposal");
  await waitFor(sessionId, "return !document.querySelector('#work-editor-heading');");
  await waitFor(sessionId, "return document.activeElement?.id === 'work-window-heading';");
  await navigate(sessionId, `/project/${scenarioId}/validation`);
  await activateButton(sessionId, "Run full validation");
  await waitFor(
    sessionId,
    "return document.querySelector('[data-full-validation-state=\"completed\"]');",
    [],
    120_000,
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll('[aria-label=\"Validation issues\"] button')].some(button => button.textContent.includes('official.workforce.candidate_shortage') && button.textContent.includes(arguments[0]));",
      [fieldPath],
    ),
    false,
    "The explicitly reviewed correction must resolve this native shortage",
  );
  // Keep the shortage-navigation case exact, then explicitly author the separate soft
  // above-minimum target before referencing this requirement from a preference.
  await navigate(sessionId, `/project/${scenarioId}/work`);
  await activateButton(sessionId, "Saved work records");
  await selectValue(sessionId, "#work-record-kind", "coverageRequirement");
  const coverageRow = await waitFor(
    sessionId,
    "const button = [...document.querySelectorAll('[aria-labelledby=\"work-records-heading\"] ul li')].find(row => row.querySelector('code')?.textContent === arguments[0])?.querySelector('button'); return button && !button.disabled ? button : null;",
    [coverageId],
  );
  await activateElement(sessionId, coverageRow);
  await activateButton(sessionId, "Edit record", editorRoot);
  await selectValue(sessionId, `${editorRoot} select[id$="-coverage.kind"]`, "atLeast");
  await setValue(sessionId, `${editorRoot} input[id$="-coverage.minimum"]`, "1");
  await activate(sessionId, `${editorRoot} input[id$="-preferred-enabled"]`);
  await setValue(sessionId, `${editorRoot} input[id$="-coverage.preferredCount"]`, "2");
  await activateButton(sessionId, "Review changes", editorRoot);
  await activateButton(sessionId, "Apply this exact reviewed proposal");
  await waitFor(
    sessionId,
    "const window = document.querySelector('[aria-labelledby=\"work-window-heading\"]'); return !document.querySelector('#work-editor-heading') && document.querySelector('#work-window-start')?.disabled === false && !!window?.querySelector(':scope > nav') && !window.querySelector(':scope > p[role=\"status\"]');",
  );
  console.log(
    "PASS: native Required rules including independent count/minute quotas, reporting-date maximum assignment count, maximum worked weekends with a persisted zero cap and native undo/redo, qualification mix, fixed assignment, mutual restriction and directed travel time; stable category-pair typing; six authored preferences including persisted workload-policy selection with native undo/redo/restart; bounded selected-shift coverage creation remains persisted for preference editors; native shortage -> exact owner/count focus -> reviewed repair",
  );
  return { coverageId };
}
async function package8PreferenceAcceptance(sessionId, scenarioId, expected, coverageId) {
  assert(coverageId, "The native coverage requirement must survive for preferred coverage");
  const people = expected.seeded.people;
  const firstPerson = people[0];
  const secondPerson = people[1];
  assert(firstPerson && secondPerson, "The native preference fixture needs two seeded people");
  const preferenceEditor = '[aria-labelledby="rule-editor-heading"]';
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await waitForElement(sessionId, "#rules-heading");
  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Together or separate");
  const togetherSeparateId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(togetherSeparateId, "A TogetherSeparate preference needs a stable UUIDv7");
  await choosePeopleReference(
    sessionId,
    "People for together/separate preference",
    firstPerson.name,
    preferenceEditor,
  );
  await choosePeopleReference(
    sessionId,
    "People for together/separate preference",
    secondPerson.name,
    preferenceEditor,
  );
  await selectValue(sessionId, "#rule-preference-priority", "high");
  await setValue(sessionId, "#rule-preference-weight", "19");
  await selectValue(sessionId, "#rule-together-separate-direction", "separate");
  await selectValue(sessionId, "#rule-together-separate-mode", "overlappingShifts");
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes(arguments[0]) && review.textContent.includes(arguments[1]) && review.textContent.includes('overlapping shifts') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
    [firstPerson.id, secondPerson.id],
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const togetherSeparate = await package8Rule(
    sessionId,
    scenarioId,
    togetherSeparateId,
    "preference",
  );
  assert.equal(togetherSeparate.class, "preference");
  assert.deepEqual(togetherSeparate.record, {
    kind: "togetherSeparate",
    id: togetherSeparateId,
    active: true,
    scope: { people: { kind: "all" } },
    priority: "high",
    weight: 19,
    direction: "separate",
    mode: "overlappingShifts",
    personIds: [firstPerson.id, secondPerson.id],
  });

  await openPersonOptions(sessionId, "Preferences", "body");
  await activateButton(sessionId, "Above-minimum coverage");
  const preferredCoverageId = await evaluate(
    sessionId,
    "return document.querySelector('#rule-identity').textContent.match(/[a-f0-9]{8}-[a-f0-9]{4}-7[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}/)?.[0];",
  );
  assert(preferredCoverageId, "A PreferredCoverage preference needs a stable UUIDv7");
  const coverageInput = `${preferenceEditor} input[aria-label=${JSON.stringify("Search Preferred coverage requirements")}]`;
  await setValue(sessionId, coverageInput, "");
  const coverageOption = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[role=\"option\"]')].find(option => option.textContent.includes(arguments[0])) ?? null;",
    [coverageId],
  );
  await activateElement(sessionId, coverageOption);
  await selectValue(sessionId, "#rule-preference-priority", "veryHigh");
  await setValue(sessionId, "#rule-preference-weight", "23");
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes(arguments[0]) && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
    [coverageId],
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const preferredCoverage = await package8Rule(
    sessionId,
    scenarioId,
    preferredCoverageId,
    "preference",
  );
  assert.equal(preferredCoverage.class, "preference");
  assert.deepEqual(preferredCoverage.record, {
    kind: "preferredCoverage",
    id: preferredCoverageId,
    active: true,
    scope: { people: { kind: "all" } },
    priority: "veryHigh",
    weight: 23,
    coverageRequirementIds: [coverageId],
  });
  return {
    coverageId,
    togetherSeparateId,
    preferredCoverageId,
    personIds: [firstPerson.id, secondPerson.id],
  };
}

async function package8PreferenceRestartAcceptance(sessionId, scenarioId, expected) {
  const preferenceEditor = '[aria-labelledby="rule-editor-heading"]';
  const historyRoot = '[aria-labelledby="history-heading"]';
  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await waitForElement(sessionId, "#rules-heading");
  await selectValue(sessionId, "#rule-class-filter", "preference");
  const togetherSeparateRow = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[aria-labelledby=\"rule-list-heading\"] button')].find(button => button.textContent.includes(arguments[0]));",
    [expected.togetherSeparateId],
  );
  await activateElement(sessionId, togetherSeparateRow);
  await waitForElement(sessionId, "#rule-editor-heading");
  assert.deepEqual(
    await evaluate(
      sessionId,
      "return [document.querySelector('#rule-preference-priority').value, document.querySelector('#rule-preference-weight').value, document.querySelector('#rule-together-separate-direction').value, document.querySelector('#rule-together-separate-mode').value];",
    ),
    ["high", "19", "separate", "overlappingShifts"],
    "Reloaded TogetherSeparate preference must retain its native fields",
  );
  const retainedPeople = await getText(sessionId, "#rule-together-separate-people-selected-items");
  for (const personId of expected.personIds) assert(retainedPeople.includes(personId));
  await activateButton(sessionId, "Edit", preferenceEditor);
  await selectValue(sessionId, "#rule-preference-priority", "normal");
  await setValue(sessionId, "#rule-preference-weight", "21");
  await selectValue(sessionId, "#rule-together-separate-direction", "together");
  await selectValue(sessionId, "#rule-together-separate-mode", "sameShift");
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes(arguments[0]) && review.textContent.includes(arguments[1]) && review.textContent.includes('same shift') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
    [expected.personIds[0], expected.personIds[1]],
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return !!review?.querySelector('[role=\"alert\"]') && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Apply and discard redo branch' && !button.disabled);",
  );
  assert.equal(
    (await package8Rule(sessionId, scenarioId, expected.togetherSeparateId, "preference")).record
      .weight,
    19,
    "Attempting to save over a redo branch must not mutate the persisted preference before confirmation",
  );
  await activateButton(sessionId, "Apply and discard redo branch");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  let togetherSeparate = await package8Rule(
    sessionId,
    scenarioId,
    expected.togetherSeparateId,
    "preference",
  );
  assert.deepEqual(togetherSeparate.record, {
    kind: "togetherSeparate",
    id: expected.togetherSeparateId,
    active: true,
    scope: { people: { kind: "all" } },
    priority: "normal",
    weight: 21,
    direction: "together",
    mode: "sameShift",
    personIds: expected.personIds,
  });
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await waitForElement(sessionId, "#history-heading");
  await activateButton(sessionId, "Undo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario undo committed");
  togetherSeparate = await package8Rule(
    sessionId,
    scenarioId,
    expected.togetherSeparateId,
    "preference",
  );
  assert.equal(togetherSeparate.record.priority, "high");
  assert.equal(togetherSeparate.record.weight, 19);
  assert.equal(togetherSeparate.record.direction, "separate");
  assert.equal(togetherSeparate.record.mode, "overlappingShifts");
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Redo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario redo committed");
  togetherSeparate = await package8Rule(
    sessionId,
    scenarioId,
    expected.togetherSeparateId,
    "preference",
  );
  assert.equal(togetherSeparate.record.priority, "normal");
  assert.equal(togetherSeparate.record.weight, 21);
  assert.equal(togetherSeparate.record.direction, "together");
  assert.equal(togetherSeparate.record.mode, "sameShift");

  await navigate(sessionId, `/project/${scenarioId}/rules`);
  await waitForElement(sessionId, "#rules-heading");
  await selectValue(sessionId, "#rule-class-filter", "preference");
  const preferredCoverageRow = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[aria-labelledby=\"rule-list-heading\"] button')].find(button => button.textContent.includes(arguments[0]));",
    [expected.preferredCoverageId],
  );
  await activateElement(sessionId, preferredCoverageRow);
  await waitForElement(sessionId, "#rule-editor-heading");
  assert.deepEqual(
    await evaluate(
      sessionId,
      "return [document.querySelector('#rule-preference-priority').value, document.querySelector('#rule-preference-weight').value];",
    ),
    ["veryHigh", "23"],
    "Reloaded PreferredCoverage preference must retain its native fields",
  );
  const retainedCoverage = await getText(
    sessionId,
    "#rule-preferred-coverage-requirements-selected-items",
  );
  assert(retainedCoverage.includes(expected.coverageId));
  await activateButton(sessionId, "Edit", preferenceEditor);
  await selectValue(sessionId, "#rule-preference-priority", "low");
  await setValue(sessionId, "#rule-preference-weight", "29");
  await activateButton(sessionId, "Review changes", preferenceEditor);
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes(arguments[0]) && [...review.querySelectorAll('button')].some(button => button.textContent.trim() === 'Save reviewed change' && !button.disabled);",
    [expected.coverageId],
  );
  await activateButton(sessionId, "Save reviewed change");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  let preferredCoverage = await package8Rule(
    sessionId,
    scenarioId,
    expected.preferredCoverageId,
    "preference",
  );
  assert.deepEqual(preferredCoverage.record, {
    kind: "preferredCoverage",
    id: expected.preferredCoverageId,
    active: true,
    scope: { people: { kind: "all" } },
    priority: "low",
    weight: 29,
    coverageRequirementIds: [expected.coverageId],
  });
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await waitForElement(sessionId, "#history-heading");
  await activateButton(sessionId, "Undo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario undo committed");
  preferredCoverage = await package8Rule(
    sessionId,
    scenarioId,
    expected.preferredCoverageId,
    "preference",
  );
  assert.equal(preferredCoverage.record.priority, "veryHigh");
  assert.equal(preferredCoverage.record.weight, 23);
  assert.deepEqual(preferredCoverage.record.coverageRequirementIds, [expected.coverageId]);
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Redo scenario change", historyRoot);
  await waitForOutcome(sessionId, "Scenario redo committed");
  preferredCoverage = await package8Rule(
    sessionId,
    scenarioId,
    expected.preferredCoverageId,
    "preference",
  );
  assert.equal(preferredCoverage.record.priority, "low");
  assert.equal(preferredCoverage.record.weight, 29);
  assert.deepEqual(preferredCoverage.record.coverageRequirementIds, [expected.coverageId]);
}

async function package8CancellationAcceptance(sessionId, scenarioId, assignmentTypeId) {
  const current = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  const commands = Array.from({ length: 200 }, () => ({
    type: "applyDomainCommand",
    payload: {
      commandType: "official.workforce.add_entity",
      payload: {
        entity: {
          kind: "shiftInstance",
          id: requestId(),
          assignmentTypeId,
          startsAt: {
            instant: "2030-01-16T08:00:00Z",
            local: "2030-01-16T08:00:00",
            offsetSeconds: 0,
          },
          endsAt: {
            instant: "2030-01-16T17:00:00Z",
            local: "2030-01-16T17:00:00",
            offsetSeconds: 0,
          },
          coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
          origin: { kind: "manual" },
          reportingAttribution: "startLocalDate",
          tags: [],
        },
      },
    },
  }));
  const fixtureStarted = Date.now();
  await nativeRequest(
    sessionId,
    "scenario_apply_command",
    {
      scenarioId,
      expectedRevision: current.revision,
      commandId: requestId(),
      actor: { actorId: null, displayName: "Native validation workload" },
      // The redo tail contains only this runner's already-verified coverage fixture.
      truncateRedo: true,
      command: { type: "applyBatch", payload: { label: "Native cancellation workload", commands } },
    },
    120_000,
  );
  console.log("Native 200-shift fixture admission milliseconds:", Date.now() - fixtureStarted);
  await navigate(sessionId, `/project/${scenarioId}/validation`);
  await waitFor(sessionId, "return document.querySelector('[data-full-validation-state]');");
  await evaluate(
    sessionId,
    `window.__validationInputEvidence = null;
    const captureInput = (event) => {
      if (event.target.id !== 'command-search') return;
      window.__validationInputEvidence = {
        focused: document.activeElement?.id,
        running: !!document.querySelector('[data-full-validation-state="running"]')
      };
      document.removeEventListener('input', captureInput, true);
    };
    document.addEventListener('input', captureInput, true);`,
  );
  await activateButton(sessionId, "Run full validation");
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "validation-palette",
        actions: [
          { type: "keyDown", value: "\uE009" },
          { type: "keyDown", value: "k" },
          { type: "keyUp", value: "k" },
          { type: "keyUp", value: "\uE009" },
        ],
      },
    ],
  });
  await waitFor(sessionId, "return document.activeElement?.id === 'command-search';");
  // The displayed phase may not appear before the request settles.
  // Capture the first WebDriver keystroke while the UI still reports a pending run.
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "validation-input",
        actions: [
          { type: "keyDown", value: "o" },
          { type: "keyUp", value: "o" },
          { type: "keyDown", value: "k" },
          { type: "keyUp", value: "k" },
        ],
      },
    ],
  });
  const inputEvidence = await evaluate(
    sessionId,
    `return {
      ...window.__validationInputEvidence,
      value: document.querySelector('#command-search')?.value
    };`,
  );
  assert.equal(inputEvidence.focused, "command-search");
  assert.equal(inputEvidence.value, "ok");
  assert.equal(inputEvidence.running, true);
  await screenshot(sessionId, "package8-input-during-native-validation.png");
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "validation-palette",
        actions: [
          { type: "keyDown", value: "\uE00C" },
          { type: "keyUp", value: "\uE00C" },
        ],
      },
    ],
  });
  await waitFor(sessionId, "return !document.querySelector('#command-search');");
  // This intentionally dense rest model reaches the native resource ceiling;
  // failure must remain failure, not readiness or a manufactured cancellation.
  await waitFor(
    sessionId,
    "return document.querySelector('[data-full-validation-state=\"failed\"]');",
    [],
    120_000,
  );
  await screenshot(sessionId, "package8-native-validation-limit.png");
  await idle(sessionId);
  await activateButton(sessionId, "Run full validation");
  const cancelStarted = Date.now();
  await activateButton(sessionId, "Request cancellation", '[aria-labelledby="validation-heading"]');
  await waitFor(
    sessionId,
    "return document.querySelector('[data-full-validation-state=\"cancelled\"]');",
    [],
    120_000,
  );
  const cancelMilliseconds = Date.now() - cancelStarted;
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelectorAll('[data-full-validation-state=\"completed\"]').length;",
    ),
    0,
  );
  await screenshot(sessionId, "package8-native-validation-cancelled.png");
  await idle(sessionId);
  await navigate(sessionId, `/project/${scenarioId}/history`);
  await activateButton(sessionId, "Undo scenario change");
  await waitForOutcome(sessionId, "Scenario undo committed", 120_000);
  console.log(
    "PASS: pending full-validation UI retained WebDriver keyboard input/focus; native cancellation stayed distinct from completion",
    JSON.stringify({ inputEvidence, cancelMilliseconds }),
  );
}

async function package8TemporalAcceptance(sessionId, originalScenarioId) {
  await navigate(sessionId, "/projects");
  await navigate(sessionId, "/projects/new");
  await setValue(sessionId, "#create-title", "Package8 native DST navigation");
  await setValue(sessionId, "#create-time-zone", "America/New_York");
  await setValue(sessionId, "#first-date", "10/31/2026");
  await setValue(sessionId, "#last-date", "11/03/2026");
  await evaluate(sessionId, "document.querySelector('form details summary').focus();");
  await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
    actions: [
      {
        type: "key",
        id: "dst-policy-keyboard",
        actions: [
          { type: "keyDown", value: " " },
          { type: "keyUp", value: " " },
        ],
      },
    ],
  });
  await waitFor(sessionId, "return document.querySelector('form details').open;");
  await selectValue(sessionId, "#create-gap", "reject");
  await selectValue(sessionId, "#create-overlap", "reject");
  await submitForm(sessionId, "#create-title");
  await waitForElement(sessionId, "#setup-calendar");
  const scenarioId = await evaluate(
    sessionId,
    "return window.location.hash.match(/^#\\/project\\/([^/]+)\\/setup$/)?.[1];",
  );
  assert(scenarioId);
  const created = await package10NativeScenarioShape(sessionId, scenarioId, true);
  assert.deepEqual(created.shape.planningDates, {
    startDate: "2026-10-31",
    endDateExclusive: "2026-11-04",
  });
  assert.equal(created.shape.settings.timeZone, "America/New_York");
  assert.equal(created.shape.settings.gapPolicy, "reject");
  assert.equal(created.shape.settings.overlapPolicy, "reject");
  assert.equal(
    Date.parse(created.shape.settings.horizon.start),
    Date.parse("2026-10-31T04:00:00Z"),
  );
  assert.equal(Date.parse(created.shape.settings.horizon.end), Date.parse("2026-11-04T05:00:00Z"));
  const typeId = requestId();
  const personId = requestId();
  const shiftId = requestId();
  const availabilityId = requestId();
  const ruleId = requestId();
  const entities = [
    {
      kind: "assignmentType",
      id: typeId,
      name: "DST native duty",
      category: "clinic",
      timeBehavior: "elapsed",
      defaultDurationMinutes: 60,
      qualifications: { kind: "unconstrained" },
      locationBehavior: { kind: "optional" },
      workloadBucketIds: [],
    },
    {
      kind: "person",
      id: personId,
      name: "DST native person",
      activeRange: { kind: "always" },
      qualificationGrants: [],
      eligibleAssignmentTypeIds: [typeId],
      teamIds: [],
      tags: [],
      workloadWeight: { numerator: 1, denominator: 1 },
    },
    {
      kind: "shiftInstance",
      id: shiftId,
      assignmentTypeId: typeId,
      startsAt: {
        instant: "2026-11-01T05:30:00Z",
        local: "2026-11-01T01:30:00",
        offsetSeconds: -14400,
      },
      endsAt: {
        instant: "2026-11-01T07:30:00Z",
        local: "2026-11-01T02:30:00",
        offsetSeconds: -18000,
      },
      coverage: { kind: "exact", count: 1, qualificationMinimums: [] },
      tags: [],
      reportingAttribution: "startLocalDate",
      origin: { kind: "manual" },
    },
    {
      kind: "availability",
      id: availabilityId,
      personId,
      availabilityKind: "unavailable",
      source: "Native DST fixture",
      note: "",
      effectiveRange: { startDate: "2026-10-31", endDateExclusive: "2026-11-03" },
      timeWindow: {
        kind: "weekly",
        windows: [
          { weekdays: ["sunday"], startTime: "03:00:00", endTime: "04:00:00", endDayOffset: 0 },
          { weekdays: ["sunday"], startTime: "01:30:00", endTime: "02:30:00", endDayOffset: 0 },
        ],
      },
    },
  ];
  const current = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  await nativeRequest(sessionId, "scenario_apply_command", {
    scenarioId,
    expectedRevision: current.revision,
    commandId: requestId(),
    actor: { actorId: null, displayName: "Native temporal fixture" },
    truncateRedo: false,
    command: {
      type: "applyBatch",
      payload: {
        label: "Admitted DST availability fixture",
        commands: [
          ...entities.map((entity) => ({
            type: "applyDomainCommand",
            payload: { commandType: "official.workforce.add_entity", payload: { entity } },
          })),
          {
            type: "applyDomainCommand",
            payload: {
              commandType: "official.workforce.add_rule",
              payload: {
                rule: {
                  kind: "availability",
                  id: ruleId,
                  active: true,
                  strength: "required",
                  scope: { people: { kind: "all" } },
                },
              },
            },
          },
        ],
      },
    },
  });
  await navigate(sessionId, `/project/${scenarioId}/validation`);
  await waitFor(
    sessionId,
    "return document.querySelector('[data-full-validation-state=\"notRun\"]');",
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return document.querySelectorAll('[aria-label=\"Validation issues\"] button').length;",
    ),
    0,
    "The admitted ambiguous weekly input has no displayed fast errors, but full validation is still not run",
  );
  assert.equal(
    await evaluate(
      sessionId,
      'return document.querySelector(\'select[id$="-group"] option[value="mustFix"]\')?.textContent.trim().match(/\\d+$/)?.[0];',
    ),
    "0",
    "The native fast-error count must be zero before explicit full validation finds the temporal error",
  );
  await screenshot(sessionId, "package8-fast-does-not-imply-full.png");
  await activateButton(sessionId, "Run full validation");
  await waitFor(
    sessionId,
    "return document.querySelector('[data-full-validation-state=\"completed\"]');",
    [],
    120_000,
  );
  const path = `/domain/entities/${availabilityId}/timeWindow/windows/1/startTime`;
  const finding = await waitFor(
    sessionId,
    "return [...document.querySelectorAll('[aria-label=\"Validation issues\"] button')].find(button => button.textContent.includes('official.workforce.temporal_review') && button.textContent.includes(arguments[0]));",
    [path],
  );
  await activateElement(sessionId, finding);
  assert(
    (await getText(sessionId, '[aria-labelledby="validation-selected-heading"]')).includes(
      "2026-11-01",
    ),
  );
  const beforeNavigation = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  const selectedFindingMessage = await getText(
    sessionId,
    '[aria-labelledby="validation-selected-heading"] > p',
  );
  await activateButton(sessionId, "Check this full validation finding");
  await waitForElement(sessionId, "[data-validation-explanation]");
  const explanation = await getText(sessionId, "[data-validation-explanation]");
  assert(
    explanation.includes(selectedFindingMessage) &&
      explanation.includes(`Checked saved revision ${beforeNavigation}`) &&
      explanation.includes("It does not prove that a schedule is feasible or infeasible"),
    "Native temporal validation evidence must recheck the exact saved finding without inventing solver proof",
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeNavigation,
    "Checking native validation evidence must not mutate the saved project",
  );
  await screenshot(sessionId, "package8-temporal-validation-evidence.png");
  await activateButton(sessionId, "Open the exact editor field");
  await waitForElement(sessionId, "#availability-editor-heading");
  const rowKey = await package7WeeklyRowKey(sessionId, "Weekly schedule 2");
  await waitFor(sessionId, "return document.activeElement?.id === arguments[0];", [
    `${rowKey}-start`,
  ]);
  assert.equal(await evaluate(sessionId, "return document.activeElement.value;"), "01:30:00");
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeNavigation,
    "Temporal finding navigation must preserve the admitted saved revision",
  );
  await screenshot(sessionId, "package8-temporal-exact-weekly-row.png");
  await activateButton(sessionId, "Discard", '[aria-labelledby="availability-editor-heading"]');
  await navigate(sessionId, "/projects");
  await navigate(sessionId, `/projects?project=${scenarioId}`);
  await activateButton(sessionId, "Delete project");
  await activateButton(sessionId, "Delete permanently", '[role="dialog"]');
  await waitForOutcome(sessionId, "Deleted");
  assert.deepEqual(
    (await projects(sessionId)).map((project) => project.scenarioId),
    [originalScenarioId],
  );
  await navigate(sessionId, `/project/${originalScenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  await idle(sessionId);
  console.log(
    "PASS: native admitted DST fixture -> full temporal finding with occurrence -> exact second authored weekly start; navigation leaves saved revision unchanged",
  );
}

async function package8FirstTimeDstWorkAcceptance(sessionId, originalScenarioId, windowTitle) {
  await navigate(sessionId, "/projects");
  await navigate(sessionId, "/projects/new");
  await setValue(sessionId, "#create-title", "Native four-day recurring duty");
  await setValue(sessionId, "#create-time-zone", "America/New_York");
  await setValue(sessionId, "#first-date", "10/31/2026");
  await setValue(sessionId, "#last-date", "11/03/2026");
  await activate(sessionId, "form details summary");
  await selectValue(sessionId, "#create-gap", "reject");
  await selectValue(sessionId, "#create-overlap", "earlier");
  await submitForm(sessionId, "#create-title");
  await waitForElement(sessionId, "#setup-calendar");
  const scenarioId = await evaluate(
    sessionId,
    "return window.location.hash.match(/^#\\/project\\/([^/]+)\\/setup$/)?.[1];",
  );
  assert(scenarioId);
  await navigate(sessionId, `/project/${scenarioId}/people`);
  await waitForElement(sessionId, "#people-list-heading");
  const peopleEditor = 'section[aria-labelledby="people-editor-heading"]';
  const peopleList = 'section[aria-labelledby="people-list-heading"]';
  await selectValue(sessionId, "#people-kind", "assignmentType");
  await activateButton(sessionId, "Create a record", peopleList);
  await setValue(sessionId, `${peopleEditor} input[name="name"]`, "Repeated-hour duty");
  await setValue(sessionId, `${peopleEditor} input[name="category"]`, "clinic");
  await setValue(sessionId, `${peopleEditor} input[id$="-duration"]`, "120");
  await selectValue(sessionId, `${peopleEditor} select[name="qualificationMode"]`, "unconstrained");
  await selectValue(sessionId, `${peopleEditor} select[name="locationMode"]`, "none");
  await selectValue(sessionId, `${peopleEditor} select[name="timeBehavior"]`, "elapsed");
  await activateButton(sessionId, "Review changes", peopleEditor);
  await activateButton(sessionId, "Save this change");
  await waitFor(sessionId, "return !document.querySelector('#people-editor-heading');");
  for (const name of ["Native Alice", "Native Bob"]) {
    await selectValue(sessionId, "#people-kind", "person");
    await activateButton(sessionId, "Create a record", peopleList);
    await setValue(sessionId, `${peopleEditor} input[name="name"]`, name);
    await openPersonOptions(sessionId, "Qualifications and work types", peopleEditor);
    await choosePeopleReference(sessionId, "Work types this person may do", "Repeated-hour duty");
    await activateButton(sessionId, "Review changes", peopleEditor);
    await activateButton(sessionId, "Save this change");
    await waitFor(sessionId, "return !document.querySelector('#people-editor-heading');");
  }
  await navigate(sessionId, `/project/${scenarioId}/work`);
  await activateButton(sessionId, "Saved work records");
  await selectValue(sessionId, "#work-record-kind", "shiftTemplate");
  await activateButton(sessionId, "Create a record: Recurring shift template");
  const workEditor = '[aria-labelledby="work-editor-heading"]';
  await setValue(sessionId, `${workEditor} input[id$="-name"]`, "Sunday repeated-hour duty");
  await choosePeopleReference(sessionId, "Work type", "Repeated-hour duty", workEditor);
  await setValue(
    sessionId,
    `${workEditor} input[id$="-recurrence.effectiveRange.startDate"]`,
    "2026-10-31",
  );
  await setValue(
    sessionId,
    `${workEditor} input[id$="-recurrence.effectiveRange.endDateExclusive"]`,
    "2026-11-04",
  );
  await activate(sessionId, `${workEditor} input[id$="-sunday"]`);
  await selectValue(sessionId, `${workEditor} select[id$="-timing.kind"]`, "localWindow");
  await setValue(sessionId, `${workEditor} input[id$="-timing-start"]`, "01:30:00");
  await setValue(sessionId, `${workEditor} input[id$="-timing-end"]`, "02:30:00");
  await setValue(sessionId, `${workEditor} input[id$="-timing-endDayOffset"]`, "0");
  await selectValue(
    sessionId,
    `${workEditor} select[name="reportingAttribution"]`,
    "startLocalDate",
  );
  await selectValue(sessionId, `${workEditor} select[id$="-coverage.kind"]`, "exact");
  await setValue(sessionId, `${workEditor} input[id$="-coverage.count"]`, "1");
  await activateButton(sessionId, "Review changes", workEditor);
  await waitForElement(sessionId, "#work-review-heading");
  await activateButton(sessionId, "Apply this exact reviewed proposal");
  await waitFor(sessionId, "return !document.querySelector('#work-editor-heading');");
  const created = await package10NativeScenarioShape(sessionId, scenarioId, true);
  assert.deepEqual(created.shape.planningDates, {
    startDate: "2026-10-31",
    endDateExclusive: "2026-11-04",
  });
  assert.equal(created.shape.entityCounts.person, 2);
  assert.equal(created.shape.entityCounts.assignmentType, 1);
  assert.equal(created.shape.entityCounts.shiftTemplate, 1);
  assert.equal(created.shape.resolvedShiftCount, 1);
  assert.equal(created.shape.settings.overlapPolicy, "earlier");
  const work = (
    await nativeSetupRequest(
      sessionId,
      scenarioId,
      "scenario_get_view",
      "official.workforce.setup.work_window",
      {
        source: { kind: "stored" },
        query: {
          schemaVersion: 1,
          viewId: "official.workforce.setup.work_window",
          parameters: { dates: created.shape.planningDates, limit: 10 },
        },
      },
    )
  ).result.view.data.result.data;
  assert.equal(work.items[0].templateName, "Sunday repeated-hour duty");
  assert.equal(work.items[0].reportingDate, "2026-11-01");
  assert.equal(work.items[0].elapsed.seconds, "7200");
  assert.equal(work.items[0].interval.startsAt.instant, "2026-11-01T05:30:00Z");
  assert.equal(work.items[0].interval.startsAt.offsetSeconds, -14400);
  assert.equal(work.items[0].interval.endsAt.instant, "2026-11-01T07:30:00Z");
  assert.equal(work.items[0].interval.endsAt.offsetSeconds, -18000);
  await activateButton(sessionId, "Shifts starting in these dates");
  await waitFor(
    sessionId,
    "return document.querySelector('[aria-labelledby=\"work-window-heading\"]')?.textContent.includes('Sunday repeated-hour duty');",
  );
  await screenshot(sessionId, "package8-four-day-recurring-duty.png");
  await activateButton(sessionId, "Saved work records");
  await activateButton(
    sessionId,
    "Sunday repeated-hour duty",
    '[aria-labelledby="work-records-heading"]',
  );
  await activateButton(sessionId, "Edit record", workEditor);
  const beforeInvalidReview = (await projects(sessionId)).find(
    (project) => project.scenarioId === scenarioId,
  ).revision;
  const invalidStart = `${workEditor} input[id$="-timing-start"]`;
  await setValue(sessionId, invalidStart, "25:61");
  await activateButton(sessionId, "Review changes", workEditor);
  await waitForElement(sessionId, '[aria-label="Native time and input diagnostics"]');
  await idle(sessionId);
  assert.equal(
    await evaluate(sessionId, "return document.querySelector(arguments[0])?.value;", [
      invalidStart,
    ]),
    "25:61",
    "Rejected local-time input must remain available in the draft",
  );
  assert.equal(
    await evaluate(sessionId, "return !!document.querySelector('#work-review-heading');"),
    false,
    "Rejected local-time input must not open an approvable review",
  );
  await waitFor(
    sessionId,
    "return document.querySelector(arguments[0])?.getAttribute('aria-invalid') === 'true';",
    [invalidStart],
    10_000,
  );
  await waitFor(
    sessionId,
    "return document.activeElement === document.querySelector(arguments[0]);",
    [invalidStart],
    10_000,
  );
  assert.equal(
    (await projects(sessionId)).find((project) => project.scenarioId === scenarioId).revision,
    beforeInvalidReview,
    "Invalid local time must remain a draft, not a saved mutation",
  );
  await screenshot(sessionId, "package8-work-invalid-local-time.png");
  await activateButton(sessionId, "Discard draft and close", workEditor);
  await waitFor(sessionId, "return !document.querySelector('#work-editor-heading');");
  await idle(sessionId);
  await activateButton(sessionId, "Shifts starting in these dates");
  await waitFor(
    sessionId,
    "return document.querySelector('[aria-labelledby=\"work-window-heading\"]')?.textContent.includes('Sunday repeated-hour duty');",
  );
  await waitFor(
    sessionId,
    'return !document.querySelector(\'[aria-labelledby="work-window-heading"] > p[role="status"]\');',
  );
  await idle(sessionId);
  const setupRoutes = [
    ["people", "people-heading"],
    ["work", "work-heading"],
    ["rules", "rules-heading"],
    ["validation", "validation-heading"],
    ["eligibility", "eligibility-setup-heading"],
    ["availability", "availability-heading"],
    ["setup", "setup-heading"],
  ];
  async function loadedRoute(route) {
    const selector = {
      work: 'section[aria-labelledby="work-heading"] details.state-panel.setup-more',
      people: 'section[aria-labelledby="people-list-heading"] ul > li',
      rules: 'section[aria-labelledby="rule-list-heading"] nav.action-row',
      validation: 'section[aria-labelledby="validation-heading"] select[id$="-group"]',
      eligibility: '[data-matrix-scroll] input[type="checkbox"]',
      availability: "#availability-person:not(:disabled)",
      setup: "#setup-calendar",
    }[route];
    assert(selector, `Unknown setup route ${route}`);
    await waitForElement(sessionId, selector);
    await idle(sessionId);
  }
  async function keyboardRoute(route, heading) {
    const href = `#/project/${scenarioId}/${route}`;
    if ((await evaluate(sessionId, "return window.location.hash;")) === href) return;
    if (route === "eligibility" || route === "availability") {
      const more = 'nav[aria-label="Saved setup sections"] details.project-nav-more';
      if (
        !(await evaluate(sessionId, "return document.querySelector(arguments[0])?.open;", [more]))
      ) {
        await evaluate(sessionId, "document.querySelector(arguments[0]).focus();", [
          `${more} > summary`,
        ]);
        await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
          actions: [
            {
              type: "key",
              id: "native-more-tools-keyboard",
              actions: [
                { type: "keyDown", value: " " },
                { type: "keyUp", value: " " },
              ],
            },
          ],
        });
        await waitFor(sessionId, "return document.querySelector(arguments[0])?.open;", [more]);
      }
    }
    const link = `nav[aria-label="Saved setup sections"] a[href="${href}"]`;
    for (let attempt = 0; attempt < 4; attempt += 1) {
      await evaluate(sessionId, "document.querySelector(arguments[0]).focus();", [link]);
      await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
        actions: [
          {
            type: "key",
            id: "native-route-keyboard",
            actions: [
              { type: "keyDown", value: "\uE007" },
              { type: "keyUp", value: "\uE007" },
            ],
          },
        ],
      });
      const outcome = await waitFor(
        sessionId,
        "return window.location.hash === arguments[1] && document.activeElement?.id === arguments[0] ? 'arrived' : document.querySelector('[role=\"dialog\"]')?.textContent ?? null;",
        [heading, href],
      );
      if (outcome === "arrived") return;
      assert(
        !outcome.includes("Your unsubmitted changes"),
        `Keyboard navigation must not discard a dirty Work draft: ${outcome}`,
      );
      assert.match(
        outcome,
        /Native work is still running|This operation cannot be interrupted|The operation has settled/u,
        "Only a native operation may defer a clean route transition",
      );
      await activateButton(sessionId, "Stay here", '[role="dialog"]');
      await waitFor(sessionId, "return !document.querySelector('[role=\"dialog\"]');");
      const previous = await evaluate(sessionId, "return window.location.hash.split('/').at(-1);");
      await loadedRoute(previous);
    }
    assert.fail(`Keyboard navigation to ${route} did not settle after native work`);
  }
  async function tabRoute(heading) {
    assert.equal(
      await evaluate(sessionId, "return document.activeElement?.id;"),
      heading,
      "A loaded route must retain its heading focus before keyboard traversal",
    );
    let previousFocus = await evaluate(sessionId, "return document.activeElement;");
    for (const reverse of [false, true]) {
      await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
        actions: [
          {
            type: "key",
            id: "native-route-tab",
            actions: reverse
              ? [
                  { type: "keyDown", value: "\uE008" },
                  { type: "keyDown", value: "\uE004" },
                  { type: "keyUp", value: "\uE004" },
                  { type: "keyUp", value: "\uE008" },
                ]
              : [
                  { type: "keyDown", value: "\uE004" },
                  { type: "keyUp", value: "\uE004" },
                ],
          },
        ],
      });
      const focus = await evaluate(
        sessionId,
        `const node = document.activeElement;
        const rect = node.getBoundingClientRect();
        return {
          element: node,
          tag: node.tagName,
          changed: node !== arguments[0],
          tabbable: node.tabIndex >= 0,
          visible: rect.top < innerHeight && rect.bottom > 0 && rect.left < innerWidth && rect.right > 0,
          outline: parseFloat(getComputedStyle(node).outlineWidth)
        };`,
        [previousFocus],
      );
      assert(focus.changed, `Tab must move focus to another control: ${JSON.stringify(focus)}`);
      assert(focus.tabbable, `Tab must reach a tabbable control: ${JSON.stringify(focus)}`);
      assert(
        focus.visible,
        `Focused control must remain visible at native zoom: ${JSON.stringify(focus)}`,
      );
      assert(
        focus.outline > 0,
        `Focused control needs a visible outline: ${JSON.stringify(focus)}`,
      );
      previousFocus = focus.element;
    }
  }
  const originalRect = await command(
    "GET",
    `/session/${encodeURIComponent(sessionId)}/window/rect`,
  );
  const enlargedRect = await command(
    "POST",
    `/session/${encodeURIComponent(sessionId)}/window/rect`,
    {
      width: 1360,
      height: 1000,
    },
  );
  assert(enlargedRect.width >= 1280, "400% native zoom needs at least 320 CSS pixels");
  const nativeWindow = await executeFile(xdotoolExecutable, [
    "search",
    "--onlyvisible",
    "--name",
    windowTitle,
  ]);
  await executeFile(xdotoolExecutable, [
    "windowfocus",
    "--sync",
    nativeWindow.stdout.trim().split(/\s+/u)[0],
  ]);
  const initialZoom = await evaluate(
    sessionId,
    "return { ratio: window.devicePixelRatio, width: window.innerWidth };",
  );
  let ratio = initialZoom.ratio;
  let zoomSteps = 0;
  for (const factor of [2, 4]) {
    while (ratio < initialZoom.ratio * (factor - 0.01)) {
      assert(zoomSteps++ < 25, `Native zoom did not reach ${factor}x`);
      await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "ctrl+equal"]);
      ratio = await waitFor(
        sessionId,
        "return window.devicePixelRatio > arguments[0] ? window.devicePixelRatio : false;",
        [ratio + initialZoom.ratio * 0.1],
      );
    }
    assert(ratio <= initialZoom.ratio * (factor + 0.05), `Native zoom passed ${factor}x`);
    for (const [route, heading] of setupRoutes) {
      await keyboardRoute(route, heading);
      await loadedRoute(route);
      await tabRoute(heading);
      if (route === "work") {
        await activateButton(sessionId, "Shifts starting in these dates");
        await waitFor(
          sessionId,
          "return document.querySelector('[aria-labelledby=\"work-window-heading\"] table tbody tr')?.textContent.includes('Sunday repeated-hour duty');",
        );
      } else if (route === "availability") {
        await package7SelectPerson(sessionId, "Native Alice");
        await waitForElement(sessionId, '[aria-labelledby="availability-records-heading"] ul');
      }
      const width = await evaluate(
        sessionId,
        `return {
          viewport: document.documentElement.clientWidth,
          content: document.documentElement.scrollWidth,
          innerWidth: window.innerWidth
        };`,
      );
      assert(
        width.content <= width.viewport + 1,
        `${route} requires page-wide horizontal scrolling at native ${factor}x zoom: ${JSON.stringify(width)}`,
      );
      assert(
        width.innerWidth <= initialZoom.width / (factor - 0.1),
        `Native zoom did not shrink the actual WebKit viewport: ${JSON.stringify(width)}`,
      );
      await screenshot(sessionId, `package8-${route}-native-zoom-${factor}x.png`);
    }
    if (factor === 4) {
      const returnFocus = await evaluate(sessionId, "return document.activeElement;");
      await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "ctrl+k"]);
      await waitFor(
        sessionId,
        `const dialog = document.querySelector('[role="dialog"]');
        const heading = document.getElementById(dialog?.getAttribute('aria-labelledby') ?? '');
        return dialog?.contains(document.activeElement) &&
          document.activeElement?.id === 'command-search' &&
          heading?.tagName === 'H2' && heading.getClientRects().length > 0;`,
      );
      const commandCount = await evaluate(
        sessionId,
        "return document.querySelectorAll('[role=\"dialog\"] button:not(:disabled)').length;",
      );
      assert(commandCount > 1, "The native command dialog must have multiple keyboard choices");
      for (let index = 0; index < commandCount; index += 1) {
        await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
          actions: [
            {
              type: "key",
              id: "native-modal-item-tab",
              actions: [
                { type: "keyDown", value: "\uE004" },
                { type: "keyUp", value: "\uE004" },
              ],
            },
          ],
        });
        await waitFor(
          sessionId,
          `const dialog = document.querySelector('[role="dialog"]');
          const button = dialog?.querySelectorAll('button:not(:disabled)')[arguments[0]];
          const field = button?.getBoundingClientRect();
          const panel = dialog?.getBoundingClientRect();
          return document.activeElement === button && field.top >= Math.max(0, panel.top) &&
            field.bottom <= Math.min(window.innerHeight, panel.bottom);`,
          [index],
        );
      }
      await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
        actions: [
          {
            type: "key",
            id: "native-modal-forward-wrap",
            actions: [
              { type: "keyDown", value: "\uE004" },
              { type: "keyUp", value: "\uE004" },
            ],
          },
        ],
      });
      await waitFor(
        sessionId,
        `const search = document.querySelector('#command-search');
        const dialog = search?.closest('[role="dialog"]');
        const field = search?.getBoundingClientRect();
        const panel = dialog?.getBoundingClientRect();
        return document.activeElement === search && field.top >= Math.max(0, panel.top) &&
          field.bottom <= Math.min(window.innerHeight, panel.bottom);`,
      );
      await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "shift+Tab"]);
      await waitFor(
        sessionId,
        "const dialog = document.querySelector('[role=\"dialog\"]'); return dialog?.contains(document.activeElement) && document.activeElement === [...dialog.querySelectorAll('button')].at(-1);",
      );
      await command("POST", `/session/${encodeURIComponent(sessionId)}/actions`, {
        actions: [
          {
            type: "key",
            id: "native-modal-tab",
            actions: [
              { type: "keyDown", value: "\uE004" },
              { type: "keyUp", value: "\uE004" },
            ],
          },
        ],
      });
      await waitFor(
        sessionId,
        `const search = document.querySelector('#command-search');
        const dialog = search?.closest('[role="dialog"]');
        const field = search?.getBoundingClientRect();
        const panel = dialog?.getBoundingClientRect();
        return document.activeElement === search && field.top >= Math.max(0, panel.top) &&
          field.bottom <= Math.min(window.innerHeight, panel.bottom);`,
      );
      await screenshot(sessionId, "package8-native-modal-zoom-4x.png");
      await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "Escape"]);
      await waitFor(
        sessionId,
        "return !document.querySelector('[role=\"dialog\"]') && document.activeElement === arguments[0];",
        [returnFocus],
      );
    }
  }
  while (ratio > initialZoom.ratio * 1.05) {
    assert(zoomSteps-- > 0, "Native zoom did not restore the starting scale");
    await executeFile(xdotoolExecutable, ["key", "--clearmodifiers", "ctrl+minus"]);
    ratio = await waitFor(
      sessionId,
      "return window.devicePixelRatio < arguments[0] ? window.devicePixelRatio : false;",
      [ratio - initialZoom.ratio * 0.1],
    );
  }
  assert(Math.abs(ratio - initialZoom.ratio) < 0.05, "Native zoom did not restore 100%");
  await command("POST", `/session/${encodeURIComponent(sessionId)}/window/rect`, {
    width: originalRect.width,
    height: originalRect.height,
  });
  console.log(
    "PASS: native 200%/400% WebKit zoom and seven loaded keyboard setup-route reflow checks",
  );
  for (const [route, heading] of setupRoutes) {
    await keyboardRoute(route, heading);
    await loadedRoute(route);
  }
  await navigate(sessionId, "/projects");
  await navigate(sessionId, `/projects?project=${scenarioId}`);
  await activateButton(sessionId, "Delete project");
  await activateButton(sessionId, "Delete permanently", '[role="dialog"]');
  await waitForOutcome(sessionId, "Deleted");
  assert.deepEqual(
    (await projects(sessionId)).map((project) => project.scenarioId),
    [originalScenarioId],
  );
  await navigate(sessionId, `/project/${originalScenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  console.log(
    "PASS: four-day New York scenario, two native-edited eligible people and reviewed Sunday repeated-hour Work template resolved as 120 minutes",
  );
}

async function package8RestartAcceptance(sessionId, scenarioId, expected) {
  const rest = await package8Rule(sessionId, scenarioId, expected.restId);
  const maximum = await package8Rule(sessionId, scenarioId, expected.maximumId);
  const maximumWorkedWeekends = await package8Rule(
    sessionId,
    scenarioId,
    expected.maximumWorkedWeekendsId,
  );
  const quota = await package8Rule(sessionId, scenarioId, expected.quotaId);
  const consecutiveWorkedDays = await package8Rule(
    sessionId,
    scenarioId,
    expected.consecutiveWorkedDaysId,
  );
  const consecutiveAssignments = await package8Rule(
    sessionId,
    scenarioId,
    expected.consecutiveAssignmentsId,
  );
  const consecutiveReportingDays = await package8Rule(
    sessionId,
    scenarioId,
    expected.consecutiveReportingDaysId,
    "preference",
  );
  const consecutiveWeekends = await package8Rule(
    sessionId,
    scenarioId,
    expected.consecutiveWeekendsId,
    "preference",
  );
  const inactive = await package8Rule(sessionId, scenarioId, expected.inactiveId);
  const skillMix = await package8Rule(sessionId, scenarioId, expected.skillMixId);
  assert.equal(rest.record.minimumMinutes, 600);
  assert.equal(rest.record.active, true);
  assert.equal(maximum.record.kind, "maximumAssignmentCount");
  assert.equal(maximum.record.calendarId, expected.calendarId);
  assert.equal(maximum.record.maximum, 3);
  assert.equal(maximumWorkedWeekends.record.kind, "maximumWorkedWeekends");
  assert.deepEqual(maximumWorkedWeekends.record.scope, {
    people: { kind: "all" },
    weekdays: ["sunday"],
  });
  assert.equal(maximumWorkedWeekends.record.maximum, 1);
  assert.equal(quota.record.kind, "workloadQuota");
  assert.deepEqual(quota.record.measure, { kind: "bucketMinutes", bucketId: expected.bucketId });
  assert.equal(quota.record.membership, "startInstant");
  assert.equal(quota.record.minimum, 0);
  assert.equal(quota.record.maximum, 480);
  assert.equal(consecutiveWorkedDays.record.kind, "maximumConsecutive");
  assert.equal(consecutiveWorkedDays.record.maximum, 2);
  assert.deepEqual(consecutiveWorkedDays.record.mode, { kind: "workedDays" });
  assert.equal(consecutiveAssignments.record.kind, "maximumConsecutive");
  assert.equal(consecutiveAssignments.record.maximum, 3);
  assert.deepEqual(consecutiveAssignments.record.mode, { kind: "assignments", breakMinutes: 15 });
  assert.equal(consecutiveReportingDays.record.kind, "consecutiveWork");
  assert.deepEqual(consecutiveReportingDays.record.mode, { kind: "reportingDays" });
  assert.deepEqual(consecutiveReportingDays.record.assignmentTypeIds, [
    expected.preferredAssignmentTypeId,
  ]);
  assert.equal(consecutiveReportingDays.record.threshold, 1);
  assert.equal(consecutiveWeekends.record.kind, "consecutiveWork");
  assert.deepEqual(consecutiveWeekends.record.mode, {
    kind: "weekends",
    calendarId: expected.anchoredWeekCalendarId,
    weekdays: ["saturday", "sunday"],
  });
  assert.deepEqual(consecutiveWeekends.record.assignmentTypeIds, [
    expected.preferredAssignmentTypeId,
  ]);
  assert.equal(consecutiveWeekends.record.threshold, 2);
  const calendarHours = await package8Rule(sessionId, scenarioId, expected.calendarHoursId);
  const rollingHours = await package8Rule(sessionId, scenarioId, expected.rollingHoursId);
  assert.equal(calendarHours.record.kind, "maximumHours");
  assert.equal(calendarHours.record.bucketId, expected.bucketId);
  assert.equal(calendarHours.record.window.calendarId, expected.calendarId);
  assert.equal(calendarHours.record.maximumMinutes, 480);
  assert.equal(rollingHours.record.kind, "maximumHours");
  assert.equal(rollingHours.record.bucketId, expected.bucketId);
  assert.equal(rollingHours.record.window.kind, "rolling");
  assert.equal(rollingHours.record.window.durationMinutes, 1440);
  assert.equal(inactive.record.active, false);
  assert.equal(skillMix.record.kind, "requiredSkillMix");
  assert.equal(skillMix.record.qualificationMinimums[0].minimum, 1);
  assert.deepEqual(skillMix.record.qualificationMinimums[0].qualifications.allQualificationIds, [
    expected.qualificationId,
  ]);
  const weekendTarget = await package8Rule(
    sessionId,
    scenarioId,
    expected.weekendTargetId,
    "preference",
  );
  assert.equal(weekendTarget.record.kind, "weekendTarget");
  assert.equal(weekendTarget.record.targetCount, 2);
  assert.deepEqual(weekendTarget.record.scope.categories, ["package8-no-matching-shift-category"]);
  const workloadBalance = await package8Rule(
    sessionId,
    scenarioId,
    expected.workloadBalanceId,
    "preference",
  );
  assert.equal(workloadBalance.record.kind, "workloadBalance");
  assert.equal(workloadBalance.record.workloadPolicyId, expected.workloadPolicyId);
  assert.equal(workloadBalance.record.priority, "high");
  assert.equal(workloadBalance.record.weight, 17);
  const fixed = await package8Rule(sessionId, scenarioId, expected.fixedId);
  assert.equal(fixed.record.kind, "fixedAssignment");
  assert.equal(fixed.record.personId, expected.fixedPersonId);
  assert.equal(fixed.record.shiftId, expected.fixedShiftId);
  const mutual = await package8Rule(sessionId, scenarioId, expected.mutualId);
  assert.equal(mutual.record.kind, "mutualAssignmentRestriction");
  assert.equal(mutual.record.mode, "overlappingShifts");
  assert.deepEqual([...mutual.record.personIds].sort(), [...expected.mutualPersonIds].sort());
  const transition = await package8Rule(sessionId, scenarioId, expected.transitionId);
  assert.equal(transition.record.kind, "transitionTime");
  assert.equal(transition.record.active, true);
  assert.deepEqual(
    [...transition.record.locationIds].sort(),
    [...expected.transitionLocationIds].sort(),
  );
  // Restore exactly Package 7's history head before its independent restart/undo checks.
  await navigate(sessionId, `/project/${scenarioId}/history`);
  for (;;) {
    const head = await appliedHistoryHead(sessionId, scenarioId);
    assert(head, "History head disappeared before Package 7 was restored");
    if (head.id === expected.historyHead.id) break;
    assert(
      head.historySequence > expected.historyHead.historySequence,
      "The original Package 7 history head was lost before restart",
    );
    await activateButton(sessionId, "Undo scenario change", '[aria-labelledby="history-heading"]');
    await waitForOutcome(sessionId, "Scenario undo committed");
  }
  console.log(
    "PASS: native-edited ConsecutiveWork reporting-day and anchored-weekend preferences (including native rejection of a daily weekend calendar), ten-hour Required rest, reporting-date maximum count, maximum worked weekends, worked-days and assignment-break maximum consecutive, fixed-period and rolling maximum hours, qualification mix, fixed named assignment, mutual restriction and directed travel time, plus persisted workload-balance policy selection, review, restart and native redo; invalid rolling duration and assignment break preserved and focused; new active singleton travel rule blocked by native shift-scope proof; stable rule identity and approval invalidation; active empty-scope refusal and explicit inactive save; full validation and mutation-induced stale status",
  );
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
    await startOperationTiming(firstSessionId);
    if (process.env.EUTHETO_E2E_LARGE_RESULT_ONLY === "1") {
      await packagedLargeResultPerformanceAcceptance(firstSessionId);
      return;
    }
    if (process.env.EUTHETO_E2E_RECIPIENT_EXPORT_ONLY === "1") {
      assert.equal(process.env.EUTHETO_E2E_PACKAGED_SOLVE, "1");
      await packagedRecipientExportAcceptance(firstSessionId, directory);
      return;
    }
    if (process.env.EUTHETO_E2E_KEYBOARD_WORKFLOW_ONLY === "1") {
      assert.equal(process.env.EUTHETO_E2E_PACKAGED_SOLVE, "1");
      await packagedKeyboardWorkflowAcceptance(firstSessionId);
      return;
    }
    if (process.env.EUTHETO_E2E_ELIGIBILITY_MATRIX_ONLY === "1") {
      await packagedEligibilityMatrixAcceptance(firstSessionId, directory);
      return;
    }
    if (process.env.EUTHETO_E2E_AVAILABILITY_CSV_ONLY === "1") {
      await packagedAvailabilityCsvAcceptance(firstSessionId, directory);
      return;
    }
    if (process.env.EUTHETO_E2E_SHIFT_CSV_ONLY === "1") {
      await packagedShiftCsvAcceptance(firstSessionId, directory);
      return;
    }
    if (process.env.EUTHETO_E2E_ELIGIBILITY_CSV_ONLY === "1") {
      await packagedEligibilityCsvAcceptance(firstSessionId, directory);
      return;
    }
    if (process.env.EUTHETO_E2E_ASSIGNMENT_CSV_ONLY === "1") {
      await packagedAssignmentCsvAcceptance(firstSessionId, directory);
      return;
    }
    if (process.env.EUTHETO_E2E_EXTERNAL_PLAN_ONLY === "1") {
      assert.equal(
        process.env.EUTHETO_E2E_PACKAGED_SOLVE,
        "1",
        "External-source acceptance requires the actual bundled worker",
      );
      await packagedExternalPlanAcceptance(firstSessionId);
      return;
    }
    if (process.env.EUTHETO_E2E_ALTERNATIVES_ONLY === "1") {
      assert.equal(
        process.env.EUTHETO_E2E_PACKAGED_SOLVE,
        "1",
        "Focused Alternatives acceptance requires the actual bundled worker",
      );
      await packagedAlternativesAcceptance(firstSessionId);
      return;
    }
    if (process.env.EUTHETO_E2E_CONDITIONAL_ONLY === "1") {
      assert.notEqual(
        process.env.EUTHETO_E2E_CONFLICT_ONLY,
        "1",
        "Focused conditional acceptance is separate from ordinary conflict acceptance",
      );
      assert.equal(
        process.env.EUTHETO_E2E_PACKAGED_SOLVE,
        "1",
        "Focused conditional acceptance requires the actual bundled worker",
      );
      await packagedConditionalCopyAcceptance(firstSessionId);
      return;
    }
    if (process.env.EUTHETO_E2E_CONFLICT_ONLY === "1") {
      assert.equal(
        process.env.EUTHETO_E2E_PACKAGED_SOLVE,
        "1",
        "Focused conflict acceptance requires the actual bundled worker",
      );
      await packagedInfeasibilityAcceptance(firstSessionId, directory, dataHome);
      return;
    }
    if (process.env.EUTHETO_E2E_PACKAGED_SOLVE === "1")
      await packagedKeyboardWorkflowAcceptance(firstSessionId);
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
    await activate(firstSessionId, "#setup-results-details > summary");
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
    const scorePolicy = await scorePolicyAcceptance(firstSessionId, scenarioId, people);
    await peopleBulkAcceptance(firstSessionId, scenarioId, people);
    const importedPeople = await peopleCsvAcceptance(firstSessionId, scenarioId, directory, people);
    const package7Expected = await package7Acceptance(
      firstSessionId,
      scenarioId,
      directory,
      importedPeople,
    );
    const package8Expected = await package8Acceptance(firstSessionId, scenarioId, {
      ...package7Expected,
      calendarId: people.calendarId,
      bucketId: people.bucketId,
      qualificationId: people.qualificationId,
      locationId: people.locationId,
      workloadPolicyId: scorePolicy.workloadPolicyId,
    });
    // The bundled run covers real setup → solve → diagnostics → accepted schedule.
    // The ordinary debug run continues through the independent cancellation/restart suite.
    if (process.env.EUTHETO_E2E_PACKAGED_SOLVE === "1") {
      await packagedBasePreferenceAcceptance(firstSessionId, scenarioId);
      await package8CoverageAcceptance(
        firstSessionId,
        scenarioId,
        package7Expected.shiftInstanceId,
      );
      await package8TemporalAcceptance(firstSessionId, scenarioId);
      await packagedInfeasibilityAcceptance(firstSessionId, directory, dataHome);
      await packagedAlternativesAcceptance(firstSessionId);
      await packagedExternalPlanAcceptance(firstSessionId);
      await packagedRecipientExportAcceptance(firstSessionId, directory);
      await packagedLargeResultPerformanceAcceptance(firstSessionId);
      return;
    }
    const package8Coverage = await package8CoverageAcceptance(
      firstSessionId,
      scenarioId,
      package7Expected.shiftInstanceId,
    );
    const package8Preferences = await package8PreferenceAcceptance(
      firstSessionId,
      scenarioId,
      package7Expected,
      package8Coverage.coverageId,
    );
    await package8CancellationAcceptance(firstSessionId, scenarioId, package7Expected.shiftTypeId);
    await package8TemporalAcceptance(firstSessionId, scenarioId);
    await package8FirstTimeDstWorkAcceptance(
      firstSessionId,
      scenarioId,
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
    await finishOperationTiming(firstSessionId);

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
    await activate(secondSessionId, "#setup-calendar-details > summary");
    assert(
      (await getText(secondSessionId, "#setup-calendar-details")).includes("en-GB"),
      "Scenario settings must persist independently from the application's formatting locale",
    );
    await scorePolicyReopenAcceptance(secondSessionId, scenarioId, scorePolicy);
    await workloadBalanceReopenAcceptance(secondSessionId, scenarioId, package8Expected);
    await package10PerformanceAcceptance(secondSessionId, scenarioId, package7Expected);
    await package8PreferenceRestartAcceptance(secondSessionId, scenarioId, package8Preferences);
    // package7 restart runs BEFORE existing people/csv restart helpers;
    // it undoes all package7 additions back to CSV command head.
    await package8RestartAcceptance(secondSessionId, scenarioId, {
      ...package8Expected,
      historyHead: package7Expected.historyHead,
    });
    await package7RestartAcceptance(secondSessionId, scenarioId, package7Expected);
    await peopleRestartAcceptance(secondSessionId, scenarioId, people.personId);
    await peopleCsvRestartAcceptance(secondSessionId, scenarioId, people, importedPeople);
    await evaluate(secondSessionId, "window.location.hash = '#/unavailable-route';");
    await waitForElement(secondSessionId, "#route-recovery-heading");
    assert.deepEqual(
      (await projects(secondSessionId)).map((project) => project.scenarioId),
      [scenarioId],
    );
    console.log(
      "PASS: native Score Policy route reads the singleton, preserves invalid raw values, rejects invalid workload membership without mutation, commits a mapped level with native people/bucket/calendar EqualShare references, reopens persisted detail after redo and restart, and verifies native undo/redo",
    );
    console.log(
      "PASS: real Workforce creation/setup and one-open-per-entry; native settings CAS/draft/self-commit/import/export/reset; four bounded backup reviews; editable export/unopened exact re-export/import; additive restore, verified safety backup, real failure/bypass/recovery; cancelled export and changed-revision deletion review; unconsumed editing shortcuts and scoped scenario undo/redo; History route committed metadata, one-step undo/redo, authoritative revision refresh and persisted scenario settings; confirmed deletion; offline About; independent restart persistence and unknown-route recovery",
    );
    console.log(
      "PASS: native supporting-record/person creation with temporal grants, existing target/location references and display fields; manual and bulk choices across concurrent revisions; no-op bulk target conflicts; native batch preview/delete and one-step undo/redo; CSV identity/proposal/report/no-change flow and restart-safe history; referenced deletion refusal and fresh-identity recovery",
    );
  } catch (error) {
    if (activeSessionId !== undefined) {
      try {
        await screenshot(activeSessionId, "native-failure.png");
        console.error(
          await evaluate(
            activeSessionId,
            "return { hash: location.hash, text: document.querySelector('main')?.innerText, active: document.activeElement?.outerHTML, details: Array.from(document.querySelectorAll('details')).map(node => ({open: node.open, text: node.innerText})), inputs: Array.from(document.querySelectorAll('input')).map(node => ({id: node.id, type: node.type, value: node.value, disabled: node.disabled, readOnly: node.readOnly})) };",
          ),
        );
      } catch (diagnosticFailure) {
        console.error("Native failure capture could not complete:", diagnosticFailure);
      }
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

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn, execFile } from "node:child_process";
import { mkdir, readFile, readdir, rename, rm, writeFile } from "node:fs/promises";
import { createConnection } from "node:net";
import { cpus, platform, release } from "node:os";
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
  const expected = JSON.parse(
    await readFile(
      new URL("../../../benchmarks/expected/workforce/v1.json", import.meta.url),
      "utf8",
    ),
  );
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
    window.__euthetoPackage10Finish = () => {
      liveObserver.disconnect();
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
      schemaVersion: 2,
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
  const webKitUserAgent = await evaluate(sessionId, "return navigator.userAgent;");
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
  const path = fileURLToPath(
    new URL("../../../.cache/e2e/package10-performance-measurements.json", import.meta.url),
  );
  await mkdir(dirname(path), { recursive: true });
  const evidence = {
    schemaVersion: 1,
    measurement: {
      generatedAt: new Date().toISOString(),
      buildMode: application.includes("/release/") ? "release" : "debug",
      host: {
        os: platform(),
        osRelease: release(),
        arch: process.arch,
        cpu: cpus()[0]?.model ?? "unknown",
        webKitUserAgent,
      },
      coldWarmMethod: {
        cold: "first measured eligibility visit in this native WebKit session",
        warm: "second eligibility route in the same native WebKit session after setup navigation and idle",
        cacheNote: "The method does not flush operating-system or WebKit caches.",
      },
      bounded: {
        maximumInteractionSamples: 32,
        maximumLongTaskEntries: 128,
        maximumLiveRegionMutations: 1024,
      },
    },
    source: sources.manifest,
    nativeScenario: {
      identity: "current native persistence scenario",
      ...nativeScenario,
    },
    cases,
  };
  await writeFile(path, `${JSON.stringify(evidence, null, 2)}\n`);
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

let requestSequence = 100;
function requestId() {
  requestSequence += 1;
  return `01900000-0000-7000-8000-${requestSequence.toString(16).padStart(12, "0")}`;
}

async function nativeRequest(sessionId, name, request, waitMilliseconds = timeout) {
  const response = await command(
    "POST",
    `/session/${encodeURIComponent(sessionId)}/execute/async`,
    {
      script: `const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke(arguments[0], {request: arguments[1]}).then(value => done({ok: value}), error => done({failure: typeof error === 'string' ? error : JSON.stringify(error)}));`,
      args: [name, { requestId: requestId(), ...request }],
    },
    waitMilliseconds,
  );
  assert.equal(response.failure, undefined, `Native ${name} failed: ${response.failure ?? ""}`);
  return response.ok;
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
  await submitForm(sessionId, "#settings-theme");
  await waitFor(
    sessionId,
    "return document.documentElement.dataset.theme === 'dark' && document.querySelector('main[data-operation-active=\"false\"]') !== null;",
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
  const people = entities
    .filter((entity) => entity.kind === "person")
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

  // Return expected state for restart verification.
  return {
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
  const maxUndos = 30;
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

async function package8Rule(sessionId, scenarioId, ruleId) {
  const viewId = "official.workforce.setup.rule_detail";
  return (
    await nativeSetupRequest(sessionId, scenarioId, "scenario_get_view", viewId, {
      source: { kind: "stored" },
      query: { schemaVersion: 1, viewId, parameters: { rule: { class: "required", ruleId } } },
    })
  ).result.view.data.result.data;
}

async function optimizeHandoffAcceptance(sessionId, scenarioId, ready, stateName) {
  const before = (await projects(sessionId)).find((project) => project.scenarioId === scenarioId);
  await navigate(sessionId, `/project/${scenarioId}/setup`);
  await waitForElement(sessionId, "#setup-calendar");
  await activate(sessionId, "#setup-results-details > summary");
  await waitForElement(
    sessionId,
    `[data-optimize-state="${ready ? "ready" : "validation-not-ready"}"]`,
  );
  const capabilities = (await nativeRequest(sessionId, "app_get_capabilities", {})).result;
  assert(capabilities.unavailableCommands.includes("solve_start"));
  assert(capabilities.unavailableCommands.includes("solve_cancel"));
  const visible = await evaluate(
    sessionId,
    `const panel = document.querySelector('[data-optimize-panel]');
    return {
      ready: panel.dataset.optimizeReady,
      controls: panel.querySelectorAll('button, a[href], input, select, textarea, [role="button"]').length,
      terms: [...panel.querySelectorAll('dt')].map(node => node.textContent.trim()),
      commandStates: [...panel.querySelectorAll('dd')].slice(0, 2).map(node => node.textContent)
    };`,
  );
  assert.equal(visible.ready, String(ready));
  assert.equal(
    visible.controls,
    0,
    "An unavailable native solve flow must not offer execution controls",
  );
  if (ready) {
    for (const mode of ["Quick", "Balanced", "Deep"]) assert(visible.terms.includes(mode));
    assert(visible.commandStates.every((state) => /\bunavailable\b/i.test(state)));
  } else {
    assert.equal(
      visible.terms.length,
      0,
      "Old or absent validation must not offer mode information as ready",
    );
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

async function package8Acceptance(sessionId, scenarioId, expected) {
  await optimizeHandoffAcceptance(sessionId, scenarioId, false, "unvalidated");
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
  await activateButton(sessionId, "Review rule changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('600 minutes') && [...review.querySelectorAll('button')].some(b => b.textContent.trim() === 'Save reviewed rule' && !b.disabled);",
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
  await activateButton(sessionId, "Review rule changes");
  await activateButton(sessionId, "Save reviewed rule");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const rest = await package8Rule(sessionId, scenarioId, restId);
  assert.equal(rest.record.minimumMinutes, 600);
  assert.equal(rest.record.active, true);

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
  await activateButton(sessionId, "Review rule changes");
  await waitFor(
    sessionId,
    "const review = document.querySelector('[aria-labelledby=\"rule-review-heading\"]'); return review?.textContent.includes('An active rule needs a successful preview') && [...review.querySelectorAll('dl > div')].some(row => row.querySelector('dt').textContent === 'People covered' && row.querySelector('dd').textContent === '0');",
  );
  assert.equal(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Save reviewed rule')?.disabled;",
    ),
    true,
  );
  await screenshot(sessionId, "package8-empty-active-scope.png");
  await activate(sessionId, "#rule-active");
  await waitFor(sessionId, "return !document.querySelector('#rule-review-heading');");
  await activateButton(sessionId, "Review rule changes");
  await activateButton(sessionId, "Save reviewed rule");
  await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
  const inactive = await package8Rule(sessionId, scenarioId, inactiveId);
  assert.equal(inactive.record.active, false);
  assert.deepEqual(inactive.record.scope.people.allTags, ["package8-no-person-has-this-tag"]);
  await navigate(sessionId, `/project/${scenarioId}/validation`);
  await waitForElement(sessionId, "[data-full-validation-stale]");
  await screenshot(sessionId, "package8-stale-validation.png");
  await optimizeHandoffAcceptance(sessionId, scenarioId, false, "stale");
  return { restId, inactiveId, counts };
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
    await activateButton(sessionId, "Review rule changes");
    await activateButton(sessionId, "Save reviewed rule");
    await waitFor(sessionId, "return !document.querySelector('#rule-editor-heading');");
    const saved = await package8Rule(sessionId, scenarioId, id);
    assert.equal(saved.record.kind, kind);
    assert.equal(saved.record.active, true);
    if (kind === "noOverlap")
      assert.deepEqual(saved.record.compatibleCategoryPairs, [
        { firstCategory: "call", secondCategory: "clinic" },
      ]);
  }
  assert.equal(
    await evaluate(
      sessionId,
      "return [...document.querySelectorAll('#rule-catalog-heading ~ ul button')].length;",
    ),
    5,
    "Only the five implemented Required kinds may offer creation",
  );
  await selectValue(sessionId, "#rule-class-filter", "preference");
  await waitFor(
    sessionId,
    "const list = document.querySelector('[aria-labelledby=\"rule-list-heading\"]'); return list?.getAttribute('aria-busy') === 'false' && list.querySelectorAll('ul li button').length === 0;",
  );
  await screenshot(sessionId, "package8-preferences-unavailable.png");
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
  await navigate(sessionId, `/project/${scenarioId}/history`);
  for (let index = 0; index < 5; index += 1) {
    await activateButton(sessionId, "Undo scenario change", '[aria-labelledby="history-heading"]');
    await waitForOutcome(sessionId, "Scenario undo committed");
  }
  console.log(
    "PASS: all five native Required kinds; stable category-pair typing; unavailable preferences; bounded selected-shift standalone coverage creation; native shortage -> exact owner/count focus without mutation -> reviewed repair",
  );
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

async function package8FirstTimeDstWorkAcceptance(sessionId, originalScenarioId) {
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
  await setValue(sessionId, `${workEditor} input[id$="-timing-start"]`, "25:61");
  await activateButton(sessionId, "Review changes", workEditor);
  await waitForElement(sessionId, '[aria-label="Native time and input diagnostics"]');
  await idle(sessionId);
  assert.equal(
    await evaluate(
      sessionId,
      'return document.querySelector(\'[aria-labelledby="work-editor-heading"] input[id$="-timing-start"]\')?.value;',
    ),
    "25:61",
    "Rejected local-time input must remain available in the draft",
  );
  assert.equal(
    await evaluate(sessionId, "return !!document.querySelector('#work-review-heading');"),
    false,
    "Rejected local-time input must not open an approvable review",
  );
  const invalidFocus = await evaluate(sessionId, "return document.activeElement?.id;");
  console.log("Native invalid local-time focus:", invalidFocus);
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
  assert(enlargedRect.width >= 1280, "400% reflow needs at least 320 CSS pixels");
  for (const factor of [2, 4]) {
    const width = await evaluate(
      sessionId,
      `document.documentElement.style.zoom = arguments[0];
      return {
        viewport: document.documentElement.clientWidth,
        content: document.documentElement.scrollWidth
      };`,
      [String(factor)],
    );
    await screenshot(sessionId, `package8-work-css-zoom-${factor}x.png`);
    assert(
      width.content <= width.viewport + 1,
      `Work view must not require page-wide horizontal scrolling at CSS zoom ${factor}x: ${JSON.stringify(width)}`,
    );
  }
  await evaluate(sessionId, "document.documentElement.style.zoom = '';");
  await command("POST", `/session/${encodeURIComponent(sessionId)}/window/rect`, {
    width: originalRect.width,
    height: originalRect.height,
  });
  for (const [route, heading] of [
    ["people", "people-heading"],
    ["work", "work-heading"],
    ["rules", "rules-heading"],
    ["validation", "validation-heading"],
    ["setup", "setup-heading"],
  ]) {
    const link = `nav[aria-label="Saved setup sections"] a[href="#/project/${scenarioId}/${route}"]`;
    let arrived = false;
    for (let attempt = 0; attempt < 2 && !arrived; attempt += 1) {
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
        "return document.activeElement?.id === arguments[0] ? 'arrived' : document.querySelector('[role=\"dialog\"]')?.textContent ?? null;",
        [heading],
      );
      if (outcome === "arrived") {
        arrived = true;
        break;
      }
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
      await waitFor(
        sessionId,
        'return !document.querySelector(\'[role="dialog"]\') && !document.querySelector(\'[aria-labelledby="work-window-heading"] > p[role="status"]\');',
      );
      await idle(sessionId);
    }
    assert(arrived, `Keyboard navigation to ${route} did not settle after native work`);
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
  const inactive = await package8Rule(sessionId, scenarioId, expected.inactiveId);
  assert.equal(rest.record.minimumMinutes, 600);
  assert.equal(rest.record.active, true);
  assert.equal(inactive.record.active, false);
  // Restore package7's history head before its independent restart/undo checks.
  await navigate(sessionId, `/project/${scenarioId}/history`);
  for (let index = 0; index < 2; index += 1) {
    await activateButton(sessionId, "Undo scenario change", '[aria-labelledby="history-heading"]');
    await waitForOutcome(sessionId, "Scenario undo committed");
  }
  console.log(
    "PASS: native ten-hour Required rest review/save/restart; stable rule identity and approval invalidation; active empty-scope refusal and explicit inactive save; full validation and mutation-induced stale status",
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
    await peopleBulkAcceptance(firstSessionId, scenarioId, people);
    const importedPeople = await peopleCsvAcceptance(firstSessionId, scenarioId, directory, people);
    const package7Expected = await package7Acceptance(
      firstSessionId,
      scenarioId,
      directory,
      importedPeople,
    );
    const package8Expected = await package8Acceptance(firstSessionId, scenarioId, package7Expected);
    await package8CoverageAcceptance(firstSessionId, scenarioId, package7Expected.shiftInstanceId);
    await package8CancellationAcceptance(firstSessionId, scenarioId, package7Expected.shiftTypeId);
    await package8TemporalAcceptance(firstSessionId, scenarioId);
    await package8FirstTimeDstWorkAcceptance(firstSessionId, scenarioId);
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
    await package10PerformanceAcceptance(secondSessionId, scenarioId, package7Expected);
    // package7 restart runs BEFORE existing people/csv restart helpers;
    // it undoes all package7 additions back to CSV command head.
    await package8RestartAcceptance(secondSessionId, scenarioId, package8Expected);
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

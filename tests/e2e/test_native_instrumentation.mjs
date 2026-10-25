// SPDX-License-Identifier: Apache-2.0
// Execute the authored in-page probes without starting a native desktop.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { runInNewContext } from "node:vm";

const source = await readFile(
  new URL(
    "../../apps/desktop/e2e/project-home.persistence.mjs",
    import.meta.url,
  ),
  "utf8",
);
const setValueHelper = source.slice(
  source.indexOf("async function setValue("),
  source.indexOf("async function getText("),
);
const workWindowStart = source.indexOf(
  '  await setValue(sessionId, "#work-window-start", "01/15/2030");',
);
const workWindowSetup = source.slice(
  workWindowStart,
  source.indexOf("  // The completed native window read", workWindowStart),
);
for (const lost of [[], ["start"], ["end"], ["start", "end"]]) {
  for (const persistent of lost.length === 0 ? [false] : [false, true]) {
    const controls = new Map(
      [
        ["start", "01/15/2030", "2030-01-15", "2030-01-05"],
        ["end", "01/18/2030", "2030-01-18", "2030-01-08"],
      ].map(([name, typed, expected, truncated]) => [
        `#work-window-${name}`,
        { name, typed, expected, truncated, value: "", attempts: 0 },
      ]),
    );
    let reads = 0;
    const run = () =>
      runInNewContext(
        `${setValueHelper}
         async function evaluate(_session, script, args = []) {
           return Function(script)(...args);
         }
         (async () => { ${workWindowSetup} })();`,
        {
          assert,
          sessionId: "fixture",
          waitForElement: (_session, selector) =>
            selector.replace(":not(:disabled):not([readonly])", ""),
          document: {
            querySelector: (selector) => ({
              type: "date",
              get value() {
                return controls.get(selector).value;
              },
              set value(_value) {
                assert.fail("Date setup must use native keyboard entry");
              },
            }),
          },
          command: async (method, path, payload) => {
            assert.equal(method, "POST");
            const match =
              /^\/session\/fixture\/element\/([^/]+)\/(clear|value)$/u.exec(
                path,
              );
            assert(
              match,
              "Date setup must only clear and type through WebDriver",
            );
            const control = controls.get(decodeURIComponent(match[1]));
            assert(control);
            if (match[2] === "clear") {
              control.value = "";
              return;
            }
            assert.equal(payload.text, control.typed);
            assert.deepEqual(
              Array.from(payload.value),
              Array.from(control.typed),
            );
            control.attempts += 1;
            control.value =
              lost.includes(control.name) &&
              (persistent || control.attempts === 1)
                ? control.truncated
                : control.expected;
          },
          activateButton: (_session, label) => {
            assert.equal(label, "Read this local-date window");
            reads += 1;
          },
        },
      );
    if (persistent) {
      await assert.rejects(
        run,
        /intended local-date window before native review/u,
      );
      assert.equal(
        reads,
        0,
        "Incorrect input must fail before requesting a native window read",
      );
    } else {
      await run();
      assert.equal(reads, 1);
    }
    for (const control of controls.values()) {
      assert.equal(
        control.attempts,
        lost.includes(control.name) ? 2 : 1,
        "Only a mismatched control may receive one bounded keyboard re-entry",
      );
    }
  }
}
const recipientModeSelection = source.slice(
  source.indexOf("function assertRecipientModeSelection("),
  source.indexOf("async function run()"),
);
for (const selected of [
  [],
  ["EUTHETO_E2E_RECIPIENT_EXPORT_ONLY"],
  ["EUTHETO_E2E_PRACTITIONER_EXPORT_ONLY"],
  ["EUTHETO_E2E_PDF_CANCELLATION_ONLY"],
]) {
  runInNewContext(
    `${recipientModeSelection}; assertRecipientModeSelection(environment);`,
    {
      assert,
      environment: Object.fromEntries(selected.map((name) => [name, "1"])),
    },
  );
}
assert.throws(
  () =>
    runInNewContext(
      `${recipientModeSelection}; assertRecipientModeSelection(environment);`,
      {
        assert,
        environment: {
          EUTHETO_E2E_PRACTITIONER_EXPORT_ONLY: "1",
          EUTHETO_E2E_PDF_CANCELLATION_ONLY: "1",
        },
      },
    ),
  /mutually exclusive/u,
);
const instrumentation = source.slice(
  source.indexOf("async function package10StartPageInstrumentation("),
  source.indexOf("async function package10AwaitSample("),
);

async function measureLongTasks(
  PerformanceObserver,
  {
    startTimeMs = 0,
    endTimeMs = 200,
    mutations = [],
    queuedMutations = [],
  } = {},
) {
  const observations = [];
  let lagTimerStarted = false;
  let now = startTimeMs;
  const page = {
    window: {},
    document: { documentElement: {} },
    MutationObserver: class {
      constructor(callback) {
        this.callback = callback;
      }
      observe() {
        this.callback(mutations);
      }
      takeRecords() {
        return queuedMutations.splice(0);
      }
      disconnect() {
        queuedMutations.length = 0;
      }
    },
    performance: { now: () => now },
    setInterval: () => {
      lagTimerStarted = true;
      return 1;
    },
    clearInterval() {},
    ...(PerformanceObserver === undefined
      ? {}
      : { PerformanceObserver: PerformanceObserver(observations) }),
  };
  await runInNewContext(
    `${instrumentation}; package10StartPageInstrumentation('test');`,
    {
      evaluate: (_sessionId, script) => runInNewContext(script, page),
    },
  );
  now = endTimeMs;
  const finish = (at = now) => {
    now = at;
    return JSON.parse(JSON.stringify(page.window.__euthetoPackage10Finish()));
  };
  return {
    result: finish(),
    finish,
    observations,
    lagTimerStarted,
  };
}

for (const supportedEntryTypes of [undefined, [], ["resource", "paint"]]) {
  const measured = await measureLongTasks(
    (observations) =>
      class {
        static supportedEntryTypes = supportedEntryTypes;
        observe(options) {
          observations.push(options); // Unsupported types may silently succeed in WebKit.
        }
        takeRecords() {
          return [];
        }
        disconnect() {}
      },
  );
  assert.equal(measured.result.longTask.supported, false);
  assert.equal(typeof measured.result.longTask.unsupportedReason, "string");
  assert.equal(measured.observations.length, 0);
  assert.equal(measured.lagTimerStarted, true);
  assert.equal(measured.result.eventLoopLag.status, "NON-equivalent");
}
assert.equal((await measureLongTasks()).result.longTask.supported, false);
const supported = await measureLongTasks(
  (observations) =>
    class {
      static supportedEntryTypes = ["longtask"];
      observe(options) {
        assert.equal(options.type, "longtask");
        assert.equal(options.buffered, true);
        assert.equal(options.entryTypes, undefined);
        observations.push(options);
      }
      takeRecords() {
        return [];
      }
      disconnect() {}
    },
);
assert.equal(supported.result.longTask.supported, true);
assert.equal(supported.result.longTask.unsupportedReason, null);
assert.equal(supported.lagTimerStarted, false);
assert.equal(supported.observations.length, 1);
const rejected = await measureLongTasks(
  () =>
    class {
      static supportedEntryTypes = ["longtask"];
      observe() {
        throw new Error("unsupported observer options");
      }
      disconnect() {}
    },
);
assert.equal(rejected.result.longTask.supported, false);
assert.equal(rejected.lagTimerStarted, true);

function taskObserver(delivered, queued = []) {
  return () =>
    class {
      static supportedEntryTypes = ["longtask"];
      constructor(callback) {
        this.callback = callback;
      }
      observe() {
        this.callback({ getEntries: () => delivered });
      }
      takeRecords() {
        return queued.splice(0);
      }
      disconnect() {
        queued.length = 0; // Like the browser, disconnect discards undelivered records.
      }
    };
}
const task = (startTime, duration = 60) => ({ startTime, duration });
const liveRegion = { nodeType: 1, matches: () => true };
const liveMutation = { target: { parentElement: liveRegion } };
const ordinaryMutation = { target: { nodeType: 1, matches: () => false } };
const bounded = await measureLongTasks(
  taskObserver(
    [...Array.from({ length: 130 }, () => task(99.9999)), task(100)],
    [task(99), task(150, 90), task(200), task(200.0001)],
  ),
  {
    startTimeMs: 100,
    mutations: [liveMutation, ordinaryMutation],
    queuedMutations: [ordinaryMutation, liveMutation],
  },
);
assert.deepEqual(
  {
    entries: bounded.result.longTask.entries,
    overflow: bounded.result.longTask.entryOverflow,
    liveCount: bounded.result.liveRegionMutationCount,
  },
  {
    entries: [
      { startTimeMs: 100, durationMs: 60 },
      { startTimeMs: 150, durationMs: 90 },
      { startTimeMs: 200, durationMs: 60 },
    ],
    overflow: false,
    liveCount: 2,
  },
  "Only task starts in the capture window count, including queued terminal records",
);
assert.equal(bounded.result.longTask.captureStartTimeMs, 100);
assert.equal(bounded.result.longTask.captureEndTimeMs, 200);
assert.match(
  bounded.result.longTask.samplingBoundary,
  /records available at finish/u,
);
assert.deepEqual(
  bounded.finish(300),
  bounded.result,
  "Later finish calls preserve the capture end and do not count drained records twice",
);

for (const overflow of [false, true]) {
  const capped = await measureLongTasks(
    taskObserver(
      Array.from({ length: 127 }, () => task(100)),
      Array.from({ length: overflow ? 2 : 1 }, () => task(150)),
    ),
    {
      startTimeMs: 100,
      mutations: Array.from({ length: 1023 }, () => liveMutation),
      queuedMutations: [
        ordinaryMutation,
        ...Array.from({ length: overflow ? 2 : 1 }, () => liveMutation),
      ],
    },
  );
  assert.equal(capped.result.longTask.entryCount, 128);
  assert.equal(capped.result.longTask.entryOverflow, overflow);
  assert.equal(capped.result.liveRegionMutationCount, 1024);
  assert.equal(capped.result.liveRegionMutationOverflow, overflow);
  assert.deepEqual(capped.finish(), capped.result);
}

async function interactionPage(longTasks = false) {
  let now = 100;
  let reads = 0;
  const frames = [];
  const timers = [];
  const intervals = [];
  const observers = [];
  const taskObservers = [];
  const listeners = new Map();
  const target = {
    addEventListener: (type, listener) => listeners.set(type, listener),
    removeEventListener: (type) => listeners.delete(type),
  };
  let content = {
    kind: "people",
    totalItems: 0,
    pageNumber: 1,
    returnedItems: 0,
  };
  const rows = () =>
    Array.from(
      { length: content.renderedRows ?? content.returnedItems },
      () => ({
        textContent: content.firstLabel,
        dataset: { index: content.firstIndex },
      }),
    );
  const table = {
    get caption() {
      return { textContent: content.caption };
    },
    get tBodies() {
      return [{ rows: rows() }];
    },
    querySelector: () => ({ textContent: content.startDate }),
    getAttribute: () => String(content.returnedItems + 1),
    classList: { contains: () => content.calendarMode === "virtual" },
  };
  const resultRoot = () => ({
    querySelector: (selector) =>
      selector.startsWith("input[value=")
        ? { checked: selector === 'input[value="' + content.kind + '"]' }
        : null,
    querySelectorAll: (selector) =>
      selector === ':scope > p[role="status"]'
        ? [
            {
              textContent: `${content.totalItems} saved rows · page ${content.pageNumber}`,
            },
          ]
        : selector === "table" && content.returnedItems > 0
          ? [table]
          : selector === "table tbody tr"
            ? rows()
            : [],
  });
  let root = resultRoot();
  const page = {
    window: {},
    document: {
      documentElement: {},
      querySelector(selector) {
        reads += 1;
        return selector === "[data-accepted-schedule]" ? root : target;
      },
    },
    performance: { now: () => now },
    requestAnimationFrame: (callback) => frames.push(callback),
    setTimeout: (callback) => timers.push(callback),
    clearTimeout() {},
    setInterval: (callback) => intervals.push(callback),
    clearInterval() {},
    MutationObserver: class {
      constructor(callback) {
        this.callback = callback;
        observers.push(this);
      }
      observe(node) {
        assert(node, "Observation requires an attached result root");
      }
      takeRecords() {
        return [];
      }
      disconnect() {}
    },
    ...(longTasks
      ? {
          PerformanceObserver: class {
            static supportedEntryTypes = ["longtask"];
            constructor(callback) {
              taskObservers.push(callback);
            }
            observe() {}
            takeRecords() {
              return [];
            }
            disconnect() {}
          },
        }
      : {}),
  };
  await runInNewContext(
    `${instrumentation}; package10StartPageInstrumentation('test');`,
    { evaluate: (_sessionId, script) => runInNewContext(script, page) },
  );
  const api = page.window;
  const snapshot = (value) => JSON.parse(JSON.stringify(value));
  return {
    api,
    arm: (kind = "input") => api.__euthetoPackage10Arm(kind, "#target", "cold"),
    armResult: (expected = content) =>
      api.__euthetoPackage10ArmResult("#target", expected, {
        temperature: "cold",
      }),
    fire: (type = "input") => listeners.get(type)?.({ key: "Enter" }),
    listener: (type = "input") => listeners.get(type),
    frame() {
      now += 10;
      assert(frames.length, "Expected a queued animation frame");
      frames.shift()();
    },
    flush() {
      while (frames.length) this.frame();
    },
    frames,
    timers,
    intervals,
    observers,
    taskObservers,
    reads: () => reads,
    read: (token) => snapshot(api.__euthetoPackage10ReadSample(token)),
    finish: () => snapshot(api.__euthetoPackage10Finish()),
    show: (next) => (content = next),
    detach: () => (root = null),
    replace: () => (root = resultRoot()),
  };
}

for (const expected of [
  { kind: "people", pageNumber: 1, returnedItems: 2 },
  { kind: "shifts", pageNumber: 2, returnedItems: 2 },
  {
    kind: "calendarPeople",
    pageNumber: 1,
    returnedItems: 10,
    calendarMode: "virtual",
    renderedRows: 4,
    firstIndex: 0,
  },
  {
    kind: "calendarShifts",
    pageNumber: 2,
    returnedItems: 10,
    calendarMode: "paged",
    renderedRows: 2,
    firstIndex: 8,
  },
]) {
  Object.assign(expected, {
    totalItems: 12,
    caption: "Saved rows",
    firstLabel: "Synthetic row",
    startDate: "2026-10-22",
  });
  const page = await interactionPage();
  const armed = page.armResult(expected);
  page.fire("keydown");
  page.frame();
  assert.equal(
    page.frames.length,
    0,
    "Old view content cannot complete the sample",
  );
  page.show(expected);
  page.observers[1].callback();
  page.flush();
  const measured = page.read(armed.token);
  assert.equal(
    measured.status,
    "done",
    "View/page/calendar changes within the root remain measurable",
  );
  assert.equal(
    measured.sample.renderedRows,
    expected.renderedRows ?? expected.returnedItems,
  );
}

for (const resultProbe of [false, true]) {
  const arm = (page) => (resultProbe ? page.armResult() : page.arm());
  const fire = (page) => page.fire(resultProbe ? "keydown" : "input");
  const normal = await interactionPage();
  const completed = arm(normal);
  const completedListener = normal.listener(resultProbe ? "keydown" : "input");
  fire(normal);
  normal.flush();
  const sample = normal.read(completed.token);
  assert.equal(sample.status, "done");
  assert.equal(
    resultProbe
      ? sample.sample.eventToSettledContentDoubleAnimationFrameMs
      : sample.sample.eventToDoubleAnimationFrameMs,
    resultProbe ? 30 : 20,
  );
  completedListener({ key: "Enter" });
  for (const timer of normal.timers) timer();
  for (const observer of normal.observers.slice(1)) observer.callback();
  normal.flush();
  assert.deepEqual(normal.read(completed.token), sample);
  assert.equal(normal.finish().samples.length, 1);

  // Exercise cancellation before the event, after it, and between each queued frame.
  for (const frameCount of [-1, 0, 1, ...(resultProbe ? [2] : [])]) {
    for (const finish of [false, true]) {
      const page = await interactionPage();
      const armed = arm(page);
      const staleListener = page.listener(resultProbe ? "keydown" : "input");
      if (frameCount >= 0) {
        fire(page);
        for (let i = 0; i < frameCount; i += 1) page.frame();
      }
      const captured = finish ? page.finish() : null;
      if (!finish) page.api.__euthetoPackage10CancelSample(armed.token);
      const cancelled = page.read(armed.token);
      const reads = page.reads();
      staleListener({ key: "Enter" });
      for (const observer of page.observers.slice(1)) observer.callback();
      for (const timer of page.timers) timer();
      page.flush();
      assert.equal(
        page.reads(),
        reads,
        "Cancelled probes must stop reading DOM",
      );
      assert.equal(cancelled.status, "unavailable");
      assert.deepEqual(page.read(armed.token), cancelled);
      assert.equal(page.finish().samples.length, 0);
      if (finish) assert.deepEqual(page.finish(), captured);
    }
  }
}

for (const longTasks of [false, true]) {
  const page = await interactionPage(longTasks);
  const captured = page.finish();
  const reads = page.reads();
  assert.equal(page.arm().status, "unavailable");
  assert.equal(page.armResult().status, "unavailable");
  assert.equal(
    page.reads(),
    reads,
    "Finished captures refuse admission before DOM reads",
  );
  page.observers[0].callback([liveMutation]);
  for (const callback of page.taskObservers)
    callback({ getEntries: () => [task(100)] });
  for (const callback of page.intervals) callback();
  assert.deepEqual(
    page.finish(),
    captured,
    "Stale callbacks cannot change a finished capture",
  );
}

for (const change of ["detach", "replace"]) {
  for (const afterEvent of [false, true]) {
    const page = await interactionPage();
    const armed = page.armResult();
    if (afterEvent) {
      page.fire("keydown");
      page.frame();
      page.frame();
    }
    page[change]();
    if (!afterEvent) page.fire("keydown");
    page.flush();
    assert.equal(page.read(armed.token).status, "unavailable");
    assert.equal(page.finish().samples.length, 0);
  }
}

for (const frameCount of [-1, 0, 1, 2]) {
  const page = await interactionPage();
  const armed = page.armResult();
  if (frameCount >= 0) {
    page.fire("keydown");
    for (let i = 0; i < frameCount; i += 1) page.frame();
  }
  page.timers[0]();
  const expired = page.read(armed.token);
  const reads = page.reads();
  for (const observer of page.observers.slice(1)) observer.callback();
  page.flush();
  assert.equal(expired.status, "unavailable");
  assert.deepEqual(page.read(armed.token), expired);
  assert.equal(page.reads(), reads, "Expired probes must stop reading DOM");
  assert.equal(page.finish().samples.length, 0);
}

const sampleCap = await interactionPage();
for (let i = 0; i < 33; i += 1) {
  const armed = sampleCap.arm();
  sampleCap.fire();
  sampleCap.flush();
  assert.equal(sampleCap.read(armed.token).status, "done");
}
assert.equal(sampleCap.finish().samples.length, 32);
assert.equal(sampleCap.finish().sampleOverflow, true);

const interactionHelper = source.slice(
  source.indexOf("async function package10InteractionSample("),
  source.indexOf("async function package10MatrixKeyboardSample("),
);
for (const failureAt of ["trigger", "wait", null]) {
  const page = await interactionPage();
  const measured = await runInNewContext(
    `${interactionHelper}; package10InteractionSample('test', 'input', '#target', 'cold', trigger);`,
    {
      evaluate: async (_sessionId, script, args) =>
        runInNewContext(`(function () { ${script} })(...args)`, {
          window: page.api,
          args,
        }),
      waitForElement: async () => {},
      trigger: async () => {
        if (failureAt === "trigger") throw new Error("trigger failed");
        page.fire();
      },
      package10AwaitSample: async (_sessionId, token) => {
        if (failureAt === "wait") throw new Error("wait failed");
        page.flush();
        return page.api.__euthetoPackage10ReadSample(token);
      },
    },
  );
  assert.equal(measured.status, failureAt ? "unavailable" : "measured");
  const pending = page.read("1");
  page.fire();
  page.flush();
  assert.equal(pending.status, failureAt ? "unavailable" : "done");
  assert.deepEqual(page.read("1"), pending);
  assert.equal(page.finish().samples.length, failureAt ? 0 : 1);
}

const cleanupFailure = await runInNewContext(
  `${interactionHelper}; package10InteractionSample('test', 'input', '#target', 'cold', trigger);`,
  {
    Error,
    waitForElement: async () => {},
    evaluate: async (_sessionId, script) => {
      if (script.includes("CancelSample"))
        throw new Error("page unavailable during cleanup");
      return { status: "armed", token: "1" };
    },
    trigger: async () => {
      throw new Error("trigger failed");
    },
  },
);
assert.equal(cleanupFailure.status, "unavailable");
assert.equal(cleanupFailure.reason, "trigger failed");

function captureScript(marker) {
  const at = source.indexOf(marker);
  assert(at >= 0);
  return source.slice(
    source.lastIndexOf("`", at - 1) + 1,
    source.indexOf("`", at),
  );
}

function validationPage() {
  const listeners = new Map();
  let running = false;
  let observer;
  const document = {
    activeElement: null,
    querySelector: (selector) =>
      selector === '[aria-labelledby="validation-heading"]'
        ? section
        : running
          ? {}
          : null,
    addEventListener: (kind, listener) => listeners.set(kind, listener),
    removeEventListener: (kind) => listeners.delete(kind),
  };
  const button = (textContent) => ({
    textContent,
    disabled: false,
    focus() {
      document.activeElement = this;
    },
  });
  const run = button("Run full validation");
  const cancel = button("Request cancellation");
  const search = { id: "command-search" };
  const section = {
    querySelectorAll: () => (running ? [run, cancel] : [run]),
    querySelector: () => (running ? {} : null),
  };
  return {
    page: {
      window: {},
      document,
      MutationObserver: class {
        constructor(callback) {
          observer = callback;
        }
        observe() {}
        disconnect() {}
      },
    },
    begin() {
      running = true;
      observer?.();
    },
    settle() {
      running = false;
    },
    type() {
      document.activeElement = search;
      listeners.get("input")?.({ target: search });
    },
    cancel() {
      listeners.get("keydown")?.({ key: "Enter", target: cancel });
    },
  };
}

// A terminal transition before either event must remain visible to the assertions.
for (const settled of [false, true]) {
  const input = validationPage();
  runInNewContext(
    captureScript("window.__validationInputEvidence = null;"),
    input.page,
  );
  input.begin();
  if (settled) input.settle();
  input.type();
  assert.equal(
    input.page.window.__validationInputEvidence.focused,
    "command-search",
  );
  assert.equal(input.page.window.__validationInputEvidence.running, !settled);

  const cancellation = validationPage();
  runInNewContext(
    captureScript("window.__validationCancelEvidence = null;"),
    cancellation.page,
  );
  cancellation.begin();
  assert.equal(
    cancellation.page.document.activeElement.textContent,
    "Request cancellation",
  );
  if (settled) cancellation.settle();
  cancellation.cancel();
  assert.equal(
    cancellation.page.window.__validationCancelEvidence.focused,
    true,
  );
  assert.equal(
    cancellation.page.window.__validationCancelEvidence.running,
    !settled,
  );
  cancellation.page.window.__validationCancelCleanup();
}

const selectHelper = source.slice(
  source.indexOf("async function selectValue("),
  source.indexOf("async function submitForm("),
);
let changes = 0;
let writes = 0;
class NativeSelect {
  disabled = true;
  options = [];
  selected = "";
  matches() {
    return this.disabled;
  }
  get value() {
    return this.selected;
  }
  set value(value) {
    writes += 1;
    this.selected = value;
  }
  onChange = null;
  dispatchEvent(event) {
    assert.equal(event.type, "change");
    changes += 1;
    this.onChange?.();
  }
}
const select = new NativeSelect();
const target = { value: "native-person-id", disabled: false };
const evaluateSelect = (_session, script, args) =>
  runInNewContext(`(function () { ${script} })(...args)`, {
    document: { querySelector: () => select },
    HTMLSelectElement: NativeSelect,
    Event,
    args,
  });
await runInNewContext(
  `${selectHelper}; selectValue('fixture', 'select', 'native-person-id');`,
  {
    assert,
    evaluate: evaluateSelect,
    waitFor: async (session, script, args) => {
      select.options = [target];
      assert.equal(
        evaluateSelect(session, script, args),
        false,
        "Disabled select must wait",
      );
      select.disabled = false;
      select.options = [];
      assert.equal(
        evaluateSelect(session, script, args),
        false,
        "Missing native option must wait",
      );
      target.disabled = true;
      select.options = [target];
      assert.equal(
        evaluateSelect(session, script, args),
        false,
        "Disabled option must wait",
      );
      target.disabled = false;
      assert.equal(evaluateSelect(session, script, args), true);
      assert.equal(writes, 0, "Loading must not receive a premature selection");
    },
  },
);
assert.equal(select.value, target.value);
assert.equal(writes, 1);
assert.equal(changes, 1);

// A controlled native-backed select can retain its old value until inspection succeeds.
const inspection = Promise.withResolvers();
select.onChange = () => {
  const requested = select.value;
  select.selected = "previous-native-person";
  void inspection.promise.then(() => {
    select.selected = requested;
  });
};
await runInNewContext(
  `${selectHelper}; selectValue('fixture', 'select', 'native-person-id');`,
  {
    assert,
    evaluate: evaluateSelect,
    waitFor: async (session, script, args) => {
      assert.equal(evaluateSelect(session, script, args), true);
    },
  },
);
assert.equal(select.value, "previous-native-person");
assert.equal(changes, 2, "Async inspection must receive exactly one change");
assert.equal(
  writes,
  2,
  "The helper must not replay a pending native inspection",
);
inspection.resolve();
await inspection.promise;
assert.equal(select.value, target.value);
assert.equal(changes, 2);
assert.equal(writes, 2);

console.log("Native instrumentation and selection readiness checks passed");

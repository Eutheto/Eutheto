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

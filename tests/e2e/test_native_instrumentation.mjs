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

async function measureLongTasks(PerformanceObserver) {
  const observations = [];
  let lagTimerStarted = false;
  const page = {
    window: {},
    document: { documentElement: {} },
    MutationObserver: class {
      observe() {}
      disconnect() {}
    },
    performance: { now: () => 0 },
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
  return {
    result: JSON.parse(JSON.stringify(page.window.__euthetoPackage10Finish())),
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

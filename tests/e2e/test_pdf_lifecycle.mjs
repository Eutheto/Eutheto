// SPDX-License-Identifier: Apache-2.0
// Headless checks of the actual Linux observation parser and active-render admission.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { runInNewContext } from "node:vm";

const source = await readFile(
  new URL(
    "../../apps/desktop/e2e/recipient-pdf-lifecycle.mjs",
    import.meta.url,
  ),
  "utf8",
);
const parsing = source.slice(
  source.indexOf("function parseProcessStat("),
  source.indexOf("async function exists("),
);
const parse = runInNewContext(`${parsing}; parseProcessStat;`, {
  assert,
  readFile,
});
if (process.platform === "linux") {
  const actual = parse(await readFile(`/proc/${process.pid}/stat`, "utf8"));
  assert.equal(actual.parentPid, process.ppid);
  assert(actual.cpuTicks >= 0n);
  assert(/^\d+$/u.test(actual.startTicks));
}
const fields = Array.from({ length: 20 }, (_, index) => String(index));
fields[0] = "R";
fields[1] = "99";
fields[11] = "120";
fields[12] = "7";
fields[19] = "456";
const parsed = parse(`123 (a process with ) parentheses) ${fields.join(" ")}`);
assert.equal(parsed.parentPid, 99);
assert.equal(parsed.cpuTicks, 127n);
assert.equal(parsed.startTicks, "456");
assert.throws(() => parse("123 broken"), /Malformed/u);
assert.throws(() => parse("123 (short) R 1"), /Incomplete/u);

const admission = source.slice(
  source.indexOf("async function activeRenderer("),
  source.indexOf("async function cleanedUp("),
);
async function observe({
  lifeline = true,
  pending = true,
  advancing = true,
  exited = false,
} = {}) {
  let clock = 0;
  let sample = 0;
  const worker = {
    pid: "13",
    parentPid: 12,
    startTicks: "1",
    directory: "/private/eutheto-pdf-test",
  };
  return runInNewContext(
    `${admission}; activeRenderer(context, '/private', new Set());`,
    {
      assert,
      Set,
      Date: { now: () => (clock += 1_000) },
      pause: async () => {},
      findWorker: async () => worker,
      processStat: async (pid) =>
        pid === 12
          ? { state: "S", startTicks: "2" }
          : exited
            ? null
            : {
                ...worker,
                state: "R",
                cpuTicks: BigInt(advancing ? sample++ : 0),
              },
      hasLifeline: async () => lifeline,
      readlink: async () => "/fixture/eutheto-desktop",
      realpath: async (value) => value,
      context: {
        application: "/fixture/eutheto-desktop",
        sessionId: "fixture",
        evaluate: async (_session, script) =>
          script.includes("=== 'pending'") ? pending : { status: "fulfilled" },
      },
    },
  );
}
const active = await observe();
assert.equal(active.lifelineThreadObserved, true);
assert(BigInt(active.cpuTicksAfter) > BigInt(active.cpuTicksBefore));
assert.equal(active.pendingAtObservation, true);
await assert.rejects(
  observe({ lifeline: false }),
  /Active PDF rendering was not established/u,
);
await assert.rejects(
  observe({ advancing: false }),
  /Active PDF rendering was not established/u,
);
await assert.rejects(
  observe({ pending: false }),
  /PDF preview must still be pending/u,
);
await assert.rejects(
  observe({ exited: true }),
  /renderer exited before active-render evidence/u,
);
console.log(
  "PDF process identity and active-render cancellation admission checks passed",
);

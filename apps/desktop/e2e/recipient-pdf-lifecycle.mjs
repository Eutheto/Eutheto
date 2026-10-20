// SPDX-License-Identifier: Apache-2.0
// Public native commands plus read-only Linux observations; never alter worker delivery.
import assert from "node:assert/strict";
import { readFile, readdir, readlink, realpath, stat } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import { setTimeout as pause } from "node:timers/promises";

function parseProcessStat(text) {
  const end = text.lastIndexOf(")");
  assert(end > text.indexOf("("), "Malformed Linux process stat");
  const fields = text
    .slice(end + 2)
    .trim()
    .split(/\s+/u);
  assert(fields.length >= 20, "Incomplete Linux process stat");
  return {
    state: fields[0],
    parentPid: Number(fields[1]),
    cpuTicks: BigInt(fields[11]) + BigInt(fields[12]),
    startTicks: fields[19],
  };
}

async function processStat(pid) {
  try {
    return parseProcessStat(await readFile(`/proc/${pid}/stat`, "utf8"));
  } catch (error) {
    if (error.code === "ENOENT" || error.code === "ESRCH") return null;
    throw error;
  }
}

async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch (error) {
    if (error.code === "ENOENT") return false;
    throw error;
  }
}

async function stagedDirectories(root) {
  return (await readdir(root, { withFileTypes: true }))
    .filter((entry) => entry.isDirectory() && entry.name.startsWith("eutheto-pdf-"))
    .map((entry) => join(root, entry.name));
}

async function findWorker(root, previous) {
  for (const pid of (await readdir("/proc")).filter((entry) => /^\d+$/u.test(entry))) {
    let executable;
    try {
      executable = await readlink(`/proc/${pid}/exe`);
    } catch (error) {
      if (["ENOENT", "ESRCH", "EACCES", "EPERM"].includes(error.code)) continue;
      throw error;
    }
    const directory = dirname(executable);
    if (
      basename(executable) !== "eutheto-pdf-worker" ||
      dirname(directory) !== root ||
      !basename(directory).startsWith("eutheto-pdf-") ||
      previous.has(directory)
    )
      continue;
    const current = await processStat(pid);
    if (current) return { pid, directory, executable, ...current };
  }
  return null;
}

async function hasLifeline(pid) {
  try {
    const tasks = await readdir(`/proc/${pid}/task`);
    const names = await Promise.all(
      tasks.map((tid) =>
        readFile(`/proc/${pid}/task/${tid}/comm`, "utf8").catch((error) => {
          if (error.code === "ENOENT" || error.code === "ESRCH") return "";
          throw error;
        }),
      ),
    );
    // Linux comm is limited to 15 bytes. This thread starts after read_request completes.
    return names.some((name) => name.trim() === "pdf-parent-life");
  } catch (error) {
    if (error.code === "ENOENT" || error.code === "ESRCH") return false;
    throw error;
  }
}

async function beginPreview(context, format = "pdf") {
  const { sessionId, scenarioId, revision, solutionId, nativeRequest, requestId, evaluate } =
    context;
  const prepared = await nativeRequest(sessionId, "operation_prepare", {
    schemaVersion: 1,
    purpose: { kind: "sharePreview" },
    context: { kind: "scenario", scenarioId, expectedRevision: revision },
  });
  const operationId = prepared.result.operationId;
  await evaluate(
    sessionId,
    `const ipc = window.__TAURI_INTERNALS__;
    const channel = ipc.transformCallback(() => {});
    const state = {status: 'pending'};
    window.__euthetoPdfLifecycle = state;
    ipc.invoke('solution_share_preview', {request: arguments[0], onProgress: '__CHANNEL__:' + channel})
      .then(response => { state.status = 'fulfilled'; state.previewId = response.result.previewId; },
        error => {
          let failure = error;
          if (typeof error === 'string') { try { failure = JSON.parse(error); } catch { failure = null; } }
          state.code = failure?.code ?? null;
          state.status = 'rejected';
        })
      .finally(() => ipc.unregisterCallback(channel));`,
    [
      {
        schemaVersion: 1,
        requestId: requestId(),
        operationId,
        scenarioId,
        expectedRevision: revision,
        solutionId,
        format,
        options: {
          includePersonNames: false,
          includeAuthoredLabels: false,
          includeStaffing: false,
          includeChanges: false,
          includeFairnessActuals: false,
        },
      },
    ],
  );
  return operationId;
}

async function releasePreview(context, operationId) {
  await context.nativeRequest(context.sessionId, "solution_share_release", {
    schemaVersion: 1,
    scenarioId: context.scenarioId,
    target: { kind: "operation", operationId },
  });
  await context.nativeRequest(context.sessionId, "operation_release", {
    schemaVersion: 1,
    operationId,
  });
}

async function activeRenderer(context, root, previous) {
  const deadline = Date.now() + 30_000;
  let worker;
  let initialTicks;
  while (Date.now() < deadline) {
    worker ??= await findWorker(root, previous);
    if (worker) {
      const current = await processStat(worker.pid);
      assert(
        current && current.startTicks === worker.startTicks && current.state !== "Z",
        "PDF renderer exited before active-render evidence could be established",
      );
      if (await hasLifeline(worker.pid)) {
        initialTicks ??= current.cpuTicks;
        if (current.cpuTicks > initialTicks) {
          const parent = await processStat(current.parentPid);
          assert(parent && parent.state !== "Z", "PDF parent application must still be alive");
          assert.equal(
            await readlink(`/proc/${current.parentPid}/exe`),
            await realpath(context.application),
          );
          const pending = await context.evaluate(
            context.sessionId,
            "return window.__euthetoPdfLifecycle?.status === 'pending';",
          );
          assert.equal(
            pending,
            true,
            "PDF preview must still be pending at active-render observation",
          );
          return {
            pid: worker.pid,
            parentPid: current.parentPid,
            startTicks: current.startTicks,
            parentStartTicks: parent.startTicks,
            directory: worker.directory,
            lifelineThreadObserved: true,
            cpuTicksBefore: String(initialTicks),
            cpuTicksAfter: String(current.cpuTicks),
            pendingAtObservation: pending,
            boundary:
              "request completely read; helper CPU advances while native PDF preview is pending",
          };
        }
      }
    }
    await pause(10);
  }
  const terminal = await context.evaluate(
    context.sessionId,
    "return window.__euthetoPdfLifecycle;",
  );
  throw new Error(
    `Active PDF rendering was not established within 30 seconds: ${JSON.stringify(terminal)}`,
  );
}

async function cleanedUp(worker) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    const current = await processStat(worker.pid);
    const workerGone = current === null || current.startTicks !== worker.startTicks;
    if (workerGone && !(await exists(worker.directory)))
      return { helperReaped: true, stagingRemoved: true };
    await pause(20);
  }
  assert.fail(
    `PDF cleanup was not established: ${JSON.stringify(
      {
        pid: worker.pid,
        worker: await processStat(worker.pid),
        stagingExists: await exists(worker.directory),
      },
      (_key, value) => (typeof value === "bigint" ? String(value) : value),
    )}`,
  );
}

export async function nativePdfCancellationAcceptance(context) {
  assert.equal(process.platform, "linux");
  assert(process.env.TMPDIR, "PDF lifecycle requires the runner's private TMPDIR");
  const root = await realpath(process.env.TMPDIR);
  const previous = new Set(await stagedDirectories(root));
  const publishedBefore = (await readdir(context.publicationDirectory)).sort();
  const operationId = await beginPreview(context);
  let cancellationRequested = false;
  try {
    const active = await activeRenderer(context, root, previous);
    const cancellation = await context.nativeRequest(context.sessionId, "operation_cancel", {
      schemaVersion: 1,
      operationId,
    });
    assert.equal(cancellation.result.acknowledgement, "cancellationRequested");
    cancellationRequested = true;
    const terminal = await context.waitFor(
      context.sessionId,
      "const state = window.__euthetoPdfLifecycle; return state?.status !== 'pending' ? state : false;",
      [],
      30_000,
    );
    assert.equal(terminal.status, "rejected");
    assert.equal(terminal.code, "operation.cancelled");
    await releasePreview(context, operationId);
    const cleanup = await cleanedUp(active);
    assert.deepEqual(
      (await readdir(context.publicationDirectory)).sort(),
      publishedBefore,
      "Cancelling preview must not publish a partial output",
    );
    // Recapture through the same public owner and immutable source after cancellation.
    const retryId = await beginPreview(context, "html");
    try {
      const retry = await context.waitFor(
        context.sessionId,
        "const state = window.__euthetoPdfLifecycle; return state?.status !== 'pending' ? state : false;",
        [],
        30_000,
      );
      assert.equal(retry.status, "fulfilled");
      assert.equal(typeof retry.previewId, "string");
    } finally {
      await releasePreview(context, retryId);
    }
    const evidence = {
      active,
      terminal,
      cleanup,
      recapture: "html preview fulfilled and released",
    };
    console.log(
      "PASS: native active PDF render cancellation and exact cleanup",
      JSON.stringify(evidence),
    );
    return evidence;
  } finally {
    // A failed observation still owns work: explicitly cancel before releasing its observer.
    if (!cancellationRequested)
      await context.nativeRequest(context.sessionId, "operation_cancel", {
        schemaVersion: 1,
        operationId,
      });
    await releasePreview(context, operationId);
  }
}

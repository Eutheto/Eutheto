import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { createConnection } from "node:net";
import { isAbsolute } from "node:path";
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
  await command("POST", `${elementPath}/clear`, {});
  await command("POST", `${elementPath}/value`, { text, value: Array.from(text) });
}

async function getText(sessionId, selector) {
  const id = await waitForElement(sessionId, selector);
  return command(
    "GET",
    `/session/${encodeURIComponent(sessionId)}/element/${encodeURIComponent(id)}/text`,
  );
}

async function waitForProjectResult(sessionId, projectTitle) {
  const deadline = Date.now() + timeout;
  let text = "";
  while (Date.now() < deadline) {
    text = await getText(sessionId, ".project-home");
    if (text.includes(projectTitle) || text.includes("Request not completed.")) return text;
    await sleep(100);
  }
  throw new Error("Project creation produced neither a saved project nor a request error");
}

async function previewSuccessiveBackups(sessionId) {
  await command("POST", `/session/${encodeURIComponent(sessionId)}/execute/sync`, {
    script: "document.querySelector('#backup-title').closest('details').open = true;",
    args: [],
  });
  // Four replacements exceed the three-review quota unless earlier native reviews are retired.
  for (let index = 1; index <= 4; index += 1) {
    const title = `Native portable review ${index.toString()}`;
    await setValue(sessionId, "#backup-title", title);
    await command("POST", `/session/${encodeURIComponent(sessionId)}/execute/sync`, {
      script: "document.querySelector('#backup-title').form.requestSubmit();",
      args: [],
    });
    const deadline = Date.now() + timeout;
    let ready = false;
    while (Date.now() < deadline) {
      ready = await command("POST", `/session/${encodeURIComponent(sessionId)}/execute/sync`, {
        script:
          "return document.querySelector('#backup-preview-heading')?.textContent.includes(arguments[0]) === true && !document.querySelector('#backup-title').form.querySelector('button').disabled;",
        args: [title],
      });
      if (ready) break;
      await sleep(100);
    }
    assert(ready, `Native backup review ${index.toString()} did not settle successfully`);
    assert.match(
      await getText(sessionId, '[aria-labelledby="backup-preview-heading"] code'),
      /^[a-f0-9]{64}$/u,
    );
  }
  await command("POST", `/session/${encodeURIComponent(sessionId)}/execute/sync`, {
    script: "document.querySelector('#backup-preview-heading').scrollIntoView({block:'start'});",
    args: [],
  });
  const screenshot = await command("GET", `/session/${encodeURIComponent(sessionId)}/screenshot`);
  assert.equal(typeof screenshot, "string", "Native screenshot must contain encoded PNG bytes");
  const artifacts = new URL("../../../.cache/e2e/", import.meta.url);
  await mkdir(artifacts, { recursive: true });
  await writeFile(new URL("portable-preview.png", artifacts), Buffer.from(screenshot, "base64"));
}

async function recordProjectOpen(sessionId, title) {
  // Real IPC exercises the installed command ACL; mock-runtime handler tests do not.
  const { before, after } = await command(
    "POST",
    `/session/${encodeURIComponent(sessionId)}/execute/async`,
    {
      script: `
        const title = arguments[0];
        const done = arguments[arguments.length - 1];
        (async () => {
          const listed = await window.__TAURI_INTERNALS__.invoke("project_list", {
            request: { schemaVersion: 1, requestId: "01900000-0000-7000-8000-000000000091", scope: "all" },
          });
          const before = listed.result.find((project) => project.title === title);
          if (!before) throw new Error("The created project was not listed");
          const opened = await window.__TAURI_INTERNALS__.invoke("project_open", {
            request: { schemaVersion: 1, requestId: "01900000-0000-7000-8000-000000000092", scenarioId: before.scenarioId },
          });
          return { before, after: opened.result };
        })().then(done, (error) => done({ error: typeof error === "string" ? error : JSON.stringify(error) }));
      `,
      args: [title],
    },
  );
  assert.equal(after.scenarioId, before.scenarioId);
  assert.equal(after.revision, before.revision, "Opening must not mutate the scenario revision");
  assert.equal(typeof after.lastOpenedAt, "string");
  assert(Number.isFinite(Date.parse(after.lastOpenedAt)), "Native opening must record a timestamp");
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

    await waitForElement(firstSessionId, ".project-home");
    await setValue(firstSessionId, "#create-title", projectTitle);
    await setValue(firstSessionId, "#first-date", "2030-01-01");
    await setValue(firstSessionId, "#last-date", "2030-01-31");
    await command("POST", `/session/${encodeURIComponent(firstSessionId)}/execute/sync`, {
      script:
        "const form = document.querySelector('.stacked-form'); if (!(form instanceof HTMLFormElement)) throw new Error('create form not found'); form.requestSubmit(); return null;",
      args: [],
    });

    const projectHomeText = await waitForProjectResult(firstSessionId, projectTitle);
    assert.match(projectHomeText, new RegExp(projectTitle, "u"));
    const projectSelector = `[aria-label=${JSON.stringify(`Open project ${projectTitle}`)}]`;
    assert.match(await getText(firstSessionId, projectSelector), /official\.workforce/u);
    await recordProjectOpen(firstSessionId, projectTitle);
    await previewSuccessiveBackups(firstSessionId);

    await deleteSession();
    const secondSessionId = await createSession();
    assert.notStrictEqual(secondSessionId, firstSessionId);
    await waitForElement(secondSessionId, ".project-home");
    assert.match(await getText(secondSessionId, projectSelector), /official\.workforce/u);

    console.log(
      "PASS: native project_open passed its ACL and recorded opening metadata; four backup reviews settled without exhausting custody; project persisted across independent application sessions",
    );
  } finally {
    try {
      await deleteSession();
    } finally {
      await stopDriver();
    }
  }
}

await run();

"""Synthetic native feasibility only; never publishes SDKs, PDFs or raw logs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

HERE = Path(__file__).resolve().parent
MAX_PDF = 32 * 1024 * 1024
STAGES = {"startup", "context", "browser", "loaded", "ready", "printing", "printed", "closed", "failed", "renderer-exit", "deny-popup", "deny-navigation", "deny-tab", "deny-favicon", "deny-resource", "deny-handler", "deny-protocol", "deny-download", "deny-print-dialog", "deny-print-job"}
STAGES |= {"lifecycle-loop-returned", "lifecycle-shutdown-entered", "lifecycle-shutdown-returned", "lifecycle-probe-returned", "lifecycle-pool-drained", "lifecycle-unload-entered", "lifecycle-unload-returned", "mac-teardown-watchdog", "mac-session-watchdog", "mac-sigterm-watchdog"}
BOUNDARIES = {"setup", "owner-start", "owner-read", "initial-observation", "ready-observation", "sandbox-observation", "renderer-injection", "host-injection", "settlement-observation", "inspector"}
OBSERVATION_FIELDS = {
    "code": {"other", "native-observation-unavailable", "observation-deadline", "observation-size-limit", "observation-argument-limit", "ambiguous-process-role", "invalid-owned-executable", "unexpected-descendant-executable", "process-scan-limit", "requires-native-64-bit-python", "requires-native-64-bit-process", "invalid-native-commandline", "invalid-integrity-sid", "unsupported-observation-platform", "invalid-process-observations", "missing-retained-process-identity", "invalid-root-pid", "invalid-runtime-root", "owned-process-limit", "root-identity-changed", "owned-process-exited-during-observation", "process-identity-changed", "process-ancestry-changed", "invalid-private-job", "sandbox-introspection-unavailable"},
    "operation": {"identity", "topology", "short-info", "image", "args", "signal"},
    "error": {"none", "permission", "not-found", "invalid", "size", "other"},
    "result": {"zero", "short", "oversized", "ok"},
}


def observation_facts(error):
    code = error.args[0] if len(error.args) == 1 and type(error.args[0]) is str else None
    facts = {"code": code if code in OBSERVATION_FIELDS["code"] else "other"}
    for attribute, field in (("operation", "operation"), ("native_error", "error"), ("native_result", "result")):
        value = getattr(error, attribute, None)
        if type(value) is str and value in OBSERVATION_FIELDS[field]:
            facts[field] = value
    return facts


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def stop(process):
    if process.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10, check=False)
    else:
        os.killpg(process.pid, signal.SIGKILL)
    process.wait(timeout=10)


def tool_failure_facts(output):
    """Project only closed source names, numeric locations/codes and fixed categories."""
    locations = []
    for name in ("main.rs", "windows.rs", "job_object.rs", "host.cc", "entry_windows.cc", "entry_mac.mm", "helper_mac.cc", "CMakeLists.txt", "windows.cmake", "macos.cmake", "cef_variables.cmake", "cef_macros.cmake", "FindCEF.cmake"):
        match = re.search(rb"(?:^|[\\/ ])" + re.escape(name.encode("ascii")) + rb"[:(]([0-9]{1,6})\b", output, re.MULTILINE)
        if match:
            locations.append({"file": name, "line": int(match.group(1))})
    codes = []
    for match in re.finditer(rb"\b([CE])([0-9]{4})\b", output):
        codes.append({"family": match.group(1).decode("ascii"), "number": int(match.group(2))})
        if len(codes) == 12:
            break
    categories = []
    for name, text in (("registry", b"failed to get "), ("linker", b"linking with"), ("missing-library", b"cannot find -l"), ("undefined-symbol", b"undefined reference"), ("missing-package", b"Could NOT find"), ("cmake-error", b"CMake Error"), ("permission", b"Permission denied"), ("rust-import", b"unresolved import"), ("rust-method", b"no method named"), ("missing-target", b"can't find crate")):
        if text in output:
            categories.append(name)
    return {"locations": locations, "codes": codes, "categories": categories}


def command(args, env, cwd, seconds=600, owner=None, operation="discovery"):
    """Bootstrap trusted source directly; all SDK/inspector work uses owned trees."""
    if operation not in {"discovery", "owner-build", "owner-tests", "publication-tests", "configure", "compile", "inspector-venv", "inspector-install", "inspector-pdf", "version"}:
        raise ValueError("unknown-tool-operation")
    tool_dir = None
    log = None
    if owner is not None:
        executable = shutil.which(str(args[0]), path=env.get("PATH"))
        if executable is None:
            raise RuntimeError("tool-unavailable")
        tool_dir = Path(tempfile.mkdtemp(prefix="cef-tool-", dir=cwd)).resolve()
        log = tool_dir / "output"
        args = [str(owner), "--command", str(seconds), str(log), str(Path(executable).absolute()), *map(str, args[1:])]
    process = subprocess.Popen(args, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               start_new_session=os.name != "nt", close_fds=True)
    output = bytearray()
    overflow = threading.Event()
    def drain():
        while chunk := process.stdout.read(8192):
            if len(output) + len(chunk) > 2 * 1024 * 1024:
                overflow.set()
                return
            output.extend(chunk)
    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    deadline = time.monotonic() + seconds + (10 if owner is not None else 0)
    try:
        while process.poll() is None:
            if overflow.is_set() or time.monotonic() >= deadline:
                raise RuntimeError("tool-limit")
            time.sleep(0.05)
        reader.join(timeout=5)
        if reader.is_alive() or overflow.is_set():
            raise RuntimeError("tool-output-limit")
        control = bytes(output)
        if owner is not None:
            terminal = re.fullmatch(rb"pid [1-9][0-9]{0,9}\nreason (complete|deadline|external-cancel|native-failure|protocol-failure|pipe-failure)\nexit (-?[0-9]+)\nsettled 1\n", control)
            if not terminal:
                raise RuntimeError("tool-settlement-unverified")
            if log.is_symlink() or not log.is_file():
                raise RuntimeError("tool-log-invalid")
            with log.open("rb") as stream:
                output = stream.read(2 * 1024 * 1024 + 1)
            if len(output) > 2 * 1024 * 1024:
                raise RuntimeError("tool-log-bound")
            if process.returncode == 0 and (terminal.group(1) != b"complete" or terminal.group(2) != b"0"):
                raise RuntimeError("tool-terminal-mismatch")
        if process.returncode:
            print(json.dumps({"toolFailed": True, "operation": operation, **tool_failure_facts(output)}), flush=True)
            raise RuntimeError("tool-failed")
        return bytes(output)
    finally:
        if owner is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                stop(process)
        stop(process)
        process.stdout.close()
        if tool_dir is not None and process.returncode == 0:
            shutil.rmtree(tool_dir)


def private_directory(path, env):
    path.mkdir(mode=0o700, parents=False, exist_ok=False)
    if os.name == "nt":
        import csv
        value = command(["whoami", "/user", "/fo", "csv", "/nh"], env, path, 15).decode().strip()
        fields = next(csv.reader([value]))
        sid = fields[-1]
        if not re.fullmatch(r"S-1-(?:\d+-)+\d+", sid):
            raise RuntimeError("private-acl-identity")
        command(["icacls", str(path), "/inheritance:r", "/grant:r", f"*{sid}:(OI)(CI)F", "*S-1-5-18:(OI)(CI)F"], env, path, 15)


def inspect_pdf(path):
    # This branch runs in its own deadline-bound interpreter and private venv.
    if os.name != "nt":
        import resource
        resource.setrlimit(resource.RLIMIT_AS, (1024 * 1024 * 1024,) * 2)
        resource.setrlimit(resource.RLIMIT_CPU, (20, 20))
    from pypdf import PdfReader
    if path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= MAX_PDF:
        raise RuntimeError("pdf-bound")
    with path.open("rb") as stream:
        pdf = PdfReader(stream, strict=True)
        if pdf.is_encrypted or not 2 <= len(pdf.pages) <= 64:
            raise RuntimeError("pdf-pages")
        text = []
        total = 0
        for index, page in enumerate(pdf.pages, 1):
            value = page.extract_text()
            total += len(value)
            if total > 1024 * 1024:
                raise RuntimeError("pdf-text-bound")
            normalized = " ".join(value.split())
            if "SYNTHETIC | Revision 7 | UTC | UNVERIFIED" not in normalized or f"Page {index} of {len(pdf.pages)}" not in normalized:
                raise RuntimeError("pdf-page-context")
            text.append(value)
        value = "\n".join(text)
        if any(value.count(f"ROW-{index:03}") != 1 for index in range(1, 65)):
            raise RuntimeError("pdf-row-census")
        normalized = " ".join(value.split())
        if "Zoë Δοκιμή — Кириллица →" not in normalized or "<img src=x onerror=alert(1)>" not in normalized:
            raise RuntimeError("pdf-labels")
        print(json.dumps({"pages": len(pdf.pages), "rows": 64, "unicode": True, "inertLabel": True, "pageContext": True}), flush=True)


def runtime_case(owner, host, runtime_root, html, mode, action, work, env, observe, inspector, expected_denial=None):
    # Short OS-temp paths are necessary for Chromium's Unix-domain socket limit.
    job = Path(tempfile.mkdtemp(prefix="cef-job-")).resolve()
    process = None
    records = {}
    settled = False
    triggered = False
    result = {"mode": mode, "action": action, "passed": False}
    boundary = "setup"
    try:
        if os.name == "nt":
            job.rmdir()
            private_directory(job, env)
        input_path = job / "input.html"
        if action == "write-failure":
            (job / "output.pdf").mkdir()
        input_path.write_bytes(html)
        boundary = "owner-start"
        process = subprocess.Popen([str(owner), str(host), str(job), str(input_path), mode],
                                   env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL, close_fds=True,
                                   start_new_session=os.name != "nt")
        messages = queue.Queue(maxsize=64)
        overflow = threading.Event()
        def drain():
            total = 0
            while True:
                line = process.stdout.readline(130)
                if not line:
                    break
                total += len(line)
                if len(line) > 128 or not line.endswith(b"\n") or total > 8192:
                    overflow.set(); break
                try:
                    messages.put_nowait(line.decode("ascii").strip())
                except (queue.Full, UnicodeDecodeError):
                    overflow.set(); break
        reader = threading.Thread(target=drain, daemon=True)
        reader.start()
        native_pid = None
        events = []
        settled = False
        host_exit = None
        reason = None
        triggered = False
        restrictions = None
        deadline = time.monotonic() + 80
        while process.poll() is None or not messages.empty() or reader.is_alive():
            boundary = "owner-read"
            if overflow.is_set() or time.monotonic() >= deadline:
                raise RuntimeError("owner-bound")
            try:
                line = messages.get(timeout=0.05)
            except queue.Empty:
                continue
            if line.startswith("pid ") and native_pid is None:
                native_pid = int(line[4:])
            elif line in STAGES:
                events.append(line)
            elif re.fullmatch(r"exit -?\d+", line):
                host_exit = int(line[5:])
            elif line in ("settled 0", "settled 1"):
                settled = line == "settled 1"
            elif line.startswith("reason ") and line[7:] in {"complete", "deadline", "stage-cancel", "external-cancel", "native-failure", "protocol-failure", "pipe-failure", "initial-observation-failure"}:
                reason = line[7:]
            else:
                raise RuntimeError("owner-protocol")
            if native_pid and (line.startswith("pid ") or (line == "ready" and mode == "observe")):
                boundary = "initial-observation" if line.startswith("pid ") else "ready-observation"
                for item in observe.snapshot(native_pid, runtime_root):
                    records.setdefault((item["pid"], item["start_identity"]), item)
                if line.startswith("pid "):
                    if not any(item["pid"] == native_pid for item in records.values()):
                        raise RuntimeError("initial-identity-missing")
                    process.stdin.write(b"a")
                    process.stdin.flush()
                    process.stdin.close()
            if line == "ready" and action in ("sandbox", "renderer-kill", "host-kill") and not triggered:
                values = list(records.values())
                if action == "sandbox":
                    boundary = "sandbox-observation"
                    restrictions = observe.sandbox_evidence(values, job)
                    triggered = True
                    if os.name == "nt":
                        process.kill()  # Actual owner death exercises kernel kill-on-close.
                    else:
                        process.terminate()  # Explicit SIGTERM handler owns Unix cleanup.
                elif action == "renderer-kill":
                    boundary = "renderer-injection"
                    triggered = observe.terminate_renderers(values)
                elif action == "host-kill":
                    boundary = "host-injection"
                    root = next((item for item in values if item["pid"] == native_pid), None)
                    triggered = bool(root and observe.terminate_process(root))
                if not triggered:
                    raise RuntimeError("fault-injection-unverified")
        process.wait(timeout=5)
        reader.join(timeout=5)
        values = list(records.values())
        boundary = "settlement-observation"
        remaining = observe.survivors(values)
        until = time.monotonic() + 5
        while remaining and time.monotonic() < until:
            time.sleep(0.05)
            remaining = observe.survivors(values)
        result.update(events=events, observedProcesses=len(values), survivors=len(remaining), ownerExit=process.returncode, hostExit=host_exit, waited=settled, reason=reason)
        if not values or remaining:
            raise RuntimeError("cleanup-unverified")
        if action == "sandbox":
            result["sandboxVerified"] = bool(restrictions and restrictions.get("verified"))
            result["passed"] = triggered and result["sandboxVerified"] and ((settled and reason == "external-cancel") or os.name == "nt")
        elif action == "write-failure":
            result["passed"] = settled and reason == "native-failure" and host_exit == 72 and "printing" in events and "failed" in events and "printed" not in events
        elif mode == "oversized":
            result["passed"] = settled and reason == "native-failure" and host_exit == 64 and "context" not in events
        elif action:
            result["passed"] = triggered and settled and reason == "native-failure" and process.returncode != 0 and (action != "renderer-kill" or ("renderer-exit" in events and host_exit == 72))
        elif mode == "normal" and process.returncode == 0 and not expected_denial:
            pdf = job / "output.pdf"
            boundary = "inspector"
            data = command([str(inspector), "-I", str(HERE / "probe.py"), "--inspect", str(pdf)], env, job, 30, owner, "inspector-pdf")
            inspected = json.loads(data)
            if type(inspected) is not dict or set(inspected) != {"pages", "rows", "unicode", "inertLabel", "pageContext"}:
                raise RuntimeError("inspector-fields")
            if type(inspected["pages"]) is not int or not 2 <= inspected["pages"] <= 64 or type(inspected["rows"]) is not int or inspected["rows"] != 64:
                raise RuntimeError("inspector-counts")
            if any(inspected[key] is not True for key in ("unicode", "inertLabel", "pageContext")):
                raise RuntimeError("inspector-fidelity")
            result.update(inspected)
            result.update(pdfSha256=digest(pdf), pdfBytes=pdf.stat().st_size, passed=settled and reason == "complete")
        elif mode in ("startup-cancel", "ready-cancel", "printing-cancel", "deadline"):
            expected = {"startup-cancel": "startup", "ready-cancel": "ready", "printing-cancel": "printing", "deadline": "ready"}[mode]
            result["passed"] = settled and reason == ("deadline" if mode == "deadline" else "stage-cancel") and process.returncode != 0 and expected in events
        elif expected_denial:
            result["passed"] = settled and reason == "native-failure" and host_exit == 72 and expected_denial in events and "printed" not in events
        if any(event in {"mac-teardown-watchdog", "mac-session-watchdog", "mac-sigterm-watchdog"} for event in events):
            result["passed"] = False
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError, KeyError) as error:
        result.update(failedBoundary=True, boundaryStage=boundary)
        if isinstance(error, observe.ObservationError):
            result["observationFailure"] = observation_facts(error)
    finally:
        try:
            if process is not None:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=8)
                    except subprocess.TimeoutExpired:
                        stop(process)
                process.stdout.close()
                process.stdin.close()
                values = list(records.values())
                if not values:
                    raise RuntimeError("cleanup-unverified")
                for item in values:
                    if observe.survivors([item]):
                        observe.terminate_process(item)
                until = time.monotonic() + 5
                while observe.survivors(values):
                    if time.monotonic() >= until:
                        raise RuntimeError("cleanup-unverified")
                    time.sleep(0.05)
                if not settled and not (os.name == "nt" and action == "sandbox" and triggered):
                    raise RuntimeError("owned-settlement-unverified")
            shutil.rmtree(job)
            result["stagingRemoved"] = True
        except (OSError, RuntimeError, subprocess.SubprocessError) as error:
            # Preserve private staging until exact owned identities are settled.
            result.update(passed=False, cleanupFailed=True, stagingRemoved=False)
            if isinstance(error, observe.ObservationError):
                result["cleanupObservationFailure"] = observation_facts(error)
    return result


def denial_cases(owner, host, runtime_root, work, env, observe, inspector):
    import http.client
    from http.server import BaseHTTPRequestHandler, HTTPServer
    hits = []
    class Sentinel(BaseHTTPRequestHandler):
        def do_GET(self):
            hits.append(1)
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"synthetic")
        def log_message(self, *_args):
            pass
    server = HTTPServer(("127.0.0.1", 0), Sentinel)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    results = []
    try:
        control = http.client.HTTPConnection("127.0.0.1", server.server_port, timeout=5)
        control.request("GET", "/control")
        response = control.getresponse()
        reachable = response.status == 200 and response.read(64) == b"synthetic"
        control.close()
        before = len(hits)
        scripts = [
            ("resource", f"fetch('http://127.0.0.1:{server.server_port}/denied')", "deny-resource"),
            ("navigation", "location.href='https://eutheto-report.invalid/escape'", "deny-navigation"),
            ("popup", "window.open('https://eutheto-report.invalid/popup')", "deny-popup"),
            ("download", "const a=document.createElement('a');a.href='data:application/octet-stream,SYNTHETIC';a.download='forbidden.txt';document.body.append(a);a.click()", "deny-download"),
        ]
        for name, script, event in scripts:
            html = ('<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><main id="recipient-report" data-recipient-ready="true">Synthetic denial</main><script>' + script + '</script>').encode()
            case_env = env | ({"EUTHETO_PROBE_POPUP_TEST": "1"} if name == "popup" else {})
            result = runtime_case(owner, host, runtime_root, html, "normal", None, work, case_env, observe, inspector, event)
            result["case"] = name
            if name == "resource":
                result.update(sentinelReachable=reachable, handledRequests=len(hits) - before)
                result["passed"] &= reachable and len(hits) == before
            results.append(result)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    return results


def checked_evidence(data):
    """Closed publication DTO; never forward arbitrary derived-code dictionaries."""
    root_fields = {"target", "phase", "passed", "source", "base", "run", "attempt", "architecture", "image", "cases", "sdkSha256", "hostSha256", "fixtureSha256", "nativeSha256", "tools"}
    if type(data) is not dict or set(data) - root_fields or not {"phase", "passed"} <= set(data):
        raise ValueError("evidence-shape")
    enums = {
        "target": {"linux64", "windows64", "macosx64", "macosarm64"},
        "phase": {"setup", "download-admission", "tooling", "build", "runtime", "complete", "runtime-failed-or-unverified", "setup-failed", "download-admission-failed", "tooling-failed", "build-failed", "runtime-failed", "missing-evidence", "evidence-refused"},
        "architecture": {"x86_64", "AMD64", "arm64", "aarch64"},
    }
    for key, value in data.items():
        if key in enums:
            if type(value) is not str or value not in enums[key]: raise ValueError("evidence-enum")
        elif key == "passed":
            if type(value) is not bool: raise ValueError("evidence-bool")
        elif key in {"source", "base"}:
            if type(value) is not str or not re.fullmatch(r"(?:[0-9a-f]{40}|local)", value): raise ValueError("evidence-source")
        elif key in {"run", "attempt", "image"}:
            if type(value) is not str or not re.fullmatch(r"(?:[0-9.]{1,40}|local)", value): raise ValueError("evidence-run")
        elif key.endswith("Sha256"):
            hashes = value if key == "nativeSha256" else {key: value}
            if type(hashes) is not dict or len(hashes) > 8: raise ValueError("evidence-hashes")
            if key == "nativeSha256" and set(hashes) - {"owner", "dll", "helper", "helper-alerts", "helper-gpu", "helper-plugin", "helper-renderer"}: raise ValueError("evidence-native")
            if any(type(item) is not str or not re.fullmatch(r"[0-9a-f]{64}", item) for item in hashes.values()): raise ValueError("evidence-hash")
        elif key == "tools":
            if type(value) is not dict or set(value) - {"python", "cargo", "rustc", "cmake", "ninja", "compiler"}: raise ValueError("evidence-tools")
            if any(type(item) is not str or not re.fullmatch(r"[0-9]+(?:\.[0-9]+){1,3}", item) for item in value.values()): raise ValueError("evidence-version")
        elif key == "cases":
            if type(value) is not list or len(value) > 20: raise ValueError("evidence-cases")
            for case in value:
                if type(case) is not dict: raise ValueError("evidence-case")
                for name, item in case.items():
                    if name in {"passed", "waited", "sandboxVerified", "unicode", "inertLabel", "pageContext", "sentinelReachable", "failedBoundary", "cleanupFailed", "stagingRemoved"}:
                        if type(item) is not bool: raise ValueError("evidence-case-bool")
                    elif name in {"observedProcesses", "survivors", "pages", "rows", "handledRequests", "pdfBytes", "ownerExit", "hostExit"}:
                        if name == "hostExit" and item is None: continue
                        bound = MAX_PDF if name == "pdfBytes" else (4294967295 if name in {"ownerExit", "hostExit"} else 128)
                        lower = -2147483648 if name in {"ownerExit", "hostExit"} else 0
                        if type(item) is not int or not lower <= item <= bound: raise ValueError("evidence-case-number")
                    elif name == "boundaryStage":
                        if type(item) is not str or item not in BOUNDARIES: raise ValueError("evidence-boundary")
                    elif name in {"observationFailure", "cleanupObservationFailure"}:
                        if type(item) is not dict or "code" not in item or set(item) - set(OBSERVATION_FIELDS): raise ValueError("evidence-observation")
                        if any(type(value) is not str or value not in OBSERVATION_FIELDS[field] for field, value in item.items()): raise ValueError("evidence-observation-value")
                    elif name == "events":
                        if type(item) is not list or len(item) > 32 or any(type(event) is not str or event not in STAGES for event in item): raise ValueError("evidence-events")
                    elif name == "pdfSha256":
                        if type(item) is not str or not re.fullmatch(r"[0-9a-f]{64}", item): raise ValueError("evidence-pdf-hash")
                    else:
                        choices = {
                            "mode": {"normal", "oversized", "startup-cancel", "ready-cancel", "printing-cancel", "deadline", "observe"},
                            "action": {None, "sandbox", "renderer-kill", "host-kill", "write-failure"},
                            "reason": {None, "complete", "deadline", "stage-cancel", "external-cancel", "native-failure", "protocol-failure", "pipe-failure", "initial-observation-failure"},
                            "case": {"missing-root", "resource", "navigation", "popup", "download"},
                        }
                        if name not in choices or (item is not None and type(item) is not str) or item not in choices[name]: raise ValueError("evidence-case-field")
    return data


def summary(path):
    try:
        if path.is_symlink() or not path.is_file():
            raise ValueError()
        with path.open("rb") as stream:
            encoded = stream.read(65537)
        if len(encoded) > 65536:
            raise ValueError()
        data = checked_evidence(json.loads(encoded))
    except (OSError, ValueError, RecursionError):
        data = {"phase": "missing-evidence", "passed": False}
    text = json.dumps(data, sort_keys=True, ensure_ascii=True)
    print(text)
    destination = os.environ.get("GITHUB_STEP_SUMMARY")
    if destination:
        with open(destination, "a", encoding="utf-8") as stream:
            stream.write("### CEF native feasibility\n\n```json\n" + text + "\n```\n")


def main(args):
    sys.path.insert(0, str(HERE))
    import provision
    import observe
    env = provision.sanitized_env()
    work = args.work_dir.resolve()
    evidence = {"target": args.target, "phase": "setup", "passed": False,
                "source": os.environ.get("GITHUB_SHA", "local"), "run": os.environ.get("GITHUB_RUN_ID", "local"),
                "attempt": os.environ.get("GITHUB_RUN_ATTEMPT", "local"), "architecture": __import__('platform').machine(),
                "image": os.environ.get("ImageVersion", "local")}
    created = False
    try:
        private_directory(work, env)
        created = True
        home = work / "home"
        home.mkdir(mode=0o700)
        env.update(HOME=str(home), USERPROFILE=str(home), TMPDIR=str(home), TEMP=str(home), TMP=str(home))
        pins = json.loads((HERE / "pins.json").read_text())
        evidence["base"] = pins["experiment_base_commit"]
        evidence["phase"] = "tooling"
        cargo = command(["rustup", "which", "--toolchain", "1.97.1", "cargo"], provision.sanitized_env(), work, 30).decode().strip()
        rustc = command(["rustup", "which", "--toolchain", "1.97.1", "rustc"], provision.sanitized_env(), work, 30).decode().strip()
        env.update(RUSTC=rustc, CARGO_HOME=str(work / "cargo-home"), CARGO_BUILD_JOBS="2")
        owner_build = work / "owner-build"
        evidence["phase"] = "build"
        command([cargo, "build", "--locked", "--release", "--manifest-path", str(HERE / "Cargo.toml"), "--target-dir", str(owner_build)], env, work, 900, operation="owner-build")
        owner = owner_build / "release" / ("cef-native-validation-owner.exe" if os.name == "nt" else "cef-native-validation-owner")
        # Bootstrap is repository-owned source compilation, not SDK execution.
        # Only after it exists may downloaded/derived SDK or inspector code run.
        command([cargo, "test", "--locked", "--release", "--manifest-path", str(HERE / "Cargo.toml"), "--target-dir", str(owner_build)], env, work, 900, owner, "owner-tests")
        command([sys.executable, "-I", "-B", "-m", "unittest", "discover", "-s", str(HERE / "tests"), "-p", "test_*.py"], env, work, 30, owner, "publication-tests")
        evidence["phase"] = "download-admission"
        sdk_work = work / "sdk"
        private_directory(sdk_work, env)
        sdk = provision.provision(args.target, sdk_work)
        evidence["sdkSha256"] = pins["targets"][args.target]["sha256"]
        evidence["phase"] = "build"
        build = work / "build"
        configure = ["cmake", "-S", str(HERE), "-B", str(build), "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", f"-DCEF_ROOT={sdk}"]
        if args.target.startswith("macos"):
            configure += ["-DPROJECT_ARCH=" + ("arm64" if args.target == "macosarm64" else "x86_64")]
        command(configure, env, work, 180, owner, "configure")
        command(["cmake", "--build", str(build), "--config", "Release", "--target", "cef-probe", "--parallel", "2"], env, work, 1200, owner, "compile")
        runtime_root = build / "Release"
        host = runtime_root / ("cef-probe.exe" if os.name == "nt" else "cef-probe")
        if args.target.startswith("macos"):
            host = runtime_root / "cef-probe.app/Contents/MacOS/cef-probe"
        evidence["hostSha256"] = digest(host)
        evidence["nativeSha256"] = {"owner": digest(owner)}
        if os.name == "nt":
            evidence["nativeSha256"]["dll"] = digest(runtime_root / "cef-probe.dll")
        elif args.target.startswith("macos"):
            for key, suffix in [("helper", ""), ("helper-alerts", " (Alerts)"), ("helper-gpu", " (GPU)"), ("helper-plugin", " (Plugin)"), ("helper-renderer", " (Renderer)")]:
                name = "cef-probe Helper" + suffix
                evidence["nativeSha256"][key] = digest(runtime_root / f"cef-probe.app/Contents/Frameworks/{name}.app/Contents/MacOS/{name}")
        evidence["tools"] = {"python": __import__("platform").python_version()}
        for name, executable in [("cargo", cargo), ("rustc", rustc), ("cmake", "cmake"), ("ninja", "ninja")]:
            version = command([executable, "--version"], env, work, 30, owner, "version")
            match = re.search(rb"\b[0-9]+(?:\.[0-9]+){1,3}\b", version)
            if match is None:
                raise RuntimeError("tool-version-unavailable")
            evidence["tools"][name] = match.group().decode("ascii")
        compiler_files = list(build.glob("CMakeFiles/*/CMakeCXXCompiler.cmake"))
        if len(compiler_files) != 1 or compiler_files[0].stat().st_size > 65536:
            raise RuntimeError("compiler-version-unavailable")
        match = re.search(rb'set\(CMAKE_CXX_COMPILER_VERSION "([0-9]+(?:\.[0-9]+){1,3})"\)', compiler_files[0].read_bytes())
        if match is None:
            raise RuntimeError("compiler-version-unavailable")
        evidence["tools"]["compiler"] = match.group(1).decode("ascii")
        venv = work / "inspector"
        command([sys.executable, "-I", "-m", "venv", str(venv)], env, work, 120, owner, "inspector-venv")
        inspector = venv / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
        command([str(inspector), "-I", "-m", "pip", "--isolated", "install", "--disable-pip-version-check", "--no-cache-dir", "--no-deps", "--only-binary=:all:", "--require-hashes", "--index-url", "https://pypi.org/simple", "-r", str(HERE / "requirements.txt")], env, work, 180, owner, "inspector-install")
        evidence["phase"] = "runtime"
        html = (HERE / "fixture.html").read_bytes()
        evidence["fixtureSha256"] = hashlib.sha256(html).hexdigest()
        cases = []
        evidence["cases"] = cases
        for mode, action in [("normal", None), ("startup-cancel", None), ("ready-cancel", None), ("printing-cancel", None), ("deadline", None), ("observe", "sandbox"), ("observe", "renderer-kill"), ("observe", "host-kill")]:
            cases.append(runtime_case(owner, host, runtime_root, html, mode, action, work, env, observe, inspector))
        malformed = html.replace(b'id="recipient-report"', b'id="missing-root"')
        case = runtime_case(owner, host, runtime_root, malformed, "normal", None, work, env, observe, inspector, "failed")
        case["case"] = "missing-root"
        cases.append(case)
        cases.append(runtime_case(owner, host, runtime_root, b"x" * (16 * 1024 * 1024 + 1), "oversized", None, work, env, observe, inspector))
        cases.append(runtime_case(owner, host, runtime_root, html, "normal", "write-failure", work, env, observe, inspector))
        cases.extend(denial_cases(owner, host, runtime_root, work, env, observe, inspector))
        evidence["passed"] = all(case["passed"] for case in cases)
        evidence["phase"] = "complete" if evidence["passed"] else "runtime-failed-or-unverified"
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError):
        evidence["phase"] += "-failed"
    finally:
        try:
            evidence = checked_evidence(evidence)
        except ValueError:
            evidence = {"phase": "evidence-refused", "passed": False}
        if created:
            (work / "evidence.json").write_text(json.dumps(evidence, sort_keys=True), encoding="utf-8")
        print(json.dumps(evidence, sort_keys=True), flush=True)
    return 0 if evidence["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", choices=["linux64", "windows64", "macosx64", "macosarm64"])
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--inspect", type=Path)
    parser.add_argument("--summary", type=Path)
    options = parser.parse_args()
    if options.inspect:
        try:
            inspect_pdf(options.inspect)
        except Exception:
            print('{"inspectionFailed":true}')
            sys.exit(1)
    elif options.summary:
        summary(options.summary)
    elif options.target and options.work_dir:
        sys.exit(main(options))
    else:
        parser.error("target and work-dir required")

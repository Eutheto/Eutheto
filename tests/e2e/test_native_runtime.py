#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Check the authored native launchers without starting a desktop or driver."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import textwrap


ROOT = Path(__file__).resolve().parents[2]
bash = shutil.which("bash")
assert bash is not None
workflow = (ROOT / ".github/workflows/ortools-worker.yml").read_text()
start = workflow.index("          native_unset=(")
end = workflow.index("          nix develop", start)
generation = textwrap.dedent(workflow[start:end])
cleared = (
    "LD_LIBRARY_PATH", "GBM_BACKENDS_PATH", "LIBGL_DRIVERS_PATH", "LIBVA_DRIVERS_PATH",
    "__EGL_VENDOR_LIBRARY_FILENAMES", "GTK_PATH", "GIO_EXTRA_MODULES", "GDK_PIXBUF_MODULE_FILE",
    "GSETTINGS_SCHEMA_DIR", "GSETTINGS_SCHEMAS_PATH",
)

with tempfile.TemporaryDirectory(prefix="eutheto-native-test-") as temporary:
    root = Path(temporary)
    deb = root / "deb with 'quotes' and $literal"
    app = deb / "usr/bin/eutheto-desktop"
    app.parent.mkdir(parents=True)
    driver = root / "WebKitWebDriver"
    observed = (*cleared, "PATH", "XDG_DATA_DIRS", "EUTHETO_RUNTIME_SENTINEL")
    fixture = (f"#!{sys.executable}\nimport json, os, sys\n"
               f"print(json.dumps([sys.argv[1:], {{k: os.environ[k] for k in {observed!r} if k in os.environ}}]))\n")
    for target in (app, driver):
        target.write_text(fixture)
        target.chmod(0o700)
    launchers = root / "launchers"
    launchers.mkdir(mode=0o700)
    environment = dict(os.environ, deb_root=str(deb), native_root=str(launchers),
                       test_bash=bash, test_original_path=os.environ["PATH"])
    environment.update({key: "/fixture/nix/path" for key in cleared})
    environment.update(XDG_DATA_DIRS="/fixture/nix/share", EUTHETO_RUNTIME_SENTINEL="retained")
    script = generation.replace("/usr/bin/WebKitWebDriver", str(driver))
    # The same shell must retain its Nix lookup after launching native children.
    script += '\n"$test_bash" "$native_root/eutheto-desktop" "space argument" \'$literal;*\' ""\n'
    script += 'test "$LD_LIBRARY_PATH" = /fixture/nix/path\n'
    script += 'test "$GSETTINGS_SCHEMAS_PATH" = /fixture/nix/path\n'
    script += 'test "$XDG_DATA_DIRS" = /fixture/nix/share\n'
    script += 'test "$PATH" = "$test_original_path"\n'
    result = subprocess.run([bash, "-ceu", script], env=environment,
                            text=True, capture_output=True, check=True)
    arguments, native = json.loads(result.stdout)
    assert arguments == ["space argument", "$literal;*", ""]
    assert all(key not in native for key in cleared)
    assert native["PATH"] == "/usr/sbin:/usr/bin:/sbin:/bin"
    assert native["XDG_DATA_DIRS"] == "/usr/local/share:/usr/share"
    assert native["EUTHETO_RUNTIME_SENTINEL"] == "retained"
    assert (launchers / driver.name).read_text().startswith("#!/bin/bash\n")
    result = subprocess.run([bash, str(launchers / driver.name), "--port=4445"], env=environment,
                            text=True, capture_output=True, check=True)
    driver_arguments, driver_environment = json.loads(result.stdout)
    assert driver_arguments == ["--port=4445"]
    assert all(key not in driver_environment for key in cleared)
    assignment = next(line.strip() for line in (ROOT / "Justfile").read_text().splitlines()
                      if '"EUTHETO_NATIVE_DRIVER=' in line)
    for override in (None, "/explicit driver/with $literal"):
        lookup = dict(os.environ, PATH=str(root))
        lookup.pop("EUTHETO_NATIVE_DRIVER", None)
        if override is not None:
            lookup["EUTHETO_NATIVE_DRIVER"] = override
        result = subprocess.run([bash, "-ceu", 'printf "%s\\n" ' + assignment],
                                env=lookup, text=True, capture_output=True, check=True)
        assert result.stdout.strip() == "EUTHETO_NATIVE_DRIVER=" + (override or str(driver))
    driver.unlink()
    result = subprocess.run([bash, "-ceu", generation.replace(
        "/usr/bin/WebKitWebDriver", str(driver))], env=environment, capture_output=True)
    assert result.returncode != 0, "A missing host driver must stop launcher preparation"

print("Native runtime launcher checks passed")

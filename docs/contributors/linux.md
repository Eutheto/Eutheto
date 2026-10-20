<!-- SPDX-License-Identifier: Apache-2.0 -->

# Linux Development

The locked Nix flake is the canonical Linux environment. It exposes `x86_64-linux` and `aarch64-linux`; an evaluated output is not, by itself, evidence that both architectures have built or launched the desktop on a real runner.

Follow [`development.md`](development.md) for the `direnv` and no-`direnv` entry paths, explicit bootstrap, repository-local caches, and cache-miss behavior. Phase 00 does not claim a non-Nix distro package matrix or a published Linux desktop artifact.

## Desktop libraries supplied by Nix

The default shell supplies the GTK3/Tauri development and runtime set, including GLib, GTK3, WebKitGTK 4.1, libsoup 3, librsvg, the current Tauri AppIndicator dependency, `pkg-config`, and an X virtual server. It also constructs `PKG_CONFIG_PATH` and the Linux runtime library path from those declared packages. The canonical `just desktop-dev` recipe launches through a repository-pinned [`nixGL`](https://github.com/nix-community/nixGL) wrapper so WebKitGTK receives a graphics userspace compatible with the Nix libraries instead of mixing them with ambient distribution libraries.

After entering `nix develop`, verify the key package contracts without consulting ambient distro packages:

```console
pkg-config --modversion glib-2.0
pkg-config --modversion gtk+-3.0
pkg-config --modversion webkit2gtk-4.1
pkg-config --modversion libsoup-3.0
pkg-config --modversion librsvg-2.0
```

A missing `.pc` file inside the Nix shell is a flake/tooling defect or an unsupported lock state; installing a global `-dev` package is not the canonical fix. Confirm that the command was run inside the shell (`EUTHETO_SHELL` is set), then report the failed `pkg-config` query and selected Nix system.

## Automatic graphics runtime

No separate `nixGL` installation or command prefix is required. On Linux, `just desktop-dev` selects the pinned Mesa wrapper for Intel, AMD, and Nouveau graphics. On `x86_64-linux` with the proprietary NVIDIA kernel module loaded, it instead selects the matching NVIDIA wrapper at command execution time. That host-selected NVIDIA path may populate the Nix cache on first launch; entering `nix develop` remains side-effect-free. macOS and native Windows keep their platform-native launch paths.

The wrapper deliberately runs inside the development shell. Reversing that order allows the shell's `LD_LIBRARY_PATH` to replace the wrapper's graphics runtime and can reproduce WebKitGTK `EGL_BAD_PARAMETER` failures. Do not prepend `/usr/lib`, preload Mesa drivers, or install distribution development packages to alter the canonical path.

The upstream proprietary NVIDIA wrapper is `x86_64-linux`-only. `aarch64-linux` uses the Mesa path and therefore requires a Mesa-supported driver such as Nouveau; a proprietary NVIDIA aarch64 desktop remains outside the currently verified development-host contract.

## Wayland and X11 diagnostics

GTK/WebKitGTK selects a display backend from the active session. Record the display inputs before diagnosing a blank window or launch failure:

```console
printf 'session=%s\nwayland=%s\ndisplay=%s\nruntime=%s\n' "$XDG_SESSION_TYPE" "$WAYLAND_DISPLAY" "$DISPLAY" "$XDG_RUNTIME_DIR"
```

For a Wayland session, the compositor socket should exist:

```console
test -n "$WAYLAND_DISPLAY" && test -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"
```

Exercise the development surface against Wayland only from a working Wayland session:

```console
GDK_BACKEND=wayland just desktop-dev
```

For X11/XWayland, verify that the selected display accepts a client before blaming WebKitGTK:

```console
xdotool getdisplaygeometry
GDK_BACKEND=x11 just desktop-dev
```

Keep the backend named in the failure report. A successful launch on XWayland does not prove native Wayland behavior, and the reverse is also true. Do not force a backend globally in `.envrc` or the Nix shell to hide one failing path.

## Virtual-display diagnostic

The Nix shell includes `Xvfb` and `xdotool`. To distinguish “no display server” from an application/WebKitGTK problem, start an isolated X server in one shell:

```console
Xvfb :99 -screen 0 1280x720x24 -nolisten tcp
```

Then, from a second Nix shell in the same checkout, probe it:

```console
DISPLAY=:99 xdotool getdisplaygeometry
DISPLAY=:99 GDK_BACKEND=x11 just desktop-dev
```

Stop `Xvfb` when the diagnostic is complete. The geometry probe proves only that a client can connect to the virtual X server. The development launch proves only the exercised development surface. The canonical `just e2e true` recipe tests the actual bundled application and requires its native drivers and a separate network namespace. See the [Phase 07 PDF validation handoff](phase07-pdf-validation.md) for prerequisites, coverage, and recorded limitations.

## WebKitGTK failure report

For a Linux desktop failure, include only the bounded diagnostics needed to reproduce it:

- Nix system and whether the shell is `default`, `full`, or `release`;
- the five `pkg-config --modversion` results above;
- `XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, and `DISPLAY` (but not an entire environment dump);
- whether the Wayland socket test or `xdotool getdisplaygeometry` succeeded;
- whether the failure reproduces with native Wayland, X11/XWayland, or the isolated `Xvfb` display; and
- the exact `just` recipe and application error output.

Do not attach local caches, credentials, databases, captured user data, or unsanitized support bundles. Do not claim AppImage, distro-package, GPU/driver, native Wayland, X11, or headless support from a compile-only result; those surfaces require their own executed evidence.

## Core-only work

The [core-only route](development.md#core-only-route) does not require launching GTK/WebKitGTK. Its success is portable core/CLI evidence only, not Linux desktop or virtual-display evidence.

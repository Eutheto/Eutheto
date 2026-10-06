# Isolated CEF native feasibility

Experimental only: no application integration, accepted-plan authority, release,
signing, CEF adoption or binary/PDF redistribution. The public workflow publishes
only synthetic bounded status/count/digest evidence. SDKs, native output and PDFs
must not be uploaded as artifacts or caches.

## Run

Prerequisites: native 64-bit Python 3.11+, Rust 1.97.1 via rustup, CMake 3.21+,
Ninja and the platform compiler. Windows requires an x64 MSVC developer environment;
macOS requires Xcode command-line tools. Linux needs the X11 development library
(`libx11-dev` on the hosted Ubuntu target), CEF's native runtime libraries,
system fonts and functioning native sandbox prerequisites. Do not disable sandboxing
to turn a refusal green. Reserve at least 6 GiB of disposable disk space.

After independent source/security review, run the same command used by CI:

```sh
python3 -B experiments/cef-native-validation/probe.py --target linux64 --work-dir /absolute/nonexistent/private-work
```

Targets: `linux64`, `windows64`, `macosx64`, `macosarm64`. The native architecture
must match. The work directory must be outside the checkout and must not exist;
the harness creates private directories and refuses replacement. On Windows,
`python` is the equivalent interpreter command. The target-only workflow runs on
`experiment/cef-154-native-validation`; it does not open or merge a PR.

`pins.json` owns immutable SDK inputs. The nested Cargo workspace/lock belongs only
to this experiment; application manifests and locks remain unchanged. `requirements.txt`
pins the pure Python BSD-3-Clause pypdf inspector wheel, installed without dependencies
or source builds. Borrowed CEF source notices are preserved in `LICENSE.txt`; this is
not a complete CEF binary redistribution license/source package.

The experiment vendors `process-wrap` 10.0.0 from upstream commit
`d02486b8d1e393a4ac27023d591b6f35c6417ffb` (published crate SHA-256
`0e3f4237d0e4741eb50bc5584db701f1299c85fa31ff0274dd6445e79dc42d12`).
Its MIT/Apache licenses and copyright are retained. The local patch changes only
the manifest and `src/windows.rs`, `src/tokio/job_object.rs`, and
`src/std/job_object.rs`: Windows completion requires the matching job key and
`ACTIVE_PROCESS_ZERO`, remembers empty state, and polls without an orphaned
blocking waiter. It adds no unsafe code and is not a product dependency cutover.
All external package versions/checksums remain those of the existing product lock.

The repository-owned Rust supervisor is bootstrapped with trusted Cargo/toolchain
inputs before any SDK code executes. That bootstrap and OS/toolchain discovery
have direct bounded capture and the runner timeout, not a claim of supervision by
the not-yet-built binary. Subsequent SDK configure/build and inspector operations
use its owned process group/JobObject, capped private logs and explicit settlement.
Cargo process-lifetime regressions and the Python publication-privacy regression
run through that owner before SDK provisioning.

Native input remains withheld until the parent retains the initial process identity
and acknowledges it. Cancellation/deadline cases must report their intended terminal
cause; pipe/protocol failures cannot masquerade as expected native refusal.
Windows bootstrap cwd changes never select the private profile/output directory.

## What is measured

- Complete native bootstrap/helper/browser lifecycle with sandbox enabled.
- A fixed synthetic HTML fixture, actual excluding screen filter, beforeprint restoration,
  64 distinct PDF row tokens, Unicode and literal inert markup, multiple pages and
  repeated revision/timezone/status/page context. The runner parses the PDF before deletion.
- Malformed/oversized input, startup/readiness/printing-stage cancellation, held-readiness
  deadline, real host/renderer termination, output-path write failure and native
  resource/navigation/popup/download refusals. The popup case alone disables Chromium's
  upstream popup blocker to reach the host's native refusal callback.
- A reachable loopback HTTP positive control and zero handled native requests to that
  sentinel; this is not a claim about all TCP traffic.
- Actual native renderer restrictions, separate from transport/content success.
  Unavailable observations fail the corresponding row, never pass by configuration alone.
- Observed process identities disappearing before private job removal. Windows owner
  termination exercises kill-on-close; Unix owner termination exercises its SIGTERM handler.
- Exact source/base/run identity, SDK/fixture hashes, compiled owner/host hashes,
  the Windows client DLL or every macOS helper image, and actual tool/compiler versions.

`evidence.json` remains in the disposable work directory. All cases are non-optional;
any failed/unverified row returns nonzero. CI always attempts a bounded summary,
including when setup/download/build fails. Superseded/cancelled CI is not evidence.
Native/tool stderr, raw process arguments, SDKs and PDFs are never published.

Failure diagnostics publish only closed operation/category tokens, known source
filenames with bounded numeric locations/compiler codes, and closed native-observation
facts. Raw messages and paths remain private. macOS lifecycle tokens locate shutdown
progress; only three exact watchdog stderr lines produce fixed fault tokens.
Watchdogs never count as successful refusal. Intentional native refusals require
the host's exit 72, not an arbitrary nonzero exit. Renderer injection terminates
the complete observed renderer set with exact identity guards, avoiding reliance on
PID ordering when Chromium has spare renderers.

Hosted run [37486042193](https://github.com/Eutheto/Eutheto/actions/runs/37486042193)
at `e9dc6546dcb7955329c15cc5d09e45197bcc09e0` failed all four targets:
Linux/Windows failed during build; both macOS targets reached printed/closed but
exited 2. Neither macOS result establishes PDF fidelity or successful shutdown.
The added diagnostics investigate these failures without disabling the sandbox
or accepting watchdog termination.

Run [37491280964](https://github.com/Eutheto/Eutheto/actions/runs/37491280964)
at `0286aff8a117199c072faaab8dc9550a94e6d641` confirmed a teardown watchdog
inside `CefShutdown` on both Macs. Renderer-set injection now reaches
`renderer-exit`, but shutdown still fails; readiness cancellation also lacks
settlement. Linux reports compilation at the inherited callback disposition
type; Windows reports owner-link failure. These are failure diagnoses, not
native acceptance.

Run [37493396623](https://github.com/Eutheto/Eutheto/actions/runs/37493396623)
at `8d1f36ad03907f70e828d5c21d6f6b19508982c0` progressed Linux past the callback
compile error to a missing link-library failure. Windows diagnostics identified
GNU `link.exe` rather than MSVC; Cargo now selects the admitted MSVC linker explicitly.
These corrections do not establish target acceptance.

Run [37496931725](https://github.com/Eutheto/Eutheto/actions/runs/37496931725)
confirmed missing `-lX11` on Linux and reached the Windows completion-constant
import after selecting MSVC. CI now installs the declared X11 link prerequisite;
the vendored binding imports `JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO` from its actual
`SystemServices` module with the matching feature. macOS shutdown remains unresolved.
Intrusive external PID-only sampling was rejected before target execution; no
external sampler or relaxed acceptance path is included.

Run [37498847783](https://github.com/Eutheto/Eutheto/actions/runs/37498847783)
at `bb4c313b7f7a3a1fd0313046ff6dcaee0e23e23b` failed all four targets.
Linux built and ran but normal execution stopped after startup; its cause is not
established. Windows progressed through owner checks and SDK admission, then failed
the build boundary. Both Macs still reached the teardown watchdog. Closed tool-boundary
reasons and two exact private native-log literals now distinguish these failures
without publishing raw logs or changing sandbox policy.

After an ordinary macOS normal-case teardown watchdog, a separate
`shutdown-diagnostic` invocation observes its own retained main-thread Mach right.
The joined observer uses fixed buffers and bounded frame reads, resumes the main
thread before symbol lookup/output, and publishes only closed nearest-symbol
categories. Unsupported state or an incomplete walk remains explicit; these
categories do not establish the blocking function or shutdown cause. No external
process attaches by PID. Resume/join failures terminate rather than releasing
unsettled observer state. The diagnostic invocation always fails acceptance, even
if shutdown returns; it never substitutes for the original unsampled case.

## Limits

This is platform feasibility, not product acceptance or redistribution clearance.
Hosted macOS 15 does not prove the SDK's minimum macOS 13 support. No signing or
notarization is performed; unsigned execution that cannot satisfy native gates is
reported as a failure. No packaged-font/all-script or screen-reader claim is made.

Snapshots are point-in-time observations, not proof that every transient descendant
was observed or that a compromised browser is universally confined. Windows token
inspection reports membership in some job, not the identity of the supervisor's
unnamed JobObject; atomic admission is owned by process-wrap and exercised by owner
termination. macOS observations establish Seatbelt presence and helper ancestry,
not every policy rule. Printing-stage cancellation does not prove exact in-flight
PDF work. A windowless macOS browser still uses the operating system's GUI session.
The Windows owner-death row deliberately has no measured job-empty acknowledgement:
it records kernel kill-on-close plus disappearance of retained sampled identities.
Other missing settlement evidence fails the row and preserves private staging after
attempting guarded cleanup. Abnormal termination without trustworthy identities is
not a cleanup proof; hosted runner teardown remains the final containment boundary.

The work directory retains locally built SDK/runtime/inspector inputs for inspection;
remove only that caller-owned directory after reviewing evidence. No generated
application artifacts, product permissions or release configuration are changed.

<!-- SPDX-License-Identifier: Apache-2.0 -->

# Phase 07 PDF validation handoff

This records the remaining native validation as of 2026-10-18. Successful compilation, component mocks and earlier HTML export evidence do not establish native PDF acceptance. Dependency-security and platform/manual acceptance remain separate gates. The exact HTML-table software use has reviewed W3C Document License (2015) accounting and full retained terms; that does not settle dependency advisories.

## Dependency remediation follow-up (2026-10-18)

The approved dependency changes are implemented: exact upstream citationberg
commit `06a591e2f237d25e1dfdedac3f3d1494c496c52d` is vendored unchanged, resolving
quick-xml 0.41.0; ZIP is pinned to 9.0.5 with its prior feature selection in the
workspace and import fuzz lockfiles. All 21 upstream files match the commit.
The source inventory hashes the complete vendored tree and rejects extra files,
links and filename aliases. Provenance retains the released registry checksum,
exact upstream commit, full licenses and unchanged-file hashes. Citationberg's
full compatibility suite passed 12 tests and one doctest, including the external
CSL styles/locales corpus.

The eight unchanged upstream CSL/XML test fixtures have their own
`CC-BY-SA-3.0` terms. Their dedicated asset group, full retained legal text,
upstream NOTICE, original XML attribution and exact-file review remain distinct
from the library's `MIT OR Apache-2.0` license. The fixtures are filesystem-read
test data, not production renderer embeds. This scoped accounting follows the
existing third-party-data policy and changes no general license allowlist;
it is not general legal or release clearance.

All six approved maintenance dispositions have enforced exact
package/version/source/checksum and informational classification checks through
2026-11-16 UTC inclusive, with no automatic renewal. The standard security
recipe passed against freshly fetched official RustSec database commit
`999ec89d5d3f10f20be14750ec8c2d8b44e0ac9b`: zero vulnerabilities. The five
quick-xml/ZIP vulnerability IDs are never ignored. Existing GTK policy remains
separate. The gate and re-review conditions are documented in
[CONTRIBUTING.md](../../CONTRIBUTING.md).

The canonical CLI/PDF package rebuild and full practitioner default/all-fields
HTML/PDF checks pass with these dependencies; the prior package is preserved.
Dependency regression evidence also includes 105 archive/import/export tests,
eight Core backup/restore tests, nine fuzz targets compiling, 84 xtask tests,
and regenerated notices/SPDX. Follow-up also passes strict Clippy across all
targets/features of the six affected crates, seven real PDF worker tests, four
Core PDF custody tests, ten CLI solution tests, 50 People CSV tests, seven native
real-worker custody tests, and 110 CLI corpus checks across 15 cases. The updated
inventory passes the actual native About command and three inventory-boundary
tests. Frontend formatting, lint, type checking and all 406 unit/component tests
pass. A fresh `just frontend-build` also passes production type checking and
Vite bundling; it does not execute the native UI. Correctness, complexity and
scoped fixture-license reviews are complete;
their findings were fixed and revalidated. This dependency follow-up does not
clear the desktop UI or manual gates below.

## Stored-project CLI follow-up (2026-10-18)

A disposable library was created through `projects create`, populated from the
synthetic `clinic-tiny` fixture through one public `scenario batch`, and solved
through the canonical CLI with the actual approved worker. Its independently
verified stored result contains ten accepted assignments across five shifts and
two assigned people (the scenario has three people). Export used stored scenario
and result UUID operands, exercising Core's database-bound PDF capture and
publication revision lease rather than the explicit-file route.

Default and all-fields HTML/PDF exports preserved the full accepted assignment
census, checked names against disclosure choices, matched receipt/file hashes,
and left the scenario unchanged. Both PDFs have five pages. Default PDF:
69,872 bytes, SHA-256
`9e20648f8d5ebab9b1875eec7e3a7992602e5786cdbe0da3812cf85a6ac9e70d`;
all-fields PDF: 74,396 bytes, SHA-256
`8e45e97bd4bbd46388589833df7edaa9a95d04c553659619f442cfc4dfb8800c`.
Existing-destination refusal returned exit 8 with
`portable.destination_exists`, preserving the published bytes.

After a public edit advanced the head to revision 2, exporting the revision 1
result without consent returned `recipient.stale_consent_required` and created
no destination. Explicit `--allow-stale` published all ten original assignments,
with `storedTerminal` source, scenario revision 1 and observed current revision
2; export left that head unchanged. The consented PDF is 70,214 bytes, SHA-256
`10a5fef5343972ca3e8ed583259d3f00136f3dc534eebdd28c7a67a94ad29971`.

Commands, receipts, source/accepted records and extracted text are retained in
the disposable checkpoint directory
`.cache/phase07-stored-pdf-yeuvfb0z/` under the original checkout. These checks
need no namespaces and add real stored-CLI evidence. They do not establish
concurrent-edit race behavior, desktop picker/focus/cancellation, or platform
acceptance. Additional CLI package hashing would repeat existing evidence;
no current desktop package or non-Linux Rust target is available to validate.

## Environment

Use the repository's pinned Nix development environment on native x86_64 Linux, with locked Rust, Node and pnpm dependencies prepared. The packaged desktop recipe also supports its documented native macOS and Windows build paths; Linux WebDriver results do not cover those platforms.

The Linux native runner needs `tauri-driver`, `WebKitWebDriver`, `xdotool`, `xvfb-run`, the repository desktop-runtime wrapper, and a working `unshare`/`ip` isolation path. The default recipe creates a user and network namespace. Its existing CI alternative, `EUTHETO_E2E_PRIVILEGED_NETNS=1`, uses approved `sudo` network-namespace creation followed by a privilege drop. Both retain network isolation. The latter requires privileges already provided by the test environment; it is not a fallback authorization to change container capabilities.

The saved cloud container rejects this non-mutating preflight:

```sh
unshare --user --map-root-user --net -- true
```

Result: `unshare failed: Operation not permitted`. No documented equivalent isolation mode is available here under current permissions. Do not omit the namespace, grant capabilities, change the network configuration or switch environments to obtain a passing result without authorization.

The same preflight in the existing outer Codex command sandbox fails while opening `/proc/self/uid_map` with `Read-only file system`. Removing the inner Docker container alone therefore does not provide a passing test path. This establishes the currently accessible sandbox limitation, not a claim that all platform execution contexts forbid namespaces.

The current pure Nix container also lacks the portable Linux worker's platform
runtime. The worker requires `/lib64/ld-linux-x86-64.so.2`, while `/lib64` and
`/lib` are absent. Direct execution returns `ENOENT`. Invoking the pinned Nix
loader explicitly then reports missing `libstdc++.so.6`; invoking that loader
with the matching pinned GCC runtime produces a protocol response. This last
command is a startup diagnostic, not a packaged solve or acceptance test.

On 2026-10-17 the current-branch Nix worker build and canonical CLI image
assembly passed, but the practitioner solve returned `backendUnavailable`.
The seven native real-worker tests compiled with `bundled-ortools` and all
failed while establishing accepted fixtures. These results do not establish
recipient publication. Nix's worker install check uses a temporary copy with
its interpreter/runtime paths adjusted, so that build check does not establish
execution of the unchanged portable artifact inside this container.

The user-approved narrow setup was subsequently applied: the standard loader
link and the exact prepared `/etc/ld.so.cache` were installed without overwriting
existing destinations. Artifact hashes remained unchanged. This was insufficient:
the Nix loader's compiled cache pathname is
`/nix/store/<glibc>/etc/ld.so.cache`, so it ignores `/etc/ld.so.cache`.
The practitioner solve then returned `backendFailed`, and all seven native
fixture tests still failed. An existing tiny real-worker test exposed the
underlying `libstdc++.so.6` loader error before `main`; its `MissingTerminal`
protocol error was a downstream symptom. No worker/protocol defect was
established. Temporary synthetic-test diagnostics were removed.

A corrected, approved Linux runtime environment is required before retrying
these acceptance paths. The two-entry setup attempt must not be represented as
a working saved-environment fix; do not modify immutable Nix store contents to
make its loader consume the cache. No further protected runtime paths, loader
configuration or process-launch rules were changed during the investigation.

A subsequent workspace-only diagnostic tested a corrected FHS runtime snapshot
using the unchanged packaged worker and all 87 private shared libraries. The
snapshot contains seven existing Debian runtime files (glibc
`2.41-12+deb13u4`, GCC runtime `14.2.0-19`; 5,907,584 bytes), each recorded by
SHA-256. Its loader uses `/etc/ld.so.cache`; the cache names a content-identified
`/opt/eutheto-runtime/<bundle>/lib` prefix reproduced inside the disposable
workspace root. Using the container's existing chroot capability, with UID,
GID and supplementary groups dropped to 1000 before execution, a cleared
environment and the supervisor's private-library-path convention, the worker
completed the repository's tiny protocol fixture: successful handshake,
Started, Incumbent and Finished/OPTIMAL, exit 0 and empty stderr. Repository
strict framing/wire decoding checked the response and final candidate. No
container configuration, namespace, package, Nix-store or artifact change was
made by this diagnostic.

Explicitly invoking the pinned Nix loader with its matching GCC library path
also completed the same diagnostic. That alternative would require changing
the application's verified direct-worker launch contract. The workspace FHS
snapshot tests the transparent runtime approach instead. Nix's existing FHS
wrappers require unavailable namespaces and have not passed here.

The user subsequently approved installing this exact snapshot. Installation
verified source hashes, prior root ownership, the exact old loader target and
old cache hash before guarded replacement. All seven installed runtime files,
notices and manifest are root-owned; product executable hashes are unchanged.
Backups of the two prior entries are retained. No package upgrade or additional
access change was made. The installation applies to this existing development
container; it has not been published as a saved-environment update.

Validation with that runtime now passes:

- All seven native `recipient_report::worker_tests` ran with `bundled-ortools`
  and the exact approved worker artifact: seven passed, none ignored.
- Canonical CLI corpus smoke passed 110 checks across 15 cases, including two
  exact-byte PDF publication checks, accepted solves with fresh verification
  and export roundtrips, expected infeasibility/refusals, and repair paths.
- The actual `practitioner-workflow` source solved and was independently
  verified. Default and all-fields HTML/PDF exports preserve all 68 selected
  assignment identities and person/work pairs, all 48 work instances and
  intervals, and all 12 people. Checks cover default name/authored-label
  exclusion, optional section headings, exact privacy switches, receipt hashes,
  per-page context, text bounds, unchanged source and no-clobber refusal.
  Default PDF: 25 pages, 307,424 bytes; all-fields PDF: 32 pages, 378,753 bytes.
  Representative first, assignment-census and workload pages were rendered and
  visually inspected for wrapping, grayscale readability and page context.

These results establish actual CLI publication and native custody tests, not
desktop picker or UI lifecycle acceptance. Preserve the diagnostic inventory,
exact file hashes, response frames, installed-state receipt and acceptance
results with the environment checkpoint. The standalone acceptance harness
counts selected Boolean assignments only, strips explicit zero-width line-break
opportunities when comparing extracted identities, and distinguishes optional
section headings from the always-present privacy disclosure labels.

## Automated commands

Run from the intended worktree within the pinned environment:

```sh
just worker-build-desktop
EUTHETO_E2E_RECIPIENT_EXPORT_ONLY=1 just e2e true false
```

The build binds both helper manifests. The E2E recipe extracts the DEB, verifies the PDF executable against the retained manifest digest, and launches that extracted application with disposable cache/config/data/runtime/state directories. Reuse the bundle only while its application inputs are unchanged.

The recipient E2E case creates and solves the synthetic `clinic-tiny` fixture. It retains keyboard review/focus behavior, CSV/ICS publication, private HTML defaults, picker cancellation, accepted source binding and unchanged scenario revision. The PDF-specific steps are now authored but **have not executed** in this environment. They select PDF by keyboard, retire the HTML review, close/reopen a PDF review with focus restoration, cancel the PDF picker, and publish default/all-fields PDFs through the native picker. Assertions compare complete captured assignment pairs and person/work identities, disclosure choices, and saved file hashes against actual native previews and publication receipts. A disposable pre-existing destination tests no-clobber after chooser overwrite confirmation. This small fixture has no non-empty recorded changes or fairness groups; the richer practitioner evidence remains separate. The observer records real native replies without changing delivery and is restored during cleanup.

Focused Prettier, ESLint and Node syntax checks pass. All 30 embedded WebDriver scripts in this case parse independently; an isolated observer check confirms unchanged reply forwarding and restoration. Correctness and complexity review cover these authored steps. None of these checks launches WebDriver, a picker, or the bundled application. Exact PDF text/layout, stale UI consent, preparation cancellation, application exit/helper cleanup and the other native acceptance below remain separate gates; the new PDF E2E steps themselves also await a namespace-capable run.

The supplemental component case is:

```sh
just frontend-browser-install
pnpm --filter @eutheto/desktop run test:browser -- src/components/RecipientShare.browser.test.ts
```

It uses the lock-pinned Playwright Chromium headless shell. This saved environment has no cached executable; the pinned browser case has not run. The older download attempt was blocked at `cdn.playwright.dev`. In the separate fresh browser task, that host now redirects to `storage.googleapis.com` for the exact Chrome-for-Testing archive and `playwright.download.prss.microsoft.com` for the exact FFmpeg archive. Both approved redirect hosts remain proxy-blocked; allowlist publication is pending with the environment owner. No duplicate installer or download was attempted here. Do not substitute an arbitrary browser, bypass the proxy, or relax browser security. The case is typechecked and uses mocked native responses: it checks format-change retirement, a fresh PDF review, stale consent and matching publication receipts, rather than actual renderer execution.

Read-only HEAD requests to the exact pinned download URL, both inside and outside the development container, return Envoy HTTP 403 during proxy CONNECT before CDN TLS. The effective policy is not exposed; its environment/workspace owner is unestablished. Changing the package manager or removing Docker would not automatically resolve this observed denial.

## Native PDF acceptance

Use only the synthetic fixtures `clinic-tiny` and `practitioner-workflow` from `domains/workforce/fixtures/v2/`; create disposable projects and independently verified accepted results through the normal workflow. The practitioner case has 12 fictional people, 48 shifts and 68 accepted assignment pairs.

1. Open an accepted result, choose Share result, select PDF and prepare a native review. Confirm source/result/revision identity and the selected disclosure fields. Changing format or disclosure options must retire the old review and require another preparation.
2. Cancel the native save picker, prepare again, then save a PDF through the native picker. Confirm the publication receipt, valid PDF bytes, no scenario mutation and no destination clobber. Repeat with optional staffing, changes and workload actuals enabled.
3. Compare the PDF against the exact captured disclosure model: all accepted assignment identities and selected work/person/optional sections remain present; private authored names/labels remain absent under defaults. Check the full practitioner census, not only one page or a file signature.
4. Extract text and bounding boxes with an independent PDF tool. Inspect representative pages visually for wrapping, readable grayscale, page context and missing glyphs. Verify repeated revision/status/time-zone/generated context on every page. Unsupported characters must fail explicitly rather than disappear.
5. Exercise preparation cancellation, closing the review, application exit during rendering, stale/source-change refusal and stale consent. Confirm helper termination, custody release, and removal of incomplete staging. Check that an HTML capture cannot be consumed as PDF or vice versa.

Still unverified in this cloud environment: actual bundled desktop PDF publication/picker behavior, PDF-specific keyboard/focus and cancellation integration, spoken assistive technology, and macOS/Windows lifecycle and final artifact behavior. The complete practitioner CLI output is now checked as recorded above. Record exact artifact digests, commands, fixture identities and results when the remaining gates are run. Manual checkpoint acceptance and authorization for Phase 08 remain with the user.

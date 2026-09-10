<!-- SPDX-License-Identifier: Apache-2.0 -->

# @eutheto/desktop — development desktop foundation

This package is the non-public Tauri/Vue development application. Its existing
[Phase 01 core shell and persistence](../../docs/roadmap/01-core-application-shell-and-persistence.md)
now consumes the [Phase 06 UI001 foundation](../../docs/roadmap/06-desktop-design-system-and-workforce-setup.md).
It exercises the real local Rust application service; it is not a mock shell,
a released installer, or evidence that Workforce setup/result screens or the
public application identity are complete.

## Implemented development behavior

At startup, Tauri opens one `EuthetoApp` service backed by
`library.sqlite3` in the platform application-data directory. Rust and SQLite
are authoritative. The Vue application loads projections from that service and
sends typed requests back to it; browser state is not a second persistence
layer.

Hash routing currently exposes only the implemented project home. Pinia retains
the transient selected-project ID; Colada caches the native project list with
explicit load/event/committed-mutation refreshes. Neither owns scenario data or
replays native writes. Existing Reka-based confirmation dialogs own focus and
prevent closing-animation or busy-state activation from authorizing a mutation.

The current `ProjectHome` surface supports:

- listing persisted active and archived projects;
- creating an `official.test` project with explicit locale, time zone, horizon,
  units, and daylight-saving gap/overlap policies;
- selecting, duplicating, archiving, unarchiving, and permanently deleting a
  project;
- previewing and applying project imports, including an explicit action for
  each identifier collision;
- previewing and creating full-library backups; and
- previewing restores in add-to-library or replace-library mode, reviewing
  collisions, and explicitly confirming the destructive replacement path.

The Rust command boundary and generated client also implement project export
preview and creation. The current `ProjectHome` component does not expose
export controls, so export is API scope rather than a claim about the visible
home screen.

The native setup boundary exposes V2 summary/readiness, bounded Workforce
views, entity detail/search, rule catalogs, command previews, and explicit full
validation. Rust owns each projection and its immutable input revision.
Preview does not commit; apply, history, undo/redo, and local settings retain
their existing application-service authority. The present Vue surface remains
a project home, not a Workforce editor.

Setup operations reserve a window/context-bound identity before work, use
bounded admission and an invocation-owned progress channel, and release
ownership on settlement or disposal. Cancellation acknowledgement is not a
terminal result. Full readiness distinguishes not-run, running, completed,
failed, cancelled, and stale input; an empty fast report is not full readiness.

Accepted-solution listing, detail, views, selection, verification, comparison,
explanations, and counterfactual requests are implemented at the existing V1
native boundary. Their retained accepted document/solution pairing remains
authoritative; nested accepted-result and verification formats retain their
own V2 versions. These APIs do not imply completed result screens or ordinary
live-solving controls.

Live solve, deferred solution-control/export, and AI command names remain
registered but unavailable where `app_get_capabilities` reports them so;
those calls return typed `unsupported` errors. The desktop does not advertise
live solving or AI availability.

## Rust-authoritative flow

```text
ProjectHome.vue
    │
    ▼
project-home.ts
    │ typed generated functions
    ▼
src/api/generated.ts
    │ Tauri invoke
    ▼
src-tauri/src/lib.rs
    │
    ▼
EuthetoApp ── SQLite library
```

The application-data database is the durable project authority. Portable
imports and exports use the managed local exchange area, while backups use the
managed backup area. Portable operations preview their artifact before
mutation. A replace-library restore requests a safety backup before replacing
the current library.

## Generated API-only Tauri imports

[`ADR-012`](../../docs/adr/012-tauri-api-and-generated-dtos.md) defines the IPC
boundary. In production frontend source,
`src/api/generated.ts` is the only file that imports
`@tauri-apps/api/core`. Vue components and controllers consume its typed
functions instead of importing `invoke` directly.

`src/api/generated.ts` is generated from the Rust-owned command and DTO
contract and is checked in. Do not edit it by hand:

1. Change the authoritative Rust DTO/command and the corresponding template
   or catalog in `xtask/src/generate.rs`; change pack-owned query contracts at
   their schema authority.
2. Run `just generate`.
3. Review the generated diff.
4. Run `just generate-check` to reject drift.

The generated client parses IPC responses, errors, and events from `unknown`,
checks the declared schema, identities, revision correlation, discriminants,
and collection bounds, and preserves lossless integer representations.
Malformed responses do not trigger automatic command replay. Native command
dispatch and the build manifest consume the same generated command catalog;
the permission and local-window configuration are checked against that boundary.

See
[generated code discipline](../../docs/contributors/generated-code-and-contracts.md)
and [generated artifacts](../../docs/architecture/generated-artifacts.md).

Tauri's `src-tauri/gen/schemas/` and `src-tauri/permissions/autogenerated/` are
separate, ignored native build outputs. Change the checked-in native inputs and
run the native build; do not commit or hand-edit those generated files.

## Capability and release boundary

The local `main` window receives the `allow-phase-01-api` permission. The
permission admits the registered protocol catalog, while
`app_get_capabilities` remains the authority for which registered commands are
implemented in this development phase. No shell or broad filesystem permission
is granted to the webview.

The configured content security policy permits local application resources and
Tauri IPC; it does not permit remote scripts or remote navigation. Tauri
bundling is disabled, and the repository desktop build uses `--no-bundle`.
There is no supported packaged-desktop E2E command.

## Development identity and portable-extension notices

These values are explicit development values, not public compatibility or
release commitments:

| Value                                | Current use                                     | Status                                                                                                  |
| ------------------------------------ | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| `dev.eutheto.phase01.desktop`        | Tauri application identifier                    | Development-only; not the approved public reverse-domain identifier                                     |
| `eutheto Phase 01 development shell` | Tauri product name and main-window title        | Development-only; not a release brand                                                                   |
| `0.1.0`                              | Workspace and Tauri version                     | Development version; not a released desktop version                                                     |
| `.eutheto`                           | Portable scenario and backup artifact extension | Proposed and reported by the API as `provisional-development-only`; not a final public file association |

The stable/beta application identifiers, final portable extension, signing
identities, updater trust, and packaged-release evidence remain open gates.
See [identity gates](../../docs/architecture/identity-gates.md).

## Development commands

Run these commands from the repository root. The `just` recipes are the
canonical repository entry points.

| Task                                                  | `just` command                     | Direct pnpm command                                                            |
| ----------------------------------------------------- | ---------------------------------- | ------------------------------------------------------------------------------ |
| Install locked dependencies                           | `just install`                     | `pnpm install --frozen-lockfile --ignore-scripts`                              |
| Run the native Tauri development application          | `just desktop-dev`                 | `pnpm --filter @eutheto/desktop run tauri dev`                                 |
| Run only the Vue/Vite development server              | `just ui-dev`                      | `pnpm --filter @eutheto/desktop run dev`                                       |
| Type-check the frontend                               | `just typecheck`                   | `pnpm --filter @eutheto/desktop run typecheck`                                 |
| Run desktop ESLint                                    | `just lint` (repository-wide)      | `pnpm --filter @eutheto/desktop run lint`                                      |
| Check desktop Prettier output                         | `just fmt-check` (repository-wide) | `pnpm --filter @eutheto/desktop run format:check`                              |
| Run the UI unit/component tests                       | `just test-ui`                     | `pnpm --filter @eutheto/desktop run test`                                      |
| Install the pinned browser explicitly                 | `just frontend-browser-install`    | `pnpm --filter @eutheto/desktop exec playwright install --only-shell chromium` |
| Run real browser dialog/keyboard/accessibility checks | `just frontend-browser-test`       | `pnpm --filter @eutheto/desktop run test:browser`                              |
| Prove Linux native persistence across app restarts    | `just e2e`                         | Use the recipe's isolated data/runtime/network environment                     |
| Build the Vue frontend                                | `just ui-build`                    | `pnpm --filter @eutheto/desktop run build`                                     |
| Build the native desktop executable without bundles   | `just desktop-build`               | `pnpm --filter @eutheto/desktop run tauri build --no-bundle`                   |

API generation is Rust-owned: use `just generate` and
`just generate-check`, which invoke the corresponding `cargo xtask` commands.

The native development application requires the pinned Rust toolchain and
platform prerequisites described in
[development setup](../../docs/contributors/development.md). The Vite-only
server does not provide the native Tauri/Rust service.

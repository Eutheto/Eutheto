# Cross-Cutting Portable Data, Backup, and Result Sharing

## Status, source, and authority

This document incorporates **Eutheto Export, Import, Backup, and Output Sharing Specification**, dated 2026-08-29, from `eutheto-export-import-backup-sharing-spec.md`, SHA-256 `679efcf2beb4c27a60ba36fae28870d51900d28180c34a17665c57dd0c7e8181`.

It is the cross-cutting roadmap contract for portable scenario data, full-library backup and restore, and immutable result sharing. Numbered phase documents remain authoritative for their implementation slices; this document defines the model and security boundaries those slices share.

The source specification recommends `.eutheto` as the one-file portable extension. This roadmap adopts `.eutheto` as the **current proposed direction**, replacing `.optplan` as the working example, but does not falsely close the repository's identity gate. Phase 11 must approve the final extension, media types, operating-system association, and compatibility commitment through the release identity ADR before public use. Until then, every mention labels `.eutheto` proposed.

Future collaboration follows [Collaborative Planning and Hosted Service](collaborative-planning-and-hosted-service.md#reuse-of-existing-authorities), the Branch-H workflow authority, without replacing this document's existing planning/scenario/result/share formats or adding a second constraint database. Requested treatment and human approval remain collaboration-layer records; accepted planning effects use existing typed commands and scenario revisions. This adds no Phase 00–12 scope or gate and does not change the mandatory pause after Phase 05 before Phase 06.

## Product outcome

Eutheto provides two intentionally separate portability systems:

1. **Editable-data portability and durability:** export one scenario, back up the portable library, import on another installation, and safely restore without depending on the source database.
2. **Generated-result sharing:** create a privacy-filtered, immutable, self-contained plan report for recipients who do not need the editable source scenario or Eutheto itself.

The product leads with intent:

- **Share result** — produce a recipient-oriented immutable report;
- **Export editable scenario** — transfer the complete portable scenario meaning;
- **Back up everything** — preserve the portable local library; and
- **Restore backup** — add to or replace the portable library under stronger safeguards.

A scenario bundle is not a report. A report is not a backup. A valid checksum is not sender authenticity. Product language and API types preserve these distinctions.

## Durable model boundaries

Four independently versioned representations prevent persistence and privacy concerns from collapsing together:

```text
Internal Scenario Model / SQLite documents
        │ explicit portable conversion
        ▼
Portable Scenario Model
        │ bundle serializer/migrations
        ▼
Proposed .eutheto bundle

Accepted immutable Result Model
        │ explicit privacy filter + share options
        ▼
Share Result Model
        ├── desktop share preview
        ├── standalone HTML renderer
        └── controlled PDF renderer
```

- **Internal Scenario Model:** authoritative editable state and persistence implementation. It may contain indexes, caches, journals, or device state that are not portable.
- **Portable Scenario Model:** implementation-independent, strict, documented semantic representation. It preserves every solver-relevant input and compatible nonsemantic extension required by the exported scope.
- **Result Model:** immutable accepted output tied to an exact scenario revision, solution/result ID, verification checksum, compiler/backend versions, options, and authoritative score.
- **Share Result Model:** purpose-built presentation data containing only fields selected and required for a recipient. It never defaults to the complete scenario or internal diagnostics.

The portable migration system and internal database migration system are separate. They may share domain conversion utilities, IDs, units, and validation, but neither treats a database schema version as a portable schema version.

## MVP artifact matrix

| Artifact | Purpose | Source authority | Principal consumer | MVP format |
|---|---|---|---|---|
| Scenario export | Move or archive one editable scenario and only its required records | Current Portable Scenario Model | Another Eutheto installation or documented tooling | One proposed `.eutheto` ZIP-compatible bundle |
| Full backup | Reconstruct the portable user library | Consistent portable snapshot | Fresh/current Eutheto installation | One proposed `.eutheto` bundle with `bundleKind: full-backup` |
| Standalone result | Share a recipient-oriented accepted plan | Privacy-filtered Share Result Model | Modern desktop browser, no Eutheto/server | One self-contained `.html` file |
| PDF result | Print/conventional record of the same share payload | Same Share Result Model and report renderer | PDF reader/printer | Direct `.pdf` export |
| Domain tabular/calendar output | Interoperate with another application | Accepted result or valid scenario data under each output's gate | CSV/ICS/JSON consumers | Existing phase-owned formats |
| Diagnostic/support output | Explain a failed or infeasible operation safely | Bounded redacted evidence | User/support | Separate previewed format; never disguised as a result |

Selected multi-scenario backup, encrypted bundles, automatic rotation, signatures, semantic merge, hosted sharing, annotations, offline map tiles, and organizational publishing are post-MVP.

## Proposed `.eutheto` portable bundle

### Container and layout

The bundle is one ZIP-compatible file with a documented, inspectable layout. The initial logical layout is:

```text
example.eutheto
├── manifest.json
├── checksums.json
├── scenarios/
│   └── <scenario-uuid>.json
├── results/
│   └── <result-uuid>.json
├── shared/
│   ├── people.json
│   ├── places.json
│   └── ...
├── preferences/
│   └── portable-preferences.json
└── assets/
    └── ...
```

Phase 01 may refine subdivisions before version 1 freezes, but the following are binding:

- `manifest.json` has a fixed known path;
- all paths are normalized relative paths;
- structured payloads use a documented strict representation;
- `checksums.json` covers every declared payload file;
- undeclared payloads are rejected unless a versioned extension area explicitly permits them;
- canonical data and optional assets are separated;
- no executable, macro, template, link, device file, or arbitrary local path is accepted;
- a single-scenario export is self-sufficient and does not depend on unrelated source-installation rows; and
- the archive never contains the internal SQLite database.

### Manifest and versions

Version 1 publishes a strict manifest with at least:

- `format: "eutheto-bundle"`;
- `formatVersion` for container/layout/checksum/compression/asset rules;
- `schemaVersion` for portable semantic data;
- stable `bundleId`;
- `bundleKind`: `scenario-export` or `full-backup` in the MVP; `selected-backup` remains reserved/post-MVP unless separately delivered;
- UTC `createdAt`;
- `createdBy` application/version/platform metadata;
- human title;
- declared scenario/result/asset counts;
- minimum compatible application metadata where useful; and
- integrity algorithm plus fixed checksums-file reference.

The import decision is controlled by supported `formatVersion`, `schemaVersion`, and required semantic capabilities—not by a display application version alone.

Format and schema versions are independent. A compression/layout/checksum change does not silently change scenario semantics; a constraint/unit/enum meaning change does not masquerade as an archive-only update.

### Stable identity and canonical values

Every durable scenario, revision, person, place, vehicle, activity, rule, integration binding, result, and bundle uses a globally unique typed ID. UUIDv7 or another approved UUID scheme is preferred for portable identities. Display names and collection positions never identify records.

Portable values are locale-independent:

- UTC creation timestamps and RFC 3339/ISO 8601 temporal encoding;
- local schedule intent paired with an IANA time-zone identifier and explicit DST policy;
- checked integer canonical durations/distances/angles/quantities under the relevant domain contract;
- WGS84 coordinates only where the scenario and privacy/provider policies allow them;
- stable enum/capability identifiers, not translated labels; and
- exact decimals represented without accidental binary floating-point meaning where exactness matters.

### Scenario-export contents

A single-scenario bundle includes every portable fact required to recreate editable meaning, as applicable:

- scenario metadata, stable revision identity, domain/pack/schema and required-capability metadata;
- referenced people, roles, places, confirmed coordinates, vehicles, activities, appointments, tasks, time windows, availability, and unavailability;
- every Required rule and Preference, grouping/relationship rule, transportation/accessibility setting, objective profile, solver strategy setting that affects scenario meaning, note, and manual override;
- imported normalized facts required to preserve meaning, subject to privacy and redistribution policy;
- credential-free integration references and reconnection state;
- portable report/share presets; and
- only necessary assets that the user/project owns or may redistribute and that satisfy type/dimension/size policy.

It excludes credentials, OAuth tokens, API keys, passwords, session cookies, private signing keys, source-machine keychain references, unrelated scenarios/contacts, disposable UI state, unneeded logs, device-specific paths, opaque noncanonical caches, and provider data whose terms prohibit redistribution.

### Integrations and provider-derived data

A portable integration binding preserves provider kind, nonsecret source description, privacy-safe external identity/provenance where needed, and `requires-reconnection`; it never preserves authentication material. The imported scenario remains inspectable before reconnection. Solve availability depends on whether valid required normalized facts were embedded.

Phase-10 live or opt-in retained conversations, transcripts, prompts, provider payloads, and credential references remain outside scenario exports, full backups, and Share Result data. Minimal applied-command provenance follows the existing history contract independently of chat retention; deleting a conversation does not erase the applied domain edit. Portable nonsecret integration metadata is descriptive, not transferred authorization: import/restore never replays proposals or approvals, starts AI/provider requests or solver jobs, or activates a microphone. Reconnection and any data-sharing permission require fresh explicit review. Post-MVP Branch-K raw audio and temporary experiment artifacts are likewise excluded; retaining an experiment is not consent to export it, and only explicitly promoted scenario meaning enters ordinary scenario portability.

User-confirmed coordinates may be scenario data. Opaque geocoding, route, traffic, transit, directions, and tile data is derived. It may be persisted or bundled only when current provider terms permit that data class and the payload records provenance, provider/model version, freshness, units, and expiry semantics. Omission of replaceable cache data cannot make the portable document structurally invalid, though rerunning may require manual data or a reconnected provider. Imported stale derived data is never silently treated as current.

Checksums detect corruption and inconsistent assembly. They do not authenticate the sender, approve semantic meaning, or justify a `trusted`/`verified source` badge.

## Portable versioning and migration

### Compatibility policy

- **Import:** support every maintained historical portable format/schema through tested migration paths.
- **Export:** always emit the current format/schema in the MVP.
- **No down-conversion:** do not offer “export for an older Eutheto” until a separately approved semantic-loss and test policy exists.
- **Newer input:** reject unsupported newer format/schema/capabilities with the file and supported versions plus an actionable update message.
- **Unknown semantics:** never ignore an unknown rule, enum, availability, transportation, accessibility, location, or other solver/safety-relevant concept.
- **Extensions:** strict core schema allows only explicit namespaced extension containers. A declared nonsemantic extension may be ignored for rendering but is preserved through read/write; semantic extensions require a known handler/capability.

A schema version changes whenever structure or meaning changes materially: renames/moves, split fields, unit changes, enum meaning changes, solver-relevant removal, constraint restructuring, or new required semantic concepts. An additive default is safe only when it preserves old behavior, not merely syntactic validity.

The implemented [portable-v2 prerequisite compatibility](../architecture/compatibility-policy.md#portable-v2-prerequisite-compatibility)
keeps outer format V1 and internal/database representations unchanged while current writers emit
global portable schema V2. Its host-owned shell contains one pack-owned domain envelope; genuine
global V1 imports migrate through the registered pack before preview or staging. This prerequisite
does not implement Workforce or later result-sharing formats.

### Migration pipeline

Portable migrations are small sequential steps: `v1 → v2 → … → current`. Each step is pure, deterministic, offline, side-effect-free, bounded, typed against its historical representation, and testable independently. It has no live-database, provider, credential, clock, locale, or network dependency.

Container-format migration runs before portable-schema migration. Every import records original/current format/schema, applied steps, warnings, source bundle/application, and migration implementation versions for diagnostics.

Exact migration is preferred. A historical ambiguity may produce a typed actionable warning with stable code, affected entity, preserved interpretation, and review path. Warnings cannot conceal loss of critical semantics. Impossible exact preservation of a required meaning is a blocking import error.

Version-1 historical fixtures become permanent compatibility assets when first released. Released migrations are immutable except for a reviewed defect fix that preserves historical expectations and adds regression evidence.

## Import pipeline

Never deserialize an archive directly into live domain state. Every import follows:

1. open through the granted-file boundary and classify without trusting extension/MIME alone;
2. stream and validate the archive under centralized limits;
3. normalize/reject paths and validate fixed manifest/count declarations;
4. verify every declared checksum and reject unexpected payloads;
5. migrate the outer format to the current logical layout;
6. parse the declared typed historical portable schema;
7. run every sequential schema migration and collect warnings/provenance;
8. validate current portable schema, IDs, references, units, time zones, enums, assets, and required capabilities;
9. convert to proposed domain/application state in staging and run pack/domain validation;
10. show exact preview, reconnection requirements, migration warnings, and collisions;
11. resolve every collision explicitly; and
12. commit the complete selected import in one transaction, or commit nothing.

### Centralized untrusted-input limits

Before version 1 freezes, Phase 01 publishes platform-consistent limits for compressed bytes, total uncompressed bytes, individual entry bytes, entries, compression ratio, path length, JSON document bytes, nesting, string length, collection/object counts, asset dimensions, and overall CPU/disk/time. Checked arithmetic validates declared and accumulated counts.

Reject absolute paths, `..` traversal, drive-letter or UNC paths, NUL/invalid UTF-8 paths, duplicate normalized paths, case-colliding paths under supported filesystems, symlinks, hard links, device files, nested archives unless explicitly permitted, extreme compression, decompression bombs, undeclared/missing checksum entries, executable content, oversized assets, and metadata-selected filesystem destinations. Prefer streaming bounded validation to extraction into a user-writable directory.

### Preview and collision choices

The preview identifies bundle type/title/creation time/source version; schema/format migration; scenario/entity/result/asset counts; domains/capabilities; included/excluded data; integration reconnection; unknown/preserved extensions; warnings/blockers; and collision choices.

For each scenario identity collision, MVP choices are:

- **Create a copy:** mint new scenario/scenario-owned IDs and remap every internal reference consistently while retaining source provenance;
- **Replace existing:** explicit reviewed replacement in the final atomic transaction; or
- **Skip:** leave that scenario unchanged and exclude its selected dependent records.
Every existing local supplemental `(section, key)` collision is also listed. Add/import requires an explicit **Replace** or **Skip** choice for each; no hidden upsert is allowed. Replace-library may overwrite all supplemental identities only after the preview discloses their exact removal scope. Skipping a scenario excludes supplemental JSON that declares a dependency on it, while Create a copy rewrites only declared identity/reference positions and preserves UUID-shaped prose and external values.


No automatic semantic merge in the MVP. Similar names are not identities. Cancel, checksum failure, validation error, unresolved collision, stale preview, I/O failure, or commit failure leaves the local library unchanged. Successful imports retain source bundle ID, original scenario ID/schema/application, timestamp, migration warnings, and ID-remap state without cluttering normal editing.

## Backup and restore

### Scenario export versus full backup

A scenario export contains one editable scenario and only records required by it. A full backup defaults to the complete portable library:

- every exact scenario revision required by a selected retained result, otherwise export fails rather than orphaning evidence;
- referenced shared people, places, vehicles, activities, rules, reusable templates, and presets;
- retained immutable accepted results and required verification/provenance records;
- user-created share/report presets;
- portable application preferences; and
- bounded PNG, JPEG, or UTF-8 plain-text assets with exact media type and retained affirmative redistribution permission.

It excludes credentials/tokens, device-only paths, nonportable keychain references, ephemeral or prohibited provider caches, window positions/disposable UI state, unrelated logs, and build/runtime caches. Users may exclude retained results or large optional assets to reduce size, but the default favors a complete understandable restore and clearly lists exclusions.

Backup assembly reads one consistent portable snapshot, writes and fsyncs a private temporary destination, reopens and verifies its exact contents/checksums, then publishes with an atomic no-clobber operation. That successful publication is the commit point. Pre-publication failure removes staging output and leaves the destination unchanged; post-publication directory-sync uncertainty is not reported as an ordinary failure after the destination changed.

### Restore safety

Restore is more consequential than scenario import. The UI clearly distinguishes:

- **Add backup data to current library** — uses the import/collision pipeline; and
- **Replace current portable library** — summarizes what will be replaced, requires explicit confirmation, attempts a timestamped pre-restore `.eutheto` safety backup, and performs one atomic transaction/swap.

If the safety backup cannot be created, explain why and require stronger explicit confirmation; never pretend it exists. A fresh installation should have a near-one-click safe restore after preview. Failed/cancelled restore preserves the pre-operation library and remains recoverable after restart.

Automatic rotating backups, retention tiers, restore-point browsing, encryption, and scheduled restore drills remain post-MVP. A normal user-selected folder may later interoperate with existing sync/backup tools without turning Eutheto into a hosted storage provider.

## Immutable generated-result sharing

### Result capsule terminology

The roadmap may call the recipient-facing immutable offline artifact a **result capsule**. This is a product concept, not another persistence or interchange authority: the MVP capsule is the existing standalone HTML or direct PDF rendered from a validated Share Result Model. It is never an editable `.eutheto` scenario, full backup, database image, solver workspace, or container for unrestricted pack code. User-facing naming remains subject to product review; the artifact contracts below do not depend on the label.

### Share Result Model

Only an independently accepted Result Model may produce assignment/plan sharing output. The Share Result Builder takes:

- immutable result/scenario revision/plan-version identity;
- verification checksum and accurate proof/limit/status;
- selected share profile and field/section options;
- domain-owned recipient views and safe explanations;
- provider/license/redaction policy; and
- explicit locale/display preferences that do not change semantic meaning.

It emits a strict versioned Share Result Model with report/result-schema version, result/scenario/revision IDs, title, generation time/application/solver versions, share profile, selected sections, people/schedule/transport/view data, explanations, provenance, and explicit privacy flags such as address/constraint/calendar-title inclusion.

The builder does not serialize the complete scenario, rejected candidates, arbitrary diagnostics, notes, source calendar titles, addresses, hidden constraints, or solver details unless the selected profile/advanced option explicitly includes an allowed field. Potentially sensitive fields default off.

### Standalone HTML is the default rich format

The report is one self-contained `.html` file that opens directly from `file://` in supported modern desktop browsers, requires no Eutheto installation/server/account/browser storage, and remains usable offline. It embeds generated HTML, compiled CSS/JavaScript, small approved assets/icons, safely encoded Share Result data, and print CSS.

Core meaning never depends on a CDN, remote script/package/font/icon, analytics/tracker, authentication, Eutheto endpoint, map tile, or other network request. Explicit optional external map links may navigate only after a clear user action; the result remains complete without them.

Offline presentation interactions may switch among available timeline/person/location/transport/table views, filter/highlight/search, expand details/explanations, adjust density/zoom, and print/save. They mutate view state only. The report cannot edit scenario meaning, rerun a solver, recompute a plan, apply commands, imply a filter changes the official result, load scenario-authored code, or silently contact a server.

### Shared presentation without a second product

Phase 07 establishes a shared result view-model/component boundary that has no Tauri, database, filesystem, server, solver, credential, or mutable scenario dependency. Desktop and report renderers reuse the accessible presentation and domain-owned view payloads where sound; desktop-only actions are injected behind explicit adapters. Byte-identical UIs are not required, but two unrelated result systems that drift in meaning are prohibited.

Workforce supplies schedule/person/coverage/fairness/transport views in Phase 07. Seating supplies table/guest/list, deterministic schematic/SVG, and accepted geometry views in Phase 09. Transportation supplies timeline/person/vehicle/coordination/leg views in Phase 14. Every canvas/chart/map has equivalent list/table content in the report.

### Safe generation and provenance

User-authored values remain inert data. Encode them in a nonexecuting payload; escape container terminators; validate after decode; render strings through text nodes; avoid `innerHTML`, `eval`, dynamic function construction, generated JavaScript concatenation, and unreviewed rich text; apply a strict rich-text allowlist only if a current requirement exists; and emit a restrictive self-contained-report CSP.

Generation validates the Share Result Model before rendering, writes to a temporary file, verifies the result can be reopened/parsed under the report contract, then atomically publishes where supported. Failure/cancellation removes staging output.

An `About this plan` area records scenario title/revision, immutable result ID, friendly plan version, generation time, Eutheto/report/result schema versions, verification/status basis, and applicable snapshot assumptions. Editing the source scenario or accepting a newer plan never mutates an existing report.

Transportation meaning remains usable without map tiles: named stops, leave/arrive times, duration/buffer, mode/driver/passengers/transfers, historical/manual basis, and warnings. Optional schematic legs or legally permitted static imagery may supplement but never replace text. Provider-restricted geometry/details are omitted with a disclosed reason.

### Privacy profiles and preview

MVP provides one strong participant-friendly default plus advanced per-field/section options. The architecture reserves named `participant-friendly`, `coordinator`, `full-analysis`, and `custom` profiles without requiring every profile in the first release.

- Participant-friendly emphasizes each person's actionable schedule, departures, destinations, transportation, and essential recipient notes while excluding private source detail and solver internals.
- Coordinator may include cross-person dependencies, vehicles/transfers, and selected explanations.
- Full analysis may include broader reasoning only after explicit privacy review.
- Custom selects allowed sections and sensitive fields.

The export dialog clearly separates **Share Result** from **Export editable scenario** and previews the exact Share Result payload that will be rendered. It discloses whether full addresses, source calendar titles, notes, constraint details, rejected alternatives, solver/objective details, transportation data, and external links are included. Sensitive options default off unless the selected profile unambiguously requires them. A privacy summary is embedded in the report.

Privacy filtering applies to derived disclosure as well as source fields: rule evidence, comparisons, counterfactuals, and small-group fairness/coverage metrics can reveal private availability, preferences, or relationships even when names and free text are removed. Each included derived field must satisfy the selected recipient profile and exact-preview contract; aliases alone are not proof of privacy. Future collaboration adds recipient authorization and publication controls under [privacy, retention, and recovery](collaborative-planning-and-hosted-service.md#privacy-retention-and-recovery), not a competing Share Result format or an AI-selected disclosure policy.

### Recipient UX and safe default names

Result actions use **Share Result…**, **Export PDF…**, and **Export editable scenario…**, with descriptions that state who can open the artifact, whether it is interactive/editable, its offline behavior, and that editable data may contain more private source detail. A recipient opening HTML sees a clear title, friendly plan version/status, summary, primary domain view, keyboard-usable navigation, privacy/provenance area, and ordinary print/download guidance—never editor or solver controls.

Suggested filenames are `<scenario-title>-Plan-v<plan-number>.html`, the equivalent `.pdf`, `<scenario-title>.eutheto`, and `Eutheto-Backup-<YYYY-MM-DD>.eutheto`. File generation sanitizes reserved/control/path characters, produces safe cross-platform names, and resolves collisions without unexpected overwrite. MVP branding stays minimal and project-owned; custom organization branding is post-MVP.

### Print and direct PDF

The standalone renderer includes reviewed `@media print` behavior: remove controls without paper meaning; preserve dates/plan version; expand or summarize appropriate details; avoid clipped timelines; control page breaks/repeated headers; support grayscale; retain accessible readable type; and add concise provenance.

PDF is a secondary MVP output from the same validated Share Result Model and reviewed report/print renderer, using a controlled local headless/webview print pipeline or equivalent deterministic project-owned renderer. It cannot use a separate privacy model or silently contact remote resources. Phase 11 closes target packaging, reliability, and dependency/license evidence; failure leaves no partial PDF.

## APIs and product state

Application APIs remain typed, cancellable, revision-bound, and job-based for potentially large operations. Required conceptual operations are:

- portable export preview/create;
- full backup preview/create;
- bundle inspect/import preview/import apply;
- backup restore preview/apply with add/replace mode;
- share-result preview/create HTML/create PDF; and
- progress/cancel/result/error records carrying operation ID, file grant, source revision/result ID, format/schema/report versions, included/excluded data, warnings, and safe failure code.

A preview is bound to the source revision/result hash, options, file identity/hash, format/schema versions, and current local library revision. Any change invalidates it. Output paths are granted by the user or application safe-save boundary; imported metadata never chooses local paths. Vue receives summaries and progress, never archive bytes, unrestricted paths, credentials, or authoritative mutable portable state.

Normal states include no scenarios/results, inspecting, migration required, review warning, unsupported newer, damaged checksum, unsafe archive, collision review, integration reconnection, backing up, restoring, report generation, cancellation, stale preview, I/O/permission failure, and successful next actions. No indefinite spinner lacks text, cancellation, or timeout/resource behavior.

## Security, privacy, and resource invariants

Assume every bundle is corrupted or malicious and every scenario string embedded into a report is attacker-controlled. Required defenses include:

- streaming/bounded archive and structured-data parsing;
- ZIP Slip, absolute/drive/UNC path, normalization/case collision, link/device, nested archive, compression-bomb, overflow, entry/count/size/depth, malformed UTF-8, duplicate ID/reference, and content-type defenses;
- strict schema and semantic validation with explicit capability handling;
- no imported code/expression/template/regex/path execution;
- atomic staging/commit and cleanup under failure/cancellation;
- source-revision/preview/hash checks against time-of-check/time-of-use races;
- structural credential/provider-restricted-data exclusion before serialization;
- safe report data encoding/rendering, restrictive CSP, no default network/analytics, and clear external links;
- safe filenames independent of user path separators/control characters;
- output size/model estimates and explicit confirmation or refusal for oversized valid artifacts; and
- local redacted diagnostics that identify the failing stage without echoing private payloads.

Portable bundles and reports may contain sensitive personal planning data. UI warnings, share preview, provider redistribution enforcement, and no-secret tests are release gates. Encryption is not claimed in the MVP; documentation tells users that an unencrypted portable/report file must be protected like its disclosed contents.

## Permanent compatibility and behavior evidence

### Portable fixtures

For each released portable format/schema, retain permanent minimal, representative domain, complex-rule, asset, provider-derived/restricted-omission, scenario-export, and full-backup fixtures. Tests cover:

- direct current import;
- every sequential migration and warning;
- current export/import semantic equivalence;
- old import/migrate/current export/current re-import semantic equivalence;
- deterministic migrations under reorder/locale/clock/network isolation;
- unknown newer/semantic failure and nonsemantic extension preservation;
- create-copy reference remapping, replace, skip, cancellation, stale preview, transaction failure, restart recovery, and fresh-install restore;
- consistent full snapshot, checksum verification, temporary-write/atomic publish, add versus replace, pre-restore safety backup, and failure rollback; and
- absence of credentials, secrets, prohibited provider data, SQLite, device paths, unrelated data, and undeclared files.

Archive/schema parser fuzz/property campaigns include path traversal, absolute/case-colliding/duplicate paths, bombs/extreme ratios, huge counts, malformed manifest/checksum/UTF-8/JSON, deep nesting, integer overflow, duplicate IDs, dangling references, invalid time zones/units/enums, future versions/capabilities, malicious strings/assets, and minimized regressions.

### Report fixtures

For every supported domain/share profile/browser target, prove:

- one file opens from `file://` without server/application/browser storage;
- zero required network requests and no CDN/font/script/tile/analytics dependency;
- selected Share Result fields exactly match preview and excluded sensitive/source fields are absent;
- malicious labels/notes cannot inject script, markup, navigation, or CSP bypass;
- principal views, filters, search, expansion, keyboard/focus/screen-reader behavior work;
- canvas/chart/map meaning has an equivalent accessible list/table;
- report provenance/result identity remains immutable after source edits;
- print/PDF output has readable page breaks, grayscale, repeated context, and no leaked controls/data;
- supported modern browsers render core meaning consistently; and
- large valid reports remain bounded and responsive.

Visual regression protects shared view components and print layout, but semantic payload/accessibility/network/security assertions remain authoritative.

## Phase ownership

| Phase | Required delivery |
|---:|---|
| 00 | Generation/drift commands, fixture directories, parser/fuzz dependencies only when implementation work reaches the owning phase; no placeholder format claims. |
| 01 | Portable model envelope, version-1 bundle/manifest/checksum/limits, current-only export, inspect/migrate/validate/preview/collision/atomic import, full backup/restore, recovery, CLI/application/Tauri contracts, and permanent version-1 fixtures. |
| 02 | Pack portable capability IDs, domain conversion/validation/migration hooks, explicit semantic extension handling, and domain-owned Share Result contribution contract. |
| 05 | Workforce portable model/import round trips and view payload groundwork; no standalone report claim yet. |
| 06 | Intent-led import/backup/share UI primitives, preview/accessibility/error states, safe file grants, and nonblocking operation shell. |
| 07 | Generic Share Result builder/renderer/component boundary; workforce standalone HTML and direct PDF; privacy preview/options; CSV/ICS/JSON coexist under distinct gates. |
| 09 | Seating Share Result payload/views, deterministic schematic/SVG integration, standalone HTML/PDF through the shared renderer, and privacy/accessibility evidence. |
| 10 | No AI authority to export/restore; conversation/provider data stays excluded, minimal command provenance remains independent of chat retention, and imported integration metadata grants no authority. |
| 11 | Final `.eutheto` identity/media-type/file-association ADR; exact target/browser/PDF packaging; public format/report documentation; compatibility/support matrix; release security review. |
| 12 | Historical migration/security/fuzz/restore/report/browser/print/PDF/large-fixture release gates on exact candidates. |
| 13 | Explicit post-MVP backup/encryption/signature/hosted-sharing branches; Branch-K experiment retention and promotion preserve exclusions, disclosure, and inert restore without adding an implicit export path. |
| 14 | Transportation portable semantics/provider restrictions and Share Result itinerary/coordination payload with complete offline non-map meaning. |

## MVP acceptance

The public MVP cannot claim this contract complete until:

1. one scenario exports to one proposed/finalized `.eutheto` file containing all portable semantic inputs and no secret/database/device-only/prohibited data;
2. a fresh offline installation inspects, previews, imports, and solves/edits the semantically equivalent scenario subject only to disclosed reconnection/freshness requirements;
3. format/schema versions, migration registry, current-only export, permanent version-1 fixtures, unknown-newer rejection, and unknown-semantic safety exist from the first release;
4. malicious/corrupt archives fail under bounded parsing before any live mutation, with actionable errors and complete rollback;
5. create-copy, replace, and skip choices use stable identity and consistent reference remapping; no automatic semantic merge exists;
6. one full backup reconstructs all selected portable data on a fresh install; add/replace restore, explicit destructive confirmation, attempted pre-restore safety backup, atomicity, and restart recovery pass;
7. accepted immutable results alone produce plan-sharing output, and source scenario edits cannot change an existing report;
8. the exact privacy-filtered Share Result preview drives both one-file HTML and direct PDF, with sensitive fields off by default and scenario data not embedded wholesale;
9. HTML opens directly from `file://`, requires no server/Eutheto/network/storage, supports principal accessible result interactions, contains safe inert data under restrictive CSP, and makes no hidden request;
10. report core meaning, including transportation, does not depend on map tiles; print/PDF and accessible list/table alternatives remain complete;
11. public docs distinguish editable export, backup/restore, result sharing, integrity, authenticity, privacy, encryption absence, provider restrictions, versions, and compatibility; and
12. Phase 12 passes the permanent compatibility, security/fuzz, browser, accessibility, print/PDF, and large-artifact evidence on the exact release candidate.

## Post-MVP opportunities

Separate approved branches may add password-encrypted bundles with an independently versioned authenticated-encryption envelope; automatic change/daily backups, retention and health monitoring; selected multi-scenario export; scheduled verification/restore drills; richer saved/per-recipient profiles; licensed offline schematic/static maps; result comparison across revisions; local annotations separated from immutable result; branding/templates; digital signatures and trust UI; optional hosted sharing that retains downloadable offline reports; QR identifiers; support import-diagnostic packages; or organization sharing policies. Advanced result-capsule work may add precomputed verified alternatives, comparison views, recipient-specific disclosure, or an explicit create-editable-copy flow. Such work versions and previews every newly included field, preserves immutable source/result identity, keeps annotations outside the accepted result, and never grants the offline artifact ambient network/filesystem/credential access. Capsule-to-scenario conversion creates a newly reviewed portable scenario or links to separately included source data; it never reconstructs editable authority from a privacy-filtered presentation payload or silently imports omitted/private fields.

Any hosted/cloud function remains additive. It cannot replace local export, inspection, backup, restore, and offline sharing or weaken data minimization, source revision identity, independent verification, provider terms, and explicit user control.

### Future organization migration and deployment recovery

[Branch-H privacy, retention, and recovery](collaborative-planning-and-hosted-service.md#privacy-retention-and-recovery) must distinguish two future profiles from the MVP scenario export and portable-library backup:

- **Organization migration** transfers a reviewed organization data scope to another authorized installation or tenant. It reuses existing portable scenario/result/share representations and separately versions necessary collaboration records. Source identities are provenance, not destination login or membership authority: preview and explicitly approve participant/account links, tenant ownership, roles, and privilege remapping under destination policy. Names, email matches, or source administrator status never grant destination access.
- **Deployment disaster recovery** restores an operator-controlled service from a consistent protected recovery snapshot under an approved runbook. It is not a user-portable bundle or a new `.eutheto` bundle kind approved here. Review recovered identities and privilege bindings against current trusted identity and revocation state; explicitly remap or reauthorize them before service activation. An old backup cannot reinstate revoked membership, publication access, or operator privilege merely because it was once valid.

Organization migration imports no live sessions, usable invitations, credentials, tokens, signing keys or secret-derived authority. Deployment recovery must not reactivate such authority merely by restoring old bytes: recovery of credential-facility state, controlled provisioning/rotation and revocation reconciliation require the separate deployment security design, never ordinary portable import. Restored collaboration/publication records begin in recovery quarantine: no automatic proposal/approval replay, solve/provider work, publication activation, webhook/notification delivery or other sends. Reconcile publication pointers, audit/outbox state, permissions, retention/deletion decisions and already-delivered effects before explicitly authorized resumption; restoring an outbox row is not permission to send it again.

Future retention policy distinguishes raw participant submissions and sensitive reasons; accepted decision/command lineage needed to explain scenario revisions; immutable publication snapshots and their access/audit metadata; and transport coordinates, itineraries, snapshots, and provider-limited data. These classes need separately justified retention/deletion rules rather than one blanket chat or backup lifetime. Deleting raw input need not erase the minimal lawful decision lineage, while retaining lineage does not authorize keeping its private source payload. Recovery must reconcile deletion/expiry records before making data available and cannot silently resurrect expired or deleted submissions or publications. These are post-MVP design and operating gates, not new current schema fields or claims that organization migration or service recovery is implemented.
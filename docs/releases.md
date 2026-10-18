<!-- SPDX-License-Identifier: Apache-2.0 -->

# Release Policy and Evidence

## Current status

The repository is in [Phase 00](roadmap/00-repository-and-reproducible-tooling.md). This document establishes policy only: it does not enable a release workflow, create a stable or beta channel, identify a signing principal, or claim that any releasable artifact exists. [Phase 11](roadmap/11-public-mvp-packaging-and-documentation.md) owns candidate construction and protected release automation; [Phase 12](roadmap/12-stabilization-and-public-release-gate.md) owns approval and publication against the exact candidate digests.

Release evidence follows [ADR-016](adr/016-release-evidence.md). Platform reproducibility boundaries follow [ADR-015](adr/015-reproducible-platform-tooling.md). Product, application, hosting, contact, and signing identities remain open in [identity gates](architecture/identity-gates.md).

## Pre-release contract changes

- Phase 02 cleanly replaces the ambiguous `SolveOptions.timeLimit` whole-minute field with `timeLimitMilliseconds` and unifies cancellable work on the shared `CancellationToken`; no compatibility alias or reinterpretation is retained.

## Phase-00 preflight boundary

`cargo xtask release verify-clean` is the implemented Phase-00 release preflight. It fails when the tracked Git tree has staged or unstaged changes, when the normal generated-source inventory drifts, when worker-protocol verification fails, or when the generated license notice/inventory or SPDX smoke SBOM differs from the committed Cargo/pnpm locks and reviewed `xtask/supply-chain-inputs.json`. The preflight only verifies repository evidence; it does not construct or authorize a release.

`cargo xtask licenses generate` owns `THIRD_PARTY_NOTICES.md` and `xtask/generated/license-inventory.json`. `cargo xtask sbom generate` owns the deterministic SPDX-2.3 JSON smoke document at `xtask/generated/sbom.spdx.json`. These products inventory locked Cargo and pnpm workspace/dependency packages plus explicitly reviewed runtime assets and retained custom-license text; they do not claim to describe the complete contents of a packaged target. `NOASSERTION` records an absent reviewed dependency-license conclusion and remains a release blocker, not a guessed license.

`cargo xtask release assemble-manifest` intentionally fails until Phase 11 supplies finalized product identity, actual target artifacts, their exact digests, and protected build/sign evidence. Solver commands likewise remain gated by the Phase-03 source/hash/protobuf/license decisions. Neither gate may be replaced with a successful empty manifest or placeholder identity.


## Channels

A channel label is a compatibility and trust commitment, not a substitute for evidence.

### Beta

Beta is an explicit opt-in channel for immutable Phase-11 candidates. A beta must not be represented as stable and must not silently receive stable users or data. Before a beta is offered, its exact artifact digests must have:

- an immutable source commit and tag, locked build inputs, target and solver manifests, checksums, signatures or attestations selected for that target, SPDX SBOMs, third-party notices, provenance, release notes, known limitations, and a supported-version matrix;
- install, launch, bundled-worker, solve, export, offline, migration, backup/recovery, and uninstall evidence for each target it claims to support;
- schema and migration evidence for every supported released input, including unknown-newer refusal and preservation of extension data where the format contract requires it;
- a distinct, finalized beta application identity and updater channel configuration if an updater is enabled; and
- signed updater metadata, key-custody and rotation evidence, and clean-machine update tests if an updater is enabled.

Beta status permits disclosed product limitations; it does not waive data-integrity, parser-bound, migration, credential, licensing, artifact-integrity, or channel-isolation requirements. Development or nightly output is not automatically trusted as beta evidence.

### Stable

Stable is authorized only by the complete [Phase-12 public-MVP release gate](roadmap/12-stabilization-and-public-release-gate.md#public-mvp-release-gate). In addition to the beta evidence above, the identical candidate digests must have complete correctness, independent-verification, migration/recovery, usability, accessibility, security/privacy, performance, packaging, licensing, documentation, manual platform, and operational evidence. Every release-blocking issue must be closed, experimental paths must be isolated or accurately labelled, and authorized maintainers must approve publication.

Stable users never move to beta without an explicit channel choice. Changing source, locks, flags, capabilities, workers, dependencies, assets, migrations, documentation, or assembled contents creates a new candidate digest and invalidates the affected evidence. A beta is therefore not promoted merely by changing a label or copying channel metadata.

## Evidence bound to each artifact set

Every staged or published candidate set must bind the following to its exact source and artifact digests:

1. desktop and CLI artifacts for every declared target, including the target-matched bundled worker and manifest;
2. a cryptographic checksum manifest and detached signatures and/or verifiable attestations;
3. an SPDX JSON SBOM for each exact artifact or assembled platform bundle;
4. generated third-party notices and the complete shipped license corpus;
5. the source archive, immutable source tag and commit, `Cargo.lock`, `pnpm-lock.yaml`, `flake.lock`, toolchain pins, Nix inputs, build flags, and supported target matrix;
6. application, core API, scenario envelope, domain schema, planning IR, worker protocol, solver, adapter, and applicable policy/catalog versions;
7. migration notes, release notes, known limitations, compatibility matrix, and benchmark evidence when a performance-sensitive contract changed;
8. provenance recording workflow identity and exact inputs and outputs; and
9. signed updater metadata for each channel actually enabled.

Evidence must distinguish reproducible unsigned core, CLI, or worker output from platform bundles changed by timestamps, code signing, notarization, or stapling. A signature proves artifact identity; it does not prove semantic correctness.

## Future protected build/sign boundary

Phase 00 records this separation but does not create credentials or enable publication:

1. unprivileged, target-specific jobs build unsigned artifacts from an immutable source tag and locked inputs;
2. those jobs generate manifests and cryptographic digests, including the bundled-worker digest and exact dependency graph;
3. digest-addressed artifacts cross into protected signing environments; pull requests, forks, ordinary build jobs, Nix derivations, caches, logs, and contributor shells receive no signing material;
4. the protected signer verifies the expected digest before signing, timestamping, notarizing, or stapling as applicable;
5. signatures and platform trust results are verified on the exact resulting artifacts;
6. final notices, SBOMs, provenance, and release manifests describe the exact assembled bundles rather than an approximate pre-signing build graph;
7. automation creates only a draft release and draft channel metadata until Phase 12 authorizes the exact digests; and
8. protected automation publishes, followed by verification of downloaded artifacts and their checksums, signatures, install behavior, update behavior, and offline behavior.

CI actions must be pinned by full commit SHA, tokens default to read-only, and release permissions exist only in protected environments. The signer must never trust a filename, mutable tag, or build-job assertion in place of digest verification.

## Publication rule

A planned date, version, tag, green build, or partial evidence packet cannot authorize publication. If a target cannot meet its gate, that target is removed from the supported matrix and all manifests and documentation are updated, or the release is delayed. Phase 12 approval and post-publication smoke evidence are required before the project may make a stable release claim.

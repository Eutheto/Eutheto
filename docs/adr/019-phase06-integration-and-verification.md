<!-- SPDX-License-Identifier: Apache-2.0 -->

# ADR-019: Temporary Phase06 integration and verification cadence

- **Status:** Proposed formal decision record; not an approved ADR.
- **Date:** 2026-09-12.
- **Operating authority:** Explicit user instruction approving this bounded Phase06 trial. This record does not appoint governance roles or close the unresolved formal approval gate.
- **Scope and expiry:** Remainder of Phase06 only, until authorized integration into `main` or abandonment, whichever occurs first.

## Context

Repeated complete native and packaged-worker pipelines for every small frontend work package delay coherent Phase06 integration. Those checks remain necessary for native/build changes, integrated milestones, final-main admission, and exact artifact acceptance; they are not equivalent to source, component, or browser checks.

The current user explicitly approved a temporary protected phase integration branch, coherent package PRs, per-package source/security checks, risk/milestone native checks, and unchanged final-main/manual gates. The [contributor Git workflow](../contributors/git-workflow.md#temporary-phase06-integration) is the operational procedure. This proposed ADR records that instruction without presenting itself as new formal governance authority.

## Trial decision

Use `phase/06-desktop` as a temporary integration branch. Short-lived package branches consume exact verified integrated phase commits. `main` remains the sole permanent trunk, with its existing protection and merge queue untouched.

The phase ruleset copies main's complete pull-request review parameters and no-bypass/deletion/non-fast-forward/linear-history protection. It requires strict up-to-date checks, squash-only PRs, and app-bound source/security/dependency/native-policy/package-policy checks, but no second merge-queue pipeline. Every integrated squash receives explicit DCO-safe metadata and immediate trusted-checker validation before dependent consumption.

Every package runs locked Nix evaluation, locked installation, DCO, canonical source checks, production frontend compilation, browser accessibility checks, and security/dependency policy. Only the workflow's conservative frontend/documentation allowance may defer native/package matrices; unknown, mixed, native/API/build/dependency/CI changes select every approved target, including Intel. Missing or unknown selection output and failed/cancelled/unexpectedly skipped work fail closed. Deferred matrices are explicitly **not run**, never accepted platform evidence.

Full exact-integrated-SHA checkpoints follow ordered Phase06 packages 3, 6, 9, and 10. Admission remains frozen until their recorded head SHAs and all required results are verified. Ordinary non-phase PR selection and full non-PR behavior remain unchanged. Optional non-distributable candidate probes keep their manual default and may be explicitly excluded from approved-artifact checkpoints.

## Consequences and alternatives

- Frontend package feedback avoids unnecessary native/package rebuilds while still compiling the production entrypoint. Cross-platform runtime regressions may be detected at the next named checkpoint rather than every frontend PR; local changed-surface acceptance and final gates remain mandatory.
- Keeping full matrices on every package was rejected for this bounded trial because it repeats expensive platform evidence without matching the change risk.
- A permanent second trunk, unprotected phase branch, bypass actors, phase force-pushes, or silently skipped gates were rejected because they obscure integration authority or evidence.
- Reusing existing workflows, selectors, target dictionaries, and canonical commands avoids a new CI orchestration service, dependency, or privileged dispatcher.

## Security, integrity, compatibility, and gates

No workflow gains write permissions, credentials, privileged untrusted execution, release authority, or new dependency inputs. Main's captured ruleset must compare unchanged after phase configuration. The trial changes verification cadence, not product trust boundaries, persistent schemas, protocol compatibility, package order, issue IDs, or acceptance criteria.

The explicit Phase06 user manual checkpoint, unfamiliar-user/accessibility evidence, full protected-main verification, and exact artifact/release gates remain required. This trial does not authorize final-main integration, signing, publication, Phase07 production, or a Phase07 integration branch. Its formal ADR/governance approval remains unresolved rather than simulated.

## Supersession and retirement

This trial is the sole time-bounded exception to per-package protected-main integration for remaining Phase06 work. It does not supersede an approved security, integrity, tooling, or release ADR. Retire the exact phase ref and ruleset deliberately after authorized main integration or abandonment; any extension or subsequent-phase policy requires a new explicit decision.

<!-- SPDX-License-Identifier: Apache-2.0 -->

# Git workflow

Use a short-lived branch and focused commits for one roadmap work package or independently reviewable change. Keep authoritative inputs, affected callers, tests, generated products, and contributor documentation in the same change. Do not combine an unrelated cleanup or dependency migration with the requested behavior.

## Temporary Phase06 integration

The user's explicit 2026-09-12 instruction authorizes a bounded operational trial on `phase/06-desktop` for the remainder of Phase06. `main` remains the only permanent integration trunk. [Proposed ADR-019](../adr/019-phase06-integration-and-verification.md) records the trial; it is not an approved ADR and does not close the governance-roster gate. This exception ends when Phase06 integrates into `main` or is abandoned. It does not authorize a Phase07 integration branch, final-main integration, signing, or publication.

Create short-lived package branches from an exact **verified, integrated phase commit**, not another worker's unintegrated branch. Keep one integration owner, coherent reviewable work packages, and all affected authoritative inputs, callers, generated artifacts, tests, and documentation together. Do not create a PR for every checkbox. Record the consumed phase SHA in each package handoff.

### Package admission and evidence

The exact phase ref has an active ruleset with no bypass actors, deletion/non-fast-forward protection, linear history, squash-only PRs, and strict up-to-date required checks. Its complete pull-request review parameter block is copied from protected `main`, including extra approval for unattributed changes. The phase branch intentionally has no merge queue; `main` retains its existing queue and every protection unchanged.

Every phase PR requires these GitHub Actions app-bound contexts:

- `Phase06 package / source checks`: locked Nix evaluation, locked dependency installation, DCO, `just check`, production `just frontend-build`, and browser keyboard/accessibility checks;
- `Locked policy, scans, and inventories`;
- `Immutable inputs and migration isolation`;
- `Phase06 package / native policy`;
- `Phase06 package / packaged-worker policy`.

The source job's `nix flake check --no-build --no-update-lock-file` is **evaluation-only**, not Nix build or native-platform evidence. Its report distinguishes this from the production frontend build. Local focused runtime/manual acceptance still applies to the changed surface.

Native and approved worker/package matrices may be deferred only when every changed path is frontend source outside `apps/desktop/src/api/`, frontend public/index assets, Markdown under `docs/`, root Markdown, or the two GitHub contributor/readme documents. API/native/domain/schema/worker/build/dependency/CI inputs, unknown paths, mixed changes, and empty diffs select the complete matrices, including Intel. Renames count both their old and new paths. `just ci-policy-test` exercises the actual selectors and gates against real Git changes and job outcomes.

The two policy gates require successful selection and every selected job's success. Explicitly deferred jobs must be skipped, and the summary says **not run**; a green policy gate is not evidence those platforms or packages were tested. Missing or unknown selection output and failed, cancelled, or unexpectedly skipped work fail closed. Ordinary `main` PR selection and all full non-PR event behavior remain unchanged. No phase push trigger duplicates the package pipelines.

### DCO-safe squash integration

Freeze package admission while integrating. Verify the reviewed head, current base, required checks, and applicable review first. Use `gh pr merge --squash --match-head-commit <reviewed-sha>` with an explicit subject, body file, and `--author-email` for the verified configured integrator identity; never use `--admin` or bypass protection.

Retain existing contributor sign-offs and place the integrator's exact `Signed-off-by: Name <email>` in the final trailer paragraph. Validate the proposed message with the trusted base DCO checker against the expected squash author. Compare the authenticated GitHub profile name, configured identity, and established successful squash metadata; stop on ambiguity. Do not invent another contributor's certification.

Immediately fetch the resulting phase SHA and use the checker and fixtures from the recorded initial phase base to validate **every commit in initial-phase-base..integrated-sha**, including the new server-created squash commit. Record the result before unfreezing admission or permitting dependent work. PR checks validate source commits, and manual dispatch runs DCO samples; neither substitutes for this integrated-range check. On failure, stop and escalate—no history rewrite, DCO exemption, or automatic repair.

### Full phase checkpoints

Run full checkpoints after these [ordered Phase06 packages](../roadmap/06-desktop-design-system-and-workforce-setup.md#ordered-work-packages):

| Boundary | Required integrated behavior |
|---|---|
| Package 3 | Shell/navigation and portability |
| Package 6 | Work/shift editor |
| Package 9 | Solve/status handoff |
| Package 10 | Accessibility/performance hardening, before manual handoff |

Freeze admission, record the integrated phase SHA, and dispatch `pr.yml`, `portable.yml`, `ortools-worker.yml`, `security.yml`, and `dependency-update.yml` on `phase/06-desktop`. For Portable, explicitly pass `-f include_candidate_builds=false` to `gh workflow run`; its default remains true for ordinary manual candidate investigations. This excludes non-distributable alternative probes, not any approved native or packaged-worker target. Confirm every run's exact head SHA and every required target before accepting the checkpoint or resuming dependent work.

A risky package's full pre-merge run is package-head evidence, not exact integrated-SHA checkpoint evidence. It neither adds a checkpoint outside the four boundaries nor replaces one at a boundary. The policy/bootstrap transition itself requires no additional integrated-phase dispatch. Record actual run scope, results, and timings; never imply deferred lanes passed.

At phase closure, preserve full protected-main/merge-queue verification, exact artifact gates, the unfamiliar-user/accessibility evidence, and the explicit [Phase06 manual checkpoint](../roadmap/06-desktop-design-system-and-workforce-setup.md#phase-exit-gate). Obtain the existing final-main authority rather than treating this trial as blanket merge permission. After authorized integration or abandonment, retire the phase branch and its exact-ref ruleset deliberately; never force-push it or carry it into the next phase.

## Before committing

1. Confirm that the change belongs to the active phase and preserves its prerequisites, non-goals, named gates, and acceptance criteria.
2. If a generated product changes, edit its authoritative input and run `just generate`; never hand-edit the product.
3. Run the narrowest canonical recipes that exercise the changed behavior. Run `just check` before proposing the branch unless a documented platform gate makes part of it inapplicable.
4. Inspect the patch for credentials, local databases, captured user data, caches, build output, and unsanitized diagnostics. None belong in Git.
5. Record new third-party code, assets, datasets, fonts, examples, and generated material in the applicable license inputs, then run `just licenses` and `just sbom`.

The root [`Justfile`](../../Justfile) and [command reference](commands.md) are authoritative. Do not replace a missing or gated recipe with an ad hoc command that appears successful.

## DCO sign-off

Every commit requires the contributor's own Developer Certificate of Origin sign-off:

```console
git commit -s
```

This adds a `Signed-off-by` trailer using the configured Git name and email. Read [`DCO.md`](../../DCO.md) before signing. Do not sign for another author. Correct every unsigned commit in the branch by amending or rebasing it before review; a sign-off added only to the final commit does not certify earlier commits.

## Optional local hooks

The repository includes small opt-in hooks in `.githooks/`. Enable them only for this checkout:

```console
git config --local core.hooksPath .githooks
```

Before changing the setting, inspect the effective hook path so that the checkout-local override does not unintentionally replace personal hooks:

```console
git config --get core.hooksPath
```

The hooks contain no independent policy or generation logic:

- `pre-commit` delegates to `just generate-check fmt-check`;
- `pre-push` delegates to `just check`.

They require `just` and the applicable pinned tools to be available, normally by working inside `nix develop` or the documented native environment. A hook failure blocks the local Git operation so the cause can be fixed; bypassing a hook does not bypass repository policy or required CI.

Disable the repository hooks with:

```console
git config --local --unset core.hooksPath
```

These hooks are a convenience, not an enforcement boundary. CI is authoritative and reruns canonical commands in pinned environments. Local hook success cannot waive CI, code-owner review, DCO, security, license, architecture, generated-drift, platform, or active phase gates.

## Dependency updates

Keep lockfiles committed and frozen outside an intentional dependency update. Dependency-update branches must preserve the exact Rust toolchain, Node/pnpm selection, flake lock, Cargo lock, pnpm lock, CI action SHAs, and generated evidence required by the roadmap.

Isolate a major Rust, Node, Tauri, OR-Tools, or schema migration as one independently reviewable major family. Do not combine major migrations into a rollup. New or changed JavaScript install scripts require explicit review and an allowlist rationale. Changes to cryptography, parsers or untrusted-input handling, credentials, updater/signing, worker protocol, or Tauri permissions require the designated ownership review.

## Push and pull request

Before pushing, make the branch reviewable:

- each commit is focused and DCO-signed;
- generated files have no unexplained drift;
- lockfile changes are intentional;
- tests and documentation describe only implemented behavior;
- failures caused by an unresolved platform or phase gate are reported rather than hidden;
- the pull request lists the exact commands run, the platform, results, and what those results prove.

Do not include vulnerability details in a public issue, commit, or pull request. Follow [`SECURITY.md`](../../SECURITY.md); if the required private reporting channel is still an unresolved gate, do not invent or infer a contact.

Review approval and passing CI do not override the active roadmap phase, an approved ADR, a compatibility contract, or a release/signing gate. Merge authority follows [`GOVERNANCE.md`](../../GOVERNANCE.md).

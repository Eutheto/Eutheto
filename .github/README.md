<!-- SPDX-License-Identifier: Apache-2.0 -->

# GitHub repository policy

This directory owns contributor intake, review policy, dependency automation, and CI/supply-chain policy for `eutheto`. It is Phase 00 infrastructure, not evidence of a public release or of deferred product behavior.

## Contents

- [`CODEOWNERS`](CODEOWNERS) assigns default and sensitive-path ownership to the `@Eutheto/maintainers` team.
- [`ISSUE_TEMPLATE/`](ISSUE_TEMPLATE/) provides structured non-security bug and feature intake. Blank issues are disabled; suspected vulnerabilities must not be disclosed publicly and are directed to the current security policy.
- [`pull_request_template.md`](pull_request_template.md) requires phase/issue traceability, DCO sign-off, exact verification evidence, and cross-cutting impact review.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) defines the rules for changing repository automation.
- `dependabot.yml` owns grouped, reviewable dependency proposals.
- `workflows/` contains the seven Phase-00 workflow entry points.

Repository-wide policy remains in [AGENTS.md](../AGENTS.md), [CONTRIBUTING.md](../CONTRIBUTING.md), [SECURITY.md](../SECURITY.md), and the active [Phase 00 roadmap](../docs/roadmap/00-repository-and-reproducible-tooling.md).

## Required workflow surfaces

| Workflow | Contract |
|---|---|
| `pr.yml` | Canonical locked-Nix pull-request checks and sanitized reports. |
| `portable.yml` | Shared exact pins, native prerequisite verification, source builds, and unbundled shell-launch smoke across supported Linux, macOS, and Windows runners. |
| `security.yml` | Dependency, license, advisory, secret, SBOM, and Tauri-capability review without exposing secrets to pull requests. |
| `benchmark.yml` | Scheduled and path-gated benchmark evidence only after an approved executable benchmark contract exists. |
| `fuzz.yml` | Scheduled and path-gated fuzz evidence only after approved targets exist; never arbitrary pull-request scripts. |
| `release.yml` | Separate build and protected signing jobs with digest verification; no publication while release, signing, updater, and identity gates are open. |
| `dependency-update.yml` | Immutable-action and isolated-major-migration policy; applicable build, generation, security, worker, and benchmark evidence is supplied by the existing owning workflows rather than repeated here. |

A workflow file or green placeholder job is not passing evidence. Unimplemented commands, unavailable runners, and later-phase product gates must be represented as `deferred` or `unavailable`, never silently skipped or reported as success.

Exact `phase/06-desktop` PRs use the bounded [Phase06 integration trial](../docs/contributors/git-workflow.md#temporary-phase06-integration): source/security/dependency checks on every package, explicitly deferred native/package policy gates only for the documented frontend/documentation allowance, and full risk/milestone matrices. Main context names/protections and non-phase event behavior remain unchanged. `just ci-policy-test` covers real selector and terminal-gate behavior; actual hosted runs remain required evidence.

## Automation invariants

All GitHub automation follows these invariants:

1. **Same commands locally and in CI.** `Justfile` is the human command authority, Nix is canonical on Linux, and `xtask` owns cross-platform generation, hashing, fixtures, license/SBOM work, and release manifests. Workflow-local shell logic does not become a competing build system.
2. **Immutable inputs.** External actions use a full 40-character commit SHA with a comment naming the verified compatible stable tag. Cargo, pnpm, Nix, worker, protocol, and generated inputs remain frozen and reviewable.
3. **Least privilege.** Read-only repository contents are the default. Write permissions and protected environments are isolated to the narrow operation that requires them. Fork pull requests receive no secret.
4. **Untrusted pull requests.** Pull-request-controlled scripts, metadata, artifacts, and caches never cross into a privileged context. `pull_request_target` is not used to execute untrusted checkout content.
5. **Deterministic caches.** Cache identity includes platform, architecture, exact lock/toolchain inputs, and a deliberate schema version. Caches never contain credentials, signing state, user data, or unsanitized diagnostics.
6. **Sanitized evidence.** Artifact names, contents, logs, and retention are explicit. Secrets, absolute user paths, local databases, captured scenarios, and unsanitized support bundles are not uploaded.
7. **Concurrency by risk.** Superseded validation may be cancelled. Protected signing/publication work is not made cancellable in a way that can leave custody or release state ambiguous.
8. **Build/sign separation.** Signing accepts only the expected build artifact after verifying its recorded digest. Signing material never enters Nix derivations, caches, ordinary artifacts, default environment variables, or pull-request jobs.
9. **Closed gates stay closed.** Current public release, signing, updater, publication, production identifier, solver, packaged-E2E, benchmark, and fuzz gates cannot be converted into passing evidence by automation scaffolding.

## Security intake

Public security issues are explicitly disallowed. The issue chooser points to the repository [security policy](../SECURITY.md), which is the sole authority for whether an approved private channel exists and what a reporter should do. Do not guess an email address, individual contact, chat room, or other recipient. Bug and feature forms must reject security-sensitive disclosure.

## Ownership and change review

The default owner and all sensitive-path owners are `@Eutheto/maintainers`. Sensitive paths include workflows, Nix and lockfiles, schemas/protocol/migrations, parsers and other untrusted-input boundaries, Tauri capabilities and permissions, credentials, updater/signing/release inputs, and license/SBOM policy. While the repository has one active maintainer, `CODEOWNERS` remains an ownership map rather than an impossible self-approval gate. Branch protection must require applicable independent ownership review once a second qualified maintainer is active.

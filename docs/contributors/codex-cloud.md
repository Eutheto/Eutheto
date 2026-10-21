# Codex Cloud contributor setup

This project keeps Ponytail 5.3.2 at commit `16352ac229eb08a9598dcae48fe9f2b342567d86` in [`.agents/`](../../.agents/README.md). The unchanged skills, supporting references and MIT notice are contributor materials. They install no application packages, plugins or hooks. Project routing lives in [AGENTS.md](../../AGENTS.md); four named local-client profiles live in [`.codex/agents/`](../../.codex/agents/).

## Role handoffs

Use Ponytail Full by default. Explain a task-local Ultra escalation before applying it; it does not change model reasoning effort or later tasks. Request Sol (`gpt-6.1-sol`, `high`) for the lead, implementation and routine correctness/complexity review. Request Astra (`gpt-6-astra`, `high`) for difficult technical planning and consequential decisions. Keep independent reviews separate from implementation.

Hosted delegation receives instructions directly. If the platform cannot select a named local profile, read its TOML and relevant skill files, pass the role instructions in the task message, and request model/effort explicitly through the supported delegation tool. Report that direct handoff rather than claiming profile registration. If explicit settings are unsupported or denied, report the limitation; do not silently claim the requested model ran.

Before launching a worker, check existing workers. Allow at most two active subagents plus the lead for this project. Coordinate file ownership and dependencies; reviewers inspect the finished diff read-only. This cap is project orchestration policy. Two successful concurrent workers do not prove that the host enforces it. Editing files does not change a running worker's settings.

## Four separate evidence layers

| Layer                          | What a successful check establishes                                                                                                     | What remains unverified                                                                           |
| ------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| Static configuration           | Required TOMLs parse with expected names/settings/instructions; pinned skills and supporting files match the checked-in digest manifest | Hosted discovery, admission, execution, backend identity and saved settings                       |
| Optional nested CLI diagnostic | Only the result of that explicitly requested CLI invocation                                                                             | Hosted platform delegation and device behavior                                                    |
| Actual hosted delegation smoke | Platform admission of requested settings, followed separately by a completed read-only task                                             | Independent backend model attestation, automatic profile discovery and saved-settings persistence |
| Device/runtime checks          | The specific application/runtime behavior actually exercised                                                                            | Other platforms, manual acceptance and checks not run                                             |

Inside the pinned development shell, run:

```sh
just codex-static-check
```

This uses Python 3.11+ standard-library `tomllib`. It reads repository inputs, runs focused fixture tests and checks pinned digests; it does not install anything, start Codex, contact a service or write global configuration. Static success is not a hosted-ready result. Real missing/malformed inputs or settings/digest mismatches exit nonzero. Review pin/manifest changes together with the upstream source and license; a digest manifest detects drift, not malicious coordinated edits.

No nested CLI startup is required for hosted readiness. Some other projects' inspectors launch `codex app-server`; Eutheto does not need such a probe. If a nested diagnostic is requested, preserve execution, timeout, parse and configuration errors as errors. A managed read-only `CODEX_HOME` or project-trust failure is evidence about that nested CLI invocation, not proof that platform delegation is broken. Never change protected home, relocate/replace `CODEX_HOME`, inject `BASH_ENV`, grant hook trust or execute untrusted hooks to make a probe pass.

For an actual hosted smoke, use the platform's supported delegation tool for one read-only Sol/high task and one read-only Astra/high planning task, within the cap. Supply the relevant role/skill instructions directly. Ask each to read this checkout's `AGENTS.md`, report the absolute worktree path and HEAD, and return a distinct completion marker. Record admission and completion separately, including requested settings, task identifiers, commit and limitations. A worker repeating its requested settings is not independent backend attestation. Do not launch smoke workers from the static checker or infer runtime success from TOMLs. Device/native acceptance remains separate and must be reported as unverified until exercised.

## Saved-environment publication is separate

Repository commits do not update saved Cloud settings. The supplied legacy setup bundle outside Git installs skills/profiles into global `CODEX_HOME`; it is not the hosted activation path described here. Do not run it against managed read-only home or bypass that restriction. Keep existing pinned Nix-in-Docker toolchain startup intact.

When publication is authorized, the owner must inspect the actual saved-environment configuration and its setup/maintenance commands. Make the reviewed commit available through the approved source mechanism, preserve toolchain startup, and replace any invocation of the legacy global-home installer with repository static validation after the checkout is available. Supply hosted role instructions through supported delegation; local TOMLs alone do not activate hosted roles. No settings field name or persistence mechanism is assumed here.

Publish only through that environment's supported settings surface, then start a fresh task and separately record static validation and actual hosted smoke evidence. Verify the worktree is visible at the same path inside the development container before running checks there. No publication, settings persistence, hosted auto-discovery or device acceptance follows from this document or a local commit.

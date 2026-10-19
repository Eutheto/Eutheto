# Repository-local agent instructions

The six directories in `skills/` are unchanged copies of `skills/ponytail*` from [DietrichGebert/ponytail](https://github.com/DietrichGebert/ponytail/tree/16352ac229eb08a9598dcae48fe9f2b342567d86), version **5.3.2**, commit **16352ac229eb08a9598dcae48fe9f2b342567d86**. They are contributor instructions, not application dependencies or runtime assets.

## Third-party license inputs

All six skills and the supporting files under `vendor/ponytail/` are MIT licensed, copyright (c) 2026 DietrichGebert. The complete, unchanged notice is [vendor/ponytail/LICENSE](vendor/ponytail/LICENSE). The unchanged upstream [README](vendor/ponytail/README.md), [benchmark report](vendor/ponytail/benchmarks/results/2026-10-07-agentic.md), and [benchmark methodology](vendor/ponytail/benchmarks/agentic/README.md) preserve the supporting references for `ponytail-gain`. Resolve that skill's upstream-relative benchmark/README references under `vendor/ponytail/`; its published benchmark is not evidence of savings in Eutheto. Other upstream links describe optional upstream integrations and do not install or authorize them here.

This file records the source and license inputs for these vendored contributor materials. The application `xtask/supply-chain-inputs.json` and `just licenses` / `just sbom` generators inventory locked Cargo/pnpm packages, explicitly reviewed vendored packages, and reviewed runtime/source-data assets. These contributor Markdown inputs are not application inventory entries. Keep the license and provenance with any copied skill. Update the pinned source deliberately; do not auto-update or edit the vendored skill contents locally.

## Routing and discovery

Root [AGENTS.md](../AGENTS.md) defines Full by default, reasoned task-local Ultra and explicit user overrides. Four named profiles are preserved in [`.codex/agents/`](../.codex/agents/) for local clients that support custom-agent TOMLs. Planning requests `gpt-6-astra` / `high`; the lead, implementation and routine reviewers request `gpt-6.1-sol` / `high`. Astra/high is for difficult planning and consequential decisions. The two-active-subagents-plus-lead limit is project policy, not proven host enforcement.

For hosted execution, pass role instructions and explicit model/effort through platform delegation when supported. Files on disk do not prove hosted registration or automatic skill discovery. Read these skill files directly if discovery is unavailable, and report the handoff method. Admission of requested settings and completed execution are evidence of those observations, not independent backend model attestation. Changing a profile does not reconfigure an existing worker.

Run `just codex-static-check` to validate profiles and pinned contributor inputs without starting a nested CLI. [Codex Cloud guidance](../docs/contributors/codex-cloud.md) separates static checks, nested CLI diagnostics, hosted smoke and device/runtime evidence. This setup installs no hooks, plugins, trust records, mode flags or global home configuration. Repository edits do not publish saved-environment settings.

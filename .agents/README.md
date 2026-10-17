# Repository-local agent instructions

The six directories in `skills/` are unchanged copies of `skills/ponytail*` from [DietrichGebert/ponytail](https://github.com/DietrichGebert/ponytail/tree/16352ac229eb08a9598dcae48fe9f2b342567d86), version **5.3.2**, commit **16352ac229eb08a9598dcae48fe9f2b342567d86**. They are contributor instructions, not application dependencies or runtime assets.

## Third-party license inputs

All six skills and the supporting files under `vendor/ponytail/` are MIT licensed, copyright (c) 2026 DietrichGebert. The complete, unchanged notice is [vendor/ponytail/LICENSE](vendor/ponytail/LICENSE). The unchanged upstream [README](vendor/ponytail/README.md), [benchmark report](vendor/ponytail/benchmarks/results/2026-10-07-agentic.md), and [benchmark methodology](vendor/ponytail/benchmarks/agentic/README.md) preserve the supporting references for `ponytail-gain`. Resolve that skill's upstream-relative benchmark/README references under `vendor/ponytail/`; its published benchmark is not evidence of savings in Eutheto. Other upstream links describe optional upstream integrations and do not install or authorize them here.

This file records the source and license inputs for these vendored contributor materials. The existing `xtask/supply-chain-inputs.json` schema and `just licenses` / `just sbom` generators inventory locked Cargo and pnpm packages only; these Markdown inputs do not add a package to those inventories. Keep the license and provenance with any copied skill. Update the pinned source deliberately; do not auto-update or edit the vendored skill contents locally.

## Routing and discovery

Root [AGENTS.md](../AGENTS.md) preserves project guidance and defines Full by default, reasoned task-local Ultra, explicit user overrides, and role handoffs. Four standalone profiles are in [`.codex/agents/`](../.codex/agents/): planning selects `gpt-6-astra` / `high`; implementation and both routine reviewers explicitly select `gpt-6.1-sol` / `high`. Astra/high is reserved for planning or genuinely difficult decisions, never routine packaging or implementation. At most two subagents may be active per project in addition to the lead. Existing workers must be stopped and explicitly relaunched when their actual settings cannot be verified; changing these files does not reconfigure a running worker.

The profile format was checked on 2026-10-17 against the [official custom-agent documentation](https://learn.chatgpt.com/docs/agent-configuration/subagents#custom-agents): `name`, `description`, and `developer_instructions` are required; model and reasoning settings are optional. No project config or permission override is needed for these standalone profiles.

Files on disk do not establish native hosted discovery or selection. That integration is unverified; use explicit role handoffs with the profile instructions and direct skill reads when the host cannot select custom profiles, and report the fallback. Invoke a repository skill using the host's discovered name when available, rather than assuming the plugin-qualified names in the unchanged upstream help apply. This setup installs no hooks, plugin activation, mode flags, trust records, or global home configuration.

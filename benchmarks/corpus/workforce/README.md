<!-- SPDX-License-Identifier: Apache-2.0 -->

# Workforce benchmark corpus

`v1.json` binds thirteen deterministic synthetic scenarios, separate correctness expectations and a Phase07-only call-out recipe. Inputs contain no real employee, patient, rota, clinic or customer data. Source identity, fixed clock, seed, license, dimensions and execution profile are recorded in the generated manifest and expectations.

Run `just generate` after changing the authoritative constructors, semantic checks or private schemas in `benchmarks/workforce-runner`. Never hand-edit generated JSON. `just workforce-corpus-check` rejects drift, missing/unindexed fixtures, unsupported versions, changed hashes, unsafe paths and changed original-domain semantics.

The corpus includes:

- AppendixF in full, preserving its active obligations rather than substituting an easier executable scenario;
- tiny, initial and full four-week clinic variants;
- clinic plus overnight, rolling-hours, specialist coverage, repair-before and deliberately infeasible coverage;
- spring/fall DST variants;
- a supported large case and a separately classified resource-pressure case.

AppendixF, full-clinic and rolling-hours currently require explicit compile refusal because their preserved active obligations are not yet executable. The pressure case requires resource-limit refusal. Neither is accepted or counted as proved infeasible. The repair recipe validates one typed unavailable-person mutation; it does not execute repair or invent an accepted baseline.

Focused commands:

```text
just workforce-corpus-generate PEOPLE SHIFTS OUTPUT
just bench-workforce ARTIFACT_ROOT MANIFEST_SHA256 OUTPUT
just workforce-corpus-smoke-cli CLI_IMAGE MANIFEST_SHA256 OUTPUT
```

The synthetic generator uses fixed seed zero and explicit bounded sizes. Benchmark/CLI output is no-clobber. Use `just bench` for one worker artifact shared across all three evidence paths. Private corpus/evidence schemas here do not change the public scenario format or grant solution acceptance authority.

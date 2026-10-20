<!-- SPDX-License-Identifier: Apache-2.0 -->

# Workforce benchmark corpus

`v2.json` binds fifteen deterministic synthetic scenarios, separate correctness expectations and a Phase07 call-out recipe. The historical `v1` corpus remains unchanged. Inputs contain no real employee, patient, rota, clinic or customer data. Source identity, fixed clock, seed, license, dimensions and execution profile are recorded in the generated manifest and expectations.

Run `just generate` after changing the authoritative constructors, semantic checks or private schemas in `benchmarks/workforce-runner`. Never hand-edit generated JSON. `just workforce-corpus-check` rejects drift, missing/unindexed fixtures, unsupported versions, changed hashes, unsafe paths and changed original-domain semantics.

The corpus includes:

- AppendixF in full, preserving its active obligations rather than substituting an easier executable scenario;
- tiny, initial and full four-week clinic variants;
- clinic plus overnight, rolling-hours, specialist coverage, repair-before and deliberately infeasible coverage;
- spring/fall DST variants;
- a supported large case and a separately classified resource-pressure case.

AppendixF preserves its full-day absence and Required coverage/rest rules and expects **infeasible**, not an accepted roster: Smith cannot take the preceding overnight call or September 10 clinic; the remaining call owner lacks the required rest, leaving only one clinician for exact-two clinic coverage. Local readiness can pass despite this cross-rule contradiction. The expectation does not claim a mapped native conflict core.

Full-clinic and rolling-hours have accepted-result expectations under the complete Phase07 compiler. The current pressure case uses100 universally eligible people and64 daily shifts (6400 unpruned pairs) and requires resource-limit refusal, never a claimed infeasibility proof. The earlier100×20 workload now fits the unchanged limits after input-sized accounting; historical v1 remains unchanged. The call-out recipe requires a genuinely accepted baseline; CLI smoke checks immutable original-source capture, repair and independent reverification rather than inventing accepted authority.

Focused commands:

```text
just workforce-corpus-generate PEOPLE SHIFTS OUTPUT
just bench-workforce ARTIFACT_ROOT MANIFEST_SHA256 OUTPUT
just workforce-corpus-smoke-cli CLI_IMAGE MANIFEST_SHA256 OUTPUT
```

The synthetic generator uses fixed seed zero and explicit bounded sizes. Benchmark/CLI output is no-clobber. Use `just bench` for one worker artifact shared across all three evidence paths. Private corpus/evidence schemas here do not change the public scenario format or grant solution acceptance authority.

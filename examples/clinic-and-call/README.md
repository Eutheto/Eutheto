<!-- SPDX-License-Identifier: Apache-2.0 -->

# Synthetic clinic-and-call CLI example

Run from the repository root with the current-source, manifest-bound CLI image
(`just worker-build-cli`). The [clinic-overnight fixture](../../domains/workforce/fixtures/v1/clinic-overnight.json)
is a generated `eutheto/scenario` v2 document for the `official.workforce` v1
domain. Its twelve fictional clinicians and 48 clinic/call shifts exercise
Eligibility, Availability, Coverage, NoOverlap, and directional MinimumRest
together. The fixed-clock, seed-zero source and
SHA-256 inventory are in the [Workforce corpus](../../benchmarks/corpus/workforce/v1.json);
the generator and fixture are Apache-2.0 licensed. No real roster or patient
data is used.

```sh
out="$(mktemp -d)"
scenario=domains/workforce/fixtures/v1/clinic-overnight.json
target/cli-package/optimizer scenario validate "$scenario"
target/cli-package/optimizer solve "$scenario" --mode deep --max-time 60s \
  --threads 1 --seed 1 --first-feasible --output "$out/result.json"
target/cli-package/optimizer solutions verify "$scenario" "$out/result.json"
target/cli-package/optimizer solutions export "$scenario" "$out/result.json" \
  --format csv --output "$out/assignments.csv"
target/cli-package/optimizer solutions export "$scenario" "$out/result.json" \
  --format ics --output "$out/schedule.ics"
```

`solve` returns an independently verified feasible plan, not a proof of
optimality or an exact person-by-person schedule. `solutions verify` independently
checks the source and accepted result again; exports contain selected assignments
only. Inspect the temporary output directory before removing it. CLI file outputs
refuse to overwrite existing files. The [versioned expectations](../../benchmarks/expected/workforce/v1.json)
record semantic acceptance, not one arbitrary tie-broken arrangement.

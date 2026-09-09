<!-- SPDX-License-Identifier: Apache-2.0 -->

# Benchmarks

Two distinct benchmark paths are implemented:

- Phase03 primitive measurements exercise the raw approved OR-Tools backend. They do not establish domain acceptance.
- [`workforce-runner/`](workforce-runner/) measures the Phase05 synthetic Workforce corpus through `HeadlessService` and exercises the same corpus through the packaged `optimizer` CLI. Accepted solutions require original-domain verification, authoritative scoring and final admission.

`just bench` checks the generated corpus, builds one approved worker artifact, and reuses its exact manifest digest for primitive measurements, Workforce measurements and CLI assembly. Reports go into a new private run directory under `EUTHETO_BENCHMARK_ARTIFACTS` (default `.cache/benchmarks`). Existing reports are never overwritten. The Benchmark workflow retains the three bounded JSON reports, not scenarios, databases or raw worker logs.

- [`corpus/workforce/`](corpus/workforce/) owns the versioned manifest and private JSON Schemas; scenario inputs live in the domain's fixture directory.
- [`expected/workforce/`](expected/workforce/) owns correctness expectations, separately hash-bound to each input.
- [`runner/`](runner/) remains a reserved boundary; execution currently belongs to the existing Phase03 xtask and the non-production Workforce runner.

Workforce observations measure **first-feasible acceptance**, not time to proven optimality: Deep mode, 60-second parent budget, the existing 30-second backend cap, seed1 and one worker thread. Evidence records actual backend/remaining-parent limits. Each case has one first-operation sample in a fresh process and three samples after one warm-up in another process. Every solve spawns a worker; OS caches are unmanaged and provider enrichment is not applicable.

Missing stage measurements are explicitly unavailable, not zero. An expected resource refusal is not an infeasibility proof. No Phase12 performance baseline, threshold or release claim is established by these observations. See the [fixture contract](../docs/architecture/test-fixtures.md).

To assemble the CLI from an already approved worker, use `cargo xtask solver build-cli --artifact-root ARTIFACT_ROOT --manifest-sha256 MANIFEST_SHA256`. The two options are required together; a mismatch fails rather than rebuilding or substituting a worker. Omitting both retains the ordinary worker-build-and-assembly path.

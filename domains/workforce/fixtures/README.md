<!-- SPDX-License-Identifier: Apache-2.0 -->

# Workforce `fixtures` boundary

`v1/` contains thirteen generated synthetic portable scenarios and `repair-callout.json`. The manifest, source provenance, input hashes and measurement profile live in [`benchmarks/corpus/workforce`](../../../benchmarks/corpus/workforce/); correctness expectations live separately in [`benchmarks/expected/workforce`](../../../benchmarks/expected/workforce/).

The authoritative constructors and original-domain semantic checks are in `benchmarks/workforce-runner`. Regenerate with `just generate`; verify with `just workforce-corpus-check`. The versioned fixture directory is an exact inventory: extra or missing entries fail checking. It must contain no captured user data.

Portable reopening and semantic preservation do not imply solver acceptance. Full AppendixF, full-clinic and rolling-hours retain active unsupported obligations and require compile refusal. Large-pressure requires resource refusal, while infeasible-coverage requires an actual infeasibility outcome.

`repair-callout.json` describes one typed unavailable-person command against repair-before. Phase07 must first obtain a genuinely independently accepted solution with the specified assignment and select it as the baseline. Pure command/meaning validation here is not repair execution; no baseline authority is fabricated.

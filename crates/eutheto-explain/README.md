<!-- SPDX-License-Identifier: Apache-2.0 -->

# `eutheto-explain`

Pure, deterministic explanation algorithms over validated domain and planning IR.

## Boundary

The crate maps solver-neutral assumption literals, performs budget-aware conflict shrinking,
compares independently accepted results, validates additive counterfactual compilations, and
interprets counterfactual terminal records. It has no solver API, persistence, async runtime,
backend launcher, clock implementation, or presentation authority.

It depends only on `eutheto-types`, `eutheto-domain-ir`, and `eutheto-planning-ir`. Backend claims
must first be converted into the validated contracts owned by those crates.

`ConflictShrinker` owns its initial groups and lends each deletion trial as a slice. Async callers
can await a bounded native re-solve without cloning the state or blocking an executor. Only timely
validated infeasibility removes a group; feasible, inconclusive, cancelled or expired trials restore
it. Completing every deletion trial establishes minimality; an explicit infeasible zero-group
trial establishes foundational infeasibility. A missing or empty native core does not.
Callers supply validated infeasibility proofs and independently reviewed feasible witnesses; the
pure state machine does not turn an unchecked backend status into either proof.

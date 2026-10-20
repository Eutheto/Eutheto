<!-- SPDX-License-Identifier: Apache-2.0 -->

# `eutheto-solver-ortools`

This crate owns the manifest-authenticated bundled-worker artifact, exact
matrix-backed compatibility report, deterministic planning-IR-to-CP-SAT
translation, strict candidate decoding, truthful bounded progress conversion,
exact adapter timing and quality evidence aggregation, isolated worker launch and
single-session protocol supervision, and the production OR-Tools
`SolverBackend` registry entry.

Translation currently covers scalar variable domains plus Boolean-or,
conjunction, implication, equivalence, one-of, cardinality-range, integer linear
comparison constraints, enforcement literals, and safely scalarizable bounded
objective terms. It requests every scalar assignment required by the backend
output contract plus domain projections, rejecting unsupported or multipass
planning features rather than omitting them. Returned candidates remain
non-authoritative and require the independent verification boundary owned by
Phase 04.

Nonempty Planning IR assumptions are supported only without objective levels
and with `WorkerThreadPolicy::Exact(1)`; automatic thread selection is rejected
even when it could resolve to one. `ExplanationMode::Standard` alone does not
select this profile. Exact signed native literals retain their source assumption
and Required-rule provenance. The native worker disables presolve for this
restricted satisfaction path; ordinary optimization remains unchanged.

An infeasible diagnostic returns canonical source literals or explicit
`NotReturned`/`InvalidCore` evidence. Whole-core count, membership, polarity and
uniqueness checks precede publication; no partial group mapping is exposed.
Empty cores do not prove foundational infeasibility. Shared outcome validation
binds this evidence to the actual profile and terminal status before a domain
mapper consumes it. This backend support does not implement Workforce diagnostic
lowering, persisted mapped conflicts, shrinking or their desktop presentation.

An independently failed first-candidate review remains quarantined even if a
later candidate could verify. A clean worker-error terminal after an incumbent
follows the same recoverable backend-error path as a crash; retained candidates
still require independent review and never inherit worker acceptance authority.

On Linux, a concurrent fork may temporarily retain a staged executable's writable
descriptor after the parent closes it. Launch admission waits only for
`ExecutableFileBusy`, within the original cancellation/deadline budget; all other
launch errors fail immediately. The wait precedes the first successfully started
worker image and is included in measured startup time. It does not redispatch a
solver or reset its budget. See [Rust issue114554](https://github.com/rust-lang/rust/issues/114554).

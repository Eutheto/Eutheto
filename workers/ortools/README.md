<!-- SPDX-License-Identifier: Apache-2.0 -->

# OR-Tools worker boundary

This directory contains the project-owned out-of-process CP-SAT worker defined
by [ADR-004](../../docs/adr/004-ortools-worker.md). The worker is version
`0.1.0`, implements solver-worker protocol 1.1, and targets OR-Tools
`9.15.6755` with a matched protobuf runtime.

## Source contract gate

Production configuration requires the generated approved
[`source-contract.json`](source-contract.json) through an explicit absolute
`EUTHETO_ORTOOLS_PHASE3_CONTRACT` path. CMake requires that file to be
byte-identical to the generated repository approval. `cargo xtask generate`
owns it and rehashes the repository patch and protocol schema; the record binds
the exact OR-Tools and protobuf sources and hashes, patch path/hash,
protoc/runtime versions, protocol schema digest, worker identity/version, and
26 reviewed cross-target CMake cache entries. The same generator owns
[`dependency-sources.json`](dependency-sources.json), which binds the exact
transitive archives and upstream patch mapping to the approved OR-Tools source.
The checked-in [`source-contract.example.json`](source-contract.example.json)
remains deliberately unapproved and cannot authorize a production build or
install.

`EUTHETO_ORTOOLS_DEVELOPMENT_BUILD=ON` is an explicit local-only escape hatch
for compilation and native testing against the pinned dependencies. It
disables all install rules and does not establish source or distribution
approval. `EUTHETO_ORTOOLS_BUILD_TESTS=ON` adds the focused native test target.

The executable communicates only through four-byte big-endian framed generated
protobuf messages on stdin/stdout. Native source consumes the generated policy
projection rather than duplicating protocol limits. The Rust supervisor,
installed-manifest approval, artifact license/SBOM payload, and domain
translation stay outside this subtree; the reviewed CMake module owns only the
ephemeral native build and runtime-closure assembly.

## Restricted assumption diagnostics

Nonempty model assumptions select a single-worker satisfaction profile:
integer and floating objectives are rejected, exactly one worker is required,
and `cp_model_presolve=false` is applied to both parameter validation and search.
Ordinary optimization and empty-assumption solves retain their normal settings.
The pinned OR-Tools [presolve defect #5141](https://github.com/google/or-tools/issues/5141)
remains open; this is a verified restricted configuration, not a general
presolve-enabled assumption claim.

The worker advertises the existing `sufficient-assumptions` capability and uses
the existing `Finished.sufficient_assumptions` field. Only an `INFEASIBLE` result
in that profile may emit a nonempty core. Counts, duplicates and exact signed
membership are checked for the whole core; invalid native cores terminate with
an `INTERNAL` worker error, never a filtered subset. Empty cores carry no
foundational-infeasibility proof. Native regressions cover signed and unordered
groups, the upstream circuit case, feasible and foundational outcomes, and
objective/thread admission.

The applied-parameter digest retains its version-1 56-byte preimage. It does not
by itself encode presolve policy: that static policy is bound by the exact model
bytes and authenticated worker binary/manifest identities.

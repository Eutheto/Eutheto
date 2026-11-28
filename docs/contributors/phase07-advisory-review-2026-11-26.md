<!-- SPDX-License-Identifier: Apache-2.0 -->

# Maintainer decision packet · 26 November 2026

**Status: both decisions pending Zebulon Rouse-Frantzich.** This draft records evidence and proposals. A DCO trailer attests authorship of this document only. The [current disposition policy](../../CONTRIBUTING.md#temporary-phase-07-maintenance-dispositions) governs every integration and release. The named owner is Phase07maintainer. The repository code owner is @Eutheto/maintainers.

## Integration evidence

Main was `c08e95806887614eae5e1ceec4b117616cd46b66`. [#98](https://github.com/Eutheto/Eutheto/pull/98) at `95ad0041745831efda88ac62c6d3eaaf26c30dba` and [#102](https://github.com/Eutheto/Eutheto/pull/102) at `c63868b379809bd24b15d8bbef50d5a156818355` produce the same Git tree in either order: `3e5aa6d388d0592be38b32a742b12e9e87f1d365`. Evidence below is from that combined tree; each standalone PR has a different dependency graph. The official [RustSec revision](https://github.com/RustSec/advisory-db/tree/b9b8a3c2cb1f2a3c76ab17a551193568c9f8b7a4) still matched upstream HEAD when checked on November 26.

| Pinned check | Combined result |
| --- | --- |
| Raw cargo-audit 0.22.0, root | Zero vulnerability entries, 13 unmaintained and one unsound warning. Exit 1 with warnings denied. |
| Raw cargo-audit, nested fuzz lock | Zero vulnerabilities and zero warnings, exit 0. |
| cargo-deny 0.18.9 | Advisories, licenses, bans and sources passed under the existing configured ignores. |
| Canonical policy | All 13 policy tests passed. The UTC precheck stopped before the full audit because November 16 had passed. Security remains blocked. |

Compared with main, the combined locks clear the SQLite, cookie and URL vulnerabilities (RUSTSEC-2026-0459, -0594 and -0592) and the rustix unsoundness warning (RUSTSEC-2026-0486). Of the 13 remaining maintenance notices, six belong to the PDF scope below and six to the second table. **The thirteenth is chrono 0.4.45, RUSTSEC-2026-0543.** Applying the repository's report validator separately to the saved raw JSON rejected this unreviewed ID. Even a fresh decision on the six would leave this independent blocker. Hold chrono for separate dependency-path and replacement review; neither decision below covers it.

## A. Phase 07 PDF worker

The six exact lock entries use `registry+https://github.com/rust-lang/crates.io-index`. Each cited advisory is active, classified unmaintained and lists no patched version. Dates below are advisory dates. No upstream fix date is recorded.

| RustSec record | Locked package | SHA-256 | Build path or use | Advisory date |
| --- | --- | --- | --- | --- |
| [RUSTSEC-2025-0141](https://rustsec.org/advisories/RUSTSEC-2025-0141.html) | bincode 1.3.3 | `b1f45e9417d87227c7a56d22e471c6206462cba514c7590c09aff4cf6d1ddcad` | syntect serialization | 2025-12-16 |
| [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html) | paste 1.0.15 | `57c0d7b74b563b49d38dae00a0c37d4d6de9b432382b2892f0574ddcae73fd0a` | hayagriva build-time macro | 2024-10-07 |
| [RUSTSEC-2026-0206](https://rustsec.org/advisories/RUSTSEC-2026-0206.html) | rustybuzz 0.20.1 | `fd3c7c96f8a08ee34eff8857b11b49b07d71d1c3f4e88f8a88d4c9e9f90b1702` | Typst text shaping | 2026-07-11 |
| [RUSTSEC-2026-0192](https://rustsec.org/advisories/RUSTSEC-2026-0192.html) | ttf-parser 0.25.1 | `d2df906b07856748fa3f6e0ad0cbaa047052d4a7dd609e231c4f72cee8c36f31` | Typst font parsing | 2026-06-28 |
| [RUSTSEC-2024-0449](https://rustsec.org/advisories/RUSTSEC-2024-0449.html) | unsafe-libyaml 0.2.11 | `673aac59facbab8a9007c7f6108d11f63b603f7cabff99fabf650fea5c32b861` | serde_yaml parser | 2024-03-25 |
| [RUSTSEC-2024-0320](https://rustsec.org/advisories/RUSTSEC-2024-0320.html) | yaml-rust 0.4.5 | `56c1936c4cc7a1c9ab21a1ebb602eb942ba868cbd44a99cb7cdc5892335e1c85` | syntect YAML data | 2024-03-20 |

The normal (non-dev) Cargo graph of eutheto-pdf-worker contains all six. Its production entry point calls Typst compilation and PDF emission. The fixed template and closed World admit reviewed embedded fonts and exclude external fonts, XML, YAML, bibliographies and syntax definitions. Parsing and shaping of embedded fonts are active. A build-time proc macro and a runtime parser have different exposure. Neither the combined changes nor the integration exercise changed the worker source, fonts or permissions. Native user acceptance was not exercised in this audit.

**Proposed A window:** after a positive owner decision and reviewed enforcement change, expire at **2026-12-17 00:00 UTC exclusive**, with a **December 10 UTC migration review**. No retrospective approval. Bring the review forward on any new advisory, package version/source/checksum or classification change, font or shaping change, newly reachable parser, or broader World/template input. Retain font identity, unsupported-glyph, Unicode, layout and closed-input checks. Changes to font/parser/shaping sources also require malformed-font robustness evidence. Use [the separate native-viewer acceptance procedure](https://github.com/Eutheto/Eutheto/pull/101) for mixed-direction visual order, reading order and clipboard behavior when evaluating a replacement.

## B. Tauri, GTK and urlpattern

The current separate date guard allows November 30 UTC inclusive. Its first failing instant is **2026-12-01 00:00 UTC**, or **November 30 at 6:00 p.m. US Central (CST)**. All entries below use the same crates.io source as above. Ten records were withdrawn on August 14; their packages remain locked but the raw report no longer returns those advisories.

| Existing policy ID | Locked package | Current RustSec status |
| --- | --- | --- |
| [RUSTSEC-2024-0411](https://rustsec.org/advisories/RUSTSEC-2024-0411.html) | gdkwayland-sys 0.18.2 | Withdrawn 2026-08-14 |
| [RUSTSEC-2024-0412](https://rustsec.org/advisories/RUSTSEC-2024-0412.html) | gdk 0.18.2 | Withdrawn 2026-08-14 |
| [RUSTSEC-2024-0413](https://rustsec.org/advisories/RUSTSEC-2024-0413.html) | atk 0.18.2 | Withdrawn 2026-08-14 |
| [RUSTSEC-2024-0414](https://rustsec.org/advisories/RUSTSEC-2024-0414.html) | gdkx11-sys 0.18.2 | Withdrawn 2026-08-14 |
| [RUSTSEC-2024-0415](https://rustsec.org/advisories/RUSTSEC-2024-0415.html) | gtk 0.18.2 | Withdrawn 2026-08-14 |
| [RUSTSEC-2024-0416](https://rustsec.org/advisories/RUSTSEC-2024-0416.html) | atk-sys 0.18.2 | Withdrawn 2026-08-14 |
| [RUSTSEC-2024-0417](https://rustsec.org/advisories/RUSTSEC-2024-0417.html) | gdkx11 0.18.2 | Withdrawn 2026-08-14 |
| [RUSTSEC-2024-0418](https://rustsec.org/advisories/RUSTSEC-2024-0418.html) | gdk-sys 0.18.2 | Withdrawn 2026-08-14 |
| [RUSTSEC-2024-0419](https://rustsec.org/advisories/RUSTSEC-2024-0419.html) | gtk3-macros 0.18.2 | Withdrawn 2026-08-14 |
| [RUSTSEC-2024-0420](https://rustsec.org/advisories/RUSTSEC-2024-0420.html) | gtk-sys 0.18.2 | Withdrawn 2026-08-14 |
| [RUSTSEC-2024-0370](https://rustsec.org/advisories/RUSTSEC-2024-0370.html) | proc-macro-error 1.0.4 | Unmaintained |
| [RUSTSEC-2025-0075](https://rustsec.org/advisories/RUSTSEC-2025-0075.html) | unic-char-range 0.9.0 | Unmaintained |
| [RUSTSEC-2025-0080](https://rustsec.org/advisories/RUSTSEC-2025-0080.html) | unic-common 0.9.0 | Unmaintained |
| [RUSTSEC-2025-0081](https://rustsec.org/advisories/RUSTSEC-2025-0081.html) | unic-char-property 0.9.0 | Unmaintained |
| [RUSTSEC-2025-0098](https://rustsec.org/advisories/RUSTSEC-2025-0098.html) | unic-ucd-version 0.9.0 | Unmaintained |
| [RUSTSEC-2025-0100](https://rustsec.org/advisories/RUSTSEC-2025-0100.html) | unic-ucd-ident 0.9.0 | Unmaintained |
| [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html) | glib 0.18.5 | Unsound |

**Proposed B decision: hold continuation.** Before the cutoff, separately review retirement of the ten withdrawn IDs and the remaining six maintenance notices. For glib 0.18.5, the advisory applies to VariantStrIter and lists a patched floor of 0.20.0. The current rationale relies on the affected iterator being unused. A textual search for VariantStrIter and array_iter_str found no references in the project Rust sources checked or in cached registry Rust sources outside glib 0.18.5 itself. That observation cannot rule out generated calls, aliases, indirect execution or other platforms. Require a locked Linux Tauri reachability review and a maintained compatible upgrade assessment before a separate glib decision. Reachable unsoundness blocks integration. Do not carry this ID into a maintenance-only disposition.

## Proposed workflow for a subsequent policy-design review

Keep an exact reviewed baseline of ID, classification, package, version, source, checksum, input exposure, owner and exclusive expiry. A new or changed identity, missing or reclassified advisory, reachable vulnerability or unsoundness, or expanded exposure must fail closed. Preserve the prohibitions on quick-xml/ZIP vulnerability IDs and all mandatory scans. Require the owner to triage even a new informational finding before it can enter that baseline.

For a still-valid, explicitly reviewed maintenance-only baseline, an unrelated vulnerability patch that preserves every constraint should be able to run all checks. Propose owner notifications at the milestone and before cutoff, with evidence linked to a tracked migration. Expiry without a fresh explicit decision blocks again. Never reset the clock on a PR update or deduplicate an unexpected finding into an existing acceptance. No notification or check change is implemented here. Validate expiry boundaries, new findings, changed package identity and changed exposure before adopting such a design. Only a subsequent reviewed implementation and passing required checks on the exact integration revision can establish readiness for manual merge.

## Requested maintainer response

1. **A — approve the proposed six-item scope through the exclusive December 17 cutoff, with December 10 review, or hold A.** Approval authorizes preparing a separate enforcement PR for maintainer review.
2. **B — confirm hold and review before November 30 at 6:00 p.m. Central, or request a separate B decision packet with different conditions.** No B extension is proposed. Chrono requires its own review regardless of either answer.

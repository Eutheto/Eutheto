<!-- SPDX-License-Identifier: Apache-2.0 -->

# Recipient PDF rendering

The [Phase 07 renderer decision](../roadmap/07-workforce-solving-results-repair-and-export.md) approved the fixed Typst route on 2026-10-17. This describes its implementation contract; it does not certify platform acceptance or close the Phase 07 manual checkpoint.

## Disclosure and authority

Core captures an accepted, independently verified result and its selected privacy options. HTML preview and PDF rendering consume the same canonical `RecipientReportV1`. PDF has a separate fixed layout, so pixel identity with browser printing is not promised. Selected content, provenance, exact rational values and stale/change information must remain present. Unsupported glyphs fail rendering rather than silently dropping text.

The renderer receives no scenario database, source-calendar data, rejected alternatives or solver diagnostics. It is not a verifier. A matching model or output checksum establishes byte identity, not authority to export. The original source custody, revision lease, stale acknowledgement and no-clobber publication checks still apply. Native preview retains the exact output bytes for later publication and charges them to the existing bounded custody store.

## Renderer boundary

`eutheto-pdf-worker` is a disposable executable with official pinned Typst and Typst PDF crates. Its project-owned template and five font faces are embedded. The custom Typst World provides the captured model and fixed template; it denies arbitrary source, file and package resolution and does not discover system fonts. These are application API restrictions, not an operating-system sandbox claim.

The five registered faces are not an inventory of every dependency asset in the executable. Typst also embeds standard PDF fonts, color profiles and supporting data through `typst-assets`. Those assets require their own attribution accounting even when the fixed template does not invoke their associated functions. The renderer's font digest identifies the registered faces; the executable digest binds the complete artifact.

The parent embeds the expected manifest during its build. That manifest binds the executable SHA-256, renderer version, Typst version, template SHA-256 and ordered font SHA-256. The runtime locates a fixed executable sibling, copies and verifies its bytes into private temporary custody, and retains that custody until the child settles. There is no runtime executable-path override or PATH search. A build without a manifest cannot render PDF.

## Framing and lifetime

The helper serves one request. Request and response begin with the respective eight-byte `EUTPDFQ1` or `EUTPDFR1` magic, a four-byte big-endian JSON-header length, a strict camel-case JSON header, and the declared raw payload. Headers are at most 4 KiB; the canonical model is at most 4 MiB and PDF output at most 32 MiB. The helper checks canonical typed serialization and the model digest. Core checks the response identity, exact size and PDF digest, EOF, and successful process exit before admitting output.

Preparation uses one inherited cancellation/deadline budget of at most 30 seconds across capture and rendering. Core owns termination and reaping even when its observer is dropped. The parent's open stdin pipe is the helper's lifeline while compiling; EOF or an extra byte terminates the helper. It launches no descendants. Failed, interrupted, oversized or malformed output never becomes a publishable capture.

## Packaging and evidence

Packaging must hash the final executable after platform fixups and deliver those exact bytes beside the application. PDF resources use their own staging directory so solver staging cannot replace them. The six retained font/data notice and full-license documents accompany the helper, and their exact file hashes appear in the generated license inventory and SPDX output. The HTML metadata conclusion includes the distinct W3C Document License (2015), with complete extracted licensing information and source/status/derivation notices for its scoped software use.

Tests of framing and fake children do not substitute for actual helper rendering and cancellation. Acceptance also requires independently inspecting a full synthetic practitioner report, long and hostile labels, optional/empty sections, page bounds, privacy exclusions, packaged CLI/desktop publication, and each supported platform's lifecycle and packaging behavior. Record unavailable checks as open gates.

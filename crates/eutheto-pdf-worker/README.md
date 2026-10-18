<!-- SPDX-License-Identifier: Apache-2.0 -->

# Recipient PDF helper

This binary renders the already approved `RecipientReportV1` using a fixed Typst
0.15.1 template. It never captures a scenario, selects disclosure, authenticates
acceptance, opens a destination, or publishes a file. The native host owns source
admission, budgets, cancellation, process kill/reap, approval and publication.

## Protocol 1

Each process handles one request. Stdin consists of `EUTPDFQ1` (eight ASCII bytes),
a big-endian unsigned 32-bit JSON header length, that many JSON bytes decoding to
`RecipientPdfRequestV1`, and exactly `modelBytes` canonical report bytes. Headers
are nonempty and at most 4096 bytes; model bytes are nonempty and at most 4 MiB.
The expected SHA-256 must be lowercase hexadecimal and match the exact bytes.
Canonical means the existing `serde_json::to_writer(RecipientReportV1)` encoding,
not sorted-key JSON. Unknown header/model fields and unsupported versions fail.

The parent **keeps stdin open** through response validation and successful child
exit. After reading the request, a control thread treats any next byte, EOF, or
read error as cancellation and immediately exits the process with status 3.
This lifeline is active during model admission, compilation, PDF encoding and
response writing. A stalled/incomplete initial frame remains subject to the
parent's timeout. The helper spawns no descendants. Successful `main` completion
exits the process even while the control thread is blocked.

Stdout consists of `EUTPDFR1`, a big-endian unsigned 32-bit JSON header length,
that many JSON bytes decoding to `RecipientPdfResponseV1`, then exactly `pdfBytes`
raw PDF bytes. PDF output is capped at 32 MiB. The parent must independently bound
and hash the full response, reject trailing bytes, match the captured model and
approved renderer identity, and require successful child exit. Public hashes
alone establish no trust. Errors use nonzero exit status and bounded error codes
on stderr; labels and compiler diagnostics are not emitted there. Unsupported
glyph errors disclose only the Unicode scalar code, never the surrounding text.

Renderer identity is version 1 and Typst version `0.15.1`. `templateSha256` hashes
the exact UTF-8 bytes of `src/report.typ`. `fontsSha256` hashes the concatenation of
each font's unsigned 64-bit big-endian length followed by its exact file bytes,
in this order: Libertinus Serif regular, bold, italic, bold-italic, then DejaVu
Sans Mono regular. All digest strings are lowercase SHA-256. Build tooling must
compute the expected identity independently and bind the executable checksum.

## Rendering and font provenance

The template reads only inert report bytes supplied in `sys.inputs`. It never
evaluates report strings as code. Its custom `World` provides only that compiled
template and five embedded font files; other source/file requests are denied,
and system time is unavailable. There is no system-font discovery, package
resolver, plugin loading, network client, or filesystem loader. All report rows
are rendered, with automatic pagination; no display filter or visible-page limit
is accepted. Unsupported glyphs refuse the whole report instead of silently
substituting missing characters.

These are restrictions on the template's exposed APIs, not an operating-system
sandbox claim. The process retains its inherited OS permissions; the parent owns
executable admission, process supervision and any platform confinement policy.

The unmodified fonts come from the official `typst-assets` 0.15.1 crate at source
commit `ab9eed7b046c6a29f6cdb8566f4b44fb46a2f57f`. Exact artifact/file SHA-256 values
and license identifiers are in [fonts/provenance.json](fonts/provenance.json).
Libertinus Serif is SIL OFL-1.1; DejaVu Sans Mono carries the Bitstream Vera license
and public-domain DejaVu changes. The complete upstream [LICENSE](fonts/UPSTREAM-LICENSE)
and [NOTICE](fonts/UPSTREAM-NOTICE) accompany these assets. The upstream notice
also describes assets not copied here; only the five named font files are
embedded. There is no direct `typst-assets` dependency or system-font fallback.

The representative fixture in `tests/fixtures/report.json` is disposable authored
test data, including hostile literal labels, long labels, non-ASCII text, complete
assignment and comparison censuses, zero workload and exact fractional actuals.
Worker tests and independent PDF text/count/layout inspection are required before
claiming renderer acceptance. This source tree alone is not runtime-distribution,
license-gate, or packaged-platform evidence.

# Phase 07 Workforce checkpoint

Use a disposable OS account on a supported desktop with its normal window manager, native viewer and screen reader. Use fictional records throughout. Record the commit (`git rev-parse HEAD`), OS, architecture, artifact SHA-256, viewer, assistive technology and versions before starting. On Linux, build the package with `just worker-build-desktop` in the pinned environment. Inspect the unsigned package under `.cache/cargo-target/release/bundle/deb/`, record its digest with `sha256sum`, install it in the disposable desktop and launch `eutheto-desktop` from that graphical session. Keep existing user libraries out of this run.

For a small reproducible case, save the following as UTF-8 CSV and map its columns to person name and external identity. Create a fictional 480-minute assignment type, mark the three people eligible, and create shifts on January 1 and 2, 2030, each from 09:00 to 17:00 local time with minimum coverage one. Lock the first shift’s assigned person and record the second shift’s assigned person as unavailable on January 2. The third eligible person provides an alternative.

```csv
Name,Personnel key
Mica Vale,demo-mica
Robin Lake,demo-robin
Ari Green,demo-ari
```

| Task | What to observe |
| --- | --- |
| Create a Workforce project. Import a small people CSV with fictional names using the column mapping and preview. Add work, availability, one Required rest rule and one Preference. Validate, correct a reported problem, undo and redo. | The preview and changed records agree. The user can explain which rule may be traded off. Reopen the project and check persistence. |
| Optimize, open a verified result and inspect the assignments, coverage, score, Why this? and Why not? Try an impossible coverage requirement in a separate disposable copy. | Check the reported revision, proof or time-limit status and the explanation. An unproved outcome must remain distinguishable from infeasibility. |
| Keep a hard lock on one assignment, enter a call-out for someone else, then repair. Open Compare latest two results. | Verify the lock, changed assignments, original and repaired result identities, and the reason for the changes. Reopen History and confirm both results remain available. |
| Export an editable scenario and a full backup separately. Share the accepted result as HTML and PDF. Change a disclosure option and prepare again. Edit the source and reopen the original result. | Explain which output restores editable data. Check the private defaults against each saved report and the source revision against the stale-consent prompt. Open the HTML offline. |
| With the screen reader running, repeat the result-to-repair-to-share path using the keyboard. Cancel the picker and return to the trigger. | Record the words heard for result status, progress, headers, row identity and errors, and where focus returns. Record skipped or repeated announcements and any color-only distinction. |
| Using the larger synthetic practitioner report, close the application through the window manager during active PDF rendering. | Record evidence that the renderer was active before close. After exit check that its process has settled, the destination is absent or retains its previous bytes, and incomplete staging is gone. Reopen the disposable project and verify revision and result history. If rendering finished first, mark the attempt inconclusive. |

For the practitioner fixture, privacy census and native viewer checks, follow [the PDF protocol](phase07-pdf-validation.md#native-pdf-acceptance) and [mixed-direction selection and clipboard check](phase07-pdf-validation.md#mixed-direction-selection-and-clipboard-check). Record actual pasted text and spoken output separately for each viewer.

## Repeatable evidence

The [ordered work packages](../roadmap/07-workforce-solving-results-repair-and-export.md#ordered-work-packages) group the implementation and the evidence needed for exit.

| Scope | Where to check it |
| --- | --- |
| WF-004 through WF-007 | [Workforce core](../../domains/workforce/core/src/) compiles and independently evaluates Required rules, preferences, quotas, weekend limits and locks. Compare the [v2 corpus](../../benchmarks/corpus/workforce/v2.json) with its [expected outcomes](../../benchmarks/expected/workforce/v2.json). |
| UI-005, UI-006, EXPLAIN-001, REPAIR-001 | The [native desktop journey](../../apps/desktop/e2e/project-home.persistence.mjs) exercises solving, explanation, locking, repair and comparison. The packaged CLI smoke includes two real solves and compares the saved pair through both CLI forms. |
| Alternatives and EXPORT-001 | Use the Alternatives, practitioner export and PDF cancellation selectors in the [Justfile](../../Justfile). Check retained alternatives and verify accepted source, disclosure, cancellation and custody before judging the visible output. |
| Acceptance hardening | Use the public keyboard workflow, large result and report cases and retain benchmark distributions from `just bench`. Record normal-desktop lifecycle, native viewer, spoken feedback, calibrated reference-machine performance and participant observations independently. |

After `just worker-build-cli`, run `just worker-smoke-cli`. For a matching Linux desktop build, the focused public journeys include:

```sh
just worker-build-desktop
EUTHETO_E2E_KEYBOARD_WORKFLOW_ONLY=1 just e2e true false
EUTHETO_E2E_PRACTITIONER_EXPORT_ONLY=1 just e2e true false
EUTHETO_E2E_PDF_CANCELLATION_ONLY=1 just e2e true false
```

The last command exercises active helper cancellation. The window-manager close in the walkthrough exercises a different application exit path.

## Record the outcome

For each row record pass, fail or untested, expected and observed behavior, source/result revisions and a reproducible symptom. Capture only synthetic output. Record the participant's own interpretation of Required vs Preference, share vs backup and feasible vs optimal. Keep reference-machine timing and large-grid responsiveness measurements with their machine, fixture, cache state and raw distributions.

Zeb records his decision against the tested candidate after feedback is resolved. Record Workforce checkpoint acceptance and authorization to begin Phase 08 separately under the [manual checkpoint policy](../roadmap/README.md#user-manual-checkpoint-gates).

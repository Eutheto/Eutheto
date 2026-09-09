<!-- SPDX-License-Identifier: Apache-2.0 -->

# Expected benchmark results

[`workforce/v1.json`](workforce/v1.json) contains generated, reviewed correctness expectations for the synthetic Phase05 Workforce corpus. Each expectation binds an exact input digest and records original-domain dimensions, semantic obligations, expected disposition and applicable score/model bounds. Changing a fixture does not make its new outcome correct merely by regenerating the expected file; the semantic change requires review.

A performance baseline must also record the toolchain, target and eligible runner class, fixed clock/time zone/locale/seed/thread count/temp policy, warm-up and sampling method, metric, and review-approved threshold. Creating or replacing a baseline requires review of the measured and semantic difference; regenerating or accepting changed output is not sufficient.

These expectations are not performance baselines. The implemented runner records observations without Phase12 timing thresholds. Raw Phase03 backend evidence and independently accepted Workforce outcomes remain distinct. See the [fixture contract](../../docs/architecture/test-fixtures.md).

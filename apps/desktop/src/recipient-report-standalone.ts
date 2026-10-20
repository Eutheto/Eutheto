// SPDX-License-Identifier: Apache-2.0
import { isRecipientReportV1, MAX_RECIPIENT_HTML_BYTES } from "./api/generated-recipient-report";
import { mountRecipientReport } from "./recipient-report";
import "./recipient-report.css";

const root = document.getElementById("recipient-report");
if (root) {
  try {
    const data = document.getElementById("recipient-data");
    const encoded = data?.textContent;
    // Inert-container escaping can enlarge the canonical model; bound both representations.
    if (!encoded || encoded.length > MAX_RECIPIENT_HTML_BYTES) {
      throw new Error("Invalid recipient data size.");
    }
    const report: unknown = JSON.parse(encoded);
    if (!isRecipientReportV1(report)) throw new Error("Unsupported recipient data.");
    mountRecipientReport(root, report, () => {
      window.print();
    });
  } catch {
    delete root.dataset.recipientReady;
    root.dataset.recipientError = "true";
    const error = document.createElement("p");
    error.setAttribute("role", "alert");
    error.textContent =
      "This report is malformed, unsupported or exceeds its limits. No plan is displayed. Request a new export from its owner.";
    root.replaceChildren(error);
  }
}

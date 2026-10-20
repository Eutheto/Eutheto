import { cleanup, render, screen } from "@testing-library/vue";
import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { defineComponent, h, onScopeDispose } from "vue";
import { afterEach, expect, it, vi } from "vitest";
import { userEvent } from "vitest/browser";
import * as api from "../api/generated";
import { createProjectHomeController, type ProjectHomeController } from "../project-home";
import { fakeApi, portableOperation, project, response } from "../testing/project-home";
import fixture from "../../../../crates/eutheto-pdf-worker/tests/fixtures/report.json";
import type { RecipientReportV1 } from "../api/generated-recipient-report";
import { messages } from "../messages";
import RecipientShare from "./RecipientShare.vue";
import "../styles.css";

vi.mock("../api/generated", { spy: true });
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("retires a PDF review on format change and publishes only the newly reviewed PDF", async () => {
  const report = fixture as RecipientReportV1;
  const source = report.source;
  const homeApi = fakeApi([{ ...project, scenarioId: source.scenarioId, revision: 2 }]);
  const previewId = "01900000-0000-7000-8000-000000000070";
  const released = vi
    .spyOn(api.SetupOperationScope.prototype, "releaseRecipientReport")
    .mockResolvedValue(response({ schemaVersion: 1, acknowledgement: "released" }));
  const preview = vi
    .spyOn(api.SetupOperationScope.prototype, "previewRecipientReport")
    .mockImplementation((_solution, options, _progress, format) =>
      portableOperation(
        response({
          schemaVersion: 1,
          previewId,
          format: format ?? "html",
          libraryRevision: 1,
          modelSha256: "a".repeat(64),
          htmlSha256: "b".repeat(64),
          pdfSha256: "c".repeat(64),
          pdfRenderer: {
            version: 1,
            typstVersion: "0.15.1",
            templateSha256: "d".repeat(64),
            fontsSha256: "e".repeat(64),
          },
          expiresAfterMilliseconds: 600000,
          report: { ...report, privacy: options },
        }),
      ),
    );
  const create = vi
    .spyOn(api.SetupOperationScope.prototype, "createRecipientReport")
    .mockImplementation(() =>
      portableOperation(
        response({
          schemaVersion: 1,
          previewId,
          format: "pdf",
          filename: "report.pdf",
          publication: {
            schemaVersion: 1,
            solutionId: source.solutionId,
            sourceRevision: 1,
            currentRevision: 2,
            modelSha256: "a".repeat(64),
            pdfSha256: "c".repeat(64),
          },
        }),
      ),
    );
  let home!: ProjectHomeController;
  render(
    defineComponent({
      setup() {
        home = createProjectHomeController(homeApi);
        onScopeDispose(() => {
          void home.dispose();
        });
        return () =>
          h(RecipientShare, {
            home,
            scenarioId: source.scenarioId,
            currentRevision: 2,
            summary: {
              result: {
                solutionId: source.solutionId,
                resultChecksum: source.acceptedResultChecksum,
              },
              scenarioRevision: 1,
              verificationChecksum: source.verificationReportChecksum,
            },
          });
      },
    }),
    { global: { plugins: [createPinia(), PiniaColada] } },
  );
  await home.load();
  await userEvent.click(screen.getByRole("button", { name: messages.recipientShare.trigger }));
  // The full renderer fixture includes these disclosed sections and authored labels.
  for (const name of Object.values(messages.recipientShare.options)) {
    await userEvent.click(screen.getByRole("checkbox", { name }));
  }
  const selector = screen.getByRole("combobox", { name: messages.recipientShare.format });
  await userEvent.selectOptions(selector, "pdf");
  await userEvent.click(screen.getByRole("button", { name: messages.recipientShare.preview }));
  await expect
    .poll(() => screen.queryByRole("button", { name: messages.recipientShare.savePdf }))
    .not.toBeNull();
  expect(preview.mock.calls[0]?.[3]).toBe("pdf");
  await userEvent.selectOptions(selector, "html");
  await expect.poll(() => released.mock.calls.length).toBeGreaterThan(0);
  expect(screen.queryByRole("button", { name: messages.recipientShare.savePdf })).toBeNull();
  expect(create).not.toHaveBeenCalled();
  await userEvent.selectOptions(selector, "pdf");
  await userEvent.click(screen.getByRole("button", { name: messages.recipientShare.preview }));
  await expect
    .poll(() => screen.queryByRole("button", { name: messages.recipientShare.savePdf }))
    .not.toBeNull();
  await userEvent.click(screen.getByRole("checkbox", { name: messages.recipientShare.consent }));
  await userEvent.click(screen.getByRole("button", { name: messages.recipientShare.savePdf }));
  await expect.poll(() => create.mock.calls.length).toBe(1);
  expect(create.mock.calls[0]?.[3]).toBe("pdf");
  await expect
    .poll(() => screen.queryByText(messages.recipientShare.saved("report.pdf")))
    .not.toBeNull();
});

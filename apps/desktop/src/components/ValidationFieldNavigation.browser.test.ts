import { cleanup, render, screen, within } from "@testing-library/vue";
import { afterEach, describe, expect, it } from "vitest";
import { defineComponent, h, nextTick, shallowRef, type VNode } from "vue";
import { createAvailabilityDraft, availabilityWeeklyDraft } from "../availability-draft";
import { messages } from "../messages";
import { createPeopleRecordDraft } from "../people-record-draft";
import { project } from "../testing/project-home";
import PeopleRecordFields from "./PeopleRecordFields.vue";
import AvailabilityFields from "./availability/AvailabilityFields.vue";
import WorkRecordFields from "./WorkRecordFields.vue";
import { createWorkRecordDraft } from "../work-record-draft";
import { parseTemporalDraft } from "./planner/temporal-field";
import "../styles.css";

afterEach(cleanup);
const personId = "01900000-0000-7000-8000-000000000001";

function renderFields(view: () => VNode): void {
  render(defineComponent({ setup: () => view }));
}

describe("native diagnostic field focus", () => {
  it("opens the authored grant page and exact expiry field without changing raw values", async () => {
    const draft = createPeopleRecordDraft("person");
    if (draft.kind !== "person") throw new Error("Person draft expected");
    const raw = {
      ...draft,
      fields: {
        ...draft.fields,
        qualificationGrants: Array.from({ length: 101 }, (_, index) => ({
          key: `grant-${String(index)}`,
          qualificationId: "",
          effectiveFrom: "2026-11-01T08:00:00Z",
          expiresAt: index === 75 ? "2026-11-01T07:00:00Z" : "",
        })),
      },
    };
    const fields = shallowRef<InstanceType<typeof PeopleRecordFields>>();
    renderFields(() =>
      h(PeopleRecordFields, {
        ref: fields,
        modelValue: raw,
        project,
        libraryEpoch: 1,
      }),
    );
    await nextTick();
    const editor = fields.value;
    if (editor === undefined) throw new Error("Fields did not mount");
    expect(await editor.focusField(["qualificationGrants", "75", "expiresAt"], () => true)).toBe(
      true,
    );
    const grant = screen.getByRole("group", { name: messages.personFields.grantNumber(76) });
    const expiry = within(grant).getByLabelText(messages.personFields.expiresAt);
    expect(document.activeElement).toBe(expiry);
    expect((expiry as HTMLInputElement).value).toBe("2026-11-01T07:00:00Z");
    expect(raw.fields.qualificationGrants[75]?.expiresAt).toBe("2026-11-01T07:00:00Z");
  });

  it("focuses the second authored weekly end, not the first row or its start", async () => {
    const raw = {
      ...createAvailabilityDraft("new", personId),
      windowKind: "weekly" as const,
      weekly: [0, 1].map(() =>
        availabilityWeeklyDraft({
          weekdays: ["sunday"],
          startTime: "01:30:00",
          endTime: "02:30:00",
          endDayOffset: 0,
        }),
      ),
    };
    const fields = shallowRef<InstanceType<typeof AvailabilityFields>>();
    renderFields(() =>
      h(AvailabilityFields, {
        ref: fields,
        draft: raw,
        project,
        libraryEpoch: 1,
        timeZone: "America/New_York",
        requestKey: "native-revision-7",
        errors: {},
      }),
    );
    await nextTick();
    const editor = fields.value;
    if (editor === undefined) throw new Error("Fields did not mount");
    expect(await editor.focusField(["timeWindow", "windows", "1", "endTime"], () => true)).toBe(
      true,
    );
    const second = raw.weekly[1];
    if (second === undefined) throw new Error("Second authored weekly row expected");
    expect(document.activeElement?.id).toBe(`${second.key}-end`);
    expect((document.activeElement as HTMLInputElement).value).toBe("02:30:00");
  });

  it("opens the authored coverage minimum page rather than treating its offset as a page", async () => {
    const draft = createWorkRecordDraft("coverageRequirement");
    if (draft.kind !== "coverageRequirement") throw new Error("Coverage requirement expected");
    const raw = {
      ...draft,
      coverage: {
        ...draft.coverage,
        kind: "exact" as const,
        count: "1",
        qualificationMinimums: Array.from({ length: 101 }, (_, index) => ({
          key: `minimum-${String(index)}`,
          minimum: String(index + 1),
          allQualificationIds: [],
          anyQualificationIds: [],
        })),
      },
    };
    const fields = shallowRef<InstanceType<typeof WorkRecordFields>>();
    renderFields(() =>
      h(WorkRecordFields, {
        ref: fields,
        modelValue: raw,
        project,
        libraryEpoch: 1,
        timeZone: "UTC",
        requestKey: "coverage-revision",
      }),
    );
    await nextTick();
    const editor = fields.value;
    if (editor === undefined) throw new Error("Fields did not mount");
    expect(
      await editor.focusField(["coverage", "qualificationMinimums", "75", "minimum"], () => true),
    ).toBe(true);
    const group = screen.getByRole("group", {
      name: `${messages.work.fields.qualificationMinimum} 76`,
    });
    expect(document.activeElement).toBe(
      within(group).getByLabelText(messages.work.fields.qualificationMinimum),
    );
    expect((document.activeElement as HTMLInputElement).value).toBe("76");
    expect(raw.coverage.qualificationMinimums[75]?.minimum).toBe("76");
  });

  it("focuses the native instance end-local control instead of its start", async () => {
    const draft = createWorkRecordDraft("shiftInstance");
    if (draft.kind !== "shiftInstance") throw new Error("Shift instance expected");
    const raw = {
      ...draft,
      interval: parseTemporalDraft({
        kind: "localInterval",
        startsAt: "2030-01-01T08:00:00",
        endsAt: "2030-01-01T17:00:00",
      }),
    };
    const fields = shallowRef<InstanceType<typeof WorkRecordFields>>();
    renderFields(() =>
      h(WorkRecordFields, {
        ref: fields,
        modelValue: raw,
        project,
        libraryEpoch: 1,
        timeZone: "UTC",
        requestKey: "instance-revision",
      }),
    );
    await nextTick();
    const editor = fields.value;
    if (editor === undefined) throw new Error("Fields did not mount");
    expect(await editor.focusField(["endsAt", "local"], () => true)).toBe(true);
    expect(document.activeElement?.id.endsWith("-interval-end")).toBe(true);
    expect((document.activeElement as HTMLInputElement).value).toBe("2030-01-01T17:00:00");
    expect(await editor.focusField(["endsAt", "offsetSeconds"], () => true)).toBe(true);
    expect(document.activeElement?.id.endsWith("-interval-end")).toBe(true);
    expect(await editor.focusField(["startsAt", "offsetSeconds"], () => true)).toBe(true);
    expect(document.activeElement?.id.endsWith("-interval-start")).toBe(true);
  });

  it("focuses the timing group for an endpoint-less native interval finding", async () => {
    const draft = createWorkRecordDraft("shiftTemplate");
    if (draft.kind !== "shiftTemplate") throw new Error("Template draft expected");
    const raw = { ...draft, timingMode: "localWindow" as const };
    const fields = shallowRef<InstanceType<typeof WorkRecordFields>>();
    renderFields(() =>
      h(WorkRecordFields, {
        ref: fields,
        modelValue: raw,
        project,
        libraryEpoch: 1,
        timeZone: "UTC",
        requestKey: "timing-revision",
      }),
    );
    await nextTick();
    const editor = fields.value;
    if (editor === undefined) throw new Error("Fields did not mount");
    expect(await editor.focusField(["timing"], () => true)).toBe(true);
    expect(document.activeElement?.tagName).toBe("FIELDSET");
    expect(document.activeElement?.id.endsWith("-timing-group")).toBe(true);
    expect(
      document.activeElement?.contains(screen.getByLabelText(messages.work.fields.timing)),
    ).toBe(true);
  });

  it("does not steal focus when ownership changes during the render await", async () => {
    const raw = createPeopleRecordDraft("person");
    const fields = shallowRef<InstanceType<typeof PeopleRecordFields>>();
    renderFields(() =>
      h("div", [
        h("button", { type: "button" }, "Current navigation"),
        h(PeopleRecordFields, { ref: fields, modelValue: raw, project, libraryEpoch: 1 }),
      ]),
    );
    await nextTick();
    const current = screen.getByRole("button", { name: "Current navigation" });
    current.focus();
    const editor = fields.value;
    if (editor === undefined) throw new Error("Fields did not mount");
    let ownsTarget = true;
    const focusing = editor.focusField(["name"], () => ownsTarget);
    ownsTarget = false;
    expect(await focusing).toBe(false);
    expect(document.activeElement).toBe(current);
  });
});

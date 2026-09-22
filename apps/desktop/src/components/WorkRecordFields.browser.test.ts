import { cleanup, render, screen, within } from "@testing-library/vue";
import { afterEach, describe, expect, it } from "vitest";
import { userEvent } from "vitest/browser";
import { h, nextTick, shallowRef } from "vue";
import { messages } from "../messages";
import { project } from "../testing/project-home";
import { createWorkRecordDraft, type WorkRecordDraft } from "../work-record-draft";
import WorkRecordFields from "./WorkRecordFields.vue";
import { parseDurationDraft } from "./planner/duration-field";
import { plannerMessage } from "./planner/messages";
import { parseTemporalDraft } from "./planner/temporal-field";
import "../styles.css";

afterEach(cleanup);
const copy = messages.work.fields;

function location(): WorkRecordDraft {
  return {
    kind: "location",
    name: "Travel",
    transitions: Array.from({ length: 101 }, (_, index) => ({
      key: `transition-${String(index)}`,
      locationId: "",
      minutes: parseDurationDraft(index === 75 ? "2e" : "0001", "minutes", 0),
    })),
  };
}
function calendar(): WorkRecordDraft {
  const draft = createWorkRecordDraft("calendar");
  if (draft.kind !== "calendar") throw new Error("Calendar draft expected");
  return {
    ...draft,
    name: "Custom calendar",
    period: {
      ...draft.period,
      kind: "custom",
      payPeriod: { anchorDate: "2026-0", startTime: "08:", lengthDays: "2e" },
      custom: Array.from({ length: 101 }, (_, index) => ({
        key: `interval-${String(index)}`,
        interval: parseTemporalDraft({
          kind: "localInterval",
          startsAt: index === 75 ? "2026-0" : "2026-09-01T08:00",
          endsAt: "2026-09-01T12:00",
        }),
      })),
    },
  };
}
function template(collection: "tags" | "excludedDates" | "qualificationMinimums"): WorkRecordDraft {
  const draft = createWorkRecordDraft("shiftTemplate");
  if (draft.kind !== "shiftTemplate") throw new Error("Template draft expected");
  const textRows = Array.from({ length: 101 }, (_, index) => ({
    key: `text-${String(index)}`,
    value: index === 75 ? "" : `  retained ${String(index)}  `,
  }));
  return {
    ...draft,
    name: "Recurring work",
    elapsedStartTime: "08:",
    tags: collection === "tags" ? textRows : draft.tags,
    recurrence: {
      ...draft.recurrence,
      excludedDates: collection === "excludedDates" ? textRows : [],
    },
    coverage: {
      ...draft.coverage,
      preferredCount: "2e",
      qualificationMinimums:
        collection === "qualificationMinimums"
          ? Array.from({ length: 101 }, (_, index) => ({
              key: `minimum-${String(index)}`,
              minimum: index === 75 ? "2e" : "0001",
              allQualificationIds: [],
              anyQualificationIds: [],
            }))
          : [],
    },
  };
}

const collections = [
  {
    name: "transitions",
    make: location,
    label: copy.transitions,
    input: copy.transitionMinutes,
    path: "transitions.75.minutes",
    raw: "2e",
    corrected: "0007",
  },
  {
    name: "custom intervals",
    make: calendar,
    label: copy.customIntervals,
    input: plannerMessage("temporal.startsAt"),
    path: "period.intervals.75.startsAt",
    raw: "2026-0",
    corrected: "2026-09-02T08:00",
  },
  {
    name: "tags",
    make: () => template("tags"),
    label: copy.tags,
    input: /^Tags \d+$/u,
    path: "tags.75",
    raw: "",
    corrected: "  corrected tag  ",
  },
  {
    name: "exclusions",
    make: () => template("excludedDates"),
    label: copy.excludedDates,
    input: /^Excluded occurrence dates \d+$/u,
    path: "recurrence.excludedDates.75",
    raw: "",
    corrected: "2026-09-02",
  },
  {
    name: "qualification minima",
    make: () => template("qualificationMinimums"),
    label: copy.qualificationMinimums,
    input: copy.qualificationMinimum,
    path: "coverage.qualificationMinimums.75.minimum",
    raw: "2e",
    corrected: "0007",
  },
];

function mountFields(initial: WorkRecordDraft) {
  const draft = shallowRef(initial);
  const errors = shallowRef<Readonly<Record<string, string>>>({});
  const { container } = render({
    render: () =>
      h(WorkRecordFields, {
        project,
        libraryEpoch: 1,
        requestKey: "collection-recovery",
        timeZone: "Europe/Paris",
        locale: "en-US",
        modelValue: draft.value,
        errors: errors.value,
        "onUpdate:modelValue": (value: WorkRecordDraft) => {
          draft.value = value;
        },
      }),
  });
  return { draft, errors, container };
}

describe("work collection error recovery", () => {
  it.each(collections)(
    "reveals newly reported off-page $name without taking over correction or browsing",
    async (collection) => {
      const initial = collection.make();
      const { draft, errors, container } = mountFields(initial);
      const group = screen.getByRole("group", { name: collection.label });
      const inputs = () => within(group).getAllByRole("textbox", { name: collection.input });
      const pager = screen.getByRole("navigation", {
        name: `${collection.label}: ${copy.collectionPages}`,
      });
      expect(inputs()).toHaveLength(50);

      // Exercise the report-to-nextTick seam consumed by WorkSetup's existing focusInvalid.
      // This supplies field feedback only; no native review or approval is simulated.
      errors.value = { [collection.path]: "Correct this field before review." };
      await nextTick();
      const invalid = container.querySelector<HTMLElement>('[aria-invalid="true"]');
      if (invalid === null) throw new Error("The reported field must be mounted");
      expect(invalid).toBe(inputs()[25]);
      invalid.focus();
      await expect.element(invalid).toHaveFocus();
      await expect.element(invalid).toHaveValue(collection.raw);
      expect(inputs()).toHaveLength(50);
      expect(draft.value).toEqual(initial);

      await userEvent.fill(invalid, collection.corrected);
      await expect.element(invalid).toHaveFocus();
      await expect.element(invalid).toHaveValue(collection.corrected);
      const corrected = draft.value;

      await userEvent.click(within(pager).getByRole("button", { name: copy.first }));
      const firstInput = group.querySelector<HTMLInputElement>("input");
      await expect.element(firstInput).toHaveFocus();
      // A fresh object containing already-reported paths must not undo deliberate paging.
      errors.value = { ...errors.value };
      await nextTick();
      await expect.element(firstInput).toHaveFocus();
      expect(inputs()).toHaveLength(50);

      // Enter on Next used to leave focus on the newly disabled last-page button.
      const next = within(pager).getByRole("button", { name: copy.next });
      next.focus();
      await userEvent.keyboard("{Enter}");
      await expect.element(group.querySelector("input")).toHaveFocus();
      await expect.element(inputs()[25] ?? null).toHaveValue(collection.corrected);
      next.focus();
      await userEvent.keyboard("{Enter}");
      await expect.element(next).toBeDisabled();
      await expect.element(group.querySelector("input")).toHaveFocus();
      expect(inputs()).toHaveLength(1);
      expect(draft.value).toEqual(corrected);
    },
  );
});

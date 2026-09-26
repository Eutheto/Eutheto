import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  OperationPresentationScheduler,
  operationPresentationText,
  type OperationPresentationCallbacks,
} from "./operation-presentation";
import type { WorkspaceOperationState } from "./project-home";

function operation(): WorkspaceOperationState {
  return {
    label: "Saving",
    cancel: null,
    phase: null,
    cancellationRequested: false,
    settled: false,
    refreshingLibrary: false,
  };
}

function callbacks() {
  const displayed: object[] = [];
  const hidden = vi.fn();
  const announcements: string[] = [];
  const value: OperationPresentationCallbacks = {
    onDisplay: (current) => displayed.push(current),
    onHide: hidden,
    onAnnouncement: (text) => announcements.push(text),
  };
  return { value, displayed, hidden, announcements };
}

describe("operation presentation timing", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("delays each operation identity and cannot display a replaced operation", () => {
    const first = operation();
    const second = operation();
    const state = callbacks();
    const scheduler = new OperationPresentationScheduler(state.value);

    scheduler.replace(first);
    vi.advanceTimersByTime(399);
    expect(state.displayed).toEqual([]);

    scheduler.replace(second);
    vi.advanceTimersByTime(1);
    expect(state.displayed).toEqual([]);
    vi.advanceTimersByTime(399);
    expect(state.displayed).toEqual([second]);
    scheduler.dispose();
  });

  it("coalesces ordinary phase updates to one trailing announcement per second", () => {
    const current = operation();
    const state = callbacks();
    const scheduler = new OperationPresentationScheduler(state.value);

    scheduler.replace(current);
    vi.advanceTimersByTime(400);
    scheduler.announce(current, "waiting");
    vi.advanceTimersByTime(200);
    scheduler.announce(current, "reading");
    scheduler.announce(current, "validating");
    vi.advanceTimersByTime(799);
    expect(state.announcements).toEqual(["waiting"]);
    vi.advanceTimersByTime(1);
    expect(state.announcements).toEqual(["waiting", "validating"]);
    scheduler.dispose();
  });

  it("drops stale trailing phases when the latest phase is already announced", () => {
    const current = operation();
    const state = callbacks();
    const scheduler = new OperationPresentationScheduler(state.value);

    scheduler.replace(current);
    vi.advanceTimersByTime(400);
    scheduler.announce(current, "waiting");
    vi.advanceTimersByTime(200);
    scheduler.announce(current, "reading");
    scheduler.announce(current, "waiting");
    vi.advanceTimersByTime(1_000);
    expect(state.announcements).toEqual(["waiting"]);
    scheduler.dispose();
  });

  it("announces cancellation and settled refresh immediately and clears stale trailing work", () => {
    const current = operation();
    const state = callbacks();
    const scheduler = new OperationPresentationScheduler(state.value);

    scheduler.replace(current);
    vi.advanceTimersByTime(400);
    scheduler.announce(current, "waiting");
    vi.advanceTimersByTime(300);
    scheduler.announce(current, "reading");
    scheduler.announce(current, "cancelling", true);
    expect(state.announcements).toEqual(["waiting", "cancelling"]);
    vi.advanceTimersByTime(1_000);
    expect(state.announcements).toEqual(["waiting", "cancelling"]);

    scheduler.announce(current, "refreshing", true);
    expect(state.announcements).toEqual(["waiting", "cancelling", "refreshing"]);
    scheduler.dispose();
  });

  it("does not resurrect display or announcements after disposal", () => {
    const current = operation();
    const state = callbacks();
    const scheduler = new OperationPresentationScheduler(state.value);

    scheduler.replace(current);
    scheduler.dispose();
    vi.advanceTimersByTime(2_000);
    expect(state.displayed).toEqual([]);
    expect(state.announcements).toEqual([]);
    expect(state.hidden).toHaveBeenCalled();
  });
});

describe("operation presentation text", () => {
  it("announces only an actual library refresh after native settlement", () => {
    const labels = {
      pending: "pending",
      refreshing: "refreshing",
      cancelling: "cancelling",
      phases: {
        waitingForAdmission: "waiting",
        capturingSnapshot: "capturing",
        buildingView: "building",
        applyingPreview: "applying",
        validating: "validating",
        preparingResponse: "preparing",
        selectingFile: "selecting",
        detectingFormat: "detecting",
        publishingReport: "reporting",
        publishingFile: "publishing",
      },
    } as const;
    const current = operation();
    current.phase = "validating";
    expect(operationPresentationText(current, labels)).toBe("validating");
    current.cancellationRequested = true;
    expect(operationPresentationText(current, labels)).toBe("cancelling");
    current.settled = true;
    expect(operationPresentationText(current, labels)).toBe("validating");
    current.refreshingLibrary = true;
    expect(operationPresentationText(current, labels)).toBe("refreshing");
  });
});

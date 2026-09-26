import type { OperationPhaseV1 } from "./api/generated";
import type { WorkspaceOperationState } from "./project-home";

export const OPERATION_DISPLAY_DELAY_MS = 400;
export const OPERATION_ANNOUNCEMENT_INTERVAL_MS = 1_000;

export interface OperationPresentationLabels {
  readonly pending: string;
  readonly refreshing: string;
  readonly cancelling: string;
  readonly phases: Readonly<Record<OperationPhaseV1, string>>;
}

export function operationPresentationText(
  operation: WorkspaceOperationState,
  labels: OperationPresentationLabels,
): string {
  if (operation.refreshingLibrary) return labels.refreshing;
  if (!operation.settled && operation.cancellationRequested) return labels.cancelling;
  return operation.phase === null ? labels.pending : labels.phases[operation.phase];
}

export function operationPresentationAnnouncementIsUrgent(
  operation: WorkspaceOperationState,
): boolean {
  return operation.cancellationRequested || operation.settled;
}

type OperationPresentationTimer = number | NodeJS.Timeout;

export interface OperationPresentationCallbacks {
  readonly onDisplay: (operation: WorkspaceOperationState) => void;
  readonly onHide: () => void;
  readonly onAnnouncement: (text: string) => void;
}

/**
 * Owns the perceptual timing boundary without owning operation state or native custody.
 * Identity checks make a late display/announcement callback harmless after replacement.
 */
export class OperationPresentationScheduler {
  private readonly callbacks: OperationPresentationCallbacks;
  private active: WorkspaceOperationState | null = null;
  private displayed = false;
  private displayTimer: OperationPresentationTimer | null = null;
  private announcementTimer: OperationPresentationTimer | null = null;
  private pendingAnnouncement: string | null = null;
  private lastAnnouncement: string | null = null;
  private lastAnnouncementAt: number | null = null;

  constructor(callbacks: OperationPresentationCallbacks) {
    this.callbacks = callbacks;
  }

  replace(operation: WorkspaceOperationState | null): void {
    this.clearDisplayTimer();
    this.clearAnnouncementTimer();
    this.active = operation;
    this.displayed = false;
    this.pendingAnnouncement = null;
    this.lastAnnouncement = null;
    this.lastAnnouncementAt = null;
    this.callbacks.onHide();
    if (operation === null) return;

    this.displayTimer = setTimeout(() => {
      this.displayTimer = null;
      if (this.active !== operation) return;
      this.displayed = true;
      this.callbacks.onDisplay(operation);
    }, OPERATION_DISPLAY_DELAY_MS);
  }

  announce(operation: WorkspaceOperationState, text: string, urgent = false): void {
    if (this.active !== operation || !this.displayed || text === "") return;
    if (urgent) {
      this.clearAnnouncementTimer();
      this.pendingAnnouncement = null;
      if (text !== this.lastAnnouncement) this.emit(text);
      return;
    }
    if (text === this.lastAnnouncement) {
      this.clearAnnouncementTimer();
      this.pendingAnnouncement = null;
      return;
    }
    if (text === this.pendingAnnouncement) return;

    const elapsed =
      this.lastAnnouncementAt === null
        ? OPERATION_ANNOUNCEMENT_INTERVAL_MS
        : performance.now() - this.lastAnnouncementAt;
    if (this.lastAnnouncementAt === null || elapsed >= OPERATION_ANNOUNCEMENT_INTERVAL_MS) {
      this.clearAnnouncementTimer();
      this.pendingAnnouncement = null;
      this.emit(text);
      return;
    }

    this.pendingAnnouncement = text;
    if (this.announcementTimer === null) {
      this.announcementTimer = setTimeout(
        () => {
          this.flushPendingAnnouncement(operation);
        },
        Math.max(0, OPERATION_ANNOUNCEMENT_INTERVAL_MS - elapsed),
      );
    }
  }

  dispose(): void {
    this.clearDisplayTimer();
    this.clearAnnouncementTimer();
    this.active = null;
    this.displayed = false;
    this.pendingAnnouncement = null;
    this.lastAnnouncement = null;
    this.lastAnnouncementAt = null;
    this.callbacks.onHide();
  }

  private flushPendingAnnouncement(operation: WorkspaceOperationState): void {
    this.announcementTimer = null;
    if (this.active !== operation || !this.displayed) {
      this.pendingAnnouncement = null;
      return;
    }
    const pending = this.pendingAnnouncement;
    this.pendingAnnouncement = null;
    if (pending) this.emit(pending);
  }

  private emit(text: string): void {
    this.lastAnnouncement = text;
    this.lastAnnouncementAt = performance.now();
    this.callbacks.onAnnouncement(text);
  }

  private clearDisplayTimer(): void {
    if (this.displayTimer !== null) clearTimeout(this.displayTimer);
    this.displayTimer = null;
  }

  private clearAnnouncementTimer(): void {
    if (this.announcementTimer !== null) clearTimeout(this.announcementTimer);
    this.announcementTimer = null;
  }
}

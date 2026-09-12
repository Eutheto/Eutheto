export function formatNumber(value: number, locale?: string): string {
  return new Intl.NumberFormat(locale).format(value);
}

export function formatDateTime(value: string, locale?: string, timeZone?: string): string {
  const instant = new Date(value);
  if (Number.isNaN(instant.getTime())) return value;
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
    ...(timeZone === undefined ? {} : { timeZone }),
  }).format(instant);
}

export function formatUnit(value: number, unit: string, locale?: string): string {
  return new Intl.NumberFormat(locale, {
    style: "unit",
    unit,
    unitDisplay: "long",
  }).format(value);
}

export const messages = {
  app: {
    eyebrow: "Local-first workspace",
    title: "Eutheto",
    lede: "Plan carefully. Keep the authoritative work on your machine.",
    boundary:
      "This development desktop manages projects and portable files. Workforce scheduling is available through the CLI; solver features and the .eutheto extension remain provisional.",
    noSelection: "No project selected",
    selectedProject: (title: string) => `Selected project: ${title}`,
  },
  operations: {
    pending: "Working with the saved project",
    busy: "Wait for the current operation to settle before starting another.",
    closed: "This workspace has closed.",
    cancel: "Cancel operation",
    cancelling: "Cancellation requested. Waiting for the native operation to settle…",
    cancelled: "Operation cancelled. Previously saved changes and safety backups remain available.",
    refreshing: "Refreshing the saved library…",
    conflictReloaded: "The saved library changed. Review the refreshed state before trying again.",
    conflictRefreshFailed: "The saved library changed. Refresh it before trying the change again.",
    cleanupFailed:
      "Native review cleanup could not be confirmed. Retry cleanup to release any retained resources.",
    retryCleanup: "Retry review cleanup",
    retryingCleanup: "Releasing retained native reviews…",
    phases: {
      waitingForAdmission: "Waiting for native admission",
      capturingSnapshot: "Reading a saved snapshot",
      buildingView: "Preparing the review",
      applyingPreview: "Applying the reviewed choices",
      validating: "Validating",
      preparingResponse: "Preparing the response",
      selectingFile: "Waiting for the native file chooser",
      detectingFormat: "Inspecting the selected file",
      publishingReport: "Saving the report",
      publishingFile: "Saving the file",
    },
  },
  navigation: {
    leaveTitle: "Leave this view?",
    dirtyDescription: "Your unsubmitted changes will be discarded. Saved changes remain intact.",
    pendingDescription:
      "Native work is still running. You can request cancellation and leave; the workspace will report its actual outcome when it settles.",
    waitingDescription:
      "This operation cannot be interrupted. Wait for it to settle before leaving.",
    settledDescription: "The operation has settled. Saved changes will not be undone by leaving.",
    stay: "Stay here",
    discardAndLeave: "Discard and leave",
    cancelAndLeave: "Cancel and leave",
    leave: "Leave view",
  },
  projects: {
    library: "Local project library",
    heading: "Projects",
    count: (count: number) => `${formatNumber(count)} ${count === 1 ? "project" : "projects"}`,
    loading: "Loading saved projects",
    loadingDescription: "Reading the authoritative local library…",
    loadFailed: "Projects could not be loaded",
    retry: "Try again",
    requestFailed: "Request not completed.",
    emptyHeading: "Begin with a local project",
    active: "Active",
    archived: "Archived",
    noActive: "No active projects. Archived work remains available below.",
    open: (title: string) => `Open project ${title}`,
    openArchived: (title: string) => `Open archived project ${title}`,
    savedMetadata: "Saved metadata",
    domainPack: "Domain pack",
    revision: "Revision",
    lastSaved: "Last saved",
    id: "Project ID",
    archive: "Archive project",
    unarchive: "Unarchive project",
    delete: "Delete project",
    duplicateAs: "Duplicate project as",
    copyTitle: "Copy title",
    duplicate: "Duplicate",
    newProject: "New project",
    createIntroduction: "Choose explicit scenario settings. They are saved with the project.",
    title: "Project title",
    description: "Description (optional)",
    timeZone: "Time zone",
    locale: "Locale",
    displayUnits: "Display units",
    metric: "Metric",
    usCustomary: "US customary",
    planningStarts: "First planning date",
    planningEnds: "Last planning date (included)",
    planningDatesHelp: "Both dates are included, using midnight in the project's time zone.",
    missingClockTime: "Missing clock time",
    repeatedClockTime: "Repeated clock time",
    reject: "Reject",
    moveForward: "Move forward",
    packPolicy: "Use domain pack policy",
    earlier: "Earlier",
    later: "Later",
    creating: "Creating…",
    create: "Create project",
  },
  deletion: {
    title: (title: string) => `Permanently delete ${title}?`,
    description: "This removes the saved project and its history. This action cannot be undone.",
    keep: "Keep project",
    confirm: "Delete permanently",
    pending: "Deleting project… Please wait for the operation to finish.",
  },
  restore: {
    review: "Review and confirm restore",
    replaceTitle: "Confirm library replacement",
    addTitle: "Confirm backup restore",
    replaceDescription:
      "Eutheto will create a safety backup, then replace the current library with the reviewed backup.",
    addDescription:
      "Eutheto will add the reviewed projects and apply the collision choices in the preview.",
    bypassDescription:
      "You are requesting replacement of the current library without a safety backup after reviewing the failure.",
    back: "Go back",
    confirm: "Confirm restore",
    previewExpired:
      "This preview is no longer available. Go back and choose the backup again to review a fresh preview.",
    pending: "Restoring backup… Please wait for the operation to finish.",
    safetyBackupFailed: "Safety backup failed:",
    bypassLabel: "Continue without a safety backup",
    bypassHelpBefore: "After reviewing the failure, enter",
    bypassHelpAfter: "exactly to make a second request for this same preview.",
  },
} as const;

import { vi, type Mock } from "vitest";
import type { FixedExclusion, ValidationIssue } from "../api/generated";
import type { ProjectHomeApi, ProjectSummary } from "../project-home";

export type ProjectHomeApiMocks = {
  [Method in keyof ProjectHomeApi]: ProjectHomeApi[Method] extends (
    ...args: infer Arguments
  ) => infer Result
    ? Mock<(...args: Arguments) => Result>
    : never;
};
export const project: ProjectSummary = {
  scenarioId: "01900000-0000-7000-8000-000000000001",
  title: "Clinic roster",
  domainPackId: "official.test",
  revision: 3,
  updatedAt: "2026-08-29T12:00:00Z",
  archived: false,
};
export const previewWarning: ValidationIssue = {
  code: "portable.preview.warning",
  severity: "warning",
  message: "Review migrated portable data.",
  fieldPath: null,
  resource: null,
};
export const fixedExclusions = [
  "local-undo-and-audit-history",
  "sqlite-and-database-internals",
  "credentials-tokens-and-keychain-references",
  "device-local-paths-and-window-state",
  "logs-caches-and-temporary-data",
  "redistribution-prohibited-provider-data",
  "executable-content",
] as const satisfies readonly FixedExclusion[];

export function response<T>(result: T, warnings: readonly ValidationIssue[] = []) {
  return {
    schemaVersion: 1 as const,
    requestId: "01900000-0000-7000-8000-000000000099",
    currentRevision: null,
    warnings,
    result,
  };
}

export function fakeApi(projects: ProjectSummary[] = []): ProjectHomeApiMocks {
  return {
    listProjects: vi.fn(() => Promise.resolve(response([...projects]))),
    createProject: vi.fn(() => Promise.resolve(response({}))),
    duplicateProject: vi.fn(() => Promise.resolve(response({}))),
    setProjectArchived: vi.fn(() => Promise.resolve(response({}))),
    deleteProject: vi.fn(() => Promise.resolve(response({}))),
    previewImport: vi.fn(() => Promise.resolve(response(portablePreview("scenario-export")))),
    applyImport: vi.fn(() => Promise.resolve(response({}))),
    previewBackup: vi.fn((title) =>
      Promise.resolve(
        response({
          title,
          byteLength: 4096,
          previewId: "01900000-0000-7000-8000-000000000070",
          digest: "b".repeat(64),
          currentRevision: null,
          libraryRevision: 1,
          backupSummary: {
            includeResults: true,
            assetSelection: "all" as const,
            excludedAssetCount: 1,
            excludedAssetIds: ["inherited-placeholder.png"],
            exclusionScope: "inherited-placeholder",
            thresholdVersion: null,
            thresholdBytes: null,
            fixedExclusions,
          },
        }),
      ),
    ),
    createBackup: vi.fn(() =>
      Promise.resolve(response({ artifactName: "before-changes.eutheto" })),
    ),
    previewRestore: vi.fn(() => Promise.resolve(response(portablePreview("full-backup")))),
    applyRestore: vi.fn(() => Promise.resolve(response({}))),
    cancelPortablePreview: vi.fn(() => Promise.resolve(response({}))),
    onAppNotification: vi.fn(() => Promise.resolve(vi.fn())),
    onLibraryRefreshRequired: vi.fn(() => Promise.resolve(vi.fn())),
    onScenarioChanged: vi.fn(() => Promise.resolve(vi.fn())),
    onScenarioValidationChanged: vi.fn(() => Promise.resolve(vi.fn())),
  };
}

export function portablePreview(bundleKind: "scenario-export" | "full-backup") {
  return {
    previewId: "01900000-0000-7000-8000-000000000010",
    bundleId: "01900000-0000-7000-8000-000000000011",
    bundleKind,
    title: bundleKind === "full-backup" ? "Nightly backup" : "Imported roster",
    createdAt: "2026-08-29T11:00:00Z",
    sourceApplication: { name: "eutheto-core", version: "0.1.0" },
    sourceFormatVersion: 1,
    sourceSchemaVersion: 1,
    counts: {
      scenarios: 1,
      scenarioRevisions: 2,
      results: 0,
      sharedRecords: 4,
      preferences: 5,
      assets: 6,
    },
    requiredCapabilities: [{ id: "portable.history", version: 1 }],
    preservedExtensions: ["example.extension"],
    includedSections: ["scenarios", "results", "shared-records", "preferences", "assets"],
    sourceBackupSelection: {
      includeResults: false,
      assetSelection: "v1-threshold" as const,
      thresholdVersion: 1,
      thresholdBytes: 16_777_216,
      excludedAssetCount: 1,
      excludedAssetIds: ["large-video.mp4"],
      fixedExclusions,
      scope: bundleKind === "full-backup" ? ("library" as const) : ("scenario" as const),
    },
    omittedAssets: [
      {
        assetId: "large-video.mp4",
        format: "eutheto/omitted-asset",
        version: 1,
        reason: "above-v1-threshold" as const,
        originalMediaType: "video/mp4",
        originalSize: 20_000_000,
        contentSha256: "a".repeat(64),
      },
    ],
    excludedSections: [],
    scenarios: [
      {
        scenarioId: project.scenarioId,
        title: project.title,
        collides: true,
        sourceRevision: 2,
        sameIdentityRevision: 6,
        sameIdentityRevisionWarning:
          "A deleted project previously used this ID; importing resumes at revision 6.",
      },
    ],
    supplementalCollisions: [],
    removedScenarios:
      bundleKind === "full-backup"
        ? [
            {
              scenarioId: project.scenarioId,
              title: project.title,
              revision: project.revision,
              archived: project.archived,
            },
          ]
        : [],
    removedSupplemental: [],
    settingsChanged: bundleKind === "full-backup" ? ["appearance"] : [],
    settingsRemoved: bundleKind === "full-backup" ? ["units"] : [],
    appliedMigrations: [
      { registry: "portable", name: "portable-v0-to-v1", fromVersion: 0, toVersion: 1 },
    ],
  } as const;
}

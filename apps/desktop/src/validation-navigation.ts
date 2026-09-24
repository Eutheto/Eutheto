import type { LocationQuery, RouteLocationRaw } from "vue-router";
import { getScenarioView, type SetupOperationScope, type ValidationIssue } from "./api/generated";
import type { WorkforceSetupEntityKind } from "./api/generated-domain-pack-contracts";
import { messages } from "./messages";
import { isEditableRule } from "./rule-draft";
import type { ValidationEditorTarget } from "./validation-target";

export interface ValidationContext {
  readonly scenarioId: string;
  readonly revision: number;
  readonly libraryEpoch: number;
}
export type DiagnosticAddress =
  | { readonly collection: "settings"; readonly fieldPath: readonly string[] }
  | Pick<ValidationEditorTarget, "collection" | "id" | "fieldPath">;
export type ValidationNavigationTarget = ValidationContext & DiagnosticAddress;
export interface ResolvedValidationAddress {
  readonly label: string;
  readonly route:
    "project-people" | "project-work" | "project-availability" | "project-rules" | null;
}

// Navigation syntax only. Native queries still validate identity and stored record kind.
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const field = /^(?:[a-zA-Z][a-zA-Z0-9]*|0|[1-9][0-9]{0,5})$/;
export function diagnosticAddress(path: string | null): DiagnosticAddress | null {
  if (path === null || path.length > 2048) return null;
  const parts = path.startsWith("/") ? path.slice(1).split("/") : path.split(".");
  if (parts.length > 16) return null;
  if (parts[0] === "settings") {
    const fieldPath = parts.slice(1);
    return fieldPath.every((part) => field.test(part))
      ? { collection: "settings", fieldPath }
      : null;
  }
  const collection = parts[1];
  const id = parts[2];
  if (
    parts[0] !== "domain" ||
    (collection !== "entities" && collection !== "rules" && collection !== "preferences") ||
    id === undefined ||
    !uuid.test(id)
  )
    return null;
  const fieldPath = parts.slice(3);
  return fieldPath.every((part) => field.test(part)) ? { collection, id, fieldPath } : null;
}

function entityRoute(kind: WorkforceSetupEntityKind): ResolvedValidationAddress["route"] {
  switch (kind) {
    case "person":
    case "qualification":
    case "team":
      return "project-people";
    case "availability":
      return "project-availability";
    case "location":
    case "workloadBucket":
    case "calendar":
    case "assignmentType":
    case "shiftTemplate":
    case "shiftInstance":
    case "coverageRequirement":
      return "project-work";
    case "baseSchedule":
    case "scorePolicy":
      return null;
  }
}

export async function resolveValidationAddress(
  scope: SetupOperationScope,
  address: DiagnosticAddress,
): Promise<ResolvedValidationAddress> {
  if (address.collection === "settings")
    return { label: messages.setup.calendar, route: "project-work" };
  if (address.collection === "entities") {
    const { result } = await getScenarioView(scope, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId: "official.workforce.setup.entity_summary",
        parameters: { entityId: address.id },
      },
    }).result;
    const summary = result.view.data.result.data;
    if (summary.entityId !== address.id)
      throw new Error(messages.validationWorkspace.targetInvalid);
    return {
      label: `${summary.name ?? messages.setup.entityKinds[summary.kind]} (${summary.entityId})`,
      route: entityRoute(summary.kind),
    };
  }
  const ruleClass = address.collection === "rules" ? "required" : "preference";
  const [detail, catalog] = await Promise.all([
    getScenarioView(scope, {
      source: { kind: "stored" },
      query: {
        schemaVersion: 1,
        viewId: "official.workforce.setup.rule_detail",
        parameters: { rule: { class: ruleClass, ruleId: address.id } },
      },
    }).result,
    getScenarioView(scope, {
      source: { kind: "stored" },
      query: { schemaVersion: 1, viewId: "eutheto.setup.rule_catalog", parameters: {} },
    }).result,
  ]);
  const record = detail.result.view.data.result.data;
  if (record.class !== ruleClass || record.record.id !== address.id)
    throw new Error(messages.validationWorkspace.targetInvalid);
  const entries =
    catalog.result.view.data.result.data[ruleClass === "required" ? "required" : "preferences"];
  const kindId = `official.workforce.${ruleClass === "required" ? "rule" : "preference"}.${record.record.kind.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`;
  const title = entries.find((entry) => entry.descriptor.id === kindId)?.descriptor.title
    .defaultText;
  return {
    label: `${title ?? record.record.kind} (${address.id})`,
    route: record.class === "required" && isEditableRule(record.record) ? "project-rules" : null,
  };
}

export function validationRoute(
  target: ValidationNavigationTarget,
  name: NonNullable<ResolvedValidationAddress["route"]>,
): RouteLocationRaw {
  const root =
    target.collection === "settings" ? ["settings"] : ["domain", target.collection, target.id];
  return {
    name,
    params: { scenarioId: target.scenarioId },
    query: {
      findingPath: `/${[...root, ...target.fieldPath].join("/")}`,
      findingRevision: String(target.revision),
      findingEpoch: String(target.libraryEpoch),
    },
  };
}

export function validationRouteTarget(
  query: LocationQuery,
  context: ValidationContext,
): ValidationNavigationTarget | null {
  const path = query.findingPath;
  const revision = query.findingRevision;
  const epoch = query.findingEpoch;
  if (
    typeof path !== "string" ||
    revision !== String(context.revision) ||
    epoch !== String(context.libraryEpoch)
  )
    return null;
  const address = diagnosticAddress(path);
  return address === null ? null : { ...context, ...address };
}

export function issueAddress(issue: ValidationIssue): DiagnosticAddress | null {
  // The authored field owner wins over a triggering Rule ResourceRef. Never infer a missing endpoint.
  return diagnosticAddress(issue.fieldPath);
}

import type { PeopleCsvColumn, PeopleCsvDetectionV1, PeopleCsvField } from "../../api/generated";
import type { PlannerFieldProps } from "./field-contracts";

type DetectionCandidate = Extract<
  PeopleCsvDetectionV1["dialects"][number],
  { status: "candidate" }
>;
export type ImportMappingSample = DetectionCandidate["samples"][number];
export type ImportMappingCell = ImportMappingSample["cells"][number];

export interface ImportMappingTableProps extends Omit<PlannerFieldProps, "required"> {
  readonly modelValue: readonly PeopleCsvColumn[];
  /** Parent-owned physical width, independent of header labels. */
  readonly expectedColumns: number;
  /** Explicitly chosen native header cells; this component never infers a header. */
  readonly headerCells?: readonly ImportMappingCell[];
  /** Explicitly chosen native detection samples, not CSV text or a source handle. */
  readonly selectedSamples?: readonly ImportMappingSample[];
  /** Caller must invalidate feedback on draft/context changes; native has no field path. */
  readonly nativeInvalidMapping?: boolean;
}

export interface ImportMappingTableEvents {
  "update:modelValue": [columns: readonly PeopleCsvColumn[]];
}

// Mirrors people_csv/types.rs and mapping.rs; native validation remains authoritative.
export const MAX_MAPPING_COLUMNS = 64;
export const MAX_MAPPING_FIELDS = 10;
const MAX_SAMPLE_RECORDS = 2;
const MAX_SAMPLE_CELL_BYTES = 64;

export const peopleCsvFields = [
  "name",
  "externalId",
  "activeRange",
  "qualificationGrants",
  "eligibleAssignmentTypeIds",
  "homeLocationId",
  "workloadWeight",
  "workloadTarget",
  "tags",
  "teamIds",
] as const satisfies readonly PeopleCsvField[];

export function allowsClear(field: PeopleCsvField): boolean {
  return field !== "name" && field !== "activeRange" && field !== "workloadWeight";
}

export function validMappingWidth(width: number): boolean {
  return Number.isInteger(width) && width >= 1 && width <= MAX_MAPPING_COLUMNS;
}

export function validSourceIndex(index: number, width: number): boolean {
  return Number.isInteger(index) && index >= 0 && index < width && index < MAX_MAPPING_COLUMNS;
}

type MappingRowError =
  "indexRange" | "duplicateIndex" | "duplicateField" | "clearUnavailable" | "mappingLimit";
export interface ImportMappingRow {
  /** Null denotes a physical column with no mapping, not an implicit mapping. */
  readonly position: number | null;
  readonly index: number;
  readonly column: PeopleCsvColumn | null;
  readonly errors: readonly MappingRowError[];
}

export function mappingRows(
  columns: readonly PeopleCsvColumn[],
  width: number,
): ImportMappingRow[] {
  const indices = new Map<number, number>();
  const fields = new Map<PeopleCsvField, number>();
  for (const column of columns) {
    indices.set(column.index, (indices.get(column.index) ?? 0) + 1);
    fields.set(column.field, (fields.get(column.field) ?? 0) + 1);
  }
  const rows: ImportMappingRow[] = columns.map((column, position) => {
    const errors: MappingRowError[] = [];
    if (!validSourceIndex(column.index, width)) errors.push("indexRange");
    if ((indices.get(column.index) ?? 0) > 1) errors.push("duplicateIndex");
    if ((fields.get(column.field) ?? 0) > 1) errors.push("duplicateField");
    if (column.blank === "clear" && !allowsClear(column.field)) errors.push("clearUnavailable");
    if (columns.length > MAX_MAPPING_FIELDS) errors.push("mappingLimit");
    return { position, index: column.index, column, errors };
  });
  if (validMappingWidth(width)) {
    for (let index = 0; index < width; index += 1) {
      if (!indices.has(index)) rows.push({ position: null, index, column: null, errors: [] });
    }
  }
  return rows;
}

/** Bound work and UTF-8 display bytes without encoding or copying an unbounded input. */
function boundedCell(cell: ImportMappingCell): ImportMappingCell {
  let bytes = 0;
  let end = 0;
  for (const character of cell.text) {
    const point = character.charCodeAt(0);
    const size = character.length === 2 ? 4 : point <= 0x7f ? 1 : point <= 0x7ff ? 2 : 3;
    if (bytes + size > MAX_SAMPLE_CELL_BYTES) break;
    bytes += size;
    end += character.length;
  }
  return {
    text: cell.text.slice(0, end),
    truncated: cell.truncated || end < cell.text.length,
  };
}

export function boundedMappingMetadata(
  headerCells: readonly ImportMappingCell[] = [],
  selectedSamples: readonly ImportMappingSample[] = [],
) {
  const headers = headerCells.slice(0, MAX_MAPPING_COLUMNS).map(boundedCell);
  const selectedRecords = selectedSamples.slice(0, MAX_SAMPLE_RECORDS);
  const samples = selectedRecords.map((sample) => ({
    record: sample.record,
    cells: sample.cells.slice(0, MAX_MAPPING_COLUMNS).map(boundedCell),
  }));
  return {
    headers,
    samples,
    omitted:
      headerCells.length > MAX_MAPPING_COLUMNS ||
      selectedSamples.length > MAX_SAMPLE_RECORDS ||
      selectedRecords.some((sample) => sample.cells.length > MAX_MAPPING_COLUMNS),
  };
}

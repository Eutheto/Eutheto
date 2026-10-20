//! Deterministic bounded people/shift summary CSV for an accepted Workforce result.
//!
//! The source document owns all people, shift labels, coverage and timestamps. The accepted
//! terminal result or checkpoint contributes only selected assignment pairs and is expected to
//! have been freshly verified by the caller. This projection never reads candidate-provided display
//! or timing values and does not establish acceptance by itself.

use crate::{
    assignment_rules::decode_workforce_assignment,
    export_source::WorkforceExportSource,
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    ids::ShiftId,
    model::{Coverage, Person, WorkforceDomainV1, WorkforceEntity},
    temporal::{ResolvedShift, ResolvedShiftOrigin, TemporalError, resolve_validated_shifts},
    validation::validate_document_controlled,
};
use eutheto_domain_api::DomainPackError;
use eutheto_domain_ir::AcceptedResult;
use eutheto_types::{OperationControl, OperationInterruption, PersonId, ScenarioDocument};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// Maximum encoded summary CSV size.
pub const MAX_SUMMARY_CSV_BYTES: usize = 16 * 1024 * 1024;
/// Maximum person plus shift rows in one summary.
pub const MAX_SUMMARY_CSV_ROWS: usize = 100_000;
/// Maximum decoded UTF-8 bytes in one emitted cell.
pub const MAX_SUMMARY_CSV_CELL_BYTES: usize = 16 * 1024;

const FRAMING: &[u8] = b"eutheto/workforce-summary,1\n";
const HEADER: &[u8] = b"kind,person_id,person_name,shift_id,shift_name,source_timezone,scenario_revision,revision_as_of,revision_status,selected_count,required,minimum,maximum,margin,shift_starts_local,shift_ends_local,shift_starts_instant,shift_ends_instant\n";

/// Bounded summary projection failures. Source values and operating-system text are never kept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SummaryCsvErrorCode {
    UnsupportedPack,
    InvalidSource,
    InvalidResult,
    InvalidAssignment,
    MissingPerson,
    MissingShift,
    DuplicateAssignment,
    ByteLimit,
    RowLimit,
    CellLimit,
    OperationLimit,
    Cancelled,
    DeadlineExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SummaryCsvError {
    pub code: SummaryCsvErrorCode,
}

impl SummaryCsvError {
    const fn new(code: SummaryCsvErrorCode) -> Self {
        Self { code }
    }
}

impl fmt::Display for SummaryCsvError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Workforce summary CSV: {:?}", self.code)
    }
}

impl std::error::Error for SummaryCsvError {}

/// Encodes source-derived person and shift rows in stable order.
///
/// Rows are emitted as all people ordered by stable person ID, followed by all resolved shifts
/// ordered by source instant and stable shift ID. A person row carries its selected assignment
/// count; a shift row carries selected count, source coverage values and margin. Blank cells mean
/// the field is not applicable. Every row repeats the source timezone and revision observation,
/// and stale observations carry `old-revision` in `revision_status`.
///
/// The caller must freshly verify the complete accepted result against `document` before calling
/// this pure projection. No partial output is returned on a bounded-resource or source error.
///
/// # Errors
/// Rejects unsupported/malformed source or result data, unknown or duplicate selected identities,
/// bounded-resource violations and operation interruption.
#[allow(clippy::too_many_lines)]
pub fn encode_workforce_summary_csv(
    document: &ScenarioDocument,
    source: WorkforceExportSource<'_>,
    current_revision: u64,
    control: &OperationControl,
) -> Result<Vec<u8>, SummaryCsvError> {
    let mut writer = SummaryWriter::new(control);
    writer.raw(FRAMING)?;
    writer.raw(HEADER)?;
    writer.check()?;

    if document.domain_pack.id.as_str() != WORKFORCE_PACK_ID
        || document.domain_pack.schema_version != 1
    {
        return Err(SummaryCsvError::new(SummaryCsvErrorCode::UnsupportedPack));
    }
    source
        .validate_binding(document)
        .map_err(|()| SummaryCsvError::new(SummaryCsvErrorCode::InvalidResult))?;
    let domain = validate_document_controlled(document, Some(control))
        .map_err(|error| source_error(&error))?;
    let resolved = resolve_validated_shifts(&domain, &document.settings, &mut |_| {
        control.check().map_err(ResolveError::Interrupted)
    })
    .map_err(resolve_error)?;
    writer.check()?;

    let people = people_by_id(&domain);
    let mut shifts = BTreeMap::<ShiftId, ResolvedShift>::new();
    let mut resolved_order = Vec::new();
    for shift in resolved {
        writer.step()?;
        if shifts.insert(shift.id, shift).is_some() {
            return Err(SummaryCsvError::new(SummaryCsvErrorCode::InvalidSource));
        }
        resolved_order.push(shift);
    }
    let metadata = shift_metadata(&domain, &shifts, control)?;
    let (selected_by_shift, selected_by_person) =
        selected_assignments(source.accepted_result(), &people, &shifts, control)?;
    let row_count = people
        .len()
        .checked_add(shifts.len())
        .ok_or_else(|| SummaryCsvError::new(SummaryCsvErrorCode::RowLimit))?;
    if row_count > MAX_SUMMARY_CSV_ROWS {
        return Err(SummaryCsvError::new(SummaryCsvErrorCode::RowLimit));
    }

    let scenario_revision = source.run_input().scenario_revision.to_string();
    let revision_as_of = current_revision.to_string();
    let revision_status = if current_revision == source.run_input().scenario_revision {
        "current"
    } else {
        "old-revision"
    };
    let timezone = document.settings.time_zone.as_str();

    for (person_id, person) in people {
        writer.step()?;
        let person_id = person_id.to_string();
        let selected_count = selected_by_person
            .get(&person.id)
            .copied()
            .unwrap_or(0)
            .to_string();
        writer.data_row(&[
            ("person", false),
            (&person_id, false),
            (person.name.as_str(), true),
            ("", false),
            ("", false),
            (timezone, false),
            (&scenario_revision, false),
            (&revision_as_of, false),
            (revision_status, false),
            (&selected_count, false),
            ("", false),
            ("", false),
            ("", false),
            ("", false),
            ("", false),
            ("", false),
            ("", false),
            ("", false),
        ])?;
    }

    for shift in &resolved_order {
        writer.step()?;
        let shift_id = &shift.id;
        let shift_metadata = metadata
            .get(shift_id)
            .ok_or_else(|| SummaryCsvError::new(SummaryCsvErrorCode::MissingShift))?;
        let shift_id_text = shift_id.to_string();
        let shift_name = shift_metadata.name;
        let selected_count = selected_by_shift.get(shift_id).copied().unwrap_or(0);
        let selected_count_text = selected_count.to_string();
        let (required, minimum, maximum, margin) =
            coverage_values(shift_metadata.coverage, selected_count);
        let required_text = optional_u32(required);
        let minimum_text = minimum.to_string();
        let maximum_text = optional_u32(maximum);
        let margin_text = optional_i64(margin);
        let starts_local = shift.interval.starts_at.local.to_string();
        let ends_local = shift.interval.ends_at.local.to_string();
        let starts_instant = shift.interval.starts_at.instant.to_string();
        let ends_instant = shift.interval.ends_at.instant.to_string();
        writer.data_row(&[
            ("shift", false),
            ("", false),
            ("", false),
            (&shift_id_text, false),
            (shift_name, true),
            (timezone, false),
            (&scenario_revision, false),
            (&revision_as_of, false),
            (revision_status, false),
            (&selected_count_text, false),
            (&required_text, false),
            (&minimum_text, false),
            (&maximum_text, false),
            (&margin_text, false),
            (&starts_local, false),
            (&ends_local, false),
            (&starts_instant, false),
            (&ends_instant, false),
        ])?;
    }
    Ok(writer.finish())
}

fn people_by_id(domain: &WorkforceDomainV1) -> BTreeMap<PersonId, &Person> {
    domain
        .entities
        .values()
        .filter_map(|entity| match entity {
            WorkforceEntity::Person(person) => Some((person.id, person)),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Copy)]
struct ShiftMetadata<'a> {
    name: &'a str,
    coverage: &'a Coverage,
}

fn shift_metadata<'a>(
    domain: &'a WorkforceDomainV1,
    shifts: &BTreeMap<ShiftId, ResolvedShift>,
    control: &OperationControl,
) -> Result<BTreeMap<ShiftId, ShiftMetadata<'a>>, SummaryCsvError> {
    shifts
        .values()
        .map(|shift| {
            control.check().map_err(interruption)?;
            let (name, coverage) = if let Some(WorkforceEntity::ShiftInstance(instance)) =
                domain.entities.get(&shift.id.as_entity_id())
            {
                (
                    assignment_type_name(domain, instance.assignment_type_id)?,
                    &instance.coverage,
                )
            } else {
                let template_id = match shift.origin {
                    ResolvedShiftOrigin::Generated { template_id, .. }
                    | ResolvedShiftOrigin::Detached { template_id, .. } => template_id,
                    ResolvedShiftOrigin::Manual => {
                        return Err(SummaryCsvError::new(SummaryCsvErrorCode::MissingShift));
                    }
                };
                let Some(WorkforceEntity::ShiftTemplate(template)) =
                    domain.entities.get(&template_id.as_entity_id())
                else {
                    return Err(SummaryCsvError::new(SummaryCsvErrorCode::MissingShift));
                };
                (template.name.as_str(), &template.coverage)
            };
            Ok((shift.id, ShiftMetadata { name, coverage }))
        })
        .collect()
}

fn assignment_type_name(
    domain: &WorkforceDomainV1,
    assignment_type_id: crate::ids::AssignmentTypeId,
) -> Result<&str, SummaryCsvError> {
    match domain.entities.get(&assignment_type_id.as_entity_id()) {
        Some(WorkforceEntity::AssignmentType(assignment_type)) => Ok(assignment_type.name.as_str()),
        _ => Err(SummaryCsvError::new(SummaryCsvErrorCode::MissingShift)),
    }
}

type SelectedAssignmentCounts = (BTreeMap<ShiftId, u32>, BTreeMap<PersonId, u32>);
fn selected_assignments(
    accepted: &AcceptedResult,
    people: &BTreeMap<PersonId, &Person>,
    shifts: &BTreeMap<ShiftId, ResolvedShift>,
    control: &OperationControl,
) -> Result<SelectedAssignmentCounts, SummaryCsvError> {
    let mut selected = BTreeSet::new();
    let mut selected_by_shift = BTreeMap::new();
    let mut selected_by_person = BTreeMap::new();
    for assignment in &accepted.solution.assignments {
        control.check().map_err(interruption)?;
        let (pair, is_selected) = decode_workforce_assignment(assignment)
            .map_err(|_| SummaryCsvError::new(SummaryCsvErrorCode::InvalidAssignment))?;
        if !people.contains_key(&pair.person_id) {
            return Err(SummaryCsvError::new(SummaryCsvErrorCode::MissingPerson));
        }
        if !shifts.contains_key(&pair.shift_id) {
            return Err(SummaryCsvError::new(SummaryCsvErrorCode::MissingShift));
        }
        if !is_selected {
            continue;
        }
        if !selected.insert(pair) {
            return Err(SummaryCsvError::new(
                SummaryCsvErrorCode::DuplicateAssignment,
            ));
        }
        let shift_count = selected_by_shift.entry(pair.shift_id).or_insert(0_u32);
        *shift_count = shift_count
            .checked_add(1)
            .ok_or_else(|| SummaryCsvError::new(SummaryCsvErrorCode::OperationLimit))?;
        let person_count = selected_by_person.entry(pair.person_id).or_insert(0_u32);
        *person_count = person_count
            .checked_add(1)
            .ok_or_else(|| SummaryCsvError::new(SummaryCsvErrorCode::OperationLimit))?;
    }
    Ok((selected_by_shift, selected_by_person))
}

fn coverage_values(
    coverage: &Coverage,
    selected_count: u32,
) -> (Option<u32>, u32, Option<u32>, Option<i64>) {
    match coverage {
        Coverage::Exact { count, .. } => {
            let required = u32::from(*count);
            (
                Some(required),
                required,
                Some(required),
                Some(i64::from(selected_count) - i64::from(required)),
            )
        }
        Coverage::AtLeast {
            minimum,
            maximum_count,
            ..
        } => {
            let minimum = u32::from(*minimum);
            (
                None,
                minimum,
                maximum_count.map(u32::from),
                Some(i64::from(selected_count) - i64::from(minimum)),
            )
        }
    }
}

fn optional_u32(value: Option<u32>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}

fn optional_i64(value: Option<i64>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}

fn interruption(error: OperationInterruption) -> SummaryCsvError {
    SummaryCsvError::new(match error {
        OperationInterruption::Cancelled => SummaryCsvErrorCode::Cancelled,
        OperationInterruption::DeadlineExceeded => SummaryCsvErrorCode::DeadlineExceeded,
    })
}

fn source_error(error: &DomainPackError) -> SummaryCsvError {
    SummaryCsvError::new(match error {
        DomainPackError::Cancelled => SummaryCsvErrorCode::Cancelled,
        DomainPackError::BudgetExpired => SummaryCsvErrorCode::DeadlineExceeded,
        DomainPackError::ResourceLimitExceeded => SummaryCsvErrorCode::OperationLimit,
        _ => SummaryCsvErrorCode::InvalidSource,
    })
}

enum ResolveError {
    Interrupted(OperationInterruption),
    Temporal(TemporalError),
}

impl From<TemporalError> for ResolveError {
    fn from(error: TemporalError) -> Self {
        Self::Temporal(error)
    }
}

fn resolve_error(error: ResolveError) -> SummaryCsvError {
    match error {
        ResolveError::Interrupted(reason) => interruption(reason),
        ResolveError::Temporal(TemporalError::Cancelled) => {
            SummaryCsvError::new(SummaryCsvErrorCode::Cancelled)
        }
        ResolveError::Temporal(TemporalError::InvalidDocument(error)) => source_error(&error),
        ResolveError::Temporal(TemporalError::Issue(_)) => {
            SummaryCsvError::new(SummaryCsvErrorCode::InvalidSource)
        }
    }
}

struct SummaryWriter<'a> {
    control: &'a OperationControl,
    output: Vec<u8>,
    row: Vec<u8>,
    rows: usize,
}

impl<'a> SummaryWriter<'a> {
    fn new(control: &'a OperationControl) -> Self {
        Self {
            control,
            output: Vec::new(),
            row: Vec::new(),
            rows: 0,
        }
    }

    fn check(&self) -> Result<(), SummaryCsvError> {
        self.control.check().map_err(interruption)
    }

    fn step(&self) -> Result<(), SummaryCsvError> {
        self.check()
    }

    fn raw(&mut self, bytes: &[u8]) -> Result<(), SummaryCsvError> {
        self.check()?;
        let total = self
            .output
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| SummaryCsvError::new(SummaryCsvErrorCode::ByteLimit))?;
        if total > MAX_SUMMARY_CSV_BYTES {
            return Err(SummaryCsvError::new(SummaryCsvErrorCode::ByteLimit));
        }
        self.output.extend_from_slice(bytes);
        Ok(())
    }

    fn data_row(&mut self, cells: &[(&str, bool)]) -> Result<(), SummaryCsvError> {
        self.check()?;
        if self.rows == MAX_SUMMARY_CSV_ROWS {
            return Err(SummaryCsvError::new(SummaryCsvErrorCode::RowLimit));
        }
        self.row.clear();
        for (index, (value, guard_formula)) in cells.iter().enumerate() {
            if index != 0 {
                self.row.push(b',');
            }
            append_cell(&mut self.row, value, *guard_formula)?;
        }
        self.row.push(b'\n');
        let total = self
            .output
            .len()
            .checked_add(self.row.len())
            .ok_or_else(|| SummaryCsvError::new(SummaryCsvErrorCode::ByteLimit))?;
        if total > MAX_SUMMARY_CSV_BYTES {
            return Err(SummaryCsvError::new(SummaryCsvErrorCode::ByteLimit));
        }
        self.output.extend_from_slice(&self.row);
        self.rows += 1;
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        self.output
    }
}

fn append_cell(row: &mut Vec<u8>, value: &str, guard_formula: bool) -> Result<(), SummaryCsvError> {
    if value.len() > MAX_SUMMARY_CSV_CELL_BYTES {
        return Err(SummaryCsvError::new(SummaryCsvErrorCode::CellLimit));
    }
    let guarded = guard_formula && formula_leading(value);
    let needs_quote = guarded
        || value
            .bytes()
            .any(|byte| matches!(byte, b',' | b'"' | b'\r' | b'\n'));
    if needs_quote {
        row.push(b'"');
    }
    if guarded {
        row.push(b'\'');
    }
    for byte in value.bytes() {
        if byte == b'"' && needs_quote {
            row.push(b'"');
        }
        row.push(byte);
    }
    if needs_quote {
        row.push(b'"');
    }
    Ok(())
}

fn formula_leading(value: &str) -> bool {
    let trimmed = value.trim_start_matches([' ', '\t', '\r', '\n']);
    trimmed
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'=' | b'+' | b'-' | b'@'))
}

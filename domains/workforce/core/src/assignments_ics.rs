//! Bounded RFC5545 calendar output for an already accepted Workforce result.
//!
//! This module is a pure result projection. Callers MUST independently verify the complete
//! source-bound accepted terminal result or checkpoint before invoking [`encode_workforce_ics`].
//! The encoder resolves shifts from the supplied immutable source document; it never trusts result
//! timestamps or result evidence.
//!
//! Content-line folding and TEXT escaping follow RFC 5545 §§3.1 and 3.3.11; VEVENT and UTC
//! DATE-TIME fields follow §§3.3.5 and 3.6.1:
//! <https://www.rfc-editor.org/rfc/rfc5545.html>.

use crate::{
    assignment_rules::decode_workforce_assignment,
    export_source::WorkforceExportSource,
    generated_workforce_pack_contract::WORKFORCE_PACK_ID,
    model::{AssignmentPair, WorkforceDomainV1, WorkforceEntity},
    temporal::{ResolvedShift, ResolvedShiftOrigin, TemporalError, resolve_validated_shifts},
    validation::validate_document_controlled,
};
use eutheto_domain_api::DomainPackError;
use eutheto_types::{
    EntityId, OperationControl, OperationInterruption, PersonId, Rfc3339Timestamp, ScenarioDocument,
};
use jiff::tz::TimeZone;
use std::{collections::BTreeMap, fmt};

/// Maximum encoded calendar size. This is deliberately below the portable result size bound.
pub const MAX_ASSIGNMENTS_ICS_BYTES: usize = 16 * 1024 * 1024;
/// Maximum emitted events in one calendar.
pub const MAX_ASSIGNMENTS_ICS_EVENTS: usize = 100_000;
/// Maximum number of logical writer operations, including folding and escaped text units.
pub const MAX_ASSIGNMENTS_ICS_OPERATIONS: usize = MAX_ASSIGNMENTS_ICS_BYTES.saturating_mul(4);
const MAX_CONTENT_LINE_OCTETS: usize = 75;

/// Bounded causes for Workforce iCalendar encoding. Values and source text are never retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentsIcsErrorCode {
    UnsupportedPack,
    InvalidSource,
    InvalidResult,
    InvalidAssignment,
    MissingPerson,
    MissingShift,
    DuplicateAssignment,
    SubsecondPrecision,
    ByteLimit,
    EventLimit,
    OperationLimit,
    Cancelled,
    DeadlineExceeded,
}

/// Safe, non-sensitive encoder failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssignmentsIcsError {
    pub code: AssignmentsIcsErrorCode,
}

impl AssignmentsIcsError {
    const fn new(code: AssignmentsIcsErrorCode) -> Self {
        Self { code }
    }
}

impl fmt::Display for AssignmentsIcsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Workforce iCalendar export: {:?}", self.code)
    }
}

impl std::error::Error for AssignmentsIcsError {}

/// Encodes a combined or filtered Workforce schedule as inert RFC5545 text.
///
/// `current_revision` is the scenario revision observed while the caller read and verified the
/// accepted result. It is retained as an explicitly time-of-observation label; the calendar does
/// not claim that the scenario remains current when a caller later publishes these bytes. A person
/// filter selects only that person's accepted assignments; `None` emits the combined calendar.
/// The source document remains the sole authority for people, shift metadata, and resolved UTC
/// endpoints. `created_at` comes from the caller's trusted clock, never from historical run data.
///
/// The calendar intentionally omits run, snapshot, source-document, result, and verification
/// fingerprints. Historical solver proof is not endorsed by this projection.
///
/// `current_revision` is retained in the signature for API compatibility and means the revision
/// observed as of the read/verification boundary.
///
/// # Errors
/// Rejects unsupported or malformed source/result data, unknown selected identities, subsecond
/// assignment endpoints, bounded-resource violations, and interruption. No partial output is
/// returned.
#[allow(clippy::too_many_lines)]
pub fn encode_workforce_ics(
    document: &ScenarioDocument,
    source: WorkforceExportSource<'_>,
    person_filter: Option<PersonId>,
    current_revision: u64,
    created_at: Rfc3339Timestamp,
    control: &OperationControl,
) -> Result<Vec<u8>, AssignmentsIcsError> {
    let mut writer = Writer::new(control);
    writer.check()?;
    if document.domain_pack.id.as_str() != WORKFORCE_PACK_ID
        || document.domain_pack.schema_version != 1
    {
        return Err(AssignmentsIcsError::new(
            AssignmentsIcsErrorCode::UnsupportedPack,
        ));
    }
    let status = source
        .validate_binding(document)
        .map_err(|()| AssignmentsIcsError::new(AssignmentsIcsErrorCode::InvalidResult))?;
    let accepted = source.accepted_result();
    let scenario_revision = source.run_input().scenario_revision;
    let domain = validate_document_controlled(document, Some(control))
        .map_err(|error| source_error(&error))?;
    let resolved = resolve_validated_shifts(&domain, &document.settings, &mut |_| {
        control.check().map_err(ResolveError::Interrupted)
    })
    .map_err(resolve_error)?;
    writer.check()?;

    let mut shifts = BTreeMap::<crate::ids::ShiftId, ResolvedShift>::new();
    for shift in resolved {
        writer.step()?;
        if shifts.insert(shift.id, shift).is_some() {
            return Err(AssignmentsIcsError::new(
                AssignmentsIcsErrorCode::InvalidSource,
            ));
        }
    }
    if let Some(person_id) = person_filter
        && find_person(&domain, person_id).is_none()
    {
        return Err(AssignmentsIcsError::new(
            AssignmentsIcsErrorCode::MissingPerson,
        ));
    }

    let mut selected = BTreeMap::<AssignmentPair, SelectedAssignment<'_>>::new();
    for assignment in &accepted.solution.assignments {
        writer.step()?;
        let (pair, chosen) = decode_workforce_assignment(assignment)
            .map_err(|_| AssignmentsIcsError::new(AssignmentsIcsErrorCode::InvalidAssignment))?;
        if !chosen {
            continue;
        }
        if find_person(&domain, pair.person_id).is_none() {
            return Err(AssignmentsIcsError::new(
                AssignmentsIcsErrorCode::MissingPerson,
            ));
        }
        if !shifts.contains_key(&pair.shift_id) {
            return Err(AssignmentsIcsError::new(
                AssignmentsIcsErrorCode::MissingShift,
            ));
        }
        if selected
            .insert(
                pair,
                SelectedAssignment {
                    id: assignment.id.as_str(),
                    pair,
                },
            )
            .is_some()
        {
            return Err(AssignmentsIcsError::new(
                AssignmentsIcsErrorCode::DuplicateAssignment,
            ));
        }
    }

    let created_at_utc = utc_basic_timestamp(created_at, false)?;
    let stale_as_of = current_revision != scenario_revision;
    writer.property("BEGIN", "VCALENDAR")?;
    writer.property("PRODID", "-//Eutheto//Workforce iCalendar Export//EN")?;
    writer.property("VERSION", "2.0")?;
    writer.property("CALSCALE", "GREGORIAN")?;
    writer.property("X-EUTHETO-FORMAT", "workforce-ics-v1")?;
    if stale_as_of {
        writer.property(
            "NAME",
            &format!("Eutheto Workforce schedule — historical revision {scenario_revision}"),
        )?;
    } else {
        writer.property("NAME", "Eutheto Workforce schedule")?;
    }
    writer.property("X-EUTHETO-SCENARIO-ID", &document.scenario_id.to_string())?;
    writer.property(
        "X-EUTHETO-SCENARIO-REVISION",
        &scenario_revision.to_string(),
    )?;
    writer.property("X-EUTHETO-REVISION-AS-OF", &current_revision.to_string())?;
    writer.property(
        "X-EUTHETO-STALE-AS-OF",
        if stale_as_of { "TRUE" } else { "FALSE" },
    )?;
    if stale_as_of {
        // Keep the established positive stale marker for consumers that only understand the
        // legacy boolean, but never emit a false current/nonstale claim after lock release.
        writer.property("X-EUTHETO-STALE", "TRUE")?;
    }
    writer.property("X-EUTHETO-STATUS", status)?;
    writer.property("X-EUTHETO-HISTORICAL-PROOF", "UNVERIFIED")?;
    writer.property(
        "X-EUTHETO-SCENARIO-TIMEZONE",
        document.settings.time_zone.as_str(),
    )?;
    if let Some(person_id) = person_filter {
        writer.property("X-EUTHETO-PERSON-ID", &person_id.to_string())?;
    }

    let mut event_count = 0_usize;
    for selected_assignment in selected.values() {
        writer.step()?;
        let person = find_person(&domain, selected_assignment.pair.person_id)
            .ok_or_else(|| AssignmentsIcsError::new(AssignmentsIcsErrorCode::MissingPerson))?;
        if person_filter.is_some_and(|person_id| person_id != person.id) {
            continue;
        }
        if event_count == MAX_ASSIGNMENTS_ICS_EVENTS {
            return Err(AssignmentsIcsError::new(
                AssignmentsIcsErrorCode::EventLimit,
            ));
        }
        event_count += 1;
        let shift = shifts
            .get(&selected_assignment.pair.shift_id)
            .ok_or_else(|| AssignmentsIcsError::new(AssignmentsIcsErrorCode::MissingShift))?;
        let start = utc_basic_timestamp(shift.interval.starts_at.instant, true)?;
        let end = utc_basic_timestamp(shift.interval.ends_at.instant, true)?;
        let (shift_name, location_id) = shift_metadata(&domain, shift)?;
        let location_name = location_id.and_then(|id| find_location(&domain, id));
        let summary = if stale_as_of {
            format!(
                "Historical revision {}: {} — {}",
                scenario_revision, person.name, shift_name
            )
        } else {
            format!("{} — {}", person.name, shift_name)
        };
        // Accepted results have separate opaque UID namespaces: re-exporting history cannot
        // replace a newer accepted result, and the result ID never enters calendar text.
        let mut uid_hash = blake3::Hasher::new();
        uid_hash.update(b"eutheto/workforce/ics-event/v1\0");
        uid_hash.update(accepted.solution.solution_id.as_uuid().as_bytes());
        uid_hash.update(person.id.as_uuid().as_bytes());
        uid_hash.update(shift.id.as_entity_id().as_uuid().as_bytes());
        let uid = format!("eutheto-{}@eutheto", uid_hash.finalize().to_hex());
        writer.property("BEGIN", "VEVENT")?;
        writer.property("UID", &uid)?;
        writer.property("DTSTAMP", &created_at_utc)?;
        writer.property("DTSTART", &start)?;
        writer.property("DTEND", &end)?;
        writer.property("SUMMARY", &summary)?;
        writer.property("TRANSP", "OPAQUE")?;
        writer.property("X-EUTHETO-ASSIGNMENT-ID", selected_assignment.id)?;
        writer.property("X-EUTHETO-PERSON-ID", &person.id.to_string())?;
        writer.property("X-EUTHETO-SHIFT-ID", &shift.id.to_string())?;
        if let Some(location_name) = location_name {
            writer.property("LOCATION", location_name)?;
        }
        writer.property("END", "VEVENT")?;
    }
    writer.property("END", "VCALENDAR")?;
    writer.finish()
}

#[derive(Clone, Copy)]
struct SelectedAssignment<'a> {
    id: &'a str,
    pair: AssignmentPair,
}

fn find_person(domain: &WorkforceDomainV1, id: PersonId) -> Option<&crate::model::Person> {
    match domain.entities.get(&EntityId::from_uuid(id.as_uuid())) {
        Some(WorkforceEntity::Person(person)) => Some(person),
        _ => None,
    }
}

fn find_location(domain: &WorkforceDomainV1, id: crate::ids::LocationId) -> Option<&str> {
    match domain.entities.get(&id.as_entity_id()) {
        Some(WorkforceEntity::Location(location)) => Some(location.name.as_str()),
        _ => None,
    }
}

fn shift_metadata<'a>(
    domain: &'a WorkforceDomainV1,
    shift: &ResolvedShift,
) -> Result<(&'a str, Option<crate::ids::LocationId>), AssignmentsIcsError> {
    match shift.origin {
        ResolvedShiftOrigin::Generated { template_id, .. } => {
            let Some(WorkforceEntity::ShiftTemplate(template)) =
                domain.entities.get(&template_id.as_entity_id())
            else {
                return Err(AssignmentsIcsError::new(
                    AssignmentsIcsErrorCode::MissingShift,
                ));
            };
            Ok((template.name.as_str(), template.location_id))
        }
        ResolvedShiftOrigin::Detached { .. } | ResolvedShiftOrigin::Manual => {
            let Some(WorkforceEntity::ShiftInstance(instance)) =
                domain.entities.get(&shift.id.as_entity_id())
            else {
                return Err(AssignmentsIcsError::new(
                    AssignmentsIcsErrorCode::MissingShift,
                ));
            };
            let Some(WorkforceEntity::AssignmentType(assignment_type)) = domain
                .entities
                .get(&instance.assignment_type_id.as_entity_id())
            else {
                return Err(AssignmentsIcsError::new(
                    AssignmentsIcsErrorCode::MissingShift,
                ));
            };
            Ok((assignment_type.name.as_str(), instance.location_id))
        }
    }
}

fn source_error(error: &DomainPackError) -> AssignmentsIcsError {
    AssignmentsIcsError::new(match error {
        DomainPackError::Cancelled => AssignmentsIcsErrorCode::Cancelled,
        DomainPackError::BudgetExpired => AssignmentsIcsErrorCode::DeadlineExceeded,
        DomainPackError::ResourceLimitExceeded => AssignmentsIcsErrorCode::OperationLimit,
        _ => AssignmentsIcsErrorCode::InvalidSource,
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

fn resolve_error(error: ResolveError) -> AssignmentsIcsError {
    match error {
        ResolveError::Interrupted(OperationInterruption::Cancelled)
        | ResolveError::Temporal(TemporalError::Cancelled) => {
            AssignmentsIcsError::new(AssignmentsIcsErrorCode::Cancelled)
        }
        ResolveError::Interrupted(OperationInterruption::DeadlineExceeded) => {
            AssignmentsIcsError::new(AssignmentsIcsErrorCode::DeadlineExceeded)
        }
        ResolveError::Temporal(TemporalError::InvalidDocument(error)) => source_error(&error),
        ResolveError::Temporal(TemporalError::Issue(_)) => {
            AssignmentsIcsError::new(AssignmentsIcsErrorCode::InvalidSource)
        }
    }
}

fn utc_basic_timestamp(
    timestamp: Rfc3339Timestamp,
    reject_subseconds: bool,
) -> Result<String, AssignmentsIcsError> {
    let raw = timestamp.as_timestamp().to_string();
    if reject_subseconds && has_nonzero_fraction(&raw) {
        return Err(AssignmentsIcsError::new(
            AssignmentsIcsErrorCode::SubsecondPrecision,
        ));
    }
    let datetime = timestamp
        .as_timestamp()
        .to_zoned(TimeZone::UTC)
        .datetime()
        .to_string();
    let (date, time) = datetime
        .split_once('T')
        .ok_or_else(|| AssignmentsIcsError::new(AssignmentsIcsErrorCode::InvalidSource))?;
    let time = time.split('.').next().unwrap_or(time);
    let date = date.replace('-', "");
    let time = time.replace(':', "");
    if date.len() != 8
        || time.len() != 6
        || !date.bytes().all(|byte| byte.is_ascii_digit())
        || !time.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(AssignmentsIcsError::new(
            AssignmentsIcsErrorCode::InvalidSource,
        ));
    }
    Ok(format!("{date}T{time}Z"))
}

fn has_nonzero_fraction(value: &str) -> bool {
    let Some((_, suffix)) = value.split_once('.') else {
        return false;
    };
    suffix
        .split(['Z', '+', '-'])
        .next()
        .is_some_and(|fraction| fraction.bytes().any(|byte| byte != b'0'))
}

struct Writer<'a> {
    output: Vec<u8>,
    control: &'a OperationControl,
    operations: usize,
}

impl<'a> Writer<'a> {
    fn new(control: &'a OperationControl) -> Self {
        Self {
            output: Vec::with_capacity(4096),
            control,
            operations: 0,
        }
    }

    fn check(&self) -> Result<(), AssignmentsIcsError> {
        self.control.check().map_err(|reason| match reason {
            OperationInterruption::Cancelled => {
                AssignmentsIcsError::new(AssignmentsIcsErrorCode::Cancelled)
            }
            OperationInterruption::DeadlineExceeded => {
                AssignmentsIcsError::new(AssignmentsIcsErrorCode::DeadlineExceeded)
            }
        })
    }

    fn step(&mut self) -> Result<(), AssignmentsIcsError> {
        self.operations = self
            .operations
            .checked_add(1)
            .ok_or_else(|| AssignmentsIcsError::new(AssignmentsIcsErrorCode::OperationLimit))?;
        if self.operations > MAX_ASSIGNMENTS_ICS_OPERATIONS {
            return Err(AssignmentsIcsError::new(
                AssignmentsIcsErrorCode::OperationLimit,
            ));
        }
        if self.operations.is_multiple_of(256) {
            self.check()?;
        }
        Ok(())
    }

    fn push(&mut self, byte: u8) -> Result<(), AssignmentsIcsError> {
        self.step()?;
        if self.output.len() >= MAX_ASSIGNMENTS_ICS_BYTES {
            return Err(AssignmentsIcsError::new(AssignmentsIcsErrorCode::ByteLimit));
        }
        self.output.push(byte);
        Ok(())
    }

    fn fragment(&mut self, line_len: &mut usize, bytes: &[u8]) -> Result<(), AssignmentsIcsError> {
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() <= MAX_CONTENT_LINE_OCTETS
            && (*line_len)
                .checked_add(bytes.len())
                .is_some_and(|length| length <= MAX_CONTENT_LINE_OCTETS)
        {
            for &byte in bytes {
                self.push(byte)?;
            }
            *line_len += bytes.len();
            return Ok(());
        }
        if *line_len != 0 {
            self.push(b'\r')?;
            self.push(b'\n')?;
            self.push(b' ')?;
            *line_len = 1;
        }
        if bytes.len() > MAX_CONTENT_LINE_OCTETS.saturating_sub(*line_len) {
            // All current call sites pass ASCII property prefixes or one escaped/UTF-8
            // character, so this branch is only a defensive bound for future callers.
            for &byte in bytes {
                if *line_len == MAX_CONTENT_LINE_OCTETS {
                    self.push(b'\r')?;
                    self.push(b'\n')?;
                    self.push(b' ')?;
                    *line_len = 1;
                }
                self.push(byte)?;
                *line_len += 1;
            }
        } else {
            for &byte in bytes {
                self.push(byte)?;
            }
            *line_len += bytes.len();
        }
        Ok(())
    }

    fn property(&mut self, name: &str, value: &str) -> Result<(), AssignmentsIcsError> {
        self.check()?;
        let mut line_len = 0;
        self.fragment(&mut line_len, name.as_bytes())?;
        self.fragment(&mut line_len, b":")?;
        let mut chars = value.chars().peekable();
        while let Some(character) = chars.next() {
            let token: &[u8] = match character {
                '\\' => b"\\\\",
                ';' => b"\\;",
                ',' => b"\\,",
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    b"\\n"
                }
                '\n' => b"\\n",
                character if character.is_control() && character != '\t' => b" ",
                character => {
                    let mut encoded = [0_u8; 4];
                    let bytes = character.encode_utf8(&mut encoded).as_bytes();
                    self.fragment(&mut line_len, bytes)?;
                    continue;
                }
            };
            self.fragment(&mut line_len, token)?;
        }
        self.push(b'\r')?;
        self.push(b'\n')?;
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>, AssignmentsIcsError> {
        if self.output.len() > MAX_ASSIGNMENTS_ICS_BYTES {
            return Err(AssignmentsIcsError::new(AssignmentsIcsErrorCode::ByteLimit));
        }
        self.control.check().map_err(|reason| match reason {
            OperationInterruption::Cancelled => {
                AssignmentsIcsError::new(AssignmentsIcsErrorCode::Cancelled)
            }
            OperationInterruption::DeadlineExceeded => {
                AssignmentsIcsError::new(AssignmentsIcsErrorCode::DeadlineExceeded)
            }
        })?;
        Ok(self.output)
    }
}

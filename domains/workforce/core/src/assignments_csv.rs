//! Bounded CSV v1 round trips of the selected Workforce schedule, not result acceptance.
//!
//! Assignment identity is derived from the canonical person/shift pair by the existing
//! Workforce projection contract. CSV preserves that exact selected identity set, but not
//! false decisions, evidence, or backend history. Callers exporting accepted results must
//! freshly verify the complete source artifact before invoking this pure codec.

use crate::{assignment_rules::decode_workforce_assignment, model::AssignmentPair};
use csv_core::{ReadRecordResult, ReaderBuilder};
use eutheto_domain_ir::{
    AssignmentValue, DomainAssignment, DomainAssignmentId, DomainEntityId, DomainEntityKindId,
    DomainEntityRef, NormalizedSolution,
};
use eutheto_types::{OperationControl, OperationInterruption};
use std::{
    collections::BTreeMap,
    fmt,
    io::{self, Read, Write},
};

pub const MAX_ASSIGNMENTS_CSV_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ASSIGNMENTS_CSV_ROWS: usize = 100_000;
pub const MAX_ASSIGNMENTS_CSV_CELL_BYTES: usize = 160;
const FRAMING: &[u8] = b"eutheto/assignments,1\nassignment_id,person_id,shift_id\n";
const CHUNK_BYTES: usize = 4096;
// Three maximally escaped, quoted cells, two commas and one normalized LF.
const RECORD_BYTES: usize = 3 * (2 * MAX_ASSIGNMENTS_CSV_CELL_BYTES + 2) + 3;

/// Bounded causes; neither input values nor operating-system error text are retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentsCsvErrorCode {
    Io,
    InvalidUtf8,
    MalformedCsv,
    InvalidVersion,
    InvalidHeader,
    FieldCount,
    InvalidIdentity,
    DuplicateAssignment,
    ByteLimit,
    RowLimit,
    CellLimit,
    Cancelled,
    DeadlineExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssignmentsCsvError {
    pub code: AssignmentsCsvErrorCode,
    /// One-based logical CSV record, including version and header, when available.
    pub row: Option<u32>,
}

impl AssignmentsCsvError {
    const fn new(code: AssignmentsCsvErrorCode, row: Option<u32>) -> Self {
        Self { code, row }
    }
}

impl fmt::Display for AssignmentsCsvError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "assignments CSV: {:?}", self.code)?;
        if let Some(row) = self.row {
            write!(formatter, " at record {row}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AssignmentsCsvError {}

fn check(control: &OperationControl) -> Result<(), AssignmentsCsvError> {
    control.check().map_err(|error| {
        AssignmentsCsvError::new(
            match error {
                OperationInterruption::Cancelled => AssignmentsCsvErrorCode::Cancelled,
                OperationInterruption::DeadlineExceeded => {
                    AssignmentsCsvErrorCode::DeadlineExceeded
                }
            },
            None,
        )
    })
}

/// Encodes only true decisions in canonical `DomainAssignmentId` order with LF endings.
/// No acceptance, scenario membership or feasibility is established by this operation.
///
/// # Errors
/// Rejects invalid projection identities, duplicate selected identities, bounded-resource
/// violations and interruption. No partial output is returned.
pub fn encode_assignments_csv(
    solution: &NormalizedSolution,
    control: &OperationControl,
) -> Result<Vec<u8>, AssignmentsCsvError> {
    check(control)?;
    let mut selected = BTreeMap::new();
    let mut encoded_bytes = FRAMING.len();
    for assignment in &solution.assignments {
        check(control)?;
        let (pair, chosen) = decode_workforce_assignment(assignment).map_err(|_| {
            AssignmentsCsvError::new(AssignmentsCsvErrorCode::InvalidIdentity, None)
        })?;
        if !chosen {
            continue;
        }
        if selected.len() == MAX_ASSIGNMENTS_CSV_ROWS {
            return Err(AssignmentsCsvError::new(
                AssignmentsCsvErrorCode::RowLimit,
                None,
            ));
        }
        // Bounded logarithmic insertion avoids an uncancellable whole-vector sort and
        // borrows identities; false decisions and evidence are never copied.
        if selected.insert(&assignment.id, pair).is_some() {
            return Err(AssignmentsCsvError::new(
                AssignmentsCsvErrorCode::DuplicateAssignment,
                None,
            ));
        }
        // Projection decoding established the two canonical 36-byte UUIDs.
        encoded_bytes += assignment.id.as_str().len() + 2 * 36 + 3;
    }
    if encoded_bytes > MAX_ASSIGNMENTS_CSV_BYTES {
        return Err(AssignmentsCsvError::new(
            AssignmentsCsvErrorCode::ByteLimit,
            None,
        ));
    }
    let mut output = Vec::with_capacity(encoded_bytes);
    output.extend_from_slice(FRAMING);
    for (id, pair) in selected {
        check(control)?;
        writeln!(&mut output, "{id},{},{}", pair.person_id, pair.shift_id)
            .map_err(|_| AssignmentsCsvError::new(AssignmentsCsvErrorCode::Io, None))?;
    }
    check(control)?;
    Ok(output)
}

/// Decodes strict comma CSV with LF/CRLF records and standard double-quoted cells.
/// Returned pairs are sorted by canonical assignment identity. The derived identity is
/// unique for each pair, so this preserves the exact selected schedule, including one
/// person on multiple shifts. This is not an assignment import or acceptance API.
///
/// # Errors
/// Rejects malformed original UTF-8/CSV, unsupported framing, invalid or conflicting
/// identities, duplicate pairs/IDs, resource limits, I/O and interruption. Reads use
/// bounded chunks (at most one byte beyond the inclusive source cap). Arbitrary blocking
/// `Read` implementations cannot be forcibly interrupted. No partial schedule is returned.
pub fn decode_assignments_csv<R: Read + ?Sized>(
    reader: &mut R,
    control: &OperationControl,
) -> Result<Vec<AssignmentPair>, AssignmentsCsvError> {
    check(control)?;
    let mut raw = RawInput::new(reader);
    let mut record = Record::new();
    let mut pairs = BTreeMap::new();
    loop {
        raw.fill(control)?;
        for &byte in &raw.bytes[..raw.valid] {
            if record.push(byte)? {
                record.finish(&mut pairs, control)?;
            }
        }
        if raw.eof {
            break;
        }
    }
    if matches!(record.state, State::Quoted | State::CarriageReturn) {
        return Err(record.error(AssignmentsCsvErrorCode::MalformedCsv));
    }
    if record.used != 0 {
        record.finish(&mut pairs, control)?;
    }
    if record.row <= 2 {
        return Err(record.error(if record.row == 1 {
            AssignmentsCsvErrorCode::InvalidVersion
        } else {
            AssignmentsCsvErrorCode::InvalidHeader
        }));
    }
    let mut output = Vec::with_capacity(pairs.len());
    for (_, pair) in pairs {
        check(control)?;
        output.push(pair);
    }
    check(control)?;
    Ok(output)
}

struct RawInput<'a, R: Read + ?Sized> {
    reader: &'a mut R,
    bytes: [u8; CHUNK_BYTES + 3],
    buffered: usize,
    valid: usize,
    count: usize,
    eof: bool,
}

impl<'a, R: Read + ?Sized> RawInput<'a, R> {
    fn new(reader: &'a mut R) -> Self {
        Self {
            reader,
            bytes: [0; CHUNK_BYTES + 3],
            buffered: 0,
            valid: 0,
            count: 0,
            eof: false,
        }
    }

    fn fill(&mut self, control: &OperationControl) -> Result<(), AssignmentsCsvError> {
        self.bytes.copy_within(self.valid..self.buffered, 0);
        self.buffered -= self.valid;
        self.valid = 0;
        loop {
            check(control)?;
            let available = MAX_ASSIGNMENTS_CSV_BYTES + 1 - self.count;
            let end = self.bytes.len().min(self.buffered + available);
            let count = match self.reader.read(&mut self.bytes[self.buffered..end]) {
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(AssignmentsCsvError::new(AssignmentsCsvErrorCode::Io, None)),
            };
            check(control)?;
            self.count += count;
            self.buffered += count;
            if self.count > MAX_ASSIGNMENTS_CSV_BYTES {
                return Err(AssignmentsCsvError::new(
                    AssignmentsCsvErrorCode::ByteLimit,
                    None,
                ));
            }
            self.eof = count == 0;
            // Validate ORIGINAL bytes before any quote removal, retaining only an
            // incomplete UTF-8 suffix (at most three bytes) across bounded fills.
            self.valid = match std::str::from_utf8(&self.bytes[..self.buffered]) {
                Ok(_) => self.buffered,
                Err(error) if error.error_len().is_none() && !self.eof => error.valid_up_to(),
                Err(_) => {
                    return Err(AssignmentsCsvError::new(
                        AssignmentsCsvErrorCode::InvalidUtf8,
                        None,
                    ));
                }
            };
            if self.valid != 0 || self.eof {
                return Ok(());
            }
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    Start,
    Unquoted,
    Quoted,
    AfterQuote,
    CarriageReturn,
}

struct Record {
    bytes: [u8; RECORD_BYTES],
    used: usize,
    cell_bytes: usize,
    fields: usize,
    row: u32,
    state: State,
}

impl Record {
    fn new() -> Self {
        Self {
            bytes: [0; RECORD_BYTES],
            used: 0,
            cell_bytes: 0,
            fields: 1,
            row: 1,
            state: State::Start,
        }
    }

    fn error(&self, code: AssignmentsCsvErrorCode) -> AssignmentsCsvError {
        AssignmentsCsvError::new(code, Some(self.row))
    }

    fn cell_byte(&mut self) -> Result<(), AssignmentsCsvError> {
        self.cell_bytes += 1;
        if self.cell_bytes > MAX_ASSIGNMENTS_CSV_CELL_BYTES {
            return Err(self.error(AssignmentsCsvErrorCode::CellLimit));
        }
        Ok(())
    }

    // Strict framing gate: csv-core itself deliberately repairs malformed quotes and
    // skips blank records. It is used for decoding only AFTER this gate accepts syntax.
    fn push(&mut self, byte: u8) -> Result<bool, AssignmentsCsvError> {
        if self.row as usize > MAX_ASSIGNMENTS_CSV_ROWS + 2 {
            return Err(self.error(AssignmentsCsvErrorCode::RowLimit));
        }
        match self.state {
            State::CarriageReturn => {
                if byte != b'\n' {
                    return Err(self.error(AssignmentsCsvErrorCode::MalformedCsv));
                }
                return Ok(true);
            }
            State::Quoted => {
                if byte == b'"' {
                    self.state = State::AfterQuote;
                } else {
                    self.cell_byte()?;
                }
            }
            State::AfterQuote if byte == b'"' => {
                self.cell_byte()?;
                self.state = State::Quoted;
            }
            _ => match byte {
                b',' => {
                    self.fields += 1;
                    if self.fields > if self.row == 1 { 2 } else { 3 } {
                        return Err(self.error(AssignmentsCsvErrorCode::FieldCount));
                    }
                    self.cell_bytes = 0;
                    self.state = State::Start;
                }
                b'\n' => return Ok(true),
                b'\r' => {
                    self.state = State::CarriageReturn;
                    return Ok(false);
                }
                b'"' if matches!(self.state, State::Start) => self.state = State::Quoted,
                _ if byte == b'"' || matches!(self.state, State::AfterQuote) => {
                    return Err(self.error(AssignmentsCsvErrorCode::MalformedCsv));
                }
                _ => {
                    self.cell_byte()?;
                    self.state = State::Unquoted;
                }
            },
        }
        if self.used >= self.bytes.len() - 1 {
            return Err(self.error(AssignmentsCsvErrorCode::CellLimit));
        }
        self.bytes[self.used] = byte;
        self.used += 1;
        Ok(false)
    }

    fn finish(
        &mut self,
        pairs: &mut BTreeMap<DomainAssignmentId, AssignmentPair>,
        control: &OperationControl,
    ) -> Result<(), AssignmentsCsvError> {
        check(control)?;
        if self.fields != if self.row == 1 { 2 } else { 3 } {
            return Err(self.error(AssignmentsCsvErrorCode::FieldCount));
        }
        // There is no BOM/encoding guessing. In particular csv-core must not silently
        // discard a BOM and thereby turn an unknown version record into valid framing.
        if self.bytes[..self.used].starts_with(&[0xef, 0xbb, 0xbf]) {
            return Err(self.error(AssignmentsCsvErrorCode::MalformedCsv));
        }
        self.bytes[self.used] = b'\n';
        let mut decoded = [0; 3 * MAX_ASSIGNMENTS_CSV_CELL_BYTES + 1];
        let mut ends = [0; 4];
        let (result, consumed, _, fields) = ReaderBuilder::new().build().read_record(
            &self.bytes[..=self.used],
            &mut decoded,
            &mut ends,
        );
        if result != ReadRecordResult::Record || consumed != self.used + 1 || fields != self.fields
        {
            return Err(self.error(AssignmentsCsvErrorCode::MalformedCsv));
        }
        let mut cells = [""; 3];
        let mut start = 0;
        for (cell, &end) in cells.iter_mut().zip(&ends[..fields]) {
            *cell = std::str::from_utf8(&decoded[start..end])
                .map_err(|_| self.error(AssignmentsCsvErrorCode::InvalidUtf8))?;
            start = end;
        }
        if self.row == 1 {
            if cells[..2] != ["eutheto/assignments", "1"] {
                return Err(self.error(AssignmentsCsvErrorCode::InvalidVersion));
            }
        } else if self.row == 2 {
            if cells != ["assignment_id", "person_id", "shift_id"] {
                return Err(self.error(AssignmentsCsvErrorCode::InvalidHeader));
            }
        } else {
            let (id, pair) = decode_pair(cells)
                .ok_or_else(|| self.error(AssignmentsCsvErrorCode::InvalidIdentity))?;
            // Canonical pair agreement makes duplicate IDs and duplicate pairs the same
            // condition. Each insertion performs bounded logarithmic sorting work.
            if pairs.insert(id, pair).is_some() {
                return Err(self.error(AssignmentsCsvErrorCode::DuplicateAssignment));
            }
        }
        check(control)?;
        self.row += 1;
        self.used = 0;
        self.cell_bytes = 0;
        self.fields = 1;
        self.state = State::Start;
        Ok(())
    }
}

fn decode_pair(cells: [&str; 3]) -> Option<(DomainAssignmentId, AssignmentPair)> {
    // Derive the purported namespace from the submitted ID, then let the existing
    // projection decoder check it. There is no second Workforce namespace authority.
    let (prefix, _) = cells[0].rsplit_once('.')?;
    let (kind, _) = prefix.rsplit_once('.')?;
    let assignment = DomainAssignment {
        id: DomainAssignmentId::new(cells[0]).ok()?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new(kind).ok()?,
            id: DomainEntityId::new(format!("{}.{}", cells[1], cells[2])).ok()?,
        },
        value: AssignmentValue::Boolean(true),
        evidence: Vec::new(),
    };
    let (pair, _) = decode_workforce_assignment(&assignment).ok()?;
    Some((assignment.id, pair))
}

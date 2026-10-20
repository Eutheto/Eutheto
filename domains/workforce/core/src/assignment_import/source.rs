use super::{AssignmentCsvDetection, AssignmentCsvReadFormat, AssignmentImportError};
use crate::{
    assignments_csv::{AssignmentsCsvError, AssignmentsCsvErrorCode, scan_assignments_csv},
    model::AssignmentPair,
    people_csv::{
        CsvError, CsvErrorCode, CsvRecord, CsvSampleCell, CsvSampleRecord, CsvSource,
        MAX_CSV_DATA_RECORDS, MAX_CSV_SOURCE_BYTES, detect_people_csv, sample_people_csv_record,
        scan_csv,
    },
};
use eutheto_domain_ir::DomainAssignmentId;
use eutheto_types::{CancellationToken, OperationControl};
use std::{
    collections::BTreeSet,
    io::{self, Read},
};

const RESERVED: &[u8] = b"eutheto/assignments";

#[derive(Default)]
enum PrefixState {
    #[default]
    Start,
    BomSecond,
    BomThird,
    EmptyRecords,
    CarriageReturn,
    Marker(usize),
}

#[derive(Default)]
struct Prefix {
    state: PrefixState,
    reserved: Option<bool>,
}

impl Prefix {
    fn marker(&mut self, index: usize, byte: u8) {
        if byte != RESERVED[index] {
            self.reserved = Some(false);
        } else if index + 1 == RESERVED.len() {
            self.reserved = Some(true);
        } else {
            self.state = PrefixState::Marker(index + 1);
        }
    }

    fn empty_record(&mut self, byte: u8) {
        match byte {
            b'\n' => self.state = PrefixState::EmptyRecords,
            b'\r' => self.state = PrefixState::CarriageReturn,
            b'"' => self.state = PrefixState::Marker(0),
            _ => self.marker(0, byte),
        }
    }

    fn observe(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.reserved.is_some() {
                break;
            }
            match self.state {
                PrefixState::Start if byte == 0xef => self.state = PrefixState::BomSecond,
                PrefixState::Start | PrefixState::EmptyRecords => self.empty_record(byte),
                PrefixState::BomSecond if byte == 0xbb => self.state = PrefixState::BomThird,
                PrefixState::BomThird if byte == 0xbf => self.state = PrefixState::EmptyRecords,
                PrefixState::CarriageReturn if byte == b'\n' => {
                    self.state = PrefixState::EmptyRecords;
                }
                PrefixState::Marker(index) => self.marker(index, byte),
                _ => self.reserved = Some(false),
            }
        }
    }
}

pub(super) fn check(cancellation: &CancellationToken) -> Result<(), CsvError> {
    if cancellation.is_cancelled() {
        Err(CsvError::source(CsvErrorCode::Cancelled))
    } else {
        Ok(())
    }
}

// Observe original bytes without buffering/replaying them or changing either parser.
// Mapped parsing already owns hashing; only strict canonical input needs this hash.
struct ObservedReader<'a, R: Read + ?Sized> {
    input: &'a mut R,
    prefix: Prefix,
    hasher: Option<blake3::Hasher>,
    raw_bytes: usize,
}

impl<'a, R: Read + ?Sized> ObservedReader<'a, R> {
    fn new(input: &'a mut R, canonical: bool) -> Self {
        Self {
            input,
            prefix: Prefix::default(),
            hasher: canonical.then(blake3::Hasher::new),
            raw_bytes: 0,
        }
    }

    fn check_format(&self, canonical: bool) -> Result<(), AssignmentImportError> {
        if self.prefix.reserved.unwrap_or(false) == canonical {
            Ok(())
        } else {
            Err(AssignmentImportError::FormatMismatch)
        }
    }
}

impl<R: Read + ?Sized> Read for ObservedReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.input.read(buffer)?;
        self.raw_bytes = self
            .raw_bytes
            .checked_add(count)
            .ok_or_else(|| io::Error::other("CSV byte count overflow"))?;
        self.prefix.observe(&buffer[..count]);
        if let Some(hasher) = &mut self.hasher {
            hasher.update(&buffer[..count]);
        }
        Ok(count)
    }
}

pub(super) enum ImportRecord<'a> {
    Canonical {
        number: u32,
        id: DomainAssignmentId,
        pair: AssignmentPair,
    },
    Mapped(CsvRecord<'a>),
}

pub(super) fn scan_source<R: Read + ?Sized>(
    input: &mut R,
    format: AssignmentCsvReadFormat,
    cancellation: &CancellationToken,
    mut visit: impl FnMut(ImportRecord<'_>) -> Result<(), CsvError>,
) -> Result<CsvSource, AssignmentImportError> {
    check(cancellation)?;
    let canonical = matches!(format, AssignmentCsvReadFormat::CanonicalV1);
    let mut reader = ObservedReader::new(input, canonical);
    let source = match format {
        AssignmentCsvReadFormat::CanonicalV1 => {
            let mut seen = BTreeSet::new();
            let control = OperationControl::Cancellation(cancellation.clone());
            let logical_records = scan_assignments_csv(
                &mut reader,
                &control,
                |number, id, pair| -> Result<(), AssignmentImportError> {
                    if number - 2 > MAX_CSV_DATA_RECORDS {
                        return Err(CsvError::at(CsvErrorCode::DataRecordLimit, number).into());
                    }
                    if !seen.insert(pair) {
                        return Err(AssignmentsCsvError {
                            code: AssignmentsCsvErrorCode::DuplicateAssignment,
                            row: Some(number),
                        }
                        .into());
                    }
                    visit(ImportRecord::Canonical { number, id, pair })?;
                    Ok(())
                },
            )?;
            CsvSource {
                raw_bytes: u64::try_from(reader.raw_bytes)
                    .map_err(|_| CsvError::source(CsvErrorCode::SourceByteLimit))?,
                blake3: reader
                    .hasher
                    .as_ref()
                    .ok_or_else(|| CsvError::source(CsvErrorCode::InvalidReview))?
                    .finalize()
                    .to_hex()
                    .to_string(),
                logical_records,
            }
        }
        AssignmentCsvReadFormat::Mapped { dialect } => {
            scan_csv(&mut reader, dialect, cancellation, |record| {
                visit(ImportRecord::Mapped(record))
            })?
        }
    };
    reader.check_format(canonical)?;
    check(cancellation)?;
    Ok(source)
}

/// Classifies reserved framing before offering mapped dialects, then validates the source.
///
/// # Errors
/// Rejects invalid canonical framing/identities, duplicate canonical pairs, unsupported
/// encoding, input/resource limits, and cancellation. No mapped fallback is attempted.
pub fn detect_assignment_csv(
    bytes: &[u8],
    cancellation: &CancellationToken,
) -> Result<AssignmentCsvDetection, AssignmentImportError> {
    check(cancellation)?;
    if bytes.len() > MAX_CSV_SOURCE_BYTES {
        return Err(CsvError::source(CsvErrorCode::SourceByteLimit).into());
    }
    let mut prefix = Prefix::default();
    for chunk in bytes.chunks(4096) {
        check(cancellation)?;
        prefix.observe(chunk);
        if prefix.reserved.is_some() {
            break;
        }
    }
    if prefix.reserved.unwrap_or(false) {
        let source = scan_source(
            &mut &*bytes,
            AssignmentCsvReadFormat::CanonicalV1,
            cancellation,
            |_| Ok(()),
        )?;
        Ok(AssignmentCsvDetection::CanonicalV1 {
            schema_version: 1,
            source,
        })
    } else {
        let detection = detect_people_csv(bytes, cancellation)?;
        Ok(AssignmentCsvDetection::Mapped {
            schema_version: 1,
            dialects: detection.dialects,
        })
    }
}

/// Returns one explicitly selected inert record only after validating the whole source.
/// Canonical cells are exact canonical identities; mapped cells use the existing sample bound.
///
/// # Errors
/// Rejects invalid source bytes/framing, mismatched parsing choice, resource limits and
/// cancellation, including failures after the requested record.
pub fn sample_assignment_csv_record<R: Read + ?Sized>(
    input: &mut R,
    format: AssignmentCsvReadFormat,
    record_number: u32,
    cancellation: &CancellationToken,
) -> Result<Option<CsvSampleRecord>, AssignmentImportError> {
    if let AssignmentCsvReadFormat::Mapped { dialect } = format {
        let mut reader = ObservedReader::new(input, false);
        let sample = sample_people_csv_record(&mut reader, dialect, record_number, cancellation)?;
        reader.check_format(false)?;
        check(cancellation)?;
        return Ok(sample);
    }
    let mut sample = match record_number {
        1 => Some(vec!["eutheto/assignments".to_owned(), "1".to_owned()]),
        2 => Some(vec![
            "assignment_id".to_owned(),
            "person_id".to_owned(),
            "shift_id".to_owned(),
        ]),
        _ => None,
    };
    scan_source(input, format, cancellation, |record| {
        if let ImportRecord::Canonical { number, id, pair } = record
            && number == record_number
        {
            sample = Some(vec![
                id.as_str().to_owned(),
                pair.person_id.to_string(),
                pair.shift_id.to_string(),
            ]);
        }
        Ok(())
    })?;
    Ok(sample.map(|cells| CsvSampleRecord {
        record: record_number,
        cells: cells
            .into_iter()
            .map(|text| CsvSampleCell {
                text,
                truncated: false,
            })
            .collect(),
    }))
}

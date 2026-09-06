use super::types::{
    CsvDialect, CsvDialectDetection, CsvError, CsvErrorCode, CsvRecord, CsvSampleCell,
    CsvSampleRecord, CsvSource, MAX_CSV_CELL_BYTES, MAX_CSV_COLUMNS, MAX_CSV_DETECTION_BYTES,
    MAX_CSV_LOGICAL_RECORDS, MAX_CSV_RECORD_BYTES, MAX_CSV_SAMPLE_CELL_BYTES,
    MAX_CSV_SAMPLE_RECORDS, MAX_CSV_SOURCE_BYTES, PeopleCsvDetection,
};
use csv_core::{ReadRecordResult, ReaderBuilder};
use eutheto_types::CancellationToken;
use std::io::{self, Read};

const INPUT_CHUNK_BYTES: usize = 4096;

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), CsvError> {
    if cancellation.is_cancelled() {
        Err(CsvError::source(CsvErrorCode::Cancelled))
    } else {
        Ok(())
    }
}

fn has_control(text: &str) -> bool {
    text.chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\r' | '\n'))
}

fn candidate_local(error: CsvError) -> bool {
    matches!(
        error.code,
        CsvErrorCode::CellLimit
            | CsvErrorCode::RecordByteLimit
            | CsvErrorCode::ColumnLimit
            | CsvErrorCode::LogicalRecordLimit
    ) || (error.record.is_some()
        && matches!(
            error.code,
            CsvErrorCode::InvalidUtf8 | CsvErrorCode::BinaryControl
        ))
}

/// Keeps original bytes separate from decoded CSV. At most three incomplete
/// UTF-8 bytes cross fills, and none are presented to csv-core before validation.
struct RawInput<'a, R: Read + ?Sized> {
    reader: &'a mut R,
    bytes: [u8; INPUT_CHUNK_BYTES + 3],
    buffered: usize,
    valid: usize,
    first: bool,
    eof: bool,
    count: u64,
    hash: blake3::Hasher,
}

impl<'a, R: Read + ?Sized> RawInput<'a, R> {
    fn new(reader: &'a mut R) -> Self {
        Self {
            reader,
            bytes: [0; INPUT_CHUNK_BYTES + 3],
            buffered: 0,
            valid: 0,
            first: true,
            eof: false,
            count: 0,
            hash: blake3::Hasher::new(),
        }
    }

    fn fill(&mut self, cancellation: &CancellationToken) -> Result<(), CsvError> {
        self.bytes.copy_within(self.valid..self.buffered, 0);
        self.buffered -= self.valid;
        self.valid = 0;
        loop {
            check_cancelled(cancellation)?;
            if !self.eof {
                // One extra original byte distinguishes the inclusive source cap
                // from genuine EOF without reading an unbounded reader ahead.
                let available = usize::try_from(MAX_CSV_SOURCE_BYTES as u64 + 1 - self.count)
                    .map_err(|_| CsvError::source(CsvErrorCode::SourceByteLimit))?;
                let end = self.bytes.len().min(self.buffered + available);
                let read = match self.reader.read(&mut self.bytes[self.buffered..end]) {
                    Ok(read) => read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err(CsvError::source(CsvErrorCode::Io)),
                };
                check_cancelled(cancellation)?;
                self.hash
                    .update(&self.bytes[self.buffered..self.buffered + read]);
                self.count += read as u64;
                self.buffered += read;
                if self.count > MAX_CSV_SOURCE_BYTES as u64 {
                    return Err(CsvError::source(CsvErrorCode::SourceByteLimit));
                }
                self.eof = read == 0;
            }
            // Preserve the first up-to-four ORIGINAL bytes for both BOM checks
            // and csv-core's own single UTF-8 BOM removal.
            if self.first && self.buffered < 4 && !self.eof {
                continue;
            }
            let bytes = &self.bytes[..self.buffered];
            if self.first
                && (bytes.starts_with(&[0xff, 0xfe])
                    || bytes.starts_with(&[0xfe, 0xff])
                    || bytes.starts_with(&[0, 0, 0xfe, 0xff]))
            {
                return Err(CsvError::source(CsvErrorCode::UnsupportedEncoding));
            }
            self.valid = match std::str::from_utf8(bytes) {
                Ok(_) => bytes.len(),
                Err(error) if error.error_len().is_none() && !self.eof => error.valid_up_to(),
                Err(_) => return Err(CsvError::source(CsvErrorCode::InvalidUtf8)),
            };
            let text = std::str::from_utf8(&bytes[..self.valid])
                .map_err(|_| CsvError::source(CsvErrorCode::InvalidUtf8))?;
            if has_control(text) {
                return Err(CsvError::source(CsvErrorCode::BinaryControl));
            }
            // A BOM-only first slice would make csv-core see EOF internally.
            // Await the next complete code point if the source has more bytes.
            let bom_only = self.first && self.valid == 3 && bytes.starts_with(&[0xef, 0xbb, 0xbf]);
            if self.eof || (self.valid != 0 && !bom_only) {
                self.first = false;
                return Ok(());
            }
        }
    }
}

pub(crate) fn scan_csv<R: Read + ?Sized>(
    reader: &mut R,
    dialect: CsvDialect,
    cancellation: &CancellationToken,
    mut visit: impl FnMut(CsvRecord<'_>) -> Result<(), CsvError>,
) -> Result<CsvSource, CsvError> {
    check_cancelled(cancellation)?;
    let mut raw = RawInput::new(reader);
    let result = scan_records(&mut raw, dialect, cancellation, &mut visit);
    // A dialect-local overflow must not conceal a later raw-source failure,
    // even when all three detection candidates exceed decoded limits early.
    if result.as_ref().is_err_and(|error| candidate_local(*error)) {
        while !raw.eof {
            raw.fill(cancellation)?;
        }
    }
    check_cancelled(cancellation)?;
    let logical_records = result?;
    Ok(CsvSource {
        raw_bytes: raw.count,
        blake3: raw.hash.finalize().to_hex().to_string(),
        logical_records,
    })
}

fn scan_records<R: Read + ?Sized>(
    raw: &mut RawInput<'_, R>,
    dialect: CsvDialect,
    cancellation: &CancellationToken,
    visit: &mut impl FnMut(CsvRecord<'_>) -> Result<(), CsvError>,
) -> Result<u32, CsvError> {
    let mut parser = ReaderBuilder::new().delimiter(dialect.delimiter()).build();
    let mut record = vec![0; MAX_CSV_RECORD_BYTES + 1].into_boxed_slice();
    let mut ends = [0; MAX_CSV_COLUMNS + 1];
    let (mut used, mut fields, mut position, mut records) = (0, 0, 0, 0);
    loop {
        check_cancelled(cancellation)?;
        if position == raw.valid && !raw.eof {
            raw.fill(cancellation)?;
            position = 0;
        }
        let field_start = if fields == 0 { 0 } else { ends[fields - 1] };
        let allowance = MAX_CSV_CELL_BYTES + 1 - (used - field_start);
        let output_end = record.len().min(used + allowance);
        let (result, consumed, written, completed) = parser.read_record(
            &raw.bytes[position..raw.valid],
            &mut record[used..output_end],
            &mut ends[fields..],
        );
        position += consumed;
        used += written;
        let previous_fields = fields;
        fields += completed;
        let number = records + 1;
        if used > MAX_CSV_RECORD_BYTES {
            return Err(CsvError::at(CsvErrorCode::RecordByteLimit, number));
        }
        if fields > MAX_CSV_COLUMNS {
            return Err(CsvError::at(CsvErrorCode::ColumnLimit, number));
        }
        let mut start = field_start;
        for &end in &ends[previous_fields..fields] {
            // csv-core reports absolute, record-relative ends even when output
            // is a tail slice. Adding `used` here would corrupt chunked fields.
            if end - start > MAX_CSV_CELL_BYTES {
                return Err(CsvError::at(CsvErrorCode::CellLimit, number));
            }
            start = end;
        }
        if used - start > MAX_CSV_CELL_BYTES {
            return Err(CsvError::at(CsvErrorCode::CellLimit, number));
        }
        check_cancelled(cancellation)?;
        match result {
            ReadRecordResult::Record => {
                records += 1;
                if records > MAX_CSV_LOGICAL_RECORDS {
                    return Err(CsvError::at(CsvErrorCode::LogicalRecordLimit, records));
                }
                let mut start = 0;
                let mut cells = [""; MAX_CSV_COLUMNS];
                for (cell, &end) in cells[..fields].iter_mut().zip(&ends[..fields]) {
                    let text = std::str::from_utf8(&record[start..end])
                        .map_err(|_| CsvError::at(CsvErrorCode::InvalidUtf8, records))?;
                    if has_control(text) {
                        return Err(CsvError::at(CsvErrorCode::BinaryControl, records));
                    }
                    *cell = text;
                    start = end;
                }
                visit(CsvRecord {
                    number: records,
                    cells: &cells[..fields],
                })?;
                check_cancelled(cancellation)?;
                used = 0;
                fields = 0;
            }
            ReadRecordResult::End => return Ok(records),
            _ if consumed == 0 && written == 0 && completed == 0 => {
                return Err(CsvError::at(CsvErrorCode::ParserNoProgress, number));
            }
            _ => {}
        }
    }
}

/// Presents every supported dialect without choosing a delimiter or header.
///
/// # Errors
///
/// Rejects invalid original UTF-8, binary controls, unsupported encodings,
/// excessive source bytes, cancellation, and excessive detection output.
/// Dialect-local record, column, and cell failures remain candidate results.
pub fn detect_people_csv(
    input: &[u8],
    cancellation: &CancellationToken,
) -> Result<PeopleCsvDetection, CsvError> {
    check_cancelled(cancellation)?;
    let mut dialects = Vec::with_capacity(3);
    for dialect in [CsvDialect::Comma, CsvDialect::Semicolon, CsvDialect::Tab] {
        let mut samples = Vec::with_capacity(MAX_CSV_SAMPLE_RECORDS);
        let mut first_columns = None;
        let mut consistent = true;
        let source = scan_csv(
            &mut io::Cursor::new(input),
            dialect,
            cancellation,
            |record| {
                let columns = u8::try_from(record.cells.len())
                    .map_err(|_| CsvError::at(CsvErrorCode::ColumnLimit, record.number))?;
                match first_columns {
                    None => first_columns = Some(columns),
                    Some(first) if first != columns => consistent = false,
                    Some(_) => {}
                }
                if samples.len() < MAX_CSV_SAMPLE_RECORDS {
                    samples.push(CsvSampleRecord {
                        record: record.number,
                        cells: record
                            .cells
                            .iter()
                            .map(|cell| {
                                let mut end = cell.len().min(MAX_CSV_SAMPLE_CELL_BYTES);
                                while !cell.is_char_boundary(end) {
                                    end -= 1;
                                }
                                CsvSampleCell {
                                    text: cell[..end].to_owned(),
                                    truncated: end != cell.len(),
                                }
                            })
                            .collect(),
                    });
                }
                Ok(())
            },
        );
        dialects.push(match source {
            Ok(source) => CsvDialectDetection::Candidate {
                dialect,
                source,
                consistent_columns: if consistent { first_columns } else { None },
                samples,
            },
            Err(error) if candidate_local(error) => {
                CsvDialectDetection::Rejected { dialect, error }
            }
            Err(error) => return Err(error),
        });
    }
    let detection = PeopleCsvDetection {
        schema_version: 1,
        dialects,
    };
    // Fixed sample cardinalities bound allocation before this independent wire
    // budget check (JSON escaping can expand a sample's source byte count).
    eutheto_domain_api::bounded_json_size(&detection, MAX_CSV_DETECTION_BYTES)
        .map_err(|_| CsvError::source(CsvErrorCode::DetectionLimit))?;
    check_cancelled(cancellation)?;
    Ok(detection)
}

#[cfg(test)]
mod tests;

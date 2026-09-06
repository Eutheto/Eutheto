use super::*;
use crate::people_csv::MAX_CSV_DATA_RECORDS;

struct Chunks<'a> {
    remaining: &'a [u8],
    maximum: usize,
}

impl Read for Chunks<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = output.len().min(self.maximum).min(self.remaining.len());
        output[..count].copy_from_slice(&self.remaining[..count]);
        self.remaining = &self.remaining[count..];
        Ok(count)
    }
}

fn collect(input: &[u8], chunk: usize) -> Result<(CsvSource, Vec<Vec<String>>), CsvError> {
    let mut rows = Vec::new();
    let source = scan_csv(
        &mut Chunks {
            remaining: input,
            maximum: chunk,
        },
        CsvDialect::Comma,
        &CancellationToken::new(),
        |record| {
            assert_eq!(record.number as usize, rows.len() + 1);
            rows.push(record.cells.iter().map(|cell| (*cell).to_owned()).collect());
            Ok(())
        },
    )?;
    Ok((source, rows))
}

fn scan(input: &[u8]) -> Result<CsvSource, CsvError> {
    scan_csv(
        &mut io::Cursor::new(input),
        CsvDialect::Comma,
        &CancellationToken::new(),
        |_| Ok(()),
    )
}

#[test]
fn rejects_raw_utf8_that_quote_stripping_would_repair() -> Result<(), Box<dyn std::error::Error>> {
    for chunk in [1, 2, 3, 4, 4096] {
        assert_eq!(
            collect(&[0x22, 0xc3, 0x22, 0xa9], chunk)
                .err()
                .ok_or("expected CSV rejection")?,
            CsvError::source(CsvErrorCode::InvalidUtf8),
        );
    }
    assert_eq!(
        detect_people_csv(&[0x22, 0xc3, 0x22, 0xa9], &CancellationToken::new())
            .err()
            .ok_or("expected CSV rejection")?,
        CsvError::source(CsvErrorCode::InvalidUtf8),
    );
    Ok(())
}

#[test]
fn original_bom_is_coalesced_stripped_once_and_hashed() -> Result<(), Box<dyn std::error::Error>> {
    for input in [
        b"\xef\xbb\xbfname,value\n".as_slice(),
        b"\xef\xbb\xbf\xc3\xa9,value\n".as_slice(),
        b"\xef\xbb\xbf\xef\xbb\xbfname,value\n".as_slice(),
    ] {
        let expected = std::str::from_utf8(&input[3..input.len() - 1])?
            .split(',')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for chunk in [1, 2, 3, 4, 4096] {
            let (source, rows) = collect(input, chunk)?;
            assert_eq!(rows, vec![expected.clone()]);
            assert_eq!(source.raw_bytes, input.len() as u64);
            assert_eq!(source.blake3, blake3::hash(input).to_hex().to_string());
        }
    }
    for chunk in [1, 2, 3, 4096] {
        let (source, rows) = collect(b"\xef\xbb\xbf", chunk)?;
        assert!(rows.is_empty());
        assert_eq!(source.logical_records, 0);
        assert_eq!(source.raw_bytes, 3);
        assert_eq!(
            source.blake3,
            blake3::hash(b"\xef\xbb\xbf").to_hex().to_string()
        );
    }
    Ok(())
}

#[test]
fn unsupported_boms_incomplete_utf8_and_controls_are_source_errors()
-> Result<(), Box<dyn std::error::Error>> {
    for input in [
        b"\xff\xfea\0".as_slice(),
        b"\xfe\xff\0a".as_slice(),
        b"\xff\xfe\0\0".as_slice(),
        b"\0\0\xfe\xff".as_slice(),
    ] {
        assert_eq!(
            collect(input, 1).err().ok_or("expected CSV rejection")?,
            CsvError::source(CsvErrorCode::UnsupportedEncoding)
        );
    }
    for input in [
        b"a\xc3".as_slice(),
        b"\xef\xbb".as_slice(),
        b"x\xf0\x9f\x98".as_slice(),
    ] {
        assert_eq!(
            collect(input, 1).err().ok_or("expected CSV rejection")?,
            CsvError::source(CsvErrorCode::InvalidUtf8)
        );
    }
    for control in [
        '\0', '\u{1}', '\u{b}', '\u{1f}', '\u{7f}', '\u{85}', '\u{9f}',
    ] {
        let input = format!("a,\"{control}\"\n");
        assert_eq!(
            collect(input.as_bytes(), 1)
                .err()
                .ok_or("expected CSV rejection")?,
            CsvError::source(CsvErrorCode::BinaryControl)
        );
    }
    assert_eq!(collect(b"\"a\tb\r\nc\"", 1)?.1, vec![vec!["a\tb\r\nc"]]);
    Ok(())
}

#[test]
fn eof_flushes_last_record_and_preserves_quoted_multiline_numbering()
-> Result<(), Box<dyn std::error::Error>> {
    let input = b"\r\n\"a\r\nb\",\"c\"\"d\"\r\n\nlast,";
    let expected = vec![vec!["a\r\nb", "c\"d"], vec!["last", ""]];
    for chunk in [1, 2, 7, 4096] {
        let (source, rows) = collect(input, chunk)?;
        assert_eq!(rows, expected);
        assert_eq!(source.logical_records, 2);
        assert_eq!(source.raw_bytes, input.len() as u64);
        assert_eq!(source.blake3, blake3::hash(input).to_hex().to_string());
    }
    // Follow csv-core's permissive EOF semantics rather than adding a second
    // quote parser that rejects a parse accepted by the pinned upstream.
    assert_eq!(
        collect(b"\"unterminated\nfield", 1)?.1,
        vec![vec!["unterminated\nfield"]]
    );
    Ok(())
}

#[test]
fn field_ends_stay_record_relative_across_unequal_chunked_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = vec![
        "a".to_owned(),
        "b".repeat(8193),
        "é".repeat(5000),
        String::new(),
        "tail".to_owned(),
    ];
    let input = format!("{}\nshort,row", expected.join(","));
    for chunk in [1, 17, 4096] {
        let (source, rows) = collect(input.as_bytes(), chunk)?;
        assert_eq!(
            rows,
            vec![expected.clone(), vec!["short".to_owned(), "row".to_owned()]]
        );
        assert_eq!(source.logical_records, 2);
    }
    Ok(())
}

#[test]
fn inclusive_cell_limit_distinguishes_eof_delimiter_and_newline()
-> Result<(), Box<dyn std::error::Error>> {
    for suffix in ["", ",next", "\nnext"] {
        let exact = format!("{}{suffix}", "a".repeat(MAX_CSV_CELL_BYTES));
        let (_, rows) = collect(exact.as_bytes(), 4096)?;
        assert_eq!(rows[0][0], "a".repeat(MAX_CSV_CELL_BYTES));
        let overflow = format!("{}{suffix}", "a".repeat(MAX_CSV_CELL_BYTES + 1));
        assert_eq!(
            scan(overflow.as_bytes())
                .err()
                .ok_or("expected CSV rejection")?,
            CsvError::at(CsvErrorCode::CellLimit, 1)
        );
    }
    let quoted = format!("\"{}\"", "\"\"".repeat(MAX_CSV_CELL_BYTES));
    assert_eq!(
        collect(quoted.as_bytes(), 37)?.1,
        vec![vec!["\"".repeat(MAX_CSV_CELL_BYTES)]]
    );
    Ok(())
}

#[test]
fn inclusive_record_byte_limit_is_decoded_not_raw() -> Result<(), Box<dyn std::error::Error>> {
    let full =
        vec!["x".repeat(MAX_CSV_CELL_BYTES); MAX_CSV_RECORD_BYTES / MAX_CSV_CELL_BYTES].join(",");
    for suffix in ["", ",", "\n"] {
        assert_eq!(
            scan(format!("{full}{suffix}").as_bytes())?.logical_records,
            1
        );
        let overflow = format!("{full},x{suffix}");
        assert_eq!(
            scan(overflow.as_bytes())
                .err()
                .ok_or("expected CSV rejection")?,
            CsvError::at(CsvErrorCode::RecordByteLimit, 1)
        );
    }
    Ok(())
}

#[test]
fn inclusive_column_limit_counts_empty_fields_at_eof_and_newline()
-> Result<(), Box<dyn std::error::Error>> {
    for suffix in ["", "\n"] {
        let exact = format!("{}{suffix}", ",".repeat(MAX_CSV_COLUMNS - 1));
        assert_eq!(
            collect(exact.as_bytes(), 1)?.1,
            vec![vec![""; MAX_CSV_COLUMNS]]
        );
        let overflow = format!("{}{suffix}", ",".repeat(MAX_CSV_COLUMNS));
        assert_eq!(
            scan(overflow.as_bytes())
                .err()
                .ok_or("expected CSV rejection")?,
            CsvError::at(CsvErrorCode::ColumnLimit, 1)
        );
    }
    let overflow_before_more_input = format!("{}x,next", ",".repeat(MAX_CSV_COLUMNS));
    assert_eq!(
        scan(overflow_before_more_input.as_bytes())
            .err()
            .ok_or("expected CSV rejection")?,
        CsvError::at(CsvErrorCode::ColumnLimit, 1)
    );
    Ok(())
}

#[test]
fn raw_source_cap_includes_ignored_lines_and_bom() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::with_capacity(MAX_CSV_SOURCE_BYTES + 1);
    input.extend_from_slice(b"\xef\xbb\xbf");
    input.resize(MAX_CSV_SOURCE_BYTES, b'\n');
    let source = scan(&input)?;
    assert_eq!(source.raw_bytes, MAX_CSV_SOURCE_BYTES as u64);
    assert_eq!(source.logical_records, 0);
    assert_eq!(source.blake3, blake3::hash(&input).to_hex().to_string());
    input.push(b'\n');
    assert_eq!(
        scan(&input).err().ok_or("expected CSV rejection")?,
        CsvError::source(CsvErrorCode::SourceByteLimit)
    );
    assert_eq!(
        detect_people_csv(&input, &CancellationToken::new())
            .err()
            .ok_or("expected CSV rejection")?,
        CsvError::source(CsvErrorCode::SourceByteLimit)
    );
    Ok(())
}

#[test]
fn logical_record_cap_is_header_neutral_and_inclusive() -> Result<(), Box<dyn std::error::Error>> {
    for suffix in ["", "\n"] {
        let input = format!(
            "a{}{}",
            "\na".repeat(MAX_CSV_LOGICAL_RECORDS as usize - 1),
            suffix
        );
        assert_eq!(
            scan(input.as_bytes())?.logical_records,
            MAX_CSV_LOGICAL_RECORDS
        );
        let overflow = format!(
            "{input}{}a{suffix}",
            if suffix.is_empty() { "\n" } else { "" }
        );
        assert_eq!(
            scan(overflow.as_bytes())
                .err()
                .ok_or("expected CSV rejection")?,
            CsvError::at(
                CsvErrorCode::LogicalRecordLimit,
                MAX_CSV_LOGICAL_RECORDS + 1
            )
        );
    }
    let input = "a\n".repeat(MAX_CSV_LOGICAL_RECORDS as usize);
    let detection = detect_people_csv(input.as_bytes(), &CancellationToken::new())?;
    for dialect in detection.dialects {
        let CsvDialectDetection::Candidate { source, .. } = dialect else {
            return Err("header-neutral candidate rejected".into());
        };
        assert_eq!(source.logical_records, MAX_CSV_DATA_RECORDS + 1);
    }
    Ok(())
}

#[test]
fn detection_keeps_dialect_local_failures_and_never_falls_back()
-> Result<(), Box<dyn std::error::Error>> {
    let too_many_comma_columns = ",".repeat(MAX_CSV_COLUMNS);
    let detection =
        detect_people_csv(too_many_comma_columns.as_bytes(), &CancellationToken::new())?;
    assert!(
        matches!(&detection.dialects[0], CsvDialectDetection::Rejected { dialect: CsvDialect::Comma, error } if error.code == CsvErrorCode::ColumnLimit)
    );
    for candidate in &detection.dialects[1..] {
        assert!(matches!(
            candidate,
            CsvDialectDetection::Candidate {
                consistent_columns: Some(1),
                ..
            }
        ));
    }
    let semicolon_fields = format!(
        "{};{}",
        "x".repeat(MAX_CSV_CELL_BYTES),
        "y".repeat(MAX_CSV_CELL_BYTES)
    );
    let detection = detect_people_csv(semicolon_fields.as_bytes(), &CancellationToken::new())?;
    assert!(
        matches!(&detection.dialects[0], CsvDialectDetection::Rejected { error, .. } if error.code == CsvErrorCode::CellLimit)
    );
    assert!(matches!(
        &detection.dialects[1],
        CsvDialectDetection::Candidate {
            consistent_columns: Some(2),
            ..
        }
    ));
    let all_too_long = "x".repeat(MAX_CSV_CELL_BYTES + 1);
    let detection = detect_people_csv(all_too_long.as_bytes(), &CancellationToken::new())?;
    assert_eq!(detection.dialects.len(), 3);
    assert!(detection.dialects.iter().all(|candidate| matches!(candidate, CsvDialectDetection::Rejected { error, .. } if error.code == CsvErrorCode::CellLimit)));
    Ok(())
}

#[test]
fn early_candidate_failure_cannot_hide_late_invalid_original_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    for tail in [b"\xc3".as_slice(), b"\0".as_slice()] {
        let mut input = vec![b'x'; MAX_CSV_CELL_BYTES + INPUT_CHUNK_BYTES];
        input.extend_from_slice(tail);
        let expected = if tail == b"\0" {
            CsvErrorCode::BinaryControl
        } else {
            CsvErrorCode::InvalidUtf8
        };
        assert_eq!(
            detect_people_csv(&input, &CancellationToken::new())
                .err()
                .ok_or("expected CSV rejection")?,
            CsvError::source(expected)
        );
    }
    Ok(())
}

#[test]
fn samples_truncate_on_utf8_boundaries_without_changing_source_or_consistency()
-> Result<(), Box<dyn std::error::Error>> {
    let cell = format!("{}éand more", "x".repeat(63));
    let input = format!("{cell},second\na,b\nlast\n");
    let detection = detect_people_csv(input.as_bytes(), &CancellationToken::new())?;
    let CsvDialectDetection::Candidate {
        source,
        samples,
        consistent_columns,
        ..
    } = &detection.dialects[0]
    else {
        return Err("comma candidate rejected".into());
    };
    assert_eq!(*source, scan(input.as_bytes())?);
    assert_eq!(source.logical_records, 3);
    assert_eq!(
        source.blake3,
        blake3::hash(input.as_bytes()).to_hex().to_string()
    );
    assert_eq!(*consistent_columns, None);
    assert_eq!(samples.len(), 2);
    assert_eq!(
        samples[0].cells[0],
        CsvSampleCell {
            text: "x".repeat(63),
            truncated: true
        }
    );
    assert_eq!(
        samples[0].cells[1],
        CsvSampleCell {
            text: "second".to_owned(),
            truncated: false
        }
    );
    assert!(serde_json::to_vec(&detection)?.len() <= MAX_CSV_DETECTION_BYTES);
    let empty = detect_people_csv(b"", &CancellationToken::new())?;
    for candidate in empty.dialects {
        let CsvDialectDetection::Candidate {
            source,
            samples,
            consistent_columns,
            ..
        } = candidate
        else {
            return Err("empty candidate rejected".into());
        };
        assert_eq!(
            source,
            CsvSource {
                raw_bytes: 0,
                blake3: blake3::hash(b"").to_hex().to_string(),
                logical_records: 0
            }
        );
        assert_eq!(consistent_columns, None);
        assert!(samples.is_empty());
    }
    Ok(())
}

#[test]
fn record_count_failure_is_dialect_local_not_a_global_data_limit()
-> Result<(), Box<dyn std::error::Error>> {
    let input = format!("x;\"{}\";\"{}\"", "\nx".repeat(5000), "\nx".repeat(5001));
    let detection = detect_people_csv(input.as_bytes(), &CancellationToken::new())?;
    assert!(
        matches!(&detection.dialects[0], CsvDialectDetection::Rejected { error, .. }
    if *error == CsvError::at(CsvErrorCode::LogicalRecordLimit, MAX_CSV_LOGICAL_RECORDS + 1))
    );
    let CsvDialectDetection::Candidate {
        source,
        consistent_columns,
        ..
    } = &detection.dialects[1]
    else {
        return Err("valid semicolon multiline candidate rejected".into());
    };
    assert_eq!(source.logical_records, 1);
    assert_eq!(*consistent_columns, Some(3));
    assert_eq!(
        source.blake3,
        blake3::hash(input.as_bytes()).to_hex().to_string()
    );
    Ok(())
}

#[test]
fn cancellation_is_observed_before_reads_between_records_and_after_read()
-> Result<(), Box<dyn std::error::Error>> {
    struct CountReads(usize);
    impl Read for CountReads {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.0 += 1;
            Ok(0)
        }
    }
    struct CancelRead<'a>(&'a CancellationToken);
    impl Read for CancelRead<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            output[0] = b'a';
            self.0.cancel();
            Ok(1)
        }
    }

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        detect_people_csv(b"", &cancellation)
            .err()
            .ok_or("expected cancellation")?,
        CsvError::source(CsvErrorCode::Cancelled)
    );
    let mut input = CountReads(0);
    assert_eq!(
        scan_csv(&mut input, CsvDialect::Comma, &cancellation, |_| Ok(())),
        Err(CsvError::source(CsvErrorCode::Cancelled))
    );
    assert_eq!(input.0, 0);

    let cancellation = CancellationToken::new();
    let mut visited = Vec::new();
    let result = scan_csv(
        &mut io::Cursor::new(b"a\nb\n"),
        CsvDialect::Comma,
        &cancellation,
        |record| {
            visited.push(record.number);
            cancellation.cancel();
            Ok(())
        },
    );
    assert_eq!(result, Err(CsvError::source(CsvErrorCode::Cancelled)));
    assert_eq!(visited, vec![1]);

    let cancellation = CancellationToken::new();
    let mut visited_after_read = false;
    let result = scan_csv(
        &mut CancelRead(&cancellation),
        CsvDialect::Comma,
        &cancellation,
        |_| {
            visited_after_read = true;
            Ok(())
        },
    );
    assert_eq!(result, Err(CsvError::source(CsvErrorCode::Cancelled)));
    assert!(!visited_after_read);
    Ok(())
}

#[test]
fn io_errors_never_expose_source_details() -> Result<(), Box<dyn std::error::Error>> {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("private path and submitted row"))
        }
    }
    let error = scan_csv(
        &mut Broken,
        CsvDialect::Comma,
        &CancellationToken::new(),
        |_| Ok(()),
    )
    .err()
    .ok_or("expected CSV rejection")?;
    assert_eq!(error, CsvError::source(CsvErrorCode::Io));
    assert!(!error.to_string().contains("private"));
    Ok(())
}

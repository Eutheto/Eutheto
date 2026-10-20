mod support;

use eutheto_types::{CancellationToken, Revision, ScenarioDocument};
use eutheto_workforce::{
    commands,
    eligibility_import::*,
    people_csv::{CsvError, CsvErrorCode, PeopleImportDisposition},
};
use serde_json::json;
use std::{
    error::Error,
    fmt::Write as _,
    io::{self, Cursor, Read},
};
use support::{fixture, id};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn address(record: u32, column: u16) -> EligibilityMatrixCellAddress {
    EligibilityMatrixCellAddress { record, column }
}

fn preview(
    document: &ScenarioDocument,
    source: &[u8],
    mapping: &EligibilityMatrixMapping,
    skips: &[EligibilityMatrixCellAddress],
) -> Result<EligibilityMatrixPreview, CsvError> {
    preview_eligibility_matrix(
        document,
        Revision::INITIAL,
        &mut Cursor::new(source),
        mapping,
        skips,
        &CancellationToken::default(),
    )
}

fn detect(source: &[u8]) -> Result<EligibilityMatrixDetection, CsvError> {
    detect_eligibility_matrix(&mut Cursor::new(source), &CancellationToken::default())
}

#[test]
fn sparse_raw_preservation_row_major_additions_and_inverse_round_trip() -> TestResult {
    let mut document = fixture()?;
    for index in [21, 22, 23] {
        let mut assignment_type = document.domain.entities[&id(4).parse()?].clone();
        assignment_type["id"] = json!(id(index));
        document
            .domain
            .entities
            .insert(id(index).parse()?, assignment_type);
    }
    let person = document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?;
    person["id"] = json!(id(1).to_ascii_uppercase());
    person["eligibleAssignmentTypeIds"] =
        json!([id(4).to_ascii_uppercase(), id(22).to_ascii_uppercase()]);
    person["display"] = json!({"color":"#102030","avatarInitials":"RV"});
    let source = format!(
        "personId\t{}\t{}\t{}\t{}\t{}\n{}\ttrue\ttrue\ttrue\t\tfalse\n",
        id(21),
        id(23),
        id(21),
        id(4),
        id(22),
        id(1).to_ascii_uppercase()
    );
    let proposal = preview(
        &document,
        source.as_bytes(),
        &EligibilityMatrixMapping::default(),
        &[address(2, 1)],
    )?;
    assert_eq!((proposal.columns, proposal.cells), (6, 5));
    assert_eq!(proposal.disposition, PeopleImportDisposition::Reviewable);
    assert_eq!(
        proposal.rows[0].issue,
        Some(EligibilityRowIssue::DuplicatePair)
    );
    assert_eq!(
        (proposal.rows[3].before, proposal.rows[3].after),
        (Some(true), Some(true))
    );
    assert_eq!(proposal.rows[3].status, EligibilityRowStatus::Unchanged);
    let batch = proposal.batch.as_ref().ok_or("batch")?;
    assert_eq!(batch.commands.len(), 1);
    let mutation = commands::apply_batch(&document, batch)?;
    let mut expected = document.clone();
    expected
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] =
        json!([id(4).to_ascii_uppercase(), id(23), id(21)]);
    assert_eq!(mutation.document, expected);
    let inverse = commands::apply_batch(&mutation.document, &mutation.inverse)?;
    assert_eq!(inverse.document, document);
    assert_eq!(
        commands::apply_batch(&inverse.document, &inverse.inverse)?.document,
        expected
    );
    Ok(())
}

#[test]
fn exact_membership_blanks_aliases_and_explicit_duplicate_skips() -> TestResult {
    let document = fixture()?;
    let mut mapping = EligibilityMatrixMapping::default();
    mapping
        .person_mappings
        .insert("Shared".to_owned(), id(1).parse()?);
    mapping
        .assignment_type_mappings
        .insert("Shared".to_owned(), id(4).parse()?);
    let source = format!(
        "\tShared\t{}\nShared\t\tfalse\n",
        id(4).to_ascii_uppercase()
    );
    let blocked = preview(&document, source.as_bytes(), &mapping, &[])?;
    assert_eq!(blocked.disposition, PeopleImportDisposition::Blocked);
    assert!(
        blocked
            .rows
            .iter()
            .all(|cell| cell.issue == Some(EligibilityRowIssue::DuplicatePair))
    );
    assert!(blocked.rows[0].after.is_none());
    let selected = preview(&document, source.as_bytes(), &mapping, &[address(2, 2)])?;
    assert_eq!(selected.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(
        (selected.rows[0].before, selected.rows[0].after),
        (Some(true), Some(true))
    );
    assert_eq!(
        selected.excluded_cells,
        [EligibilityMatrixExcludedCell {
            cell: address(2, 2),
            skipped: true,
            issue: Some(EligibilityRowIssue::DuplicatePair),
        }]
    );
    for token in ["TRUE", "False", " true", "false ", "1", " "] {
        let source = format!("\tShared\nShared\t{token}\n");
        let blocked = preview(&document, source.as_bytes(), &mapping, &[])?;
        assert_eq!(
            blocked.rows[0].issue,
            Some(EligibilityRowIssue::InvalidMembership)
        );
        assert_eq!(blocked.disposition, PeopleImportDisposition::Blocked);
        let skipped = preview(&document, source.as_bytes(), &mapping, &[address(2, 1)])?;
        assert_eq!(skipped.disposition, PeopleImportDisposition::NoChanges);
        assert_eq!(
            skipped.excluded_cells[0].issue,
            Some(EligibilityRowIssue::InvalidMembership)
        );
    }
    for (person, assignment_type, issue) in [
        (
            "unknown".to_owned(),
            id(4),
            EligibilityRowIssue::UnresolvedPerson,
        ),
        (
            id(1),
            "unknown".to_owned(),
            EligibilityRowIssue::UnresolvedAssignmentType,
        ),
        (id(5), id(4), EligibilityRowIssue::UnresolvedPerson),
        (id(1), id(5), EligibilityRowIssue::UnresolvedAssignmentType),
        ("x".repeat(257), id(4), EligibilityRowIssue::InvalidToken),
        (id(1), "x".repeat(257), EligibilityRowIssue::InvalidToken),
    ] {
        let source = format!("\t{assignment_type}\n{person}\t\n");
        let blocked = preview(&document, source.as_bytes(), &mapping, &[])?;
        assert_eq!(blocked.rows[0].issue, Some(issue));
        assert!(blocked.rows[0].after.is_none());
        assert_eq!(blocked.disposition, PeopleImportDisposition::Blocked);
    }
    mapping.person_mappings.insert(id(999), id(1).parse()?);
    assert_eq!(
        preview(&document, source.as_bytes(), &mapping, &[])
            .err()
            .ok_or("UUID alias")?
            .code,
        CsvErrorCode::InvalidMapping
    );
    Ok(())
}

#[test]
fn skipped_valid_blank_keeps_known_membership_facts() -> TestResult {
    for eligible in [false, true] {
        let mut document = fixture()?;
        document
            .domain
            .entities
            .get_mut(&id(1).parse()?)
            .ok_or("person")?["eligibleAssignmentTypeIds"] =
            if eligible { json!([id(4)]) } else { json!([]) };
        let source = format!("personId\t{}\n{}\t\n", id(4), id(1));
        let proposal = preview(
            &document,
            source.as_bytes(),
            &EligibilityMatrixMapping::default(),
            &[address(2, 1)],
        )?;
        let row = &proposal.rows[0];
        assert_eq!(row.status, EligibilityRowStatus::Skipped);
        assert_eq!(row.issue, None);
        assert_eq!((row.before, row.after), (Some(eligible), Some(eligible)));
        assert_eq!(proposal.disposition, PeopleImportDisposition::NoChanges);
        assert!(proposal.batch.is_none());
    }
    Ok(())
}

#[test]
fn direct_uuid_spellings_and_mapping_normalization_keep_native_meaning() -> TestResult {
    let document = fixture()?;
    for (person, assignment_type) in [
        (id(1).to_ascii_uppercase(), id(4).to_ascii_uppercase()),
        (id(1).replace('-', ""), id(4).replace('-', "")),
        (format!("{{{}}}", id(1)), format!("{{{}}}", id(4))),
        (format!("urn:uuid:{}", id(1)), format!("urn:uuid:{}", id(4))),
    ] {
        let source = format!("personId\t{assignment_type}\n{person}\tfalse\n");
        let proposal = preview(
            &document,
            source.as_bytes(),
            &EligibilityMatrixMapping::default(),
            &[],
        )?;
        assert_eq!(proposal.rows[0].person_id, Some(id(1).parse()?));
        assert_eq!(proposal.rows[0].assignment_type_id, Some(id(4).parse()?));
        assert_eq!(proposal.disposition, PeopleImportDisposition::Reviewable);
    }
    for alias in [
        "",
        "x\n",
        "x\t",
        "x".repeat(257).as_str(),
        "00000000-0000-4000-8000-000000000001",
    ] {
        let mut mapping = EligibilityMatrixMapping::default();
        mapping
            .assignment_type_mappings
            .insert(alias.to_owned(), id(4).parse()?);
        assert_eq!(
            preview(&document, b"\tType\n", &mapping, &[])
                .err()
                .ok_or("bad alias")?
                .code,
            CsvErrorCode::InvalidMapping
        );
    }
    Ok(())
}

#[test]
fn shape_and_sample_validate_every_suffix_with_native_coordinates() -> TestResult {
    let document = fixture()?;
    for source in [
        b"".as_slice(),
        b"\n\n",
        b"personId\n",
        b"person\tType\n",
        b"\tType\nStaff\n",
        b"\tType\nStaff\ttrue\textra\n",
    ] {
        assert_eq!(
            detect(source).err().ok_or("shape accepted")?.code,
            CsvErrorCode::MatrixShape
        );
        assert_eq!(
            sample_eligibility_matrix_record(
                &mut Cursor::new(source),
                1,
                &CancellationToken::default()
            )
            .err()
            .ok_or("suffix hidden")?
            .code,
            CsvErrorCode::MatrixShape
        );
        assert_eq!(
            preview(
                &document,
                source,
                &EligibilityMatrixMapping::default(),
                &[address(2, 1)]
            )
            .err()
            .ok_or("skip hid shape")?
            .code,
            CsvErrorCode::MatrixShape
        );
    }
    let header_only = preview(
        &document,
        b"personId\tmissing\n",
        &EligibilityMatrixMapping::default(),
        &[],
    )?;
    assert_eq!(header_only.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(header_only.cells, 0);
    assert!(
        header_only.rows.is_empty() && header_only.batch.is_none() && header_only.review.is_some()
    );
    let source = format!(
        "\u{feff}personId\t\"Duty\nname\"\r\n\r\n\"Staff\talias\"\ttrue\r\n{}\tfalse\r\n",
        id(1)
    );
    let detection = detect(source.as_bytes())?;
    assert_eq!(
        (
            detection.columns,
            detection.cells,
            detection.source.logical_records
        ),
        (2, 2, 3)
    );
    assert_eq!(detection.samples[0].cells[1].text, "Duty\nname");
    assert_eq!(detection.samples[1].cells[0].text, "Staff\talias");
    assert_eq!(detection.samples[1].record, 2);
    assert!(
        sample_eligibility_matrix_record(
            &mut Cursor::new(&source),
            99,
            &CancellationToken::default()
        )?
        .is_none()
    );
    let long = format!("\t{}\n", "é".repeat(200));
    let short = detect(long.as_bytes())?;
    assert_eq!(short.samples[0].cells[1].text.len(), 64);
    assert!(short.samples[0].cells[1].truncated);
    let inspected = sample_eligibility_matrix_record(
        &mut Cursor::new(&long),
        1,
        &CancellationToken::default(),
    )?
    .ok_or("sample")?;
    assert_eq!(inspected.cells[1].text.len(), 256);
    assert!(inspected.cells[1].truncated);
    for suffix in [b"\xff".as_slice(), b"\0"] {
        let mut invalid = b"\tDuty\nStaff\ttrue\n".to_vec();
        invalid.extend_from_slice(suffix);
        assert!(
            sample_eligibility_matrix_record(
                &mut Cursor::new(&invalid),
                1,
                &CancellationToken::default()
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn all_cells_count_and_skips_are_sorted_unique_in_rectangle() -> TestResult {
    let document = fixture()?;
    let source = format!("\t{}\t{}\n{}\t\ttrue\n", id(4), id(4), id(1));
    for skips in [
        vec![address(1, 1)],
        vec![address(2, 0)],
        vec![address(2, 3)],
        vec![address(3, 1)],
        vec![address(2, 1), address(2, 1)],
        vec![address(2, 2), address(2, 1)],
    ] {
        assert_eq!(
            preview(
                &document,
                source.as_bytes(),
                &EligibilityMatrixMapping::default(),
                &skips
            )
            .err()
            .ok_or("bad skips")?
            .code,
            CsvErrorCode::InvalidDecision
        );
    }
    let mut rectangle = "personId".to_owned();
    for index in 0..10 {
        write!(rectangle, "\tType{index}")?;
    }
    rectangle.push('\n');
    for _ in 0..1000 {
        rectangle.push_str("Staff\t\t\t\t\t\t\t\t\t\t\n");
    }
    assert_eq!(
        detect(rectangle.as_bytes())?.cells,
        MAX_ELIGIBILITY_MATRIX_CELLS
    );
    let skips: Vec<_> = (2..=1001)
        .flat_map(|record| (1..=10).map(move |column| address(record, column)))
        .collect();
    let omitted = preview(
        &document,
        rectangle.as_bytes(),
        &EligibilityMatrixMapping::default(),
        &skips,
    )?;
    assert_eq!(omitted.cells, 10_000);
    assert_eq!(omitted.excluded_cells.len(), 10_000);
    assert_eq!(omitted.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(
        decode_eligibility_matrix_review(&serde_json::to_vec(
            omitted.review.as_ref().ok_or("review")?
        )?)?,
        omitted.review.ok_or("review")?
    );
    rectangle.push_str("Staff\t\t\t\t\t\t\t\t\t\t\n");
    assert_eq!(
        detect(rectangle.as_bytes()).err().ok_or("census")?.code,
        CsvErrorCode::MatrixCellLimit
    );
    assert_eq!(
        sample_eligibility_matrix_record(
            &mut Cursor::new(&rectangle),
            1,
            &CancellationToken::default()
        )
        .err()
        .ok_or("sample census")?
        .code,
        CsvErrorCode::MatrixCellLimit
    );
    let maximum_width = format!("personId{}\n", "\tDuty".repeat(63));
    assert_eq!(detect(maximum_width.as_bytes())?.columns, 64);
    let over_width = format!("personId{}\n", "\tDuty".repeat(64));
    assert_eq!(
        detect(over_width.as_bytes())
            .err()
            .ok_or("column cap")?
            .code,
        CsvErrorCode::ColumnLimit
    );
    let exact_records = format!("\tDuty\n{}", "Staff\t\n".repeat(10_000));
    assert_eq!(
        detect(exact_records.as_bytes())?.source.logical_records,
        10_001
    );
    Ok(())
}

struct Chunks<'a> {
    remaining: &'a [u8],
    maximum: usize,
    consumed: usize,
}
impl Read for Chunks<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = output.len().min(self.maximum).min(self.remaining.len());
        output[..count].copy_from_slice(&self.remaining[..count]);
        self.remaining = &self.remaining[count..];
        self.consumed += count;
        Ok(count)
    }
}

#[test]
fn every_reader_enforces_inclusive_source_cap_without_accepting_or_overreading_prefix() -> TestResult
{
    let document = fixture()?;
    let mut exact = b"\tDuty\n".to_vec();
    exact.resize(MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES, b'\n');
    for maximum in [1, 7, 4096, usize::MAX] {
        let mut reader = Chunks {
            remaining: &exact,
            maximum,
            consumed: 0,
        };
        let detection = detect_eligibility_matrix(&mut reader, &CancellationToken::default())?;
        assert_eq!(reader.consumed, MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES);
        assert_eq!(
            detection.source.raw_bytes,
            MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES as u64
        );
        assert_eq!(
            detection.source.blake3,
            blake3::hash(&exact).to_hex().to_string()
        );
    }
    let no_op = preview(&document, &exact, &EligibilityMatrixMapping::default(), &[])?;
    let review = no_op.review.as_ref().ok_or("review")?;
    let digest = no_op.approval_digest.as_deref().ok_or("approval")?;
    let mut excessive = exact.clone();
    excessive.extend_from_slice(b"\n\n\n\nnot retained");
    for entry in 0..4 {
        let mut reader = Chunks {
            remaining: &excessive,
            maximum: 7,
            consumed: 0,
        };
        let error = match entry {
            0 => detect_eligibility_matrix(&mut reader, &CancellationToken::default()).err(),
            1 => sample_eligibility_matrix_record(&mut reader, 1, &CancellationToken::default())
                .err(),
            2 => preview_eligibility_matrix(
                &document,
                Revision::INITIAL,
                &mut reader,
                &EligibilityMatrixMapping::default(),
                &[],
                &CancellationToken::default(),
            )
            .err(),
            _ => rebuild_eligibility_matrix_review(
                &document,
                Revision::INITIAL,
                &mut reader,
                review,
                digest,
                &CancellationToken::default(),
            )
            .err(),
        }
        .ok_or("over-cap accepted")?;
        assert_eq!(error.code, CsvErrorCode::SourceByteLimit);
        assert_eq!(reader.consumed, MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES + 1);
    }
    Ok(())
}

#[test]
fn cancellation_is_checked_before_and_after_reading() -> TestResult {
    struct CancelRead<'a>(&'a CancellationToken);
    impl Read for CancelRead<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            output[0] = b'\t';
            self.0.cancel();
            Ok(1)
        }
    }
    let cancellation = CancellationToken::default();
    let mut reader = CancelRead(&cancellation);
    assert_eq!(
        detect_eligibility_matrix(&mut reader, &cancellation)
            .err()
            .ok_or("read cancellation")?
            .code,
        CsvErrorCode::Cancelled
    );
    let document = fixture()?;
    let mut reader = Chunks {
        remaining: b"\tDuty\n",
        maximum: 7,
        consumed: 0,
    };
    assert_eq!(
        preview_eligibility_matrix(
            &document,
            Revision::INITIAL,
            &mut reader,
            &EligibilityMatrixMapping::default(),
            &[],
            &cancellation
        )
        .err()
        .ok_or("cancelled preview")?
        .code,
        CsvErrorCode::Cancelled
    );
    assert_eq!(reader.consumed, 0);
    assert_eq!(
        sample_eligibility_matrix_record(&mut reader, 1, &cancellation)
            .err()
            .ok_or("cancelled sample")?
            .code,
        CsvErrorCode::Cancelled
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[test]
fn review_exact_decode_and_rebuild_bind_source_shape_policy_facts_and_versions() -> TestResult {
    let document = fixture()?;
    let source = format!("personId\t{}\n{}\tfalse\n", id(4), id(1));
    let proposal = preview(
        &document,
        source.as_bytes(),
        &EligibilityMatrixMapping::default(),
        &[],
    )?;
    let review = proposal.review.as_ref().ok_or("review")?;
    let digest = proposal.approval_digest.as_deref().ok_or("approval")?;
    assert_eq!(
        review.parser_version,
        "csv-core-0.1.13/eligibility-matrix-1"
    );
    assert_eq!(review.format, "eutheto/workforce-eligibility-matrix-review");
    assert_eq!(
        &decode_eligibility_matrix_review(&serde_json::to_vec(review)?)?,
        review
    );
    assert_eq!(
        rebuild_eligibility_matrix_review(
            &document,
            Revision::INITIAL,
            &mut Cursor::new(&source),
            review,
            digest,
            &CancellationToken::default()
        )?,
        proposal
    );
    for (key, value) in [
        ("schemaVersion", json!(2)),
        ("format", json!("other")),
        (
            "parserVersion",
            json!("csv-core-0.1.13/eligibility-matrix-2"),
        ),
        ("limitsVersion", json!(2)),
    ] {
        let mut altered = serde_json::to_value(review)?;
        altered[key] = value;
        assert_eq!(
            decode_eligibility_matrix_review(&serde_json::to_vec(&altered)?)
                .err()
                .ok_or("version")?
                .code,
            CsvErrorCode::UnsupportedVersion
        );
    }
    for (key, value) in [
        ("columns", json!(1)),
        ("cells", json!(2)),
        ("skippedCells", json!([{"record":2,"column":2}])),
        ("extra", json!(true)),
    ] {
        let mut altered = serde_json::to_value(review)?;
        altered[key] = value;
        assert!(decode_eligibility_matrix_review(&serde_json::to_vec(&altered)?).is_err());
    }
    let mut excessive = serde_json::to_value(review)?;
    excessive["source"]["rawBytes"] = json!(MAX_ELIGIBILITY_MATRIX_SOURCE_BYTES + 1);
    assert_eq!(
        decode_eligibility_matrix_review(&serde_json::to_vec(&excessive)?)
            .err()
            .ok_or("review cap")?
            .code,
        CsvErrorCode::InvalidReview
    );
    for key in [
        "scenarioBlake3",
        "rowsBlake3",
        "changesBlake3",
        "excludedBlake3",
        "validationBlake3",
    ] {
        let mut altered = serde_json::to_value(review)?;
        altered[key] = json!("0".repeat(64));
        let altered = decode_eligibility_matrix_review(&serde_json::to_vec(&altered)?)?;
        assert_eq!(
            rebuild_eligibility_matrix_review(
                &document,
                Revision::INITIAL,
                &mut Cursor::new(&source),
                &altered,
                digest,
                &CancellationToken::default()
            )
            .err()
            .ok_or("facts not bound")?
            .code,
            CsvErrorCode::StaleReview
        );
    }
    for decisions in [false, true] {
        let mut altered = review.clone();
        if decisions {
            altered.skipped_cells = vec![address(2, 1)];
        } else {
            altered
                .mapping
                .person_mappings
                .insert("unused".to_owned(), id(1).parse()?);
        }
        assert_eq!(
            rebuild_eligibility_matrix_review(
                &document,
                Revision::INITIAL,
                &mut Cursor::new(&source),
                &altered,
                digest,
                &CancellationToken::default()
            )
            .err()
            .ok_or("policy not bound")?
            .code,
            CsvErrorCode::StaleReview
        );
    }
    for altered in [source.replace("false", "true"), format!("{source}\n")] {
        assert_eq!(
            rebuild_eligibility_matrix_review(
                &document,
                Revision::INITIAL,
                &mut Cursor::new(altered),
                review,
                digest,
                &CancellationToken::default()
            )
            .err()
            .ok_or("source not bound")?
            .code,
            CsvErrorCode::StaleReview
        );
    }
    let mut changed = document.clone();
    changed
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["name"] = json!("Changed");
    for (snapshot, revision) in [(&changed, Revision::INITIAL), (&document, Revision::new(1))] {
        assert_eq!(
            rebuild_eligibility_matrix_review(
                snapshot,
                revision,
                &mut Cursor::new(&source),
                review,
                digest,
                &CancellationToken::default()
            )
            .err()
            .ok_or("snapshot not bound")?
            .code,
            CsvErrorCode::StaleReview
        );
    }
    Ok(())
}

#[test]
fn skipped_cells_do_not_hide_decoded_bounds_and_batch_counts_changed_people() -> TestResult {
    let mut document = fixture()?;
    let oversized = format!(
        "personId\t{}\n{}\t{}\n",
        id(4),
        id(1),
        "x".repeat(16 * 1024 + 1)
    );
    assert_eq!(
        preview(
            &document,
            oversized.as_bytes(),
            &EligibilityMatrixMapping::default(),
            &[address(2, 1)]
        )
        .err()
        .ok_or("skipped cell bound")?
        .code,
        CsvErrorCode::CellLimit
    );
    let original = document.domain.entities[&id(1).parse()?].clone();
    let mut source = format!("personId\t{}\n", id(4));
    for index in 1000..2001 {
        let mut person = original.clone();
        person["id"] = json!(id(index));
        person["externalId"] = json!(format!("staff-{index}"));
        document.domain.entities.insert(id(index).parse()?, person);
        writeln!(source, "{}\tfalse", id(index))?;
    }
    assert_eq!(
        preview(
            &document,
            source.as_bytes(),
            &EligibilityMatrixMapping::default(),
            &[]
        )
        .err()
        .ok_or("command ceiling")?
        .code,
        CsvErrorCode::InvalidBatch
    );
    let unchanged = preview(
        &document,
        source.replace("false", "true").as_bytes(),
        &EligibilityMatrixMapping::default(),
        &[],
    )?;
    assert_eq!(unchanged.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(unchanged.cells, 1001);
    assert!(unchanged.batch.is_none());
    Ok(())
}

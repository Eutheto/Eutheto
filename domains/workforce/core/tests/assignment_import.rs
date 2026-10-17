mod support;

use eutheto_types::{CancellationToken, Revision, ScenarioDocument};
use eutheto_workforce::{
    assignment_import::*,
    commands,
    people_csv::{CsvDialect, PeopleImportDisposition},
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error, io::Cursor};
use support::{fixture, id};

type TestResult = Result<(), Box<dyn Error>>;
const FRAMING: &str = "eutheto/assignments,1\nassignment_id,person_id,shift_id\n";

fn row(person: u32, shift: u32) -> String {
    format!(
        "official.workforce.assignment.{}.{},{},{}\n",
        id(person),
        id(shift),
        id(person),
        id(shift)
    )
}

fn mapped() -> AssignmentCsvMapping {
    AssignmentCsvMapping::Mapped {
        dialect: CsvDialect::Comma,
        has_header: true,
        expected_columns: 2,
        person_column: 0,
        shift_column: 1,
        person_mappings: BTreeMap::new(),
        shift_mappings: BTreeMap::new(),
    }
}

fn target(replace: bool) -> Result<AssignmentImportTarget, Box<dyn Error>> {
    let id = id(20).parse()?;
    let name = "Imported starting plan".to_owned();
    Ok(if replace {
        AssignmentImportTarget::Replace { id, name }
    } else {
        AssignmentImportTarget::Create { id, name }
    })
}

fn preview(
    document: &ScenarioDocument,
    source: &[u8],
    mapping: &AssignmentCsvMapping,
    skips: &[u32],
    allow_empty: bool,
) -> Result<AssignmentImportPreview, Box<dyn Error>> {
    Ok(preview_assignment_csv(
        document,
        Revision::INITIAL,
        &mut Cursor::new(source),
        mapping,
        &target(false)?,
        skips,
        allow_empty,
        &CancellationToken::default(),
    )?)
}

#[test]
fn canonical_import_is_unverified_reversible_and_bound_to_original_bytes() -> TestResult {
    let original = fixture()?;
    let source = format!("{FRAMING}{}{}", row(1, 8), row(999, 8));
    let blocked = preview(
        &original,
        source.as_bytes(),
        &AssignmentCsvMapping::CanonicalV1,
        &[],
        false,
    )?;
    assert_eq!(blocked.disposition, PeopleImportDisposition::Blocked);
    assert!(blocked.batch.is_none() && blocked.approval_digest.is_none());
    let ready = preview(
        &original,
        source.as_bytes(),
        &AssignmentCsvMapping::CanonicalV1,
        &[4],
        false,
    )?;
    assert_eq!(ready.disposition, PeopleImportDisposition::Reviewable);
    assert_eq!(
        ready.excluded_rows,
        [AssignmentExcludedRow {
            record: 4,
            skipped: true,
            issue: Some(AssignmentRowIssue::UnresolvedPerson)
        }]
    );
    let review = decode_assignment_csv_review(&serde_json::to_vec(
        ready.review.as_ref().ok_or("missing review")?,
    )?)?;
    let approval = ready.approval_digest.as_deref().ok_or("missing approval")?;
    let rebuilt = rebuild_assignment_csv_review(
        &original,
        Revision::INITIAL,
        &mut Cursor::new(source.as_bytes()),
        &review,
        approval,
        &CancellationToken::default(),
    )?;
    let changed = commands::apply_batch(&original, rebuilt.batch.as_ref().ok_or("missing batch")?)?;
    assert_eq!(
        changed.document.domain.locked_assignments,
        original.domain.locked_assignments
    );
    assert_eq!(changed.document.domain.rules, original.domain.rules);
    let plan = &changed.document.domain.entities[&id(20).parse()?];
    assert_eq!(plan["kind"], "externalStartingPlan");
    assert_eq!(
        plan["assignments"],
        json!([{ "personId": id(1), "shiftId": id(8) }])
    );
    assert_eq!(
        plan["originalImport"]["sourceBlake3"],
        blake3::hash(source.as_bytes()).to_hex().as_str()
    );
    assert_eq!(
        commands::apply_batch(&changed.document, &changed.inverse)?.document,
        original
    );
    let changed_skipped_row = source.replace(&id(999), &id(998));
    assert!(
        rebuild_assignment_csv_review(
            &original,
            Revision::INITIAL,
            &mut Cursor::new(changed_skipped_row),
            &review,
            approval,
            &CancellationToken::default()
        )
        .is_err()
    );
    let mut changed_context = original.clone();
    changed_context.metadata.title = "Another title".to_owned();
    assert!(
        rebuild_assignment_csv_review(
            &changed_context,
            Revision::INITIAL,
            &mut Cursor::new(source),
            &review,
            approval,
            &CancellationToken::default()
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn reserved_canonical_prefix_never_falls_back_to_mapped_input() -> TestResult {
    let document = fixture()?;
    for source in [
        "eutheto/assignments;1\nperson;shift\n",
        "\u{feff}eutheto/assignments,1\nassignment_id,person_id,shift_id\n",
        "\r\n\neutheto/assignments,1\nassignment_id,person_id,shift_id\n",
        "\"eutheto/assignments,1\nperson,shift\n",
        "eutheto/assignments,2\nassignment_id,person_id,shift_id\n",
        "eutheto/assignments-damaged,1\nperson,shift\n",
    ] {
        assert!(
            detect_assignment_csv(source.as_bytes(), &CancellationToken::default()).is_err(),
            "{source:?}"
        );
        assert!(
            preview(&document, source.as_bytes(), &mapped(), &[], true).is_err(),
            "{source:?}"
        );
    }
    let quoted = "\"eutheto/assignments\",\"1\"\r\nassignment_id,person_id,shift_id\r\n";
    assert!(matches!(
        detect_assignment_csv(quoted.as_bytes(), &CancellationToken::default())?,
        AssignmentCsvDetection::CanonicalV1 { .. }
    ));
    assert!(matches!(
        detect_assignment_csv(
            b"eutheto/assignmentz,1\nperson,shift\n",
            &CancellationToken::default()
        )?,
        AssignmentCsvDetection::Mapped { .. }
    ));
    Ok(())
}

#[test]
fn canonical_duplicates_and_identity_disagreement_are_source_fatal_even_when_skipped() -> TestResult
{
    let document = fixture()?;
    let duplicate = format!("{FRAMING}{}{}", row(1, 8), row(1, 8));
    assert!(
        preview(
            &document,
            duplicate.as_bytes(),
            &AssignmentCsvMapping::CanonicalV1,
            &[3, 4],
            true
        )
        .is_err()
    );
    let disagreement = format!(
        "{FRAMING}official.workforce.assignment.{}.{},{},{}\n",
        id(1),
        id(8),
        id(1),
        id(7)
    );
    assert!(
        preview(
            &document,
            disagreement.as_bytes(),
            &AssignmentCsvMapping::CanonicalV1,
            &[3],
            true
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn mapped_duplicates_require_explicit_choice_without_automatic_partial_apply() -> TestResult {
    let document = fixture()?;
    let source = format!(
        "person,shift\n{},{}\n{},{}\n{},{}\n",
        id(1),
        id(8),
        id(1),
        id(8),
        id(1),
        id(7)
    );
    let blocked = preview(&document, source.as_bytes(), &mapped(), &[], false)?;
    assert_eq!(blocked.disposition, PeopleImportDisposition::Blocked);
    assert_eq!(
        blocked
            .rows
            .iter()
            .map(|row| row.status)
            .collect::<Vec<_>>(),
        [
            AssignmentRowStatus::Blocked,
            AssignmentRowStatus::Blocked,
            AssignmentRowStatus::Included
        ]
    );
    assert!(blocked.batch.is_none() && blocked.review.is_none());
    let ready = preview(&document, source.as_bytes(), &mapped(), &[3], false)?;
    assert_eq!(ready.disposition, PeopleImportDisposition::Reviewable);
    assert_eq!(
        ready.excluded_rows,
        [AssignmentExcludedRow {
            record: 3,
            skipped: true,
            issue: Some(AssignmentRowIssue::DuplicatePair)
        }]
    );
    assert_eq!(ready.changes.ok_or("missing changes")?.after_pair_count, 2);
    Ok(())
}

#[test]
fn exact_aliases_are_endpoint_specific_and_cannot_shadow_uuid_identity() -> TestResult {
    let document = fixture()?;
    let mut mapping = mapped();
    let AssignmentCsvMapping::Mapped {
        person_mappings,
        shift_mappings,
        ..
    } = &mut mapping
    else {
        unreachable!()
    };
    person_mappings.insert("same".to_owned(), id(1).parse()?);
    shift_mappings.insert("same".to_owned(), id(8).parse()?);
    let ready = preview(
        &document,
        b"person,shift\nsame,same\n",
        &mapping,
        &[],
        false,
    )?;
    assert_eq!(ready.disposition, PeopleImportDisposition::Reviewable);
    for source in [
        "person,shift\n same,same\n",
        "person,shift\nSame,same\n",
        "person,shift\n,same\n",
    ] {
        let blocked = preview(&document, source.as_bytes(), &mapping, &[], false)?;
        assert_eq!(blocked.disposition, PeopleImportDisposition::Blocked);
    }
    let AssignmentCsvMapping::Mapped {
        person_mappings, ..
    } = &mut mapping
    else {
        unreachable!()
    };
    person_mappings.insert(id(5), id(1).parse()?);
    assert!(
        preview(
            &document,
            b"person,shift\nsame,same\n",
            &mapping,
            &[],
            false
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn retained_work_membership_does_not_require_eligibility_or_resolvable_generation() -> TestResult {
    let mut document = fixture()?;
    document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("missing person")?["eligibleAssignmentTypeIds"] = json!([]);
    // The fixture's generated occurrence is ambiguous under its Reject overlap policy.
    let source = format!("person,shift\n{},{}\n", id(1), id(7));
    assert_eq!(
        preview(&document, source.as_bytes(), &mapped(), &[], false)?.disposition,
        PeopleImportDisposition::Reviewable
    );
    for (person, shift) in [(5, 7), (1, 6), (1, 999)] {
        let source = format!("person,shift\n{},{}\n", id(person), id(shift));
        assert_eq!(
            preview(&document, source.as_bytes(), &mapped(), &[], false)?.disposition,
            PeopleImportDisposition::Blocked
        );
    }
    Ok(())
}

#[test]
fn empty_is_explicit_and_replacement_is_complete_with_fresh_provenance() -> TestResult {
    let original = fixture()?;
    assert!(preview(&original, b"", &mapped(), &[], true).is_err());
    let empty = preview(&original, b"person,shift\n", &mapped(), &[], false)?;
    assert_eq!(empty.disposition, PeopleImportDisposition::Blocked);
    let source = format!("person,shift\n{},{}\n", id(1), id(8));
    let created = preview(&original, source.as_bytes(), &mapped(), &[], false)?;
    let current =
        commands::apply_batch(&original, created.batch.as_ref().ok_or("missing create")?)?.document;
    let replaced = preview_assignment_csv(
        &current,
        Revision::INITIAL,
        &mut Cursor::new(b"person,shift\n"),
        &mapped(),
        &target(true)?,
        &[],
        true,
        &CancellationToken::default(),
    )?;
    assert_eq!(replaced.disposition, PeopleImportDisposition::Reviewable);
    let changes = replaced.changes.as_ref().ok_or("missing changes")?;
    assert_eq!(
        (changes.before_pair_count, changes.after_pair_count),
        (1, 0)
    );
    assert_eq!(changes.removed_pairs[0].shift_id, id(8).parse()?);
    let mutation = commands::apply_batch(
        &current,
        replaced.batch.as_ref().ok_or("missing replacement")?,
    )?;
    let plan = &mutation.document.domain.entities[&id(20).parse()?];
    assert_eq!(plan["assignments"], json!([]));
    assert_eq!(
        plan["originalImport"]["sourceBlake3"],
        blake3::hash(b"person,shift\n").to_hex().as_str()
    );
    assert_ne!(
        plan["originalImport"]["pairContentHash"],
        current.domain.entities[&id(20).parse()?]["originalImport"]["pairContentHash"]
    );
    assert_eq!(
        commands::apply_batch(&mutation.document, &mutation.inverse)?.document,
        current
    );
    Ok(())
}

#[test]
fn every_skipped_row_remains_in_the_report_and_counts_toward_the_limit() -> TestResult {
    let document = fixture()?;
    let mut source = String::from("person,shift\n");
    for _ in 0..10_000 {
        source.push_str("unknown,unknown\n");
    }
    let skips: Vec<_> = (2..=10_001).collect();
    let ready = preview(&document, source.as_bytes(), &mapped(), &skips, true)?;
    assert_eq!(ready.disposition, PeopleImportDisposition::Reviewable);
    assert_eq!(ready.excluded_rows.len(), 10_000);
    assert_eq!(
        ready
            .excluded_rows
            .last()
            .ok_or("missing final exclusion")?
            .record,
        10_001
    );
    assert!(
        ready
            .excluded_rows
            .iter()
            .all(|row| row.skipped && row.issue == Some(AssignmentRowIssue::UnresolvedPerson))
    );
    source.push_str("unknown,unknown\n");
    assert!(preview(&document, source.as_bytes(), &mapped(), &skips, true).is_err());
    Ok(())
}

#[test]
fn canonical_last_record_is_inspectable_without_the_mapped_record_ceiling() -> TestResult {
    let mut source = FRAMING.to_owned();
    for person in 1_000..11_000 {
        source.push_str(&row(person, 8));
    }
    let token = CancellationToken::default();
    let AssignmentCsvDetection::CanonicalV1 { source: facts, .. } =
        detect_assignment_csv(source.as_bytes(), &token)?
    else {
        return Err("wrong format".into());
    };
    assert_eq!(facts.logical_records, 10_002);
    let sample = sample_assignment_csv_record(
        &mut Cursor::new(source.as_bytes()),
        AssignmentCsvReadFormat::CanonicalV1,
        10_002,
        &token,
    )?
    .ok_or("missing last record")?;
    assert_eq!(sample.cells[1].text, id(10_999));
    assert!(sample.cells.iter().all(|cell| !cell.truncated));
    source.push_str(&row(11_000, 8));
    assert!(detect_assignment_csv(source.as_bytes(), &token).is_err());
    assert!(
        sample_assignment_csv_record(
            &mut Cursor::new(source),
            AssignmentCsvReadFormat::CanonicalV1,
            3,
            &token
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn mapped_dialects_and_quoted_bom_input_preserve_exact_pair_meaning() -> TestResult {
    let document = fixture()?;
    for (dialect, delimiter) in [
        (CsvDialect::Comma, ','),
        (CsvDialect::Semicolon, ';'),
        (CsvDialect::Tab, '\t'),
    ] {
        let mut mapping = mapped();
        let AssignmentCsvMapping::Mapped {
            dialect: selected, ..
        } = &mut mapping
        else {
            unreachable!()
        };
        *selected = dialect;
        let source = format!(
            "\u{feff}\"person\"{delimiter}\"shift\"\r\n\"{}\"{delimiter}\"{}\"\r\n",
            id(1),
            id(8)
        );
        let ready = preview(&document, source.as_bytes(), &mapping, &[], false)?;
        assert_eq!(ready.disposition, PeopleImportDisposition::Reviewable);
        assert_eq!(ready.rows[0].person_id, Some(id(1).parse()?));
        assert_eq!(ready.rows[0].shift_id, Some(id(8).parse()?));
        assert_eq!(
            ready.source.blake3,
            blake3::hash(source.as_bytes()).to_hex().as_str()
        );
    }
    Ok(())
}

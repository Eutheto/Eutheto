mod support;

use eutheto_types::{CancellationToken, Revision, ScenarioDocument};
use eutheto_workforce::{
    commands,
    eligibility_import::*,
    people_csv::{CsvDialect, CsvError, CsvErrorCode, PeopleImportDisposition},
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error, fmt::Write as _, io::Cursor};
use support::{fixture, id};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn mapping() -> EligibilityCsvMapping {
    EligibilityCsvMapping {
        dialect: CsvDialect::Comma,
        has_header: true,
        expected_columns: 3,
        person_column: 0,
        assignment_type_column: 1,
        eligible_column: 2,
        person_mappings: BTreeMap::new(),
        assignment_type_mappings: BTreeMap::new(),
    }
}

fn preview(
    document: &ScenarioDocument,
    source: &[u8],
    mapping: &EligibilityCsvMapping,
    skips: &[u32],
) -> Result<EligibilityImportPreview, CsvError> {
    preview_eligibility_csv(
        document,
        Revision::INITIAL,
        &mut Cursor::new(source),
        mapping,
        skips,
        &CancellationToken::default(),
    )
}

fn rich_fixture() -> TestResult<ScenarioDocument> {
    let mut document = fixture()?;
    for index in [21, 22, 23] {
        let mut assignment_type = document.domain.entities[&id(4).parse()?].clone();
        assignment_type["id"] = json!(id(index));
        document
            .domain
            .entities
            .insert(id(index).parse()?, assignment_type);
    }
    document.domain.entities.insert(
        id(12).parse()?,
        json!({"kind":"team","id":id(12),"name":"Blue"}),
    );
    let person = document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?;
    person["id"] = json!(id(1).to_ascii_uppercase());
    person["eligibleAssignmentTypeIds"] =
        json!([id(4).to_ascii_uppercase(), id(22).to_ascii_uppercase()]);
    person["homeLocationId"] = json!(id(5).to_ascii_uppercase());
    person["teamIds"] = json!([id(12).to_ascii_uppercase()]);
    person["display"] = json!({"color":"#102030","avatarInitials":"RV"});
    person["qualificationGrants"] = json!([{"qualificationId":id(11).to_ascii_uppercase(),"effectiveFrom":"2020-01-01T00:00:00+00:00","expiresAt":"2030-01-01T00:00:00+00:00"}]);
    Ok(document)
}

#[test]
fn sparse_typed_edits_preserve_exact_raw_person_and_membership_order() -> TestResult {
    let document = rich_fixture()?;
    let mut policy = mapping();
    policy
        .person_mappings
        .insert("Staff".to_owned(), id(1).parse()?);
    policy
        .assignment_type_mappings
        .insert("New type".to_owned(), id(21).to_ascii_uppercase().parse()?);
    let source = format!(
        "person,type,eligible\nStaff,New type,true\n{},{},true\n{},{},false\n{},{},true\n",
        id(1).to_ascii_uppercase(),
        id(4),
        id(1),
        id(22),
        id(1),
        id(23)
    );
    let proposal = preview(&document, source.as_bytes(), &policy, &[])?;
    assert_eq!(proposal.disposition, PeopleImportDisposition::Reviewable);
    assert_eq!(
        proposal
            .rows
            .iter()
            .map(|row| row.status)
            .collect::<Vec<_>>(),
        [
            EligibilityRowStatus::Changed,
            EligibilityRowStatus::Unchanged,
            EligibilityRowStatus::Changed,
            EligibilityRowStatus::Changed
        ]
    );
    let batch = proposal.batch.as_ref().ok_or("batch")?;
    assert_eq!(batch.commands.len(), 1);
    let mutation = commands::apply_batch(&document, batch)?;
    let mut expected = document.clone();
    expected
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] =
        json!([id(4).to_ascii_uppercase(), id(21), id(23)]);
    assert_eq!(mutation.document, expected);
    let inverse = commands::apply_batch(&mutation.document, &mutation.inverse)?;
    assert_eq!(inverse.document, document);
    assert_eq!(
        commands::apply_batch(&inverse.document, &inverse.inverse)?.document,
        expected
    );
    let unchanged = format!(
        "person,type,eligible\n{},{},true\n{},{},false\n",
        id(1).to_ascii_uppercase(),
        id(4),
        id(1),
        id(22)
    );
    let no_op = preview(&expected, unchanged.as_bytes(), &mapping(), &[])?;
    assert_eq!(no_op.disposition, PeopleImportDisposition::NoChanges);
    assert!(no_op.batch.is_none());
    assert_eq!(
        no_op
            .rows
            .iter()
            .map(|row| (row.before, row.after))
            .collect::<Vec<_>>(),
        [(Some(true), Some(true)), (Some(false), Some(false))]
    );
    Ok(())
}

#[test]
fn equivalent_mapping_uuid_spellings_produce_the_same_native_review() -> TestResult {
    let document = fixture()?;
    let mut policy = mapping();
    policy
        .person_mappings
        .insert("Staff".to_owned(), id(1).parse()?);
    policy
        .assignment_type_mappings
        .insert("Duty".to_owned(), id(4).parse()?);
    let source = b"person,type,eligible\nStaff,Duty,false\n";
    let expected = preview(&document, source, &policy, &[])?;
    assert_eq!(expected.disposition, PeopleImportDisposition::Reviewable);
    for (person, assignment_type) in [
        (id(1).to_ascii_uppercase(), id(4).to_ascii_uppercase()),
        (id(1).replace('-', ""), id(4).replace('-', "")),
        (format!("{{{}}}", id(1)), format!("{{{}}}", id(4))),
        (format!("urn:uuid:{}", id(1)), format!("urn:uuid:{}", id(4))),
    ] {
        let mut wire = serde_json::to_value(&policy)?;
        wire["personMappings"]["Staff"] = json!(person);
        wire["assignmentTypeMappings"]["Duty"] = json!(assignment_type);
        assert_eq!(
            preview(&document, source, &serde_json::from_value(wire)?, &[])?,
            expected
        );
    }
    Ok(())
}

#[test]
fn configured_membership_does_not_require_qualification_or_solver_feasibility() -> TestResult {
    let mut document = fixture()?;
    let person = document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?;
    person["qualificationGrants"] = json!([]);
    person["eligibleAssignmentTypeIds"] = json!([]);
    let source = format!("person,type,eligible\n{},{},true\n", id(1), id(4));
    let proposal = preview(&document, source.as_bytes(), &mapping(), &[])?;
    let mutation = commands::apply_batch(&document, proposal.batch.as_ref().ok_or("batch")?)?;
    let mut expected = document.clone();
    expected
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["eligibleAssignmentTypeIds"] = json!([id(4)]);
    assert_eq!(mutation.document, expected);
    Ok(())
}

#[test]
fn duplicate_typed_pairs_need_explicit_skips_and_aliases_cannot_shadow_ids() -> TestResult {
    let document = fixture()?;
    let mut policy = mapping();
    policy
        .person_mappings
        .insert("Staff".to_owned(), id(1).parse()?);
    policy
        .assignment_type_mappings
        .insert("Clinic".to_owned(), id(4).parse()?);
    let source = format!(
        "person,type,eligible\nStaff,Clinic,false\n{},{},true\n",
        id(1).to_ascii_uppercase(),
        id(4).to_ascii_uppercase()
    );
    let blocked = preview(&document, source.as_bytes(), &policy, &[])?;
    assert_eq!(blocked.disposition, PeopleImportDisposition::Blocked);
    assert!(blocked.approval_digest.is_none() && blocked.batch.is_none());
    assert!(
        blocked
            .rows
            .iter()
            .all(|row| row.issue == Some(EligibilityRowIssue::DuplicatePair))
    );
    let selected = preview(&document, source.as_bytes(), &policy, &[3])?;
    let mutation = commands::apply_batch(&document, selected.batch.as_ref().ok_or("batch")?)?;
    assert_eq!(
        mutation.document.domain.entities[&id(1).parse()?]["eligibleAssignmentTypeIds"],
        json!([])
    );
    assert_eq!(
        selected.excluded_rows,
        [EligibilityExcludedRow {
            record: 3,
            skipped: true,
            issue: Some(EligibilityRowIssue::DuplicatePair)
        }]
    );
    policy.person_mappings.insert(id(999), id(1).parse()?);
    assert_eq!(
        preview(&document, source.as_bytes(), &policy, &[])
            .err()
            .ok_or("UUID alias accepted")?
            .code,
        CsvErrorCode::InvalidMapping
    );
    Ok(())
}

#[test]
fn invalid_membership_and_wrong_kind_targets_never_become_changes() -> TestResult {
    let document = fixture()?;
    for token in ["TRUE", " true", "1", ""] {
        let source = format!("person,type,eligible\n{},{},{token}\n", id(1), id(4));
        let proposal = preview(&document, source.as_bytes(), &mapping(), &[])?;
        assert_eq!(proposal.disposition, PeopleImportDisposition::Blocked);
        assert_eq!(
            proposal.rows[0].issue,
            Some(EligibilityRowIssue::InvalidMembership)
        );
        assert!(proposal.batch.is_none());
    }
    let source = format!(
        "person,type,eligible\n{},{},true\n{},{},true\n",
        id(5),
        id(4),
        id(1),
        id(5)
    );
    let proposal = preview(&document, source.as_bytes(), &mapping(), &[])?;
    assert_eq!(
        proposal
            .rows
            .iter()
            .map(|row| row.issue)
            .collect::<Vec<_>>(),
        [
            Some(EligibilityRowIssue::UnresolvedPerson),
            Some(EligibilityRowIssue::UnresolvedAssignmentType)
        ]
    );
    assert!(proposal.batch.is_none() && proposal.approval_digest.is_none());
    Ok(())
}

#[test]
fn complete_skipped_report_and_review_bind_every_original_byte() -> TestResult {
    let document = fixture()?;
    let source = format!(
        "person,type,eligible\n{}",
        "unknown,missing,true\n".repeat(10_000)
    );
    let skips: Vec<_> = (2..=10_001).collect();
    let proposal = preview(&document, source.as_bytes(), &mapping(), &skips)?;
    assert_eq!(proposal.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(proposal.excluded_rows.len(), 10_000);
    assert_eq!(
        proposal.excluded_rows.last().ok_or("last row")?,
        &EligibilityExcludedRow {
            record: 10_001,
            skipped: true,
            issue: Some(EligibilityRowIssue::UnresolvedPerson)
        }
    );
    let review = decode_eligibility_csv_review(&serde_json::to_vec(
        proposal.review.as_ref().ok_or("review")?,
    )?)?;
    let digest = proposal.approval_digest.as_deref().ok_or("digest")?;
    let rebuilt = rebuild_eligibility_csv_review(
        &document,
        Revision::INITIAL,
        &mut Cursor::new(&source),
        &review,
        digest,
        &CancellationToken::default(),
    )?;
    assert_eq!(rebuilt, proposal);
    let tampered = source.replacen("unknown", "different", 1);
    assert_eq!(
        rebuild_eligibility_csv_review(
            &document,
            Revision::INITIAL,
            &mut Cursor::new(tampered),
            &review,
            digest,
            &CancellationToken::default()
        )
        .err()
        .ok_or("changed skipped bytes")?
        .code,
        CsvErrorCode::StaleReview
    );
    assert_eq!(
        preview(
            &document,
            format!("{source}unknown,missing,true\n").as_bytes(),
            &mapping(),
            &skips
        )
        .err()
        .ok_or("record ceiling")?
        .code,
        CsvErrorCode::LogicalRecordLimit
    );
    Ok(())
}

#[test]
fn mapping_and_eof_admission_reject_oversized_aliases_and_nonexistent_skips() -> TestResult {
    let document = fixture()?;
    let mut policy = mapping();
    policy.assignment_type_mappings = (0..10_001)
        .map(|index| (format!("type-{index}"), id(4).parse()))
        .map(|(token, value)| value.map(|id| (token, id)))
        .collect::<Result<_, _>>()?;
    assert!(serde_json::to_vec(&policy)?.len() < 1024 * 1024);
    assert_eq!(
        preview(&document, b"person,type,eligible\n", &policy, &[])
            .err()
            .ok_or("alias count")?
            .code,
        CsvErrorCode::InvalidMapping
    );
    assert_eq!(
        preview(&document, b"", &mapping(), &[])
            .err()
            .ok_or("missing header")?
            .code,
        CsvErrorCode::HeaderMismatch
    );
    assert_eq!(
        preview(&document, b"person,type,eligible\n", &mapping(), &[2])
            .err()
            .ok_or("missing record")?
            .code,
        CsvErrorCode::InvalidDecision
    );
    Ok(())
}

#[test]
fn batch_limit_counts_changed_people_not_display_axes_or_unchanged_rows() -> TestResult {
    let mut document = fixture()?;
    let original = document.domain.entities[&id(1).parse()?].clone();
    let mut source = String::from("person,type,eligible\n");
    for index in 1000..2001 {
        let mut person = original.clone();
        person["id"] = json!(id(index));
        person["externalId"] = json!(format!("staff-{index}"));
        document.domain.entities.insert(id(index).parse()?, person);
        writeln!(source, "{},{},false", id(index), id(4))?;
    }
    assert_eq!(
        preview(&document, source.as_bytes(), &mapping(), &[])
            .err()
            .ok_or("command ceiling")?
            .code,
        CsvErrorCode::InvalidBatch
    );
    let no_op = preview(
        &document,
        source.replace("false", "true").as_bytes(),
        &mapping(),
        &[],
    )?;
    assert_eq!(no_op.disposition, PeopleImportDisposition::NoChanges);
    assert!(no_op.batch.is_none());
    assert_eq!(no_op.rows.len(), 1001);
    Ok(())
}

#[test]
fn source_failures_precede_skip_choices_and_cancellation() -> TestResult {
    let document = fixture()?;
    assert_eq!(
        preview(
            &document,
            b"person,type,eligible\n\xff,type,true\n",
            &mapping(),
            &[2]
        )
        .err()
        .ok_or("invalid UTF-8")?
        .code,
        CsvErrorCode::InvalidUtf8
    );
    let oversized = format!(
        "person,type,eligible\n{},{},{}\n",
        id(1),
        id(4),
        "x".repeat(16 * 1024 + 1)
    );
    assert_eq!(
        preview(&document, oversized.as_bytes(), &mapping(), &[2])
            .err()
            .ok_or("oversized skipped cell")?
            .code,
        CsvErrorCode::CellLimit
    );
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    assert_eq!(
        preview_eligibility_csv(
            &document,
            Revision::INITIAL,
            &mut Cursor::new(b""),
            &mapping(),
            &[],
            &cancellation
        )
        .err()
        .ok_or("cancelled preview")?
        .code,
        CsvErrorCode::Cancelled
    );
    Ok(())
}

#[test]
fn approval_rejects_other_revisions_snapshots_and_review_versions() -> TestResult {
    let document = fixture()?;
    let source = format!("person,type,eligible\n{},{},false\n", id(1), id(4));
    let proposal = preview(&document, source.as_bytes(), &mapping(), &[])?;
    let review = proposal.review.as_ref().ok_or("review")?;
    let digest = proposal.approval_digest.as_deref().ok_or("approval")?;
    let mut altered = document.clone();
    altered
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("person")?["name"] = json!("Changed name");
    for (snapshot, revision) in [(&document, Revision::new(1)), (&altered, Revision::INITIAL)] {
        assert_eq!(
            rebuild_eligibility_csv_review(
                snapshot,
                revision,
                &mut Cursor::new(&source),
                review,
                digest,
                &CancellationToken::default()
            )
            .err()
            .ok_or("stale approval")?
            .code,
            CsvErrorCode::StaleReview
        );
    }
    let mut newer = review.clone();
    newer.schema_version = 2;
    assert_eq!(
        decode_eligibility_csv_review(&serde_json::to_vec(&newer)?)
            .err()
            .ok_or("newer review")?
            .code,
        CsvErrorCode::UnsupportedVersion
    );
    Ok(())
}

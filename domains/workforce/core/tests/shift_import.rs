mod support;

use eutheto_types::{
    CancellationToken, GapPolicy, Horizon, OverlapPolicy, Revision, ScenarioDocument,
};
use eutheto_workforce::{
    commands,
    people_csv::{
        BlankPolicy, CsvDialect, CsvError, CsvErrorCode, MAX_CSV_CELL_BYTES, MAX_CSV_REVIEW_BYTES,
        MAX_CSV_SOURCE_BYTES, PeopleImportDisposition,
    },
    shift_import::*,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    io::{Cursor, Read},
};
use support::{fixture, id};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn mapping(fields: &[ShiftCsvField]) -> ShiftCsvMapping {
    let columns = fields
        .iter()
        .zip(1..)
        .map(|(field, index)| ShiftCsvColumn {
            index,
            field: *field,
            blank: BlankPolicy::Preserve,
        })
        .collect::<Vec<_>>();
    ShiftCsvMapping {
        dialect: CsvDialect::Tab,
        has_header: true,
        expected_columns: columns.last().map_or(1, |column| column.index + 1),
        identity_column: Some(0),
        columns,
        reference_mappings: BTreeMap::new(),
    }
}
fn complete_mapping() -> ShiftCsvMapping {
    mapping(&[
        ShiftCsvField::AssignmentTypeId,
        ShiftCsvField::LocationId,
        ShiftCsvField::StartsAt,
        ShiftCsvField::EndsAt,
        ShiftCsvField::Coverage,
        ShiftCsvField::Tags,
        ShiftCsvField::ReportingAttribution,
    ])
}
fn source(rows: &[Vec<String>]) -> String {
    let width = rows.first().map_or(1, Vec::len);
    let mut text = (0..width)
        .map(|index| format!("column{index}"))
        .collect::<Vec<_>>()
        .join("\t");
    text.push('\n');
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index > 0 {
                text.push('\t');
            }
            if cell.contains(['"', '\t', '\r', '\n']) {
                text.push('\"');
                text.push_str(&cell.replace('\"', "\"\""));
                text.push('\"');
            } else {
                text.push_str(cell);
            }
        }
        text.push('\n');
    }
    text
}
fn complete_row(shift: String) -> Vec<String> {
    vec![
        shift,
        id(4),
        id(5),
        "2026-11-01T09:00:00".to_owned(),
        "2026-11-01T10:00:00".to_owned(),
        json!({"kind":"exact","count":1,"qualificationMinimums":[]}).to_string(),
        "[]".to_owned(),
        "startLocalDate".to_owned(),
    ]
}
fn decision(record: u32, add: bool, target: u32) -> TestResult<ShiftCsvRowDecision> {
    Ok(ShiftCsvRowDecision {
        record,
        decision: if add {
            ShiftCsvDecision::Add {
                shift_id: id(target).parse()?,
            }
        } else {
            ShiftCsvDecision::Update {
                shift_id: id(target).parse()?,
            }
        },
    })
}
fn preview(
    document: &ScenarioDocument,
    source: &str,
    mapping: &ShiftCsvMapping,
    decisions: &[ShiftCsvRowDecision],
) -> Result<ShiftImportPreview, CsvError> {
    preview_shift_csv(
        document,
        Revision::INITIAL,
        &mut Cursor::new(source.as_bytes()),
        mapping,
        decisions,
        &BTreeSet::new(),
        &CancellationToken::default(),
    )
}
fn shift(document: &mut ScenarioDocument) -> TestResult<&mut Value> {
    document
        .domain
        .entities
        .get_mut(&id(8).parse()?)
        .ok_or_else(|| "shift".into())
}
fn assert_blocked(preview: &ShiftImportPreview, issue: ShiftCsvRowIssue) {
    assert_eq!(preview.disposition, PeopleImportDisposition::Blocked);
    assert_eq!(preview.rows[0].issue, Some(issue));
    assert!(preview.batch.is_none());
    assert!(preview.review.is_none());
}

#[test]
fn supplied_absent_and_unknown_identities_preserve_explicit_selection() -> TestResult {
    let document = fixture()?;
    let policy = mapping(&[ShiftCsvField::Tags]);
    let supplied = preview(
        &document,
        &source(&[vec![id(8), "[\"reviewed\"]".to_owned()]]),
        &policy,
        &[],
    )?;
    assert_eq!(
        supplied.rows[0].identity,
        ShiftCsvIdentity::Supplied {
            shift_id: id(8).parse()?
        }
    );
    assert_eq!(supplied.rows[0].status, ShiftCsvRowStatus::Updated);
    let absent_source = source(&[vec![String::new(), "[\"reviewed\"]".to_owned()]]);
    let absent = preview(&document, &absent_source, &policy, &[])?;
    assert_eq!(absent.rows[0].identity, ShiftCsvIdentity::Absent {});
    assert_eq!(absent.rows[0].status, ShiftCsvRowStatus::Unresolved);
    assert!(absent.rows[0].shift_id.is_none());
    assert_eq!(
        preview(
            &document,
            &absent_source,
            &policy,
            &[decision(2, false, 8)?]
        )?
        .rows[0]
            .status,
        ShiftCsvRowStatus::Updated
    );
    let unknown_source = source(&[complete_row(id(99))]);
    let unknown = preview(&document, &unknown_source, &complete_mapping(), &[])?;
    assert_eq!(unknown.rows[0].status, ShiftCsvRowStatus::Unresolved);
    assert_eq!(
        unknown.rows[0].identity,
        ShiftCsvIdentity::Supplied {
            shift_id: id(99).parse()?
        }
    );
    let added = preview(
        &document,
        &unknown_source,
        &complete_mapping(),
        &[decision(2, true, 99)?],
    )?;
    assert_eq!(added.rows[0].status, ShiftCsvRowStatus::Added);
    assert_eq!(
        added.rows[0].after.as_ref().ok_or("after")?["id"],
        json!(id(99))
    );
    assert_eq!(
        added.rows[0].after.as_ref().ok_or("after")?["origin"],
        json!({"kind":"manual"})
    );
    assert_eq!(
        preview(
            &document,
            &unknown_source,
            &complete_mapping(),
            &[decision(2, true, 98)?]
        )
        .err()
        .ok_or("redirect")?
        .code,
        CsvErrorCode::InvalidDecision
    );
    assert_eq!(
        preview(
            &document,
            &unknown_source,
            &complete_mapping(),
            &[decision(2, false, 99)?]
        )
        .err()
        .ok_or("unknown update")?
        .code,
        CsvErrorCode::InvalidDecision
    );
    Ok(())
}

#[test]
fn invalid_and_owned_identities_cannot_be_added() -> TestResult {
    let document = fixture()?;
    let policy = mapping(&[ShiftCsvField::Tags]);
    for token in [
        "not an identity".to_owned(),
        id(8).replace("7000", "4000"),
        format!(" {}", id(8)),
    ] {
        let invalid = preview(
            &document,
            &source(&[vec![token, "[]".to_owned()]]),
            &policy,
            &[],
        )?;
        assert_eq!(invalid.rows[0].identity, ShiftCsvIdentity::Invalid {});
        assert_blocked(&invalid, ShiftCsvRowIssue::InvalidIdentity);
    }
    // Entity, occurrence-ledger, nested score-policy definition, scenario and lock identities.
    for target in [4, 7, 10, 100, 14] {
        let reserved = preview(
            &document,
            &source(&[complete_row(id(target))]),
            &complete_mapping(),
            &[decision(2, true, target)?],
        )?;
        assert_blocked(&reserved, ShiftCsvRowIssue::ReservedIdentity);
    }
    let mut extension_document = document.clone();
    extension_document.extensions.insert(
        "nonsemantic.example.identity".to_owned(),
        json!({(id(90)):{"id":id(90)}}),
    );
    assert_blocked(
        &preview(
            &extension_document,
            &source(&[complete_row(id(90))]),
            &complete_mapping(),
            &[decision(2, true, 90)?],
        )?,
        ShiftCsvRowIssue::ReservedIdentity,
    );
    let reserved = BTreeSet::from([id(91).parse()?]);
    let wrapper = preview_shift_csv(
        &document,
        Revision::INITIAL,
        &mut Cursor::new(source(&[complete_row(id(91))])),
        &complete_mapping(),
        &[decision(2, true, 91)?],
        &reserved,
        &CancellationToken::default(),
    )?;
    assert_blocked(&wrapper, ShiftCsvRowIssue::ReservedIdentity);
    let mut no_identity = complete_mapping();
    no_identity.identity_column = None;
    assert_eq!(
        preview(
            &document,
            &source(&[complete_row(String::new())]),
            &no_identity,
            &[decision(2, true, 98)?]
        )?
        .rows[0]
            .identity,
        ShiftCsvIdentity::Absent {}
    );
    Ok(())
}

#[test]
fn complete_new_rows_require_authored_fields_and_optional_defaults_only() -> TestResult {
    let document = fixture()?;
    for missing in [1, 3, 4, 5, 7] {
        let mut row = complete_row(id(99));
        row[missing].clear();
        assert_blocked(
            &preview(
                &document,
                &source(&[row]),
                &complete_mapping(),
                &[decision(2, true, 99)?],
            )?,
            ShiftCsvRowIssue::MissingRequiredField,
        );
    }
    for omitted in [
        ShiftCsvField::AssignmentTypeId,
        ShiftCsvField::StartsAt,
        ShiftCsvField::EndsAt,
        ShiftCsvField::Coverage,
        ShiftCsvField::ReportingAttribution,
    ] {
        let mut policy = complete_mapping();
        policy.columns.retain(|column| column.field != omitted);
        assert_blocked(
            &preview(
                &document,
                &source(&[complete_row(id(99))]),
                &policy,
                &[decision(2, true, 99)?],
            )?,
            ShiftCsvRowIssue::MissingRequiredField,
        );
    }
    let mut optional_document = document.clone();
    optional_document
        .domain
        .entities
        .get_mut(&id(4).parse()?)
        .ok_or("type")?["locationBehavior"] = json!({"kind":"optional"});
    let mut policy = complete_mapping();
    policy.columns.retain(|column| {
        !matches!(
            column.field,
            ShiftCsvField::LocationId | ShiftCsvField::Tags
        )
    });
    let added = preview(
        &optional_document,
        &source(&[complete_row(String::new())]),
        &policy,
        &[decision(2, true, 99)?],
    )?;
    let after = added.rows[0].after.as_ref().ok_or("after")?;
    assert!(after.get("locationId").is_none());
    assert_eq!(after["tags"], json!([]));
    let mutation = commands::apply_batch(&optional_document, added.batch.as_ref().ok_or("batch")?)?;
    assert_eq!(
        commands::apply_batch(&mutation.document, &mutation.inverse)?.document,
        optional_document
    );
    Ok(())
}

#[test]
fn partial_updates_preserve_raw_endpoints_and_detached_origin_with_exact_inverse() -> TestResult {
    let mut document = fixture()?;
    document.domain.locked_assignments.clear();
    document
        .domain
        .entities
        .get_mut(&id(6).parse()?)
        .ok_or("template")?["occurrenceIdentities"] = json!({});
    let raw = shift(&mut document)?;
    raw["id"] = json!(id(8).to_ascii_uppercase());
    raw["assignmentTypeId"] = json!(id(4).to_ascii_uppercase());
    raw["locationId"] = json!(id(5).replace('-', ""));
    raw["startsAt"]["instant"] = json!("2026-11-01T05:30:00+00:00");
    raw["startsAt"]["local"] = json!("2026-11-01T01:30:00.000");
    raw["origin"] = json!({"kind":"detached","templateId":id(6).to_ascii_uppercase(),"occurrenceDate":"2026-11-01"});
    let original = raw.clone();
    // Reject-fold scenario is deliberate: no unmapped endpoint may be re-resolved.
    let policy = mapping(&[
        ShiftCsvField::Tags,
        ShiftCsvField::AssignmentTypeId,
        ShiftCsvField::LocationId,
    ]);
    let proposal = preview(
        &document,
        &source(&[vec![id(8), "[\"changed\"]".to_owned(), id(4), id(5)]]),
        &policy,
        &[],
    )?;
    let mut expected = original.clone();
    expected["tags"] = json!(["changed"]);
    assert_eq!(proposal.rows[0].before.as_ref(), Some(&original));
    assert_eq!(proposal.rows[0].after.as_ref(), Some(&expected));
    let mutation = commands::apply_batch(&document, proposal.batch.as_ref().ok_or("batch")?)?;
    assert_eq!(mutation.document.domain.entities[&id(8).parse()?], expected);
    let undo = commands::apply_batch(&mutation.document, &mutation.inverse)?;
    assert_eq!(undo.document, document);
    assert_eq!(
        commands::apply_batch(&undo.document, &undo.inverse)?.document,
        mutation.document
    );
    let preserve = preview(
        &document,
        &source(&[vec![id(8), String::new(), String::new(), String::new()]]),
        &policy,
        &[],
    )?;
    assert_eq!(preserve.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(preserve.rows[0].before.as_ref(), Some(&original));
    assert!(preserve.rows[0].after.is_none());
    assert!(preserve.batch.is_none());
    Ok(())
}

#[test]
fn mapped_equivalent_local_times_preserve_raw_spelling_and_policy_changes_are_real_edits()
-> TestResult {
    let mut document = fixture()?;
    document.settings.overlap_policy = OverlapPolicy::Earlier;
    let raw = shift(&mut document)?;
    raw["startsAt"]["instant"] = json!("2026-11-01T05:30:00+00:00");
    raw["startsAt"]["local"] = json!("2026-11-01T01:30:00.000");
    raw["endsAt"]["instant"] = json!("2026-11-01T07:30:00+00:00");
    raw["endsAt"]["local"] = json!("2026-11-01T02:30:00.000");
    let original = raw.clone();
    let text = source(&[vec![
        id(8),
        "2026-11-01T01:30".to_owned(),
        "2026-11-01T02:30".to_owned(),
    ]]);
    let policy = mapping(&[ShiftCsvField::StartsAt, ShiftCsvField::EndsAt]);
    let no_op = preview(&document, &text, &policy, &[])?;
    assert_eq!(no_op.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(no_op.rows[0].before.as_ref(), Some(&original));
    assert!(no_op.rows[0].after.is_none());
    document.settings.overlap_policy = OverlapPolicy::Later;
    let changed = preview(&document, &text, &policy, &[])?;
    assert_eq!(changed.rows[0].status, ShiftCsvRowStatus::Updated);
    let after = changed.rows[0].after.as_ref().ok_or("after")?;
    assert_eq!(after["startsAt"]["instant"], json!("2026-11-01T06:30:00Z"));
    assert_eq!(after["endsAt"], original["endsAt"]);
    let mutation = commands::apply_batch(&document, changed.batch.as_ref().ok_or("batch")?)?;
    assert_eq!(
        commands::apply_batch(&mutation.document, &mutation.inverse)?.document,
        document
    );
    Ok(())
}

#[test]
fn coverage_inner_sets_preserve_raw_but_outer_rows_counts_options_and_tags_are_authored()
-> TestResult {
    let mut document = fixture()?;
    document.domain.entities.insert(
        id(12).parse()?,
        json!({"kind":"qualification","id":id(12),"name":"Other","description":""}),
    );
    let a = json!({"qualifications":{"allQualificationIds":[id(11).to_ascii_uppercase(),id(12)],"anyQualificationIds":[id(12),id(11)]},"minimum":1});
    let b = json!({"qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":0});
    shift(&mut document)?["coverage"] = json!({"kind":"atLeast","minimum":1,"preferredCount":2,"maximumCount":3,"qualificationMinimums":[a.clone(),b.clone(),a.clone()]});
    shift(&mut document)?["tags"] = json!(["first", "second"]);
    let raw = shift(&mut document)?.clone();
    let mut equivalent = raw["coverage"].clone();
    for index in [0, 2] {
        equivalent["qualificationMinimums"][index]["qualifications"]["allQualificationIds"] =
            json!([id(12), id(11)]);
        equivalent["qualificationMinimums"][index]["qualifications"]["anyQualificationIds"] =
            json!([id(11), id(12).to_ascii_uppercase()]);
    }
    let policy = mapping(&[ShiftCsvField::Coverage, ShiftCsvField::Tags]);
    let noop = preview(
        &document,
        &source(&[vec![
            id(8),
            equivalent.to_string(),
            "[\"first\",\"second\"]".to_owned(),
        ]]),
        &policy,
        &[],
    )?;
    assert_eq!(noop.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(noop.rows[0].before.as_ref(), Some(&raw));
    assert!(noop.rows[0].after.is_none());
    let mut variants = Vec::new();
    let mut preferred = equivalent.clone();
    preferred["preferredCount"] = json!(3);
    variants.push(preferred);
    let mut maximum = equivalent.clone();
    maximum["maximumCount"] = json!(4);
    variants.push(maximum);
    let mut minimum = equivalent.clone();
    minimum["minimum"] = json!(0);
    variants.push(minimum);
    let mut row_order = equivalent.clone();
    row_order["qualificationMinimums"]
        .as_array_mut()
        .ok_or("minima")?
        .swap(0, 1);
    variants.push(row_order);
    let mut multiplicity = equivalent.clone();
    multiplicity["qualificationMinimums"]
        .as_array_mut()
        .ok_or("minima")?
        .pop();
    variants.push(multiplicity);
    let mut qualification_minimum = equivalent.clone();
    qualification_minimum["qualificationMinimums"][0]["minimum"] = json!(0);
    variants.push(qualification_minimum);
    variants.push(json!({"kind":"exact","count":2,"qualificationMinimums":[a,b]}));
    for coverage in variants {
        let changed = preview(
            &document,
            &source(&[vec![id(8), coverage.to_string(), String::new()]]),
            &policy,
            &[],
        )?;
        assert_eq!(changed.rows[0].status, ShiftCsvRowStatus::Updated);
        assert_eq!(
            changed.rows[0].after.as_ref().ok_or("after")?["coverage"],
            coverage
        );
        assert_eq!(
            changed.rows[0].after.as_ref().ok_or("after")?["tags"],
            raw["tags"]
        );
        let mutation = commands::apply_batch(&document, changed.batch.as_ref().ok_or("batch")?)?;
        assert_eq!(
            commands::apply_batch(&mutation.document, &mutation.inverse)?.document,
            document
        );
    }
    let reorder_tags = preview(
        &document,
        &source(&[vec![
            id(8),
            equivalent.to_string(),
            "[\"second\",\"first\"]".to_owned(),
        ]]),
        &policy,
        &[],
    )?;
    assert_eq!(reorder_tags.rows[0].status, ShiftCsvRowStatus::Updated);
    assert_eq!(
        reorder_tags.rows[0].after.as_ref().ok_or("after")?["coverage"],
        raw["coverage"]
    );
    Ok(())
}

#[test]
fn raw_unknown_keys_optional_nulls_bad_references_and_unsafe_json_cannot_be_laundered() -> TestResult
{
    let document = fixture()?;
    let policy = mapping(&[ShiftCsvField::Coverage]);
    for cell in [
        r#"{"kind":"exact","count":1,"qualificationMinimums":[],"unknown":true}"#,
        r#"{"kind":"atLeast","minimum":1,"preferredCount":null,"qualificationMinimums":[]}"#,
        r#"{"kind":"atLeast","minimum":1,"maximumCount":null,"qualificationMinimums":[]}"#,
        r#"{"kind":"exact","count":1,"count":2,"qualificationMinimums":[]}"#,
        r#"{"kind":"exact","count":1,"qualificationMinimums":[{"minimum":1,"qualifications":{"allQualificationIds":[],"anyQualificationIds":[],"extra":1}}]}"#,
        r#"{"kind":"atLeast","minimum":3,"preferredCount":1,"qualificationMinimums":[]}"#,
        r#"{"kind":"exact","count":1,"qualificationMinimums":[],"password":"secret"}"#,
    ] {
        let blocked = preview(
            &document,
            &source(&[vec![id(8), cell.to_owned()]]),
            &policy,
            &[],
        )?;
        assert_eq!(blocked.disposition, PeopleImportDisposition::Blocked);
        assert!(blocked.rows[0].after.is_none());
    }
    let mut alias_policy = complete_mapping();
    alias_policy.reference_mappings = [("Clinic", 4), ("North", 5), ("Clinician", 11)]
        .into_iter()
        .map(|(token, index)| Ok((token.to_owned(), id(index).parse()?)))
        .collect::<TestResult<_>>()?;
    let mut row = complete_row(id(99));
    row[1] = "Clinic".to_owned();
    row[2] = "North".to_owned();
    row[5] = json!({"kind":"atLeast","minimum":1,"maximumCount":4,"qualificationMinimums":[{"minimum":1,"qualifications":{"allQualificationIds":["Clinician"],"anyQualificationIds":[]}}]}).to_string();
    let added = preview(
        &document,
        &source(&[row]),
        &alias_policy,
        &[decision(2, true, 99)?],
    )?;
    assert_eq!(added.rows[0].status, ShiftCsvRowStatus::Added);
    assert_eq!(
        added.rows[0].after.as_ref().ok_or("after")?["coverage"]["qualificationMinimums"][0]["qualifications"]
            ["allQualificationIds"],
        json!([id(11)])
    );
    for token in [
        id(5),
        id(99),
        " Clinic".to_owned(),
        id(4).replace("7000", "4000"),
    ] {
        assert_blocked(
            &preview(
                &document,
                &source(&[vec![id(8), token]]),
                &mapping(&[ShiftCsvField::AssignmentTypeId]),
                &[],
            )?,
            ShiftCsvRowIssue::InvalidReference,
        );
    }
    for token in [id(4), id(4).replace("7000", "4000")] {
        let mut policy = mapping(&[ShiftCsvField::AssignmentTypeId]);
        policy.reference_mappings.insert(token, id(4).parse()?);
        assert_eq!(
            preview(&document, "column0\tcolumn1\n", &policy, &[])
                .err()
                .ok_or("shadow mapping")?
                .code,
            CsvErrorCode::InvalidMapping
        );
    }
    Ok(())
}

#[test]
fn duplicate_targets_include_noops_bad_cells_unknown_ids_and_absent_decisions_but_not_skips()
-> TestResult {
    let document = fixture()?;
    let policy = mapping(&[ShiftCsvField::Tags]);
    for rows in [
        vec![
            vec![id(8), "[]".to_owned()],
            vec![id(8).to_ascii_uppercase(), "[]".to_owned()],
        ],
        vec![
            vec![id(8), "not JSON".to_owned()],
            vec![id(8), "[\"change\"]".to_owned()],
        ],
        vec![vec![id(99), "[]".to_owned()], vec![id(99), "[]".to_owned()]],
    ] {
        let blocked = preview(&document, &source(&rows), &policy, &[])?;
        assert!(
            blocked
                .rows
                .iter()
                .all(|row| row.status == ShiftCsvRowStatus::Blocked
                    && row.issue == Some(ShiftCsvRowIssue::DuplicateTarget))
        );
        assert_eq!(blocked.excluded_rows.len(), 2);
    }
    let rows = vec![
        vec![String::new(), "[]".to_owned()],
        vec![id(8), "[]".to_owned()],
    ];
    assert_blocked(
        &preview(
            &document,
            &source(&rows),
            &policy,
            &[decision(2, false, 8)?],
        )?,
        ShiftCsvRowIssue::DuplicateTarget,
    );
    let skipped = preview(
        &document,
        &source(&[
            vec![id(8), "not JSON".to_owned()],
            vec![id(8), "[]".to_owned()],
        ]),
        &policy,
        &[ShiftCsvRowDecision {
            record: 2,
            decision: ShiftCsvDecision::Skip {},
        }],
    )?;
    assert_eq!(skipped.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(skipped.rows[0].status, ShiftCsvRowStatus::Skipped);
    assert_eq!(skipped.rows[1].status, ShiftCsvRowStatus::Unchanged);
    assert!(skipped.rows[0].issue.is_none());
    let wrong_width = "id\ttags\nnot-an-id\n";
    assert_eq!(
        preview(
            &document,
            wrong_width,
            &policy,
            &[ShiftCsvRowDecision {
                record: 2,
                decision: ShiftCsvDecision::Skip {}
            }]
        )?
        .rows[0]
            .status,
        ShiftCsvRowStatus::Skipped
    );
    assert_blocked(
        &preview(&document, wrong_width, &policy, &[])?,
        ShiftCsvRowIssue::InvalidIdentity,
    );
    Ok(())
}

#[test]
fn clear_only_affects_optional_location_and_tags() -> TestResult {
    let mut document = fixture()?;
    document
        .domain
        .entities
        .get_mut(&id(4).parse()?)
        .ok_or("type")?["locationBehavior"] = json!({"kind":"optional"});
    shift(&mut document)?["tags"] = json!(["old"]);
    let mut policy = mapping(&[ShiftCsvField::LocationId, ShiftCsvField::Tags]);
    for column in &mut policy.columns {
        column.blank = BlankPolicy::Clear;
    }
    let changed = preview(
        &document,
        &source(&[vec![id(8), String::new(), String::new()]]),
        &policy,
        &[],
    )?;
    let after = changed.rows[0].after.as_ref().ok_or("after")?;
    assert!(after.get("locationId").is_none());
    assert_eq!(after["tags"], json!([]));
    for field in [
        ShiftCsvField::AssignmentTypeId,
        ShiftCsvField::StartsAt,
        ShiftCsvField::EndsAt,
        ShiftCsvField::Coverage,
        ShiftCsvField::ReportingAttribution,
    ] {
        let mut invalid = mapping(&[field]);
        invalid.columns[0].blank = BlankPolicy::Clear;
        assert_eq!(
            preview(&document, "id\tfield\n", &invalid, &[])
                .err()
                .ok_or("clear required")?
                .code,
            CsvErrorCode::InvalidMapping
        );
    }
    Ok(())
}

#[test]
fn local_intent_refuses_offsets_annotations_and_nonpositive_or_implicit_overnight_intervals()
-> TestResult {
    let document = fixture()?;
    for local in [
        "2026-11-01T09:00:00Z",
        "2026-11-01T09:00:00-05:00",
        "2026-11-01T09:00:00[America/New_York]",
        "2026-11-01T09:00:00-05:00[America/New_York]",
        "2026-11-01",
        " 2026-11-01T09:00:00",
    ] {
        let mut row = complete_row(id(99));
        row[3] = local.to_owned();
        assert_blocked(
            &preview(
                &document,
                &source(&[row]),
                &complete_mapping(),
                &[decision(2, true, 99)?],
            )?,
            ShiftCsvRowIssue::InvalidLocalTime,
        );
    }
    for (start, end) in [
        ("2026-11-01T09:00", "2026-11-01T09:00"),
        ("2026-11-01T22:00", "2026-11-01T06:00"),
    ] {
        let mut row = complete_row(id(99));
        row[3] = start.to_owned();
        row[4] = end.to_owned();
        assert_blocked(
            &preview(
                &document,
                &source(&[row]),
                &complete_mapping(),
                &[decision(2, true, 99)?],
            )?,
            ShiftCsvRowIssue::NonpositiveInterval,
        );
    }
    let mut row = complete_row(id(99));
    row[3] = "2026-11-01T22:00".to_owned();
    row[4] = "2026-11-02T06:00".to_owned();
    assert_eq!(
        preview(
            &document,
            &source(&[row]),
            &complete_mapping(),
            &[decision(2, true, 99)?]
        )?
        .rows[0]
            .status,
        ShiftCsvRowStatus::Added
    );
    Ok(())
}

#[test]
fn gap_and_fold_resolution_use_the_captured_scenario_policy_and_expose_original_intent()
-> TestResult {
    let mut document = fixture()?;
    let mut row = complete_row(id(99));
    row[3] = "2026-11-01T01:30:00".to_owned();
    row[4] = "2026-11-01T02:30:00".to_owned();
    assert_blocked(
        &preview(
            &document,
            &source(&[row.clone()]),
            &complete_mapping(),
            &[decision(2, true, 99)?],
        )?,
        ShiftCsvRowIssue::UnresolvableLocalTime,
    );
    for (policy, instant, offset) in [
        (OverlapPolicy::Earlier, "2026-11-01T05:30:00Z", -14400),
        (OverlapPolicy::Later, "2026-11-01T06:30:00Z", -18000),
    ] {
        document.settings.overlap_policy = policy;
        let resolved = preview(
            &document,
            &source(&[row.clone()]),
            &complete_mapping(),
            &[decision(2, true, 99)?],
        )?;
        let after = resolved.rows[0].after.as_ref().ok_or("after")?;
        assert_eq!(after["startsAt"]["instant"], json!(instant));
        assert_eq!(after["startsAt"]["offsetSeconds"], json!(offset));
        assert_eq!(after["startsAt"]["local"], json!("2026-11-01T01:30:00"));
    }
    document.domain.locked_assignments.clear();
    document.domain.entities.remove(&id(6).parse()?);
    document.domain.entities.remove(&id(8).parse()?);
    document.settings.horizon = Horizon::new(
        "2026-03-08T05:00:00Z".parse()?,
        "2026-03-09T04:00:00Z".parse()?,
    )?;
    row[3] = "2026-03-08T02:30:00".to_owned();
    row[4] = "2026-03-08T04:00:00".to_owned();
    document.settings.gap_policy = GapPolicy::Reject;
    assert_blocked(
        &preview(
            &document,
            &source(&[row.clone()]),
            &complete_mapping(),
            &[decision(2, true, 99)?],
        )?,
        ShiftCsvRowIssue::UnresolvableLocalTime,
    );
    document.settings.gap_policy = GapPolicy::MoveForward;
    let resolved = preview(
        &document,
        &source(&[row]),
        &complete_mapping(),
        &[decision(2, true, 99)?],
    )?;
    let after = resolved.rows[0].after.as_ref().ok_or("after")?;
    assert_eq!(after["startsAt"]["local"], json!("2026-03-08T02:30:00"));
    assert_eq!(after["startsAt"]["instant"], json!("2026-03-08T07:30:00Z"));
    assert_eq!(after["startsAt"]["offsetSeconds"], json!(-14400));
    Ok(())
}

#[test]
fn review_round_trip_rebuild_stale_source_snapshot_revision_census_and_tamper() -> TestResult {
    let document = fixture()?;
    let text = source(&[vec![id(8), "[\"reviewed\"]".to_owned()]]);
    let approved = preview(&document, &text, &mapping(&[ShiftCsvField::Tags]), &[])?;
    let review = approved.review.as_ref().ok_or("review")?;
    let digest = approved.approval_digest.as_deref().ok_or("digest")?;
    let bytes = serde_json::to_vec(review)?;
    assert_eq!(decode_shift_csv_review(&bytes)?, *review);
    assert_eq!(
        rebuild_shift_csv_review(
            &document,
            Revision::INITIAL,
            &mut Cursor::new(&text),
            review,
            digest,
            &BTreeSet::new(),
            &CancellationToken::default()
        )?,
        approved
    );
    let mut altered = document.clone();
    altered.metadata.title = "Changed".to_owned();
    for (snapshot, revision, input, census) in [
        (&document, Revision::new(1), text.clone(), BTreeSet::new()),
        (&altered, Revision::INITIAL, text.clone(), BTreeSet::new()),
        (
            &document,
            Revision::INITIAL,
            format!("{text}\n"),
            BTreeSet::new(),
        ),
        (
            &document,
            Revision::INITIAL,
            text.clone(),
            BTreeSet::from([id(90).parse()?]),
        ),
    ] {
        assert_eq!(
            rebuild_shift_csv_review(
                snapshot,
                revision,
                &mut Cursor::new(input),
                review,
                digest,
                &census,
                &CancellationToken::default()
            )
            .err()
            .ok_or("stale")?
            .code,
            CsvErrorCode::StaleReview
        );
    }
    let mut tampered = review.clone();
    tampered.rows_blake3 = "0".repeat(64);
    assert_eq!(
        rebuild_shift_csv_review(
            &document,
            Revision::INITIAL,
            &mut Cursor::new(&text),
            &tampered,
            digest,
            &BTreeSet::new(),
            &CancellationToken::default()
        )
        .err()
        .ok_or("tampered")?
        .code,
        CsvErrorCode::StaleReview
    );
    Ok(())
}

#[test]
fn review_wire_rejects_unknown_fields_versions_duplicate_keys_and_oversize() -> TestResult {
    let document = fixture()?;
    let text = source(&[vec![id(8), "[\"reviewed\"]".to_owned()]]);
    let approved = preview(&document, &text, &mapping(&[ShiftCsvField::Tags]), &[])?;
    let review = approved.review.as_ref().ok_or("review")?;
    let mut raw = serde_json::to_value(review)?;
    raw["unexpected"] = json!(true);
    assert_eq!(
        decode_shift_csv_review(&serde_json::to_vec(&raw)?)
            .err()
            .ok_or("unknown review")?
            .code,
        CsvErrorCode::InvalidReview
    );
    let mut newer = review.clone();
    newer.schema_version = 2;
    assert_eq!(
        decode_shift_csv_review(&serde_json::to_vec(&newer)?)
            .err()
            .ok_or("newer")?
            .code,
        CsvErrorCode::UnsupportedVersion
    );
    assert_eq!(
        decode_shift_csv_review(&vec![b' '; MAX_CSV_REVIEW_BYTES + 1])
            .err()
            .ok_or("review cap")?
            .code,
        CsvErrorCode::ReviewLimit
    );
    assert_eq!(
        decode_shift_csv_review(b"{\"format\":\"one\",\"format\":\"two\"}")
            .err()
            .ok_or("duplicate review key")?
            .code,
        CsvErrorCode::InvalidReview
    );
    Ok(())
}

#[test]
fn full_excluded_and_skipped_reports_have_no_200_row_truncation_and_source_before_skip()
-> TestResult {
    let document = fixture()?;
    let policy = mapping(&[ShiftCsvField::Tags]);
    let rows = (0..10_000)
        .map(|_| vec!["invalid identity".to_owned(), "not JSON".to_owned()])
        .collect::<Vec<_>>();
    let text = source(&rows);
    let skips = (2..=10_001)
        .map(|record| ShiftCsvRowDecision {
            record,
            decision: ShiftCsvDecision::Skip {},
        })
        .collect::<Vec<_>>();
    let skipped = preview(&document, &text, &policy, &skips)?;
    assert_eq!(skipped.disposition, PeopleImportDisposition::NoChanges);
    assert_eq!(skipped.rows.len(), 10_000);
    assert_eq!(skipped.excluded_rows.len(), 10_000);
    assert_eq!(skipped.excluded_rows.last().ok_or("last")?.record, 10_001);
    assert!(serde_json::to_vec(&skipped.excluded_rows)?.len() <= MAX_SHIFT_CSV_REPORT_BYTES);
    let blocked = preview(&document, &text, &policy, &[])?;
    assert_eq!(blocked.excluded_rows.len(), 10_000);
    assert_eq!(
        blocked.excluded_rows.last().ok_or("last")?.issue,
        Some(ShiftCsvRowIssue::InvalidIdentity)
    );
    let extra = format!("{text}invalid\tinvalid\n");
    assert_eq!(
        preview(&document, &extra, &policy, &skips)
            .err()
            .ok_or("record cap")?
            .code,
        CsvErrorCode::LogicalRecordLimit
    );
    let skip = [ShiftCsvRowDecision {
        record: 2,
        decision: ShiftCsvDecision::Skip {},
    }];
    let oversized = source(&[vec![String::new(), "x".repeat(MAX_CSV_CELL_BYTES + 1)]]);
    assert_eq!(
        preview(&document, &oversized, &policy, &skip)
            .err()
            .ok_or("cell cap")?
            .code,
        CsvErrorCode::CellLimit
    );
    assert_eq!(
        preview_shift_csv(
            &document,
            Revision::INITIAL,
            &mut Cursor::new(b"id\ttags\n\xff\tbad\n"),
            &policy,
            &skip,
            &BTreeSet::new(),
            &CancellationToken::default()
        )
        .err()
        .ok_or("UTF8")?
        .code,
        CsvErrorCode::InvalidUtf8
    );
    assert_eq!(
        preview(&document, "id\ttags\n", &policy, &skip)
            .err()
            .ok_or("missing decision record")?
            .code,
        CsvErrorCode::InvalidDecision
    );
    let raw_oversized = format!("id\ttags\n{}", "\n".repeat(MAX_CSV_SOURCE_BYTES));
    assert_eq!(
        preview(&document, &raw_oversized, &policy, &[])
            .err()
            .ok_or("source cap")?
            .code,
        CsvErrorCode::SourceByteLimit
    );
    Ok(())
}

#[test]
fn mapping_decision_and_header_bounds_are_native_admission() -> TestResult {
    let document = fixture()?;
    let mut policy = mapping(&[ShiftCsvField::Tags]);
    policy.columns[0].index = 0;
    assert_eq!(
        preview(&document, "id\ttags\n", &policy, &[])
            .err()
            .ok_or("duplicate column")?
            .code,
        CsvErrorCode::InvalidMapping
    );
    policy = mapping(&[ShiftCsvField::Tags]);
    policy
        .reference_mappings
        .insert("x".repeat(257), id(4).parse()?);
    assert_eq!(
        preview(&document, "id\ttags\n", &policy, &[])
            .err()
            .ok_or("oversized alias")?
            .code,
        CsvErrorCode::MappingLimit
    );
    let duplicate = [decision(2, false, 8)?, decision(2, false, 8)?];
    assert_eq!(
        preview(
            &document,
            "id\ttags\n",
            &mapping(&[ShiftCsvField::Tags]),
            &duplicate
        )
        .err()
        .ok_or("duplicate decision")?
        .code,
        CsvErrorCode::InvalidDecision
    );
    let too_many = vec![
        ShiftCsvRowDecision {
            record: 2,
            decision: ShiftCsvDecision::Skip {}
        };
        10_001
    ];
    assert_eq!(
        preview(
            &document,
            "id\ttags\n",
            &mapping(&[ShiftCsvField::Tags]),
            &too_many
        )
        .err()
        .ok_or("decision cap")?
        .code,
        CsvErrorCode::DecisionLimit
    );
    assert_eq!(
        preview(&document, "", &complete_mapping(), &[])
            .err()
            .ok_or("missing header")?
            .code,
        CsvErrorCode::HeaderMismatch
    );
    Ok(())
}

#[test]
fn cancellation_before_and_during_source_read_is_refused() -> TestResult {
    struct CancelRead<'a> {
        token: &'a CancellationToken,
    }
    impl Read for CancelRead<'_> {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            self.token.cancel();
            Ok(0)
        }
    }
    let document = fixture()?;
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    assert_eq!(
        preview_shift_csv(
            &document,
            Revision::INITIAL,
            &mut Cursor::new(""),
            &complete_mapping(),
            &[],
            &BTreeSet::new(),
            &cancellation
        )
        .err()
        .ok_or("cancelled")?
        .code,
        CsvErrorCode::Cancelled
    );
    let cancellation = CancellationToken::default();
    assert_eq!(
        preview_shift_csv(
            &document,
            Revision::INITIAL,
            &mut CancelRead {
                token: &cancellation
            },
            &complete_mapping(),
            &[],
            &BTreeSet::new(),
            &cancellation
        )
        .err()
        .ok_or("cancel during read")?
        .code,
        CsvErrorCode::Cancelled
    );
    Ok(())
}

#[test]
fn actual_mutation_count_is_one_bounded_atomic_batch_not_chunked() -> TestResult {
    let document = fixture()?;
    let rows = (1000..2001)
        .map(|index| complete_row(id(index)))
        .collect::<Vec<_>>();
    let decisions = (1000..2001)
        .zip(2..)
        .map(|(index, record)| decision(record, true, index))
        .collect::<TestResult<Vec<_>>>()?;
    assert_eq!(
        preview(&document, &source(&rows), &complete_mapping(), &decisions)
            .err()
            .ok_or("mutation cap")?
            .code,
        CsvErrorCode::MutationLimit
    );
    assert_eq!(document, fixture()?);
    Ok(())
}

#[test]
fn cumulative_raw_projection_admission_refuses_before_retaining_the_next_tree() -> TestResult {
    let mut document = fixture()?;
    let original = document.domain.entities[&id(8).parse()?].clone();
    let tags = (0..1000)
        .map(|index| format!("tag-{index:04}-{}", "x".repeat(185)))
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    for index in 1000..1030 {
        let mut raw = original.clone();
        raw["id"] = json!(id(index));
        raw["tags"] = json!(tags);
        document.domain.entities.insert(id(index).parse()?, raw);
        rows.push(vec![id(index), "endLocalDate".to_owned()]);
    }
    let result = preview(
        &document,
        &source(&rows),
        &mapping(&[ShiftCsvField::ReportingAttribution]),
        &[],
    );
    assert_eq!(
        result.err().ok_or("aggregate projections")?.code,
        CsvErrorCode::PreviewLimit
    );
    assert_eq!(document.domain.entities[&id(8).parse()?], original);
    Ok(())
}

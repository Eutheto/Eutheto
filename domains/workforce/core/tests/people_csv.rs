mod support;

use eutheto_domain_api::{
    ContractJsonLimits, DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainPackError,
};
use eutheto_types::{
    CancellationToken, DomainCommandEnvelope, MAX_SCENARIO_DOCUMENT_BYTES, Revision,
    ScenarioDocument, ValidationSeverity,
};
use eutheto_workforce::{
    commands,
    model::WorkforceEntity,
    people_csv::*,
    validation::{MAX_DISPLAY_BYTES, MAX_REFERENCE_ITEMS, validate_document},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt::Write as _,
    io::{self, Cursor, Read},
};
use support::{fixture, id};

type TestResult = Result<(), Box<dyn Error>>;

fn mapping(document: &ScenarioDocument) -> Result<PeopleCsvMapping, Box<dyn Error>> {
    let domain = validate_document(document)?;
    let WorkforceEntity::Person(person) = domain
        .entities
        .get(&id(1).parse()?)
        .ok_or("missing person")?
    else {
        return Err("wrong entity kind".into());
    };
    Ok(PeopleCsvMapping {
        dialect: CsvDialect::Comma,
        has_header: true,
        expected_columns: 2,
        columns: vec![
            ColumnMapping {
                index: 0,
                field: PersonField::ExternalId,
                blank: BlankPolicy::Preserve,
            },
            ColumnMapping {
                index: 1,
                field: PersonField::Name,
                blank: BlankPolicy::Preserve,
            },
        ],
        new_person_defaults: NewPersonDefaults {
            active_range: person.active_range,
            qualification_grants: person.qualification_grants.clone(),
            eligible_assignment_type_ids: person.eligible_assignment_type_ids.clone(),
            home_location_id: person.home_location_id,
            workload_weight: person.workload_weight,
            workload_target: person.workload_target,
            tags: person.tags.clone(),
            team_ids: person.team_ids.clone(),
            display: person.display.clone(),
        },
        reference_mappings: BTreeMap::new(),
    })
}

fn add(record: u32, person: u32) -> Result<RowDecision, Box<dyn Error>> {
    Ok(RowDecision {
        record,
        decision: IdentityDecision::Add {
            person_id: id(person).parse()?,
        },
    })
}

fn preview(
    document: &ScenarioDocument,
    input: &[u8],
    mapping: &PeopleCsvMapping,
    decisions: &[RowDecision],
) -> Result<PeopleImportPreview, CsvError> {
    preview_people_csv(
        document,
        Revision::INITIAL,
        &mut Cursor::new(input),
        mapping,
        decisions,
        &CancellationToken::default(),
    )
}

fn row_statuses(preview: &PeopleImportPreview) -> Vec<PeopleRowStatus> {
    preview.rows.iter().map(|row| row.status).collect()
}

#[test]
fn reviewed_mixed_import_rebuilds_exact_batch_and_ordinary_inverse() -> TestResult {
    let mut original = fixture()?;
    let person = original
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("missing person")?;
    person["id"] = json!(id(1).to_ascii_uppercase());
    person["eligibleAssignmentTypeIds"] = json!([id(4).to_ascii_uppercase()]);
    let mapping = mapping(&original)?;
    let source = b"external,name\nstaff-01,River revised\nstaff-02,New colleague\nbroken\nstaff-03,Skipped\n";
    let decisions = [
        add(3, 20)?,
        RowDecision {
            record: 5,
            decision: IdentityDecision::Skip,
        },
    ];
    let preview = preview(&original, source, &mapping, &decisions)?;
    assert_eq!(preview.disposition, PeopleImportDisposition::Reviewable);
    assert_eq!(
        row_statuses(&preview),
        [
            PeopleRowStatus::Updated,
            PeopleRowStatus::Added,
            PeopleRowStatus::Rejected,
            PeopleRowStatus::Skipped
        ]
    );
    assert_eq!(
        preview.rejected_rows,
        [RejectedRow {
            record: 4,
            code: RowRejectionCode::ColumnCount
        }]
    );
    assert_eq!(preview.validation_issues.len(), 1);
    let warning = &preview.validation_issues[0];
    assert_eq!(warning.severity, ValidationSeverity::Warning);
    assert_eq!(warning.code, "columnCount");
    assert_eq!(warning.field_path.as_deref(), Some("/peopleCsv/records/4"));
    assert!(warning.resource.is_none());
    let review = preview.review.as_ref().ok_or("missing review")?;
    assert_eq!(
        review.validation_blake3,
        blake3::hash(&serde_json::to_vec(&preview.validation_issues)?)
            .to_hex()
            .to_string()
    );
    let decoded = decode_people_csv_review(&serde_json::to_vec(review)?)?;
    assert_eq!(&decoded, review);
    let rebuilt = rebuild_review(
        &original,
        Revision::INITIAL,
        &mut Cursor::new(source),
        &decoded,
        preview
            .approval_digest
            .as_deref()
            .ok_or("missing approval")?,
        &CancellationToken::default(),
    )?;
    assert_eq!(rebuilt.batch, preview.batch);
    assert_eq!(rebuilt.validation_issues, preview.validation_issues);
    let batch = rebuilt.batch.as_ref().ok_or("missing batch")?;
    assert_eq!(batch.commands.len(), 2);
    let changed = commands::apply_batch(&original, batch)?;
    assert_eq!(
        changed
            .document
            .domain
            .entities
            .get(&id(1).parse()?)
            .ok_or("missing person")?["name"],
        "River revised"
    );
    assert_eq!(
        changed
            .document
            .domain
            .entities
            .get(&id(20).parse()?)
            .ok_or("missing new person")?["externalId"],
        "staff-02"
    );
    assert!(
        !changed
            .document
            .domain
            .entities
            .values()
            .any(|person| person["externalId"] == "staff-03")
    );
    assert_eq!(
        commands::apply_batch(&changed.document, &changed.inverse)?.document,
        original
    );
    Ok(())
}

#[test]
fn names_and_changed_or_absent_external_ids_never_establish_identity() -> TestResult {
    let original = fixture()?;
    let mapping = mapping(&original)?;
    for source in [
        b"external,name\nSTAFF-01,River\n".as_slice(),
        b"external,name\n,River\n",
    ] {
        let unresolved = preview(&original, source, &mapping, &[])?;
        assert_eq!(row_statuses(&unresolved), [PeopleRowStatus::Unresolved]);
        assert_eq!(unresolved.disposition, PeopleImportDisposition::Blocked);
        assert!(
            unresolved.batch.is_none()
                && unresolved.review.is_none()
                && unresolved.approval_digest.is_none()
        );
    }
    let added = preview(
        &original,
        b"external,name\n,River\n",
        &mapping,
        &[add(2, 20)?],
    )?;
    let changed = commands::apply_batch(&original, added.batch.as_ref().ok_or("missing add")?)?;
    assert!(
        changed
            .document
            .domain
            .entities
            .get(&id(20).parse()?)
            .ok_or("missing new person")?
            .get("externalId")
            .is_none()
    );
    let decision = RowDecision {
        record: 2,
        decision: IdentityDecision::Update {
            person_id: id(1).parse()?,
        },
    };
    let updated = preview(
        &original,
        b"external,name\nrenamed-id,River\n",
        &mapping,
        &[decision],
    )?;
    let changed =
        commands::apply_batch(&original, updated.batch.as_ref().ok_or("missing update")?)?;
    assert_eq!(
        changed
            .document
            .domain
            .entities
            .get(&id(1).parse()?)
            .ok_or("missing person")?["externalId"],
        "renamed-id"
    );
    Ok(())
}

#[test]
fn invalid_decisions_cannot_retarget_exact_matches_or_reuse_owned_ids() -> TestResult {
    let mut original = fixture()?;
    let mut second = original
        .domain
        .entities
        .get(&id(1).parse()?)
        .ok_or("missing person")?
        .clone();
    second["id"] = json!(id(20));
    second["externalId"] = json!("staff-02");
    original.domain.entities.insert(id(20).parse()?, second);
    let mapping = mapping(&original)?;
    let source = b"external,name\nstaff-01,River\n";
    let mismatch = RowDecision {
        record: 2,
        decision: IdentityDecision::Update {
            person_id: id(20).parse()?,
        },
    };
    assert_eq!(
        preview(&original, source, &mapping, &[mismatch])
            .err()
            .ok_or("expected CSV rejection")?
            .code,
        CsvErrorCode::InvalidDecision
    );
    for owned in [11, 10, 100] {
        assert_eq!(
            preview(
                &original,
                b"external,name\nnew,River\n",
                &mapping,
                &[add(2, owned)?]
            )
            .err()
            .ok_or("expected CSV rejection")?
            .code,
            CsvErrorCode::InvalidDecision
        );
    }
    for record in [0, 1, 3] {
        assert_eq!(
            preview(
                &original,
                source,
                &mapping,
                &[RowDecision {
                    record,
                    decision: IdentityDecision::Skip
                }]
            )
            .err()
            .ok_or("expected CSV rejection")?
            .code,
            CsvErrorCode::InvalidDecision
        );
    }
    Ok(())
}

#[test]
fn duplicate_rows_require_explicit_skip_without_an_order_winner() -> TestResult {
    let original = fixture()?;
    let mapping = mapping(&original)?;
    let duplicate_existing = b"external,name\nstaff-01,River\nstaff-01,River\n";
    let conflict = preview(&original, duplicate_existing, &mapping, &[])?;
    assert_eq!(
        row_statuses(&conflict),
        [PeopleRowStatus::Conflict, PeopleRowStatus::Conflict]
    );
    assert!(conflict.batch.is_none() && conflict.approval_digest.is_none());
    let skipped = [RowDecision {
        record: 3,
        decision: IdentityDecision::Skip,
    }];
    let unchanged = preview(&original, duplicate_existing, &mapping, &skipped)?;
    assert_eq!(unchanged.disposition, PeopleImportDisposition::NoChanges);
    assert!(unchanged.batch.is_none());
    let source = b"external,name\nnew,First\nnew,Second\n";
    let decisions = [add(2, 20)?, add(3, 21)?];
    let conflict = preview(&original, source, &mapping, &decisions)?;
    assert_eq!(
        row_statuses(&conflict),
        [PeopleRowStatus::Conflict, PeopleRowStatus::Conflict]
    );
    assert_eq!(conflict.disposition, PeopleImportDisposition::Blocked);
    let reviewed = preview(&original, source, &mapping, &[add(2, 20)?, skipped[0]])?;
    let mutation = commands::apply_batch(
        &original,
        reviewed.batch.as_ref().ok_or("missing reviewed add")?,
    )?;
    assert!(
        mutation
            .document
            .domain
            .entities
            .contains_key(&id(20).parse()?)
    );
    assert!(
        !mutation
            .document
            .domain
            .entities
            .contains_key(&id(21).parse()?)
    );
    Ok(())
}

#[test]
fn approval_is_independent_and_binds_source_revision_document_mapping_and_decisions() -> TestResult
{
    let original = fixture()?;
    let mapping = mapping(&original)?;
    let source = b"external,name\nnew,River\n";
    let initial = preview(&original, source, &mapping, &[add(2, 20)?])?;
    let review = initial.review.as_ref().ok_or("missing review")?;
    let digest = initial.approval_digest.as_deref().ok_or("missing digest")?;
    let token = CancellationToken::default();
    let alternative = preview(
        &original,
        source,
        &mapping,
        &[RowDecision {
            record: 2,
            decision: IdentityDecision::Update {
                person_id: id(1).parse()?,
            },
        }],
    )?;
    // The replacement contains a coherent, freshly recomputed set of internal hashes.
    assert_eq!(
        rebuild_review(
            &original,
            Revision::INITIAL,
            &mut Cursor::new(source),
            alternative.review.as_ref().ok_or("missing alternative")?,
            digest,
            &token
        )
        .err()
        .ok_or("expected CSV rejection")?
        .code,
        CsvErrorCode::StaleReview
    );
    let changed_bytes = [b"\xef\xbb\xbf".as_slice(), source].concat();
    assert_eq!(
        rebuild_review(
            &original,
            Revision::INITIAL,
            &mut Cursor::new(changed_bytes),
            review,
            digest,
            &token
        )
        .err()
        .ok_or("expected CSV rejection")?
        .code,
        CsvErrorCode::StaleReview
    );
    assert_eq!(
        rebuild_review(
            &original,
            Revision::new(1),
            &mut Cursor::new(source),
            review,
            digest,
            &token
        )
        .err()
        .ok_or("expected CSV rejection")?
        .code,
        CsvErrorCode::StaleReview
    );
    let mut changed_document = original.clone();
    changed_document.metadata.title = "Changed outside revision journal".to_owned();
    assert_eq!(
        rebuild_review(
            &changed_document,
            Revision::INITIAL,
            &mut Cursor::new(source),
            review,
            digest,
            &token
        )
        .err()
        .ok_or("expected CSV rejection")?
        .code,
        CsvErrorCode::StaleReview
    );
    let mut changed_mapping = review.clone();
    changed_mapping.mapping.columns.swap(0, 1);
    assert_eq!(
        rebuild_review(
            &original,
            Revision::INITIAL,
            &mut Cursor::new(source),
            &changed_mapping,
            digest,
            &token
        )
        .err()
        .ok_or("expected CSV rejection")?
        .code,
        CsvErrorCode::StaleReview
    );
    Ok(())
}

fn json_row(external: &str, value: &Value) -> Result<Vec<u8>, Box<dyn Error>> {
    let cell = serde_json::to_string(value)?.replace('"', "\"\"");
    Ok(format!("external,value\n{external},\"{cell}\"\n").into_bytes())
}

#[test]
fn raw_structured_cells_are_rejected_before_normalization_or_identity_approval() -> TestResult {
    let original = fixture()?;
    let mut mapping = mapping(&original)?;
    let malformed = [
        (PersonField::WorkloadWeight, json!([1, 1])),
        (
            PersonField::WorkloadWeight,
            json!({"numerator":1,"denominator":1,"extra":true}),
        ),
        (
            PersonField::QualificationGrants,
            json!([{"qualificationId":id(11),"effectiveFrom":null}]),
        ),
        (
            PersonField::WorkloadTarget,
            json!({"bucketId":id(3),"calendarId":id(2),"membership":{"reportingDate":{}},"target":120}),
        ),
        (PersonField::Tags, json!([{}])),
    ];
    for (field, value) in malformed {
        mapping.columns[1].field = field;
        let result = preview(&original, &json_row("unmatched", &value)?, &mapping, &[])?;
        assert_eq!(result.disposition, PeopleImportDisposition::NoChanges);
        assert_eq!(row_statuses(&result), [PeopleRowStatus::Rejected]);
        assert_eq!(result.rejected_rows[0].code, RowRejectionCode::InvalidCell);
        assert!(result.batch.is_none());
    }
    mapping.columns[1].field = PersonField::QualificationGrants;
    let wrong_kind = preview(
        &original,
        &json_row("unmatched", &json!([{"qualificationId":id(5)}]))?,
        &mapping,
        &[],
    )?;
    assert_eq!(
        wrong_kind.rejected_rows[0].code,
        RowRejectionCode::InvalidReference
    );
    mapping.columns[1].field = PersonField::WorkloadTarget;
    let unchanged = preview(
        &original,
        &json_row(
            "staff-01",
            &json!({"bucketId":id(3),"calendarId":id(2),"membership":"reportingDate","target":120}),
        )?,
        &mapping,
        &[],
    )?;
    assert_eq!(unchanged.disposition, PeopleImportDisposition::NoChanges);
    Ok(())
}

#[test]
fn decoded_reviews_reject_unknown_versions_fields_and_alternate_struct_forms() -> TestResult {
    let original = fixture()?;
    let mapping = mapping(&original)?;
    let result = preview(&original, b"external,name\nstaff-01,River\n", &mapping, &[])?;
    let review = result.review.ok_or("missing no-change review")?;
    let mut value = serde_json::to_value(&review)?;
    value["schemaVersion"] = json!(2);
    assert_eq!(
        decode_people_csv_review(&serde_json::to_vec(&value)?)
            .err()
            .ok_or("expected CSV rejection")?
            .code,
        CsvErrorCode::UnsupportedVersion
    );
    value["schemaVersion"] = json!(1);
    value["mapping"]["newPersonDefaults"]["workloadWeight"] = json!([1, 1]);
    assert_eq!(
        decode_people_csv_review(&serde_json::to_vec(&value)?)
            .err()
            .ok_or("expected CSV rejection")?
            .code,
        CsvErrorCode::InvalidReview
    );
    let mut value = serde_json::to_value(review)?;
    value["unapprovedAuthority"] = json!(true);
    assert_eq!(
        decode_people_csv_review(&serde_json::to_vec(&value)?)
            .err()
            .ok_or("expected CSV rejection")?
            .code,
        CsvErrorCode::InvalidReview
    );
    Ok(())
}

#[test]
fn review_decoder_enforces_duplicate_keys_and_inclusive_byte_cap() -> TestResult {
    let original = fixture()?;
    let result = preview(
        &original,
        b"external,name\nstaff-01,River\n",
        &mapping(&original)?,
        &[],
    )?;
    let review = result.review.ok_or("missing review")?;
    let encoded = serde_json::to_vec(&review)?;
    let duplicate = format!(
        "{{\"schemaVersion\":1,{}",
        std::str::from_utf8(&encoded[1..])?
    );
    assert_eq!(
        decode_people_csv_review(duplicate.as_bytes())
            .err()
            .ok_or("duplicate review member was accepted")?
            .code,
        CsvErrorCode::InvalidReview
    );
    let mut padded = encoded;
    padded.resize(MAX_CSV_REVIEW_BYTES, b' ');
    assert_eq!(decode_people_csv_review(&padded)?, review);
    padded.push(b' ');
    assert_eq!(
        decode_people_csv_review(&padded)
            .err()
            .ok_or("oversized review was accepted")?
            .code,
        CsvErrorCode::ReviewLimit
    );
    Ok(())
}

#[test]
fn data_and_rejected_row_caps_do_not_turn_into_partial_success() -> TestResult {
    let original = fixture()?;
    let mut mapping = mapping(&original)?;
    let source = format!("external,name\n{}", "x,River\n".repeat(10_000));
    let skipped: Vec<_> = (2..=10_001)
        .map(|record| RowDecision {
            record,
            decision: IdentityDecision::Skip,
        })
        .collect();
    let result = preview(&original, source.as_bytes(), &mapping, &skipped)?;
    assert_eq!(result.rows.len(), 10_000);
    assert_eq!(result.disposition, PeopleImportDisposition::NoChanges);
    mapping.has_header = false;
    let skipped: Vec<_> = (1..=10_000)
        .map(|record| RowDecision {
            record,
            decision: IdentityDecision::Skip,
        })
        .collect();
    assert_eq!(
        preview(
            &original,
            "x,River\n".repeat(10_001).as_bytes(),
            &mapping,
            &skipped
        )
        .err()
        .ok_or("expected CSV rejection")?
        .code,
        CsvErrorCode::DataRecordLimit
    );
    mapping.has_header = true;
    let rejected = format!("external,name\n{}", "broken\n".repeat(200));
    assert_eq!(
        preview(&original, rejected.as_bytes(), &mapping, &[])?
            .rejected_rows
            .len(),
        200
    );
    let overflow = format!("{rejected}broken\n");
    assert_eq!(
        preview(&original, overflow.as_bytes(), &mapping, &[])
            .err()
            .ok_or("expected CSV rejection")?
            .code,
        CsvErrorCode::RejectedReportLimit
    );
    Ok(())
}

struct CancelAtEof<'a> {
    input: Cursor<&'a [u8]>,
    token: CancellationToken,
}
impl Read for CancelAtEof<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let read = self.input.read(output)?;
        if read == 0 {
            self.token.cancel();
        }
        Ok(read)
    }
}

#[test]
fn cancellation_after_rows_were_processed_cannot_return_an_approved_batch() -> TestResult {
    let original = fixture()?;
    let mapping = mapping(&original)?;
    let token = CancellationToken::default();
    let mut input = CancelAtEof {
        input: Cursor::new(b"external,name\nnew,River\n"),
        token: token.clone(),
    };
    assert_eq!(
        preview_people_csv(
            &original,
            Revision::INITIAL,
            &mut input,
            &mapping,
            &[add(2, 20)?],
            &token
        )
        .err()
        .ok_or("expected CSV rejection")?
        .code,
        CsvErrorCode::Cancelled
    );
    Ok(())
}

const CLEAR_TAG_PEOPLE: u32 = 15;

fn near_limit_tagged_people()
-> Result<(ScenarioDocument, PeopleCsvMapping, Vec<u8>), Box<dyn Error>> {
    const TAGS_PER_PERSON: usize = 2_250;

    fn nodes(value: &Value) -> usize {
        1 + match value {
            Value::Array(values) => values.iter().map(nodes).sum(),
            Value::Object(values) => values.values().map(nodes).sum(),
            _ => 0,
        }
    }

    let mut original = fixture()?;
    "P".clone_into(&mut original.metadata.title);
    original.metadata.description.clear();
    original.domain.entities.clear();
    original.domain.locked_assignments.clear();
    original.domain.rules.clear();
    original.domain.preferences.clear();
    original.extensions.clear();
    for person in 1..=CLEAR_TAG_PEOPLE {
        original.domain.entities.insert(
            id(person).parse()?,
            json!({
                "kind": "person", "id": id(person), "name": format!("Person {person}"),
                "externalId": format!("staff-{person:02}"),
                "activeRange": {"kind": "always"}, "qualificationGrants": [],
                "eligibleAssignmentTypeIds": [],
                "workloadWeight": {"numerator": 1, "denominator": 1},
                "tags": [], "teamIds": []
            }),
        );
    }
    // Capture small defaults before enlarging the existing people. Tags are display
    // text (256 bytes), not semantic tokens (64 bytes), and are unique within each row.
    let mut mapping = mapping(&original)?;
    mapping.columns[1] = ColumnMapping {
        index: 1,
        field: PersonField::Tags,
        blank: BlankPolicy::Clear,
    };
    for (person, value) in original.domain.entities.values_mut().enumerate() {
        value["tags"] = json!(
            (0..TAGS_PER_PERSON)
                .map(|tag| {
                    let prefix = format!("{person:02}-{tag:04}");
                    let quotes = MAX_DISPLAY_BYTES - prefix.len() - 19;
                    format!("{prefix}{}{}", "\"".repeat(quotes), "x".repeat(19))
                })
                .collect::<Vec<_>>()
        );
    }
    let cap = usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES)?;
    let target_bytes = cap - 1;
    let base_bytes = serde_json::to_vec(&original)?.len();
    assert!(base_bytes < target_bytes);
    let mut padding = target_bytes - base_bytes;
    for value in original.domain.entities.values_mut() {
        for tag in value["tags"].as_array_mut().ok_or("missing tags")? {
            let Value::String(text) = tag else {
                return Err("tag is not display text".into());
            };
            // Replacing an ASCII suffix with quotes adds JSON bytes without more nodes.
            let extra = padding.min(19);
            text.truncate(text.len() - extra);
            text.push_str(&"\"".repeat(extra));
            padding -= extra;
        }
    }
    assert_eq!(
        padding, 0,
        "fixture must fit below the per-tag display limit"
    );
    let original_bytes = serde_json::to_vec(&original)?;
    assert_eq!(original_bytes.len(), target_bytes);
    let document_nodes = nodes(&serde_json::to_value(&original)?);
    assert!(document_nodes < ContractJsonLimits::DEFAULT.max_collection_items);
    validate_document(&original)?;
    Ok((original, mapping, original_bytes))
}

#[test]
fn clearing_large_tags_is_reviewable_despite_an_oversized_inverse() -> TestResult {
    let (original, mapping, original_bytes) = near_limit_tagged_people()?;
    let cap = usize::try_from(MAX_SCENARIO_DOCUMENT_BYTES)?;

    // Ordinary updates restore the exact original entity in each inverse envelope.
    // Even without the preview's label, those envelopes exceed the document cap;
    // this is inverse amplification, not excessive forward commands or CSV bytes.
    let expected_inverse = DomainBatchCommand {
        schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: original.domain_pack.id.clone(),
        scenario_schema_version: original.domain_pack.schema_version,
        label: None,
        commands: original
            .domain
            .entities
            .values()
            .rev()
            .map(|entity| DomainCommandEnvelope {
                command_type: commands::UPDATE_ENTITY.to_owned(),
                payload: json!({"entity": entity}),
            })
            .collect(),
    };
    let inverse_bytes = serde_json::to_vec(&expected_inverse)?.len();
    assert!(
        inverse_bytes > cap,
        "expected inverse has {inverse_bytes} bytes"
    );
    assert!(matches!(
        expected_inverse.validate_inverse_bounds(),
        Err(DomainPackError::BatchInverseTooLarge)
    ));
    let mut expected_forward = expected_inverse;
    expected_forward.commands.reverse();
    for command in &mut expected_forward.commands {
        command.payload["entity"]["tags"] = json!([]);
    }
    expected_forward.validate_bounds()?;
    assert!(serde_json::to_vec(&expected_forward)?.len() < 16 * 1024);
    let mut source = String::from("external,tags\n");
    for person in 1..=CLEAR_TAG_PEOPLE {
        writeln!(source, "staff-{person:02},")?;
    }
    assert!(source.len() < 1024);

    let result = preview(&original, source.as_bytes(), &mapping, &[])?;
    assert_eq!(result.disposition, PeopleImportDisposition::Reviewable);
    assert_eq!(
        row_statuses(&result),
        vec![PeopleRowStatus::Updated; usize::try_from(CLEAR_TAG_PEOPLE)?]
    );
    assert!(result.rejected_rows.is_empty());
    assert!(result.validation_issues.is_empty());
    let batch = result.batch.as_ref().ok_or("missing accepted batch")?;
    assert_eq!(batch.commands, expected_forward.commands);
    batch.validate_bounds()?;
    assert!(serde_json::to_vec(&result)?.len() <= MAX_CSV_PREVIEW_BYTES);

    assert!(result.review.is_some() && result.approval_digest.is_some());
    assert_eq!(serde_json::to_vec(&original)?, original_bytes);
    Ok(())
}

#[test]
fn aggregate_capacity_blocks_individually_valid_rows_without_approval_authority() -> TestResult {
    let mut original = fixture()?;
    let mut next = 1_000;
    while original.domain.entities.len() < MAX_REFERENCE_ITEMS - 1 {
        original.domain.entities.insert(
            id(next).parse()?,
            json!({"kind":"location", "id":id(next), "name":"Private location", "transitions":[]}),
        );
        next += 1;
    }
    let mapping = mapping(&original)?;
    for (external, person) in [("private-a", 20), ("private-b", 21)] {
        let source = format!("external,name\n{external},Private person\n");
        let single = preview(&original, source.as_bytes(), &mapping, &[add(2, person)?])?;
        assert_eq!(single.disposition, PeopleImportDisposition::Reviewable);
        let changed = commands::apply_batch(
            &original,
            single
                .batch
                .as_ref()
                .ok_or("missing individually valid batch")?,
        )?;
        assert_eq!(changed.document.domain.entities.len(), MAX_REFERENCE_ITEMS);
    }
    let source = format!(
        "external,name\nprivate-a,Private person\nprivate-b,Private person\n{}",
        "private-rejected-cell\n".repeat(MAX_CSV_REJECTED_ROWS)
    );
    let result = preview(
        &original,
        source.as_bytes(),
        &mapping,
        &[add(2, 20)?, add(3, 21)?],
    )?;
    assert_eq!(result.disposition, PeopleImportDisposition::Blocked);
    assert!(result.batch.is_none());
    assert!(result.review.is_none());
    assert!(result.approval_digest.is_none());
    assert_eq!(result.rows[0].status, PeopleRowStatus::Added);
    assert_eq!(result.rows[1].status, PeopleRowStatus::Added);
    assert_eq!(result.rejected_rows.len(), MAX_CSV_REJECTED_ROWS);
    assert_eq!(result.validation_issues.len(), MAX_CSV_VALIDATION_ISSUES);
    for (index, issue) in result.validation_issues[..MAX_CSV_REJECTED_ROWS]
        .iter()
        .enumerate()
    {
        assert_eq!(issue.severity, ValidationSeverity::Warning);
        assert_eq!(issue.code, "columnCount");
        assert_eq!(
            issue.field_path,
            Some(format!("/peopleCsv/records/{}", index + 4))
        );
    }
    let issue = result
        .validation_issues
        .last()
        .ok_or("missing aggregate error")?;
    assert_eq!(issue.severity, ValidationSeverity::Error);
    assert_eq!(issue.code, "invalidProposedState");
    assert_eq!(
        issue.field_path.as_deref(),
        Some("/peopleCsv/proposedState")
    );
    assert!(issue.resource.is_none());
    let report = serde_json::to_string(&result.validation_issues)?;
    assert!(report.len() <= MAX_CSV_VALIDATION_BYTES);
    for private in [
        "private-a",
        "private-b",
        "Private person",
        "private-rejected-cell",
        "Private location",
    ] {
        assert!(!report.contains(private));
    }
    // Report overflow remains an operation failure even when the proposed batch is invalid.
    let overflow = format!("{source}private-overflow\n");
    assert_eq!(
        preview(
            &original,
            overflow.as_bytes(),
            &mapping,
            &[add(2, 20)?, add(3, 21)?],
        )
        .err()
        .ok_or("expected report overflow")?
        .code,
        CsvErrorCode::RejectedReportLimit
    );
    Ok(())
}

#[test]
fn warning_binding_rejects_tampering_even_with_a_recomputed_approval_digest() -> TestResult {
    let original = fixture()?;
    let source = b"external,name\nstaff-01,Revised\nprivate-rejected-cell\n";
    let initial = preview(&original, source, &mapping(&original)?, &[])?;
    let mut review = initial.review.ok_or("missing partial-import review")?;
    let digest = initial.approval_digest.ok_or("missing approval")?;
    let approval_for = |review: &PeopleCsvReview| -> Result<String, Box<dyn Error>> {
        let canonical_review = serde_json::to_value(review)?;
        Ok(blake3::hash(
            format!(
                "{{\"domain\":\"eutheto/workforce-people-csv-approval\",\"version\":1,\"review\":{canonical_review}}}"
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string())
    };
    // Establish that the forged approval uses the real contract, so the second
    // rejection exercises fresh report equality rather than a malformed digest.
    assert_eq!(approval_for(&review)?, digest);
    // Replace the real warning report with a different, validly encoded report hash.
    let mut altered_issues = initial.validation_issues;
    altered_issues[0].severity = ValidationSeverity::Error;
    review.validation_blake3 = blake3::hash(&serde_json::to_vec(&altered_issues)?)
        .to_hex()
        .to_string();
    let encoded = serde_json::to_vec(&review)?;
    let review = decode_people_csv_review(&encoded)?;
    let forged_digest = approval_for(&review)?;
    for approval in [&digest, &forged_digest] {
        assert_eq!(
            rebuild_review(
                &original,
                Revision::INITIAL,
                &mut Cursor::new(source),
                &review,
                approval,
                &CancellationToken::default(),
            )
            .err()
            .ok_or("altered warning binding was accepted")?
            .code,
            CsvErrorCode::StaleReview
        );
    }
    Ok(())
}

#[test]
fn reviews_without_a_valid_validation_binding_require_fresh_review() -> TestResult {
    let original = fixture()?;
    let initial = preview(
        &original,
        b"external,name\nstaff-01,Revised\nprivate-rejected-cell\n",
        &mapping(&original)?,
        &[],
    )?;
    let review = initial.review.ok_or("missing review")?;
    let mut absent = serde_json::to_value(&review)?;
    absent
        .as_object_mut()
        .ok_or("review is not an object")?
        .remove("validationBlake3");
    assert_eq!(
        decode_people_csv_review(&serde_json::to_vec(&absent)?)
            .err()
            .ok_or("old unbound review was accepted")?
            .code,
        CsvErrorCode::InvalidReview
    );
    for invalid in [
        Value::Null,
        json!("private-invalid-digest"),
        json!("A".repeat(64)),
    ] {
        let mut malformed = serde_json::to_value(&review)?;
        malformed["validationBlake3"] = invalid;
        assert_eq!(
            decode_people_csv_review(&serde_json::to_vec(&malformed)?)
                .err()
                .ok_or("invalid validation binding was accepted")?
                .code,
            CsvErrorCode::InvalidReview
        );
    }
    Ok(())
}

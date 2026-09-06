use super::*;
use crate::model::{ActiveRange, PersonDisplay, QualificationGrant, WorkloadWeight};
use crate::people_csv::types::{ColumnMapping, CsvDialect, NewPersonDefaults};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const ID: &str = "018F7B40-A000-7000-8000-00000000000B";
const CANONICAL_ID: &str = "018f7b40-a000-7000-8000-00000000000b";
const FIELDS: [PersonField; 10] = [
    PersonField::Name,
    PersonField::ExternalId,
    PersonField::ActiveRange,
    PersonField::QualificationGrants,
    PersonField::EligibleAssignmentTypeIds,
    PersonField::HomeLocationId,
    PersonField::WorkloadWeight,
    PersonField::WorkloadTarget,
    PersonField::Tags,
    PersonField::TeamIds,
];

fn mapping(fields: &[PersonField]) -> PeopleCsvMapping {
    PeopleCsvMapping {
        dialect: CsvDialect::Comma,
        has_header: false,
        expected_columns: u8::try_from(fields.len().max(1)).unwrap_or(64),
        columns: fields
            .iter()
            .zip(0_u8..)
            .map(|(&field, index)| ColumnMapping {
                index,
                field,
                blank: BlankPolicy::Preserve,
            })
            .collect(),
        new_person_defaults: NewPersonDefaults {
            active_range: ActiveRange::Always {},
            qualification_grants: Vec::new(),
            eligible_assignment_type_ids: Vec::new(),
            home_location_id: None,
            workload_weight: WorkloadWeight {
                numerator: 1,
                denominator: 1,
            },
            workload_target: None,
            tags: Vec::new(),
            team_ids: Vec::new(),
            display: None,
        },
        reference_mappings: BTreeMap::new(),
    }
}

fn row(cells: &[&str], mapping: &PeopleCsvMapping) -> Result<PersonPatch, RowRejectionCode> {
    map_row(CsvRecord { number: 2, cells }, mapping)
}

fn set(patch: &PersonPatch, field: PersonField) -> Result<&Value, Box<dyn std::error::Error>> {
    match patch.fields.get(&field) {
        Some(FieldEdit::Set(value)) => Ok(value),
        _ => Err("expected an explicit set edit".into()),
    }
}

#[test]
fn all_ten_wire_forms_preserve_text_and_resolve_only_references() -> TestResult {
    let mut policy = mapping(&FIELDS);
    policy
        .reference_mappings
        .insert("ref".to_owned(), ID.parse()?);
    validate_mapping(&policy)?;
    let grants = format!(
        r#"[{{"qualificationId":"ref","effectiveFrom":"2026-10-01T00:00:00+00:00"}},{{"qualificationId":"{ID}"}}]"#
    );
    let eligibility = format!(r#"["ref","{ID}"]"#);
    let cells = [
        "  ref  ",
        "ref",
        r#"{"kind":"dateRange","startDate":"2026-10-01","endDateExclusive":"2026-10-31"}"#,
        &grants,
        &eligibility,
        "ref",
        r#"{"numerator":2,"denominator":4}"#,
        r#"{"bucketId":"ref","calendarId":"ref","membership":"reportingDate","target":120}"#,
        r#"["ref","  ref  "]"#,
        &eligibility,
    ];
    let original = cells;
    let patch = row(&cells, &policy).map_err(|_| "valid wire forms rejected")?;
    assert_eq!(patch.fields.len(), 10);
    assert_eq!(set(&patch, PersonField::Name)?, &json!("  ref  "));
    assert_eq!(set(&patch, PersonField::ExternalId)?, &json!("ref"));
    assert_eq!(
        set(&patch, PersonField::ActiveRange)?,
        &serde_json::from_str::<Value>(cells[2])?
    );
    assert_eq!(
        set(&patch, PersonField::QualificationGrants)?,
        &json!([
            {"qualificationId":CANONICAL_ID,"effectiveFrom":"2026-10-01T00:00:00+00:00"},
            {"qualificationId":ID}
        ])
    );
    assert_eq!(
        set(&patch, PersonField::EligibleAssignmentTypeIds)?,
        &json!([CANONICAL_ID, ID])
    );
    assert_eq!(
        set(&patch, PersonField::HomeLocationId)?,
        &json!(CANONICAL_ID)
    );
    assert_eq!(
        set(&patch, PersonField::WorkloadWeight)?,
        &json!({"numerator":2,"denominator":4})
    );
    assert_eq!(
        set(&patch, PersonField::WorkloadTarget)?,
        &json!({
            "bucketId":CANONICAL_ID,"calendarId":CANONICAL_ID,"membership":"reportingDate","target":120
        })
    );
    assert_eq!(set(&patch, PersonField::Tags)?, &json!(["ref", "  ref  "]));
    assert_eq!(
        set(&patch, PersonField::TeamIds)?,
        &json!([CANONICAL_ID, ID])
    );
    assert_eq!(cells, original);
    Ok(())
}

#[test]
fn blank_preserve_and_clear_are_distinct_and_whitespace_is_not_blank() -> TestResult {
    let policy = mapping(&FIELDS);
    validate_mapping(&policy)?;
    let patch = row(&[""; 10], &policy).map_err(|_| "preserve rejected")?;
    assert!(patch.fields.is_empty());
    let clearable = [
        PersonField::ExternalId,
        PersonField::HomeLocationId,
        PersonField::WorkloadTarget,
        PersonField::QualificationGrants,
        PersonField::EligibleAssignmentTypeIds,
        PersonField::Tags,
        PersonField::TeamIds,
    ];
    let mut policy = mapping(&clearable);
    for column in &mut policy.columns {
        column.blank = BlankPolicy::Clear;
    }
    validate_mapping(&policy)?;
    let patch = row(&[""; 7], &policy).map_err(|_| "clear rejected")?;
    for field in &clearable[..3] {
        assert!(matches!(patch.fields.get(field), Some(FieldEdit::Remove)));
    }
    for &field in &clearable[3..] {
        assert_eq!(set(&patch, field)?, &json!([]));
    }
    for field in [PersonField::Name, PersonField::ExternalId] {
        let policy = mapping(&[field]);
        let patch = row(&[" \t "], &policy).map_err(|_| "exact scalar rejected")?;
        assert_eq!(set(&patch, field)?, &json!(" \t "));
    }
    for field in &FIELDS[2..] {
        assert!(row(&[" "], &mapping(&[*field])).is_err());
    }
    Ok(())
}

#[test]
fn mapping_rejects_ambiguous_columns_fields_and_required_clear() {
    for width in [0, 65, u8::MAX] {
        let mut policy = mapping(&[]);
        policy.expected_columns = width;
        assert_eq!(
            validate_mapping(&policy),
            Err(CsvError::source(CsvErrorCode::InvalidMapping))
        );
    }
    let mut policy = mapping(&[PersonField::Name, PersonField::Tags]);
    policy.columns[1].index = 0;
    assert!(validate_mapping(&policy).is_err());
    policy.columns[1].index = 2;
    assert!(validate_mapping(&policy).is_err());
    policy.columns[1].index = u8::MAX;
    assert!(validate_mapping(&policy).is_err());
    policy.columns[1].index = 1;
    policy.columns[1].field = PersonField::Name;
    assert!(validate_mapping(&policy).is_err());
    for field in [
        PersonField::Name,
        PersonField::ActiveRange,
        PersonField::WorkloadWeight,
    ] {
        let mut policy = mapping(&[field]);
        policy.columns[0].blank = BlankPolicy::Clear;
        assert_eq!(
            validate_mapping(&policy),
            Err(CsvError::source(CsvErrorCode::InvalidMapping))
        );
    }
    let mut policy = mapping(&FIELDS);
    policy.columns.push(policy.columns[0].clone());
    assert_eq!(
        validate_mapping(&policy),
        Err(CsvError::source(CsvErrorCode::MappingLimit))
    );
}

#[test]
fn explicit_width_accepts_last_column_and_rejects_short_or_long_rows() -> TestResult {
    let mut policy = mapping(&[PersonField::Name]);
    policy.expected_columns = 64;
    policy.columns[0].index = 63;
    validate_mapping(&policy)?;
    let mut cells = ["ignored"; 64];
    cells[63] = "Last person";
    let patch = row(&cells, &policy).map_err(|_| "last column rejected")?;
    assert_eq!(set(&patch, PersonField::Name)?, &json!("Last person"));
    assert!(matches!(
        row(&cells[..63], &policy),
        Err(RowRejectionCode::ColumnCount)
    ));
    assert!(matches!(
        row(&[""; 65], &policy),
        Err(RowRejectionCode::ColumnCount)
    ));
    Ok(())
}

#[test]
fn token_table_is_exact_bounded_and_cannot_shadow_any_uuid() -> TestResult {
    let mut policy = mapping(&[PersonField::HomeLocationId]);
    let id: EntityId = ID.parse()?;
    for key in [
        "",
        "ref\n",
        ID,
        "00000000-0000-4000-8000-000000000001",
        "018f7b40a0007000800000000000000b",
        "{018f7b40-a000-7000-8000-00000000000b}",
        "urn:uuid:018f7b40-a000-7000-8000-00000000000b",
    ] {
        policy.reference_mappings.clear();
        policy.reference_mappings.insert(key.to_owned(), id);
        assert_eq!(
            validate_mapping(&policy),
            Err(CsvError::source(CsvErrorCode::InvalidMapping))
        );
    }
    policy.reference_mappings.clear();
    policy
        .reference_mappings
        .insert("r".repeat(MAX_TOKEN_BYTES), id);
    validate_mapping(&policy)?;
    policy
        .reference_mappings
        .insert("r".repeat(MAX_TOKEN_BYTES + 1), id);
    assert_eq!(
        validate_mapping(&policy),
        Err(CsvError::source(CsvErrorCode::MappingLimit))
    );
    policy.reference_mappings.clear();
    policy.reference_mappings.insert(" Ref ".to_owned(), id);
    validate_mapping(&policy)?;
    let patch = row(&[" Ref "], &policy).map_err(|_| "exact token rejected")?;
    assert_eq!(
        set(&patch, PersonField::HomeLocationId)?,
        &json!(CANONICAL_ID)
    );
    let patch = row(&[ID], &policy).map_err(|_| "direct UUID rejected")?;
    assert_eq!(set(&patch, PersonField::HomeLocationId)?, &json!(ID));
    for cell in [
        "Ref",
        " ref ",
        "missing",
        "\" Ref \"",
        "\"018f7b40-a000-7000-8000-00000000000b\"",
        "00000000-0000-4000-8000-000000000001",
    ] {
        assert!(matches!(
            row(&[cell], &policy),
            Err(RowRejectionCode::InvalidReference)
        ));
    }
    policy.reference_mappings.insert(
        "invalid-id".to_owned(),
        EntityId::from_uuid("00000000-0000-4000-8000-000000000001".parse()?),
    );
    assert!(validate_mapping(&policy).is_err());
    Ok(())
}

#[test]
fn malformed_json_and_unsafe_or_overdeep_values_reject_before_materialization() {
    for field in [
        PersonField::ActiveRange,
        PersonField::QualificationGrants,
        PersonField::EligibleAssignmentTypeIds,
        PersonField::WorkloadWeight,
        PersonField::WorkloadTarget,
        PersonField::Tags,
        PersonField::TeamIds,
    ] {
        for cell in ["null", "{", "[] trailing", "\"text\"", "false"] {
            assert!(matches!(
                row(&[cell], &mapping(&[field])),
                Err(RowRejectionCode::InvalidCell)
            ));
        }
    }
    for (field, cell) in [
        (PersonField::Tags, r#"["/etc/passwd"]"#),
        (PersonField::Tags, r#"["<script>bad</script>"]"#),
        (
            PersonField::ActiveRange,
            r#"{"kind":"always","kind":"dateRange"}"#,
        ),
        (PersonField::ActiveRange, r#"{"credential":"hidden"}"#),
    ] {
        assert!(matches!(
            row(&[cell], &mapping(&[field])),
            Err(RowRejectionCode::InvalidCell)
        ));
    }
    let deep = format!("{}0{}", "[".repeat(40), "]".repeat(40));
    assert!(matches!(
        row(&[&deep], &mapping(&[PersonField::Tags])),
        Err(RowRejectionCode::InvalidCell)
    ));
    let large = "x".repeat(MAX_CSV_CELL_BYTES + 1);
    assert!(matches!(
        row(&[&large], &mapping(&[PersonField::Name])),
        Err(RowRejectionCode::InvalidCell)
    ));
}

#[test]
fn alternate_struct_arrays_reject_and_nested_enum_objects_are_not_laundered() -> TestResult {
    for (field, cell) in [
        (PersonField::WorkloadWeight, "[1,1]"),
        (PersonField::ActiveRange, r#"["always"]"#),
        (PersonField::QualificationGrants, r#"[["ref",null,null]]"#),
        (
            PersonField::WorkloadTarget,
            r#"["ref","ref","reportingDate",10]"#,
        ),
        (PersonField::TeamIds, r#"[{"id":"ref"}]"#),
    ] {
        assert!(row(&[cell], &mapping(&[field])).is_err());
    }
    let mut policy = mapping(&[PersonField::WorkloadTarget]);
    policy
        .reference_mappings
        .insert("ref".to_owned(), ID.parse()?);
    let cell = r#"{"bucketId":"ref","calendarId":"ref","membership":{"reportingDate":null},"target":10,"unknown":"ref"}"#;
    let patch = row(&[cell], &policy).map_err(|_| "raw nested shape rejected")?;
    assert_eq!(
        set(&patch, PersonField::WorkloadTarget)?,
        &json!({
            "bucketId":CANONICAL_ID,"calendarId":CANONICAL_ID,
            "membership":{"reportingDate":null},"target":10,"unknown":"ref"
        })
    );
    Ok(())
}

#[test]
fn unknown_keys_optional_nulls_and_nonreference_positions_survive_unchanged() -> TestResult {
    let mut policy = mapping(&[
        PersonField::QualificationGrants,
        PersonField::ActiveRange,
        PersonField::Tags,
    ]);
    policy
        .reference_mappings
        .insert("ref".to_owned(), ID.parse()?);
    let cells = [
        r#"[{"qualificationId":"ref","effectiveFrom":"ref","expiresAt":null,"unknown":{"qualificationId":"ref"}}]"#,
        r#"{"kind":"always","unknown":["ref"]}"#,
        r#"[null,{"qualificationId":"ref"}]"#,
    ];
    let patch = row(&cells, &policy).map_err(|_| "bounded raw shape rejected")?;
    assert_eq!(
        set(&patch, PersonField::QualificationGrants)?,
        &json!([{
            "qualificationId":CANONICAL_ID,"effectiveFrom":"ref","expiresAt":null,
            "unknown":{"qualificationId":"ref"}
        }])
    );
    assert_eq!(
        set(&patch, PersonField::ActiveRange)?,
        &serde_json::from_str::<Value>(cells[1])?
    );
    assert_eq!(
        set(&patch, PersonField::Tags)?,
        &serde_json::from_str::<Value>(cells[2])?
    );
    Ok(())
}

#[test]
fn default_collections_and_strings_are_bounded_before_serialization() -> TestResult {
    let baseline = mapping(&[]);
    let mut policy = baseline.clone();
    policy.new_person_defaults.qualification_grants = vec![
        QualificationGrant {
            qualification_id: ID.parse()?,
            effective_from: None,
            expires_at: None,
        };
        MAX_REFERENCE_ITEMS
    ];
    validate_mapping(&policy)?;
    policy
        .new_person_defaults
        .qualification_grants
        .push(policy.new_person_defaults.qualification_grants[0]);
    assert_eq!(
        validate_mapping(&policy),
        Err(CsvError::source(CsvErrorCode::MappingLimit))
    );
    policy = baseline.clone();
    policy.new_person_defaults.eligible_assignment_type_ids =
        vec![ID.parse()?; MAX_REFERENCE_ITEMS];
    validate_mapping(&policy)?;
    policy
        .new_person_defaults
        .eligible_assignment_type_ids
        .push(ID.parse()?);
    assert!(validate_mapping(&policy).is_err());
    policy = baseline.clone();
    policy.new_person_defaults.team_ids = vec![ID.parse()?; MAX_REFERENCE_ITEMS];
    validate_mapping(&policy)?;
    policy.new_person_defaults.team_ids.push(ID.parse()?);
    assert!(validate_mapping(&policy).is_err());
    policy = baseline.clone();
    policy.new_person_defaults.tags = vec!["tag".to_owned(); MAX_REFERENCE_ITEMS];
    validate_mapping(&policy)?;
    policy.new_person_defaults.tags.push("tag".to_owned());
    assert!(validate_mapping(&policy).is_err());
    policy = baseline.clone();
    policy.new_person_defaults.tags = vec!["x".repeat(MAX_DISPLAY_BYTES)];
    validate_mapping(&policy)?;
    policy.new_person_defaults.tags[0].push('x');
    assert!(validate_mapping(&policy).is_err());
    policy = baseline;
    policy.new_person_defaults.display = Some(PersonDisplay {
        color: Some("#123abc".to_owned()),
        avatar_initials: Some("x".repeat(16)),
    });
    validate_mapping(&policy)?;
    policy
        .new_person_defaults
        .display
        .as_mut()
        .ok_or("missing display")?
        .avatar_initials = Some("x".repeat(17));
    assert!(validate_mapping(&policy).is_err());
    policy
        .new_person_defaults
        .display
        .as_mut()
        .ok_or("missing display")?
        .avatar_initials = None;
    policy
        .new_person_defaults
        .display
        .as_mut()
        .ok_or("missing display")?
        .color = Some("x".repeat(8));
    assert!(validate_mapping(&policy).is_err());
    Ok(())
}

#[test]
fn mapping_budget_is_independent_of_each_typed_limit() -> TestResult {
    let mut policy = mapping(&[]);
    let id: EntityId = ID.parse()?;
    policy.reference_mappings = (0..MAX_REFERENCE_ITEMS)
        .map(|index| (format!("r{index:05}"), id))
        .collect();
    validate_mapping(&policy)?;
    policy.reference_mappings.insert("extra".to_owned(), id);
    assert_eq!(
        validate_mapping(&policy),
        Err(CsvError::source(CsvErrorCode::MappingLimit))
    );
    policy.reference_mappings.clear();
    policy.new_person_defaults.tags = vec!["x".repeat(MAX_DISPLAY_BYTES); MAX_REFERENCE_ITEMS];
    assert!(serde_json::to_vec(&policy)?.len() > MAX_CSV_MAPPING_BYTES);
    assert_eq!(
        validate_mapping(&policy),
        Err(CsvError::source(CsvErrorCode::MappingLimit))
    );
    // Oversized caller-owned strings must be rejected by length, before cloning
    // into defaults or asking any serializer/safety walker to traverse the text.
    policy.new_person_defaults.tags = vec!["x".repeat(MAX_CSV_MAPPING_BYTES + 1)];
    assert_eq!(
        validate_mapping(&policy),
        Err(CsvError::source(CsvErrorCode::MappingLimit))
    );
    Ok(())
}

#[test]
fn serialized_mapping_byte_ceiling_is_inclusive() -> TestResult {
    let mut policy = mapping(&[]);
    let baseline_bytes = serde_json::to_vec(&policy)?.len();
    let full_tags = (MAX_CSV_MAPPING_BYTES - baseline_bytes) / (MAX_DISPLAY_BYTES + 3);
    policy.new_person_defaults.tags = vec!["x".repeat(MAX_DISPLAY_BYTES); full_tags];
    let mut remaining = MAX_CSV_MAPPING_BYTES - serde_json::to_vec(&policy)?.len();
    if remaining < 3 {
        policy.new_person_defaults.tags[0].truncate(MAX_DISPLAY_BYTES - 3);
        remaining += 3;
    }
    policy
        .new_person_defaults
        .tags
        .push("x".repeat(remaining - 3));
    assert_eq!(serde_json::to_vec(&policy)?.len(), MAX_CSV_MAPPING_BYTES);
    validate_mapping(&policy)?;
    // Escaping adds one serialized byte without violating the typed string bound.
    policy.new_person_defaults.tags[0].replace_range(..1, "\"");
    assert_eq!(
        serde_json::to_vec(&policy)?.len(),
        MAX_CSV_MAPPING_BYTES + 1
    );
    assert_eq!(
        validate_mapping(&policy),
        Err(CsvError::source(CsvErrorCode::MappingLimit))
    );
    Ok(())
}

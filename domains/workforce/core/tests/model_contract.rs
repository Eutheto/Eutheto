use eutheto_types::{GapPolicy, Horizon, OverlapPolicy, ScenarioSettings, UnitSystem};
use eutheto_workforce::model::{
    ActiveRange, LockState, ShiftTemplate, WorkforceDomainV1, WorkforceEntity, planning_dates,
};
use serde_json::{Value, json};
use std::error::Error;

fn person() -> Value {
    json!({
        "kind": "person",
        "id": "018f7b40-a000-7000-8000-000000000001",
        "name": "River",
        "activeRange": {"kind": "always"},
        "qualificationGrants": [],
        "eligibleAssignmentTypeIds": [],
        "workloadWeight": {"numerator": 1, "denominator": 1},
        "tags": [],
        "teamIds": []
    })
}

#[test]
fn typed_records_preserve_omission_and_reject_unknown_or_null_fields() -> Result<(), Box<dyn Error>>
{
    let original = person();
    let entity: WorkforceEntity = serde_json::from_value(original.clone())?;
    assert_eq!(serde_json::to_value(entity)?, original);

    let mut null_reference = original.clone();
    null_reference["homeLocationId"] = Value::Null;
    assert!(serde_json::from_value::<WorkforceEntity>(null_reference).is_err());
    let mut unknown_field = original;
    unknown_field["unrecognizedPolicy"] = json!(true);
    assert!(serde_json::from_value::<WorkforceEntity>(unknown_field).is_err());
    Ok(())
}

#[test]
fn fieldless_variants_reject_hidden_parameters() {
    assert!(
        serde_json::from_value::<ActiveRange>(json!({
            "kind": "always", "startDate": "2026-09-01"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<LockState>(json!({
            "kind": "hard", "stabilityWeight": 10
        }))
        .is_err()
    );
}

#[test]
fn typed_occurrence_ledger_rejects_normalized_duplicate_keys() -> Result<(), Box<dyn Error>> {
    let lower = "018f7b40-abcd-7000-8000-000000000002";
    let upper = lower.to_ascii_uppercase();
    let mut template = json!({
        "id": "018f7b40-a000-7000-8000-000000000001",
        "name": "Clinic",
        "assignmentTypeId": "018f7b40-a000-7000-8000-000000000003",
        "recurrence": {
            "weekdays": ["monday"],
            "effectiveRange": {"startDate": "2026-09-01", "endDateExclusive": "2026-10-01"},
            "excludedDates": []
        },
        "timing": {"kind": "elapsedDuration", "startTime": "09:00:00", "durationMinutes": 60},
        "coverage": {"kind": "exact", "count": 1, "qualificationMinimums": []},
        "tags": [],
        "reportingAttribution": "startLocalDate",
        "occurrenceIdentities": {
            (lower): {"id": lower, "localStartDate": "2026-09-07"}
        }
    });
    let decoded: ShiftTemplate = serde_json::from_value(template.clone())?;
    assert_eq!(
        decoded.occurrence_identities[&lower.parse()?]
            .local_start_date
            .to_string(),
        "2026-09-07"
    );
    template["occurrenceIdentities"][&upper] = json!({"id": lower, "localStartDate": "2026-09-14"});
    assert!(serde_json::from_value::<ShiftTemplate>(template).is_err());
    Ok(())
}

#[test]
fn four_map_body_never_defaults_missing_storage_sections() -> Result<(), Box<dyn Error>> {
    let body = serde_json::to_value(WorkforceDomainV1::default())?;
    let mut missing = body.clone();
    missing
        .as_object_mut()
        .ok_or("expected domain object")?
        .remove("lockedAssignments");
    assert!(serde_json::from_value::<WorkforceDomainV1>(missing).is_err());
    let decoded: WorkforceDomainV1 = serde_json::from_value(body.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, body);
    Ok(())
}

#[test]
fn planning_dates_use_local_midnights_not_twenty_four_hour_steps() -> Result<(), Box<dyn Error>> {
    let mut settings = ScenarioSettings {
        time_zone: "America/New_York".parse()?,
        locale: "en-US".parse()?,
        units: UnitSystem::Metric,
        horizon: Horizon::new(
            "2026-03-08T05:00:00Z".parse()?,
            "2026-03-09T04:00:00Z".parse()?,
        )?,
        gap_policy: GapPolicy::Reject,
        overlap_policy: OverlapPolicy::Earlier,
    };
    let dates = planning_dates(&settings)?;
    assert_eq!(dates.first_date.to_string(), "2026-03-08");
    assert_eq!(dates.last_date, dates.first_date);
    settings.horizon.end = "2026-03-09T05:00:00Z".parse()?;
    assert!(planning_dates(&settings).is_err());
    Ok(())
}

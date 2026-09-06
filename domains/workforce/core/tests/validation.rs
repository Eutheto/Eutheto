mod support;

use eutheto_domain_api::{ContractJsonLimits, DomainPackError};
use eutheto_types::{GapPolicy, Horizon, ScenarioDocument};
use eutheto_workforce::validation::{decode_document, validate_document};
use serde_json::{Value, json};
use std::error::Error;
use support::{fixture, id};

fn entity(document: &mut ScenarioDocument, index: u32) -> Result<&mut Value, Box<dyn Error>> {
    document
        .domain
        .entities
        .get_mut(&id(index).parse()?)
        .ok_or_else(|| "fixture record is missing".into())
}

#[test]
fn complete_document_and_nonsemantic_data_survive_bounded_decoding() -> Result<(), Box<dyn Error>> {
    let document = fixture()?;
    assert_eq!(decode_document(&serde_json::to_vec(&document)?)?, document);
    let typed = validate_document(&document)?;
    assert_eq!(
        serde_json::to_value(typed)?,
        serde_json::to_value(&document.domain)?
    );
    let mut draft = document;
    draft.domain.entities.remove(&id(9).parse()?);
    validate_document(&draft)?;
    draft.extensions.insert(
        "semantic.example.override".to_owned(),
        json!({"coverage":false}),
    );
    assert!(validate_document(&draft).is_err());
    Ok(())
}

#[test]
fn references_cannot_cross_kinds_or_reuse_nested_owned_identity() -> Result<(), Box<dyn Error>> {
    let original = fixture()?;
    validate_document(&original)?;
    let mut wrong_kind = original.clone();
    entity(&mut wrong_kind, 1)?["eligibleAssignmentTypeIds"] = json!([id(11)]);
    assert!(validate_document(&wrong_kind).is_err());
    let mut collision = original;
    let policy = entity(&mut collision, 9)?["workloadPolicies"][id(10)].clone();
    let mut moved = policy;
    moved["id"] = json!(id(7));
    entity(&mut collision, 9)?["workloadPolicies"] = json!({(id(7).to_ascii_uppercase()):moved});
    assert!(validate_document(&collision).is_err());
    assert!(decode_document(&serde_json::to_vec(&collision)?).is_err());
    Ok(())
}

#[test]
fn person_shape_errors_precede_dependent_population_checks() -> Result<(), Box<dyn Error>> {
    let mut document = fixture()?;
    entity(&mut document, 1)?["tags"] = json!(["night", "night"]);
    entity(&mut document, 9)?["workloadPolicies"][id(10)]["targetMode"] =
        json!({"kind":"explicit","targets":[{"personId":id(1),"target":120}]});
    let original_error = validate_document(&document)
        .err()
        .ok_or("duplicate tags accepted")?;
    let mut policy = document
        .domain
        .entities
        .remove(&id(9).parse()?)
        .ok_or("missing policy")?;
    policy["id"] = json!(id(0));
    document.domain.entities.insert(id(0).parse()?, policy);
    let reordered_error = validate_document(&document)
        .err()
        .ok_or("duplicate tags accepted")?;
    assert_eq!(original_error, reordered_error);
    Ok(())
}

#[test]
fn reference_record_sets_preserve_order_without_requiring_uuid_sorting()
-> Result<(), Box<dyn Error>> {
    let mut document = fixture()?;
    let mut person = entity(&mut document, 1)?.clone();
    person["id"] = json!(id(12));
    person["externalId"] = json!("staff-02");
    document.domain.entities.insert(id(12).parse()?, person);
    for index in [13, 15] {
        document.domain.entities.insert(
            id(index).parse()?,
            json!({"kind":"location","id":id(index),"name":"Satellite","transitions":[]}),
        );
    }
    entity(&mut document, 5)?["transitions"] =
        json!([{"locationId":id(13),"minutes":10},{"locationId":id(15),"minutes":20}]);
    entity(&mut document, 9)?["workloadPolicies"][id(10)]["targetMode"] = json!({"kind":"explicit","targets":[{"personId":id(1),"target":120},{"personId":id(12),"target":60}]});
    validate_document(&document)?;

    // CreateCopy rewrites reference UUIDs without reordering these value records.
    entity(&mut document, 5)?["transitions"]
        .as_array_mut()
        .ok_or("missing transition fixture")?
        .reverse();
    entity(&mut document, 9)?["workloadPolicies"][id(10)]["targetMode"]["targets"]
        .as_array_mut()
        .ok_or("missing target fixture")?
        .reverse();
    assert_eq!(decode_document(&serde_json::to_vec(&document)?)?, document);

    let mut duplicate = document.clone();
    entity(&mut duplicate, 5)?["transitions"][1]["locationId"] = json!(id(15));
    assert!(validate_document(&duplicate).is_err());
    entity(&mut document, 9)?["workloadPolicies"][id(10)]["targetMode"]["targets"][1]["personId"] =
        json!(id(12));
    assert!(validate_document(&document).is_err());
    Ok(())
}

#[test]
fn detached_occurrences_cannot_compete_with_ledger_dates() -> Result<(), Box<dyn Error>> {
    let mut document = fixture()?;
    entity(&mut document, 8)?["origin"] =
        json!({"kind":"detached","templateId":id(6),"occurrenceDate":"2026-11-01"});
    assert!(validate_document(&document).is_err());
    entity(&mut document, 8)?["origin"]["occurrenceDate"] = json!("2026-11-08");
    validate_document(&document)?;
    entity(&mut document, 6)?["occurrenceIdentities"] = json!({});
    assert!(
        validate_document(&document).is_err(),
        "lock must not outlive its occurrence definition"
    );
    Ok(())
}

#[test]
fn contradictory_coverage_is_not_malformed_but_reversed_counts_are() -> Result<(), Box<dyn Error>> {
    let mut document = fixture()?;
    entity(&mut document, 6)?["coverage"] = json!({"kind":"exact","count":0,"qualificationMinimums":[{
        "qualifications":{"allQualificationIds":[id(11)],"anyQualificationIds":[]},"minimum":2
    }]});
    validate_document(&document)?;
    entity(&mut document, 6)?["coverage"] = json!({"kind":"atLeast","minimum":2,"preferredCount":1,"maximumCount":3,"qualificationMinimums":[]});
    assert!(validate_document(&document).is_err());
    Ok(())
}

#[test]
fn only_inactive_rules_can_retain_empty_explicit_scopes() -> Result<(), Box<dyn Error>> {
    let mut document = fixture()?;
    let rule = json!({"kind":"maximumHours","id":id(15),"active":false,"strength":"required",
        "scope":{"people":{"kind":"selected","personIds":[]}}, "bucketId":id(3),
        "window":{"kind":"calendar","calendarId":id(2),"membership":"reportingDate"},"maximumMinutes":120});
    document.domain.rules.insert(id(15).parse()?, rule);
    assert_eq!(decode_document(&serde_json::to_vec(&document)?)?, document);
    document
        .domain
        .rules
        .get_mut(&id(15).parse()?)
        .ok_or("missing rule")?["active"] = json!(true);
    assert!(validate_document(&document).is_err());
    Ok(())
}

#[test]
fn explicit_fold_instants_are_authority_but_forged_offsets_are_not() -> Result<(), Box<dyn Error>> {
    let mut document = fixture()?;
    validate_document(&document)?;
    entity(&mut document, 8)?["startsAt"]["instant"] = json!("2026-11-01T06:30:00Z");
    entity(&mut document, 8)?["startsAt"]["offsetSeconds"] = json!(-18000);
    validate_document(&document)?;
    entity(&mut document, 8)?["startsAt"]["offsetSeconds"] = json!(-14400);
    assert!(validate_document(&document).is_err());
    Ok(())
}

#[test]
fn gap_intent_requires_the_exact_move_forward_result() -> Result<(), Box<dyn Error>> {
    let mut document = fixture()?;
    document.settings.horizon = Horizon::new(
        "2026-03-08T05:00:00Z".parse()?,
        "2026-03-09T04:00:00Z".parse()?,
    )?;
    document.settings.gap_policy = GapPolicy::MoveForward;
    entity(&mut document, 8)?["startsAt"] = json!({"instant":"2026-03-08T07:30:00Z","local":"2026-03-08T02:30:00","offsetSeconds":-14400});
    entity(&mut document, 8)?["endsAt"] = json!({"instant":"2026-03-08T08:30:00Z","local":"2026-03-08T04:30:00","offsetSeconds":-14400});
    validate_document(&document)?;
    document.settings.gap_policy = GapPolicy::Reject;
    assert!(validate_document(&document).is_err());
    document.settings.gap_policy = GapPolicy::MoveForward;
    entity(&mut document, 8)?["startsAt"]["instant"] = json!("2026-03-08T08:00:00Z");
    assert!(validate_document(&document).is_err());
    Ok(())
}

#[test]
fn workload_target_modes_have_no_population_or_target_fallback() -> Result<(), Box<dyn Error>> {
    let mut document = fixture()?;
    entity(&mut document, 1)?
        .as_object_mut()
        .ok_or("person must be object")?
        .remove("workloadTarget");
    assert!(validate_document(&document).is_err());
    entity(&mut document, 9)?["workloadPolicies"][id(10)]["targetMode"] =
        json!({"kind":"weightedEqualShare"});
    validate_document(&document)?;
    entity(&mut document, 9)?["workloadPolicies"][id(10)]["targetMode"] =
        json!({"kind":"explicit","targets":[{"personId":id(1),"target":120}]});
    validate_document(&document)?;
    entity(&mut document, 1)?["tags"] = json!(["day"]);
    assert!(validate_document(&document).is_err());
    Ok(())
}

#[test]
fn raw_byte_limit_is_inclusive_and_parse_diagnostics_do_not_echo_data() -> Result<(), Box<dyn Error>>
{
    let document = fixture()?;
    let mut bytes = serde_json::to_vec(&document)?;
    bytes.resize(ContractJsonLimits::DEFAULT.max_serialized_bytes, b' ');
    assert_eq!(decode_document(&bytes)?, document);
    bytes.push(b' ');
    assert!(decode_document(&bytes).is_err());
    let mut malformed = document;
    let sentinel = "sensitive-unrecognized-person-value";
    entity(&mut malformed, 1)?["kind"] = json!(sentinel);
    let error = decode_document(&serde_json::to_vec(&malformed)?)
        .err()
        .ok_or("unknown entity kind was accepted")?;
    assert!(!error.to_string().contains(sentinel));
    malformed.metadata.title = "a".repeat(ContractJsonLimits::DEFAULT.max_serialized_bytes + 1);
    assert!(validate_document(&malformed).is_err());
    Ok(())
}

#[test]
fn workforce_versions_do_not_invent_migration_history() -> Result<(), Box<dyn Error>> {
    let mut document = fixture()?;
    document.domain_pack.schema_version = 0;
    assert!(matches!(
        decode_document(&serde_json::to_vec(&document)?),
        Err(DomainPackError::UnsupportedVersion(0))
    ));
    document.domain_pack.schema_version = 2;
    assert!(matches!(
        decode_document(&serde_json::to_vec(&document)?),
        Err(DomainPackError::UnsupportedVersion(2))
    ));
    Ok(())
}

#[test]
fn mixed_uuid_spelling_cannot_turn_a_person_into_an_occurrence() -> Result<(), Box<dyn Error>> {
    let mut document = fixture()?;
    entity(&mut document, 6)?["occurrenceIdentities"] = json!({
        (id(1).to_ascii_uppercase()): {"id":id(1),"localStartDate":"2026-11-01"}
    });
    document
        .domain
        .locked_assignments
        .get_mut(&id(14).parse()?)
        .ok_or("lock missing")?["shiftId"] = json!(id(1));
    assert!(validate_document(&document).is_err());
    assert!(decode_document(&serde_json::to_vec(&document)?).is_err());
    Ok(())
}

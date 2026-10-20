#![forbid(unsafe_code)]

mod support;

use eutheto_domain_api::{
    DomainPack, DomainSetupQueryV1, DomainViewInput, SetupContinuationV1, SetupViewContext,
};
use eutheto_domain_ir::{
    AssignmentValue, DomainAssignment, DomainAssignmentId, DomainEntityId, DomainEntityKindId,
    DomainEntityRef, NormalizedSolution,
};
use eutheto_types::{
    CancellationToken, OperationControl, OverlapPolicy, Revision, ScenarioDocument,
};
use eutheto_workforce::WorkforcePack;
use serde_json::{Value, json};
use std::error::Error;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const VIEW: &str = "official.workforce.result.schedule";

fn source() -> TestResult<(ScenarioDocument, NormalizedSolution)> {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = OverlapPolicy::Earlier;
    let mut second = document
        .domain
        .entities
        .get(&support::id(1).parse()?)
        .ok_or("missing source person")?
        .clone();
    second["id"] = json!(support::id(20));
    second["name"] = json!("Oak");
    second["externalId"] = json!("staff-20");
    document
        .domain
        .entities
        .insert(support::id(20).parse()?, second);
    let identity = format!("{}.{}", support::id(1), support::id(7));
    let assignment = DomainAssignment {
        id: DomainAssignmentId::new(format!("official.workforce.assignment.{identity}"))?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new("official.workforce.assignment")?,
            id: DomainEntityId::new(identity)?,
        },
        value: AssignmentValue::Boolean(true),
        evidence: Vec::new(),
    };
    let solution = NormalizedSolution {
        schema_version: 1,
        pack_id: document.domain_pack.id.clone(),
        scenario_id: document.scenario_id,
        scenario_revision: 1,
        projection_version: 1,
        solution_id: support::id(700).parse()?,
        assignments: vec![assignment],
    };
    Ok((document, solution))
}
fn source_with_base(
    base_assignments: &[(u32, u32)],
    selected: &[(u32, u32)],
) -> TestResult<(ScenarioDocument, NormalizedSolution)> {
    let (mut document, mut solution) = source()?;
    document.domain.entities.insert(
        support::id(30).parse()?,
        json!({
            "kind": "baseSchedule",
            "id": support::id(30),
            "sourceSolutionId": support::id(700),
            "sourceRevision": 1,
            "assignments": base_assignments
                .iter()
                .map(|(person, shift)| json!({
                    "personId": support::id(*person),
                    "shiftId": support::id(*shift),
                }))
                .collect::<Vec<_>>(),
        }),
    );
    solution.assignments = selected
        .iter()
        .map(|(person, shift)| selected_assignment(*person, *shift))
        .collect::<TestResult<Vec<_>>>()?;
    solution.canonicalize()?;
    Ok((document, solution))
}

fn selected_assignment(person: u32, shift: u32) -> TestResult<DomainAssignment> {
    let identity = format!("{}.{}", support::id(person), support::id(shift));
    Ok(DomainAssignment {
        id: DomainAssignmentId::new(format!("official.workforce.assignment.{identity}"))?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new("official.workforce.assignment")?,
            id: DomainEntityId::new(identity)?,
        },
        value: AssignmentValue::Boolean(true),
        evidence: Vec::new(),
    })
}

#[allow(clippy::too_many_lines)]
fn timeline_source() -> TestResult<(ScenarioDocument, NormalizedSolution)> {
    let (mut document, mut solution) = source()?;
    let mut south = document
        .domain
        .entities
        .get(&support::id(5).parse()?)
        .ok_or("missing source location")?
        .clone();
    south["id"] = json!(support::id(15));
    south["name"] = json!("South");
    document
        .domain
        .entities
        .insert(support::id(15).parse()?, south);

    let shift = document
        .domain
        .entities
        .get_mut(&support::id(8).parse()?)
        .ok_or("missing manual source shift")?;
    shift["locationId"] = json!(support::id(15));
    shift["startsAt"] = json!({
        "instant":"2026-11-01T07:30:00Z",
        "local":"2026-11-01T02:30:00",
        "offsetSeconds":-18000
    });
    shift["endsAt"] = json!({
        "instant":"2026-11-01T08:30:00Z",
        "local":"2026-11-01T03:30:00",
        "offsetSeconds":-18000
    });

    let shift_template = json!({
        "kind":"shiftInstance",
        "id":support::id(30),
        "assignmentTypeId":support::id(4),
        "locationId":support::id(5),
        "startsAt":{
            "instant":"2026-11-01T04:30:00Z",
            "local":"2026-11-01T00:30:00",
            "offsetSeconds":-14400
        },
        "endsAt":{
            "instant":"2026-11-01T05:00:00Z",
            "local":"2026-11-01T01:00:00",
            "offsetSeconds":-14400
        },
        "coverage":{"kind":"exact","count":1,"qualificationMinimums":[]},
        "tags":[],
        "reportingAttribution":"startLocalDate",
        "origin":{"kind":"manual"}
    });
    let repeated_shift = json!({
        "kind":"shiftInstance",
        "id":support::id(31),
        "assignmentTypeId":support::id(4),
        "locationId":support::id(5),
        "startsAt":{
            "instant":"2026-11-01T06:30:00Z",
            "local":"2026-11-01T01:30:00",
            "offsetSeconds":-18000
        },
        "endsAt":{
            "instant":"2026-11-01T07:00:00Z",
            "local":"2026-11-01T02:00:00",
            "offsetSeconds":-18000
        },
        "coverage":{"kind":"exact","count":1,"qualificationMinimums":[]},
        "tags":[],
        "reportingAttribution":"startLocalDate",
        "origin":{"kind":"manual"}
    });
    let unassigned_tail = json!({
        "kind":"shiftInstance",
        "id":support::id(32),
        "assignmentTypeId":support::id(4),
        "locationId":support::id(5),
        "startsAt":{
            "instant":"2026-11-01T08:30:00Z",
            "local":"2026-11-01T03:30:00",
            "offsetSeconds":-18000
        },
        "endsAt":{
            "instant":"2026-11-01T09:00:00Z",
            "local":"2026-11-01T04:00:00",
            "offsetSeconds":-18000
        },
        "coverage":{"kind":"exact","count":1,"qualificationMinimums":[]},
        "tags":[],
        "reportingAttribution":"startLocalDate",
        "origin":{"kind":"manual"}
    });
    document
        .domain
        .entities
        .insert(support::id(30).parse()?, shift_template);
    document
        .domain
        .entities
        .insert(support::id(31).parse()?, repeated_shift);
    document
        .domain
        .entities
        .insert(support::id(32).parse()?, unassigned_tail);
    solution.assignments.push(selected_assignment(20, 30)?);
    solution.assignments.push(selected_assignment(20, 7)?);
    solution.assignments.push(selected_assignment(1, 31)?);
    solution.assignments.push(selected_assignment(20, 8)?);
    solution.canonicalize()?;
    Ok((document, solution))
}

fn view(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    kind: &str,
    limit: u32,
    continuation: Option<SetupContinuationV1>,
    context: SetupViewContext,
) -> TestResult<Value> {
    view_parameters(
        document,
        solution,
        json!({"kind":kind,"limit":limit}),
        continuation,
        context,
    )
}

fn view_parameters(
    document: &ScenarioDocument,
    solution: &NormalizedSolution,
    parameters: Value,
    continuation: Option<SetupContinuationV1>,
    context: SetupViewContext,
) -> TestResult<Value> {
    let query = DomainSetupQueryV1 {
        schema_version: 1,
        view_id: VIEW.to_owned(),
        parameters,
        continuation,
    };
    let output = WorkforcePack.build_view(
        DomainViewInput::AcceptedSolution {
            document,
            solution,
            objective_context: None,
            view_id: VIEW,
            query: &query,
            context,
        },
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    assert_eq!(output.view.view_id, VIEW);
    Ok(output.view.data)
}

#[test]
fn accepted_view_rejects_foreign_or_stale_normalized_solution_bindings() -> TestResult {
    let (document, solution) = source()?;
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [92; 32],
    };
    let query = DomainSetupQueryV1 {
        schema_version: 1,
        view_id: VIEW.to_owned(),
        parameters: json!({"kind":"assignments","limit":10}),
        continuation: None,
    };
    let valid = view_parameters(
        &document,
        &solution,
        json!({"kind":"decision","personId":support::id(1),"shiftId":support::id(7)}),
        None,
        context,
    )?;
    assert_eq!(valid["result"]["selected"], json!(true));
    for field in ["pack", "scenario", "revision", "projection"] {
        let mut foreign = solution.clone();
        match field {
            "pack" => foreign.pack_id = "official.other".parse()?,
            "scenario" => foreign.scenario_id = support::id(701).parse()?,
            "revision" => foreign.scenario_revision += 1,
            "projection" => foreign.projection_version += 1,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                WorkforcePack.build_view(
                    DomainViewInput::AcceptedSolution {
                        document: &document,
                        solution: &foreign,
                        objective_context: None,
                        view_id: VIEW,
                        query: &query,
                        context,
                    },
                    &OperationControl::Cancellation(CancellationToken::new()),
                ),
                Err(eutheto_domain_api::DomainPackError::InvalidPayload { path, .. })
                    if path == "/solution"
            ),
            "{field}"
        );
    }
    Ok(())
}

#[test]
fn saved_decision_distinguishes_selected_unselected_and_unmodeled_pairs() -> TestResult {
    let (document, mut solution) = source()?;
    let mut unselected = selected_assignment(20, 7)?;
    unselected.value = AssignmentValue::Boolean(false);
    solution.assignments.push(unselected.clone());
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [91; 32],
    };
    for (person, shift, expected) in [(1, 7, Some(true)), (20, 7, Some(false)), (20, 8, None)] {
        let data = view_parameters(
            &document,
            &solution,
            json!({"kind":"decision","personId":support::id(person),"shiftId":support::id(shift)}),
            None,
            context,
        )?;
        assert_eq!(data["result"]["selected"], json!(expected));
        let expected_id = expected.map(|_| {
            format!(
                "official.workforce.assignment.{}.{}",
                support::id(person),
                support::id(shift)
            )
        });
        assert_eq!(data["result"]["assignmentId"], json!(expected_id));
    }
    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"decision","personId":support::id(999),"shiftId":support::id(7)}),
            None,
            context,
        )
        .is_err()
    );
    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"decision","personId":support::id(1)}),
            None,
            context,
        )
        .is_err()
    );
    Ok(())
}

fn cancelled_view(document: &ScenarioDocument, solution: &NormalizedSolution) -> TestResult {
    let query = DomainSetupQueryV1 {
        schema_version: 1,
        view_id: VIEW.to_owned(),
        parameters: json!({"kind":"fairness","limit":100}),
        continuation: None,
    };
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        WorkforcePack.build_view(
            DomainViewInput::AcceptedSolution {
                document,
                solution,
                objective_context: None,
                view_id: VIEW,
                query: &query,
                context: SetupViewContext {
                    revision: Revision::try_new(1)?,
                    query_fingerprint: [1; 32],
                },
            },
            &OperationControl::Cancellation(cancellation),
        ),
        Err(eutheto_domain_api::DomainPackError::Cancelled)
    ));
    Ok(())
}

#[test]
fn accepted_pages_include_unassigned_people_vacant_shifts_and_only_selected_pairs() -> TestResult {
    let (document, solution) = source()?;
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [1; 32],
    };
    let first = view(&document, &solution, "people", 1, None, context)?;
    assert_eq!(first["schemaVersion"], 1);
    assert_eq!(first["timeZone"], "America/New_York");
    assert_eq!(first["result"]["totalItems"], 2);
    assert_eq!(first["result"]["items"][0]["personId"], support::id(1));
    assert_eq!(first["result"]["items"][0]["selectedCount"], 1);
    let cursor: SetupContinuationV1 =
        serde_json::from_value(first["result"]["continuation"].clone())?;
    let last = view(
        &document,
        &solution,
        "people",
        1,
        Some(cursor.clone()),
        context,
    )?;
    assert_eq!(last["result"]["items"][0]["personId"], support::id(20));
    assert_eq!(last["result"]["items"][0]["selectedCount"], 0);
    assert!(last["result"]["continuation"].is_null());

    let shifts = view(&document, &solution, "shifts", 100, None, context)?;
    assert_eq!(shifts["result"]["totalItems"], 2);
    assert_eq!(shifts["result"]["items"][0]["shiftId"], support::id(7));
    assert_eq!(shifts["result"]["items"][0]["selectedCount"], 1);
    assert_eq!(shifts["result"]["items"][1]["shiftId"], support::id(8));
    assert_eq!(shifts["result"]["items"][1]["selectedCount"], 0);
    assert_eq!(shifts["result"]["items"][1]["margin"], -1);
    assert_eq!(
        shifts["result"]["items"][0]["startsAt"]["offsetSeconds"],
        -14400
    );

    let assignments = view(&document, &solution, "assignments", 100, None, context)?;
    assert_eq!(assignments["result"]["totalItems"], 1);
    assert_eq!(assignments["result"]["items"][0]["personName"], "River");
    assert_eq!(assignments["result"]["items"][0]["shiftId"], support::id(7));

    let changed_fingerprint = SetupViewContext {
        query_fingerprint: [2; 32],
        ..context
    };
    assert!(
        view(
            &document,
            &solution,
            "people",
            1,
            Some(cursor.clone()),
            changed_fingerprint
        )
        .is_err()
    );
    let changed_revision = SetupViewContext {
        revision: Revision::try_new(2)?,
        ..context
    };
    assert!(
        view(
            &document,
            &solution,
            "people",
            1,
            Some(cursor.clone()),
            changed_revision
        )
        .is_err()
    );
    assert!(view(&document, &solution, "shifts", 1, Some(cursor), context).is_err());
    assert!(view(&document, &solution, "people", 101, None, context).is_err());
    Ok(())
}
#[test]
fn base_diff_without_base_is_explicitly_empty() -> TestResult {
    let (document, solution) = source()?;
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [41; 32],
    };
    let data = view(&document, &solution, "baseDiff", 100, None, context)?;
    assert_eq!(data["result"]["kind"], "baseDiff");
    assert!(data["result"]["baseSchedule"].is_null());
    assert_eq!(data["result"]["totalItems"], 0);
    assert_eq!(data["result"]["items"], json!([]));
    assert!(data["result"]["continuation"].is_null());
    Ok(())
}

#[test]
fn base_diff_reports_added_removed_and_reassignment_rows_from_source() -> TestResult {
    let (document, solution) = source_with_base(&[(1, 7), (1, 8)], &[(20, 7), (1, 8)])?;
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [42; 32],
    };
    let data = view(&document, &solution, "baseDiff", 100, None, context)?;
    assert_eq!(
        data["result"]["baseSchedule"],
        json!({
            "id": support::id(30),
            "sourceSolutionId": support::id(700),
            "sourceRevision": 1,
        })
    );
    assert_eq!(data["result"]["totalItems"], 2);
    assert_eq!(
        data["result"]["items"],
        json!([
            {
                "change": "removed",
                "personId": support::id(1),
                "shiftId": support::id(7),
                "personName": "River",
                "shiftLabel": "Sunday Clinic",
            },
            {
                "change": "added",
                "personId": support::id(20),
                "shiftId": support::id(7),
                "personName": "Oak",
                "shiftLabel": "Sunday Clinic",
            },
        ])
    );
    Ok(())
}

#[test]
fn base_diff_identical_selection_has_no_changes() -> TestResult {
    let (document, solution) = source_with_base(&[(1, 7)], &[(1, 7)])?;
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [43; 32],
    };
    let data = view(&document, &solution, "baseDiff", 100, None, context)?;
    assert_eq!(data["result"]["totalItems"], 0);
    assert_eq!(data["result"]["items"], json!([]));
    assert!(data["result"]["continuation"].is_null());
    assert!(!data["result"]["baseSchedule"].is_null());
    Ok(())
}

#[test]
fn base_diff_pages_are_stable_and_reject_forged_or_stale_cursors() -> TestResult {
    let (document, solution) = source_with_base(&[(1, 7), (1, 8)], &[(20, 7), (20, 8)])?;
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [44; 32],
    };
    let first = view(&document, &solution, "baseDiff", 1, None, context)?;
    assert_eq!(first["result"]["totalItems"], 4);
    assert_eq!(first["result"]["items"][0]["personId"], support::id(1));
    assert_eq!(first["result"]["items"][0]["shiftId"], support::id(7));
    assert_eq!(first["result"]["items"][0]["change"], "removed");
    let cursor: SetupContinuationV1 =
        serde_json::from_value(first["result"]["continuation"].clone())?;

    let second = view(
        &document,
        &solution,
        "baseDiff",
        2,
        Some(cursor.clone()),
        context,
    )?;
    assert_eq!(second["result"]["totalItems"], 4);
    assert_eq!(second["result"]["items"][0]["personId"], support::id(1));
    assert_eq!(second["result"]["items"][0]["shiftId"], support::id(8));
    assert_eq!(second["result"]["items"][0]["change"], "removed");
    assert_eq!(second["result"]["items"][0]["shiftLabel"], "Clinic");
    assert_eq!(second["result"]["items"][1]["personId"], support::id(20));
    assert_eq!(second["result"]["items"][1]["shiftId"], support::id(7));
    assert_eq!(second["result"]["items"][1]["change"], "added");

    let mut forged = cursor.clone();
    forged.position = json!({
        "kind": "baseDiff",
        "personId": support::id(99),
        "shiftId": support::id(7),
        "change": "removed",
    });
    assert!(view(&document, &solution, "baseDiff", 1, Some(forged), context).is_err());
    let mut mismatched_source = cursor.clone();
    mismatched_source.scenario_id = support::id(999).parse()?;
    assert!(
        view(
            &document,
            &solution,
            "baseDiff",
            1,
            Some(mismatched_source),
            context
        )
        .is_err()
    );

    let changed_revision = SetupViewContext {
        revision: Revision::try_new(2)?,
        ..context
    };
    assert!(
        view(
            &document,
            &solution,
            "baseDiff",
            1,
            Some(cursor),
            changed_revision
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn base_diff_large_page_keeps_exact_total_and_continuation() -> TestResult {
    let base_assignments = (100..=200)
        .map(|index| (index + 2_000, index + 3_000))
        .collect::<Vec<_>>();
    let (mut document, solution) = source_with_base(&base_assignments, &[])?;
    let person_template = document
        .domain
        .entities
        .get(&support::id(1).parse()?)
        .ok_or("missing person template")?
        .clone();
    let shift_template = document
        .domain
        .entities
        .get(&support::id(8).parse()?)
        .ok_or("missing shift template")?
        .clone();
    for index in 100..=200 {
        let person_id = support::id(index + 2_000);
        let mut person = person_template.clone();
        person["id"] = json!(person_id);
        person["name"] = json!(format!("Person {index}"));
        person["externalId"] = json!(format!("staff-{index}"));
        document.domain.entities.insert(person_id.parse()?, person);

        let shift_id = support::id(index + 3_000);
        let mut shift = shift_template.clone();
        shift["id"] = json!(shift_id);
        document.domain.entities.insert(shift_id.parse()?, shift);
    }
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [45; 32],
    };
    let first = view(&document, &solution, "baseDiff", 100, None, context)?;
    assert_eq!(first["result"]["totalItems"], 101);
    assert_eq!(
        first["result"]["items"]
            .as_array()
            .ok_or("missing first items")?
            .len(),
        100
    );
    let cursor: SetupContinuationV1 =
        serde_json::from_value(first["result"]["continuation"].clone())?;
    let second = view(&document, &solution, "baseDiff", 100, Some(cursor), context)?;
    assert_eq!(second["result"]["totalItems"], 101);
    assert_eq!(
        second["result"]["items"]
            .as_array()
            .ok_or("missing second items")?
            .len(),
        1
    );
    assert!(second["result"]["continuation"].is_null());
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn calendar_pages_preserve_original_local_days_empty_cells_staffing_and_cursors() -> TestResult {
    let (mut document, solution) = source()?;
    document.settings.horizon.end = "2026-11-08T05:00:00Z".parse()?;
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [15; 32],
    };
    let first = view(&document, &solution, "calendarPeople", 1, None, context)?;
    assert_eq!(first["timeZone"], "America/New_York");
    assert_eq!(
        first["result"]["planningDates"],
        json!({"startDate":"2026-11-01","endDateExclusive":"2026-11-08"})
    );
    assert_eq!(
        first["result"]["window"],
        json!({"startDate":"2026-11-01","endDateExclusive":"2026-11-08"})
    );
    assert_eq!(first["result"]["totalItems"], 2);
    assert_eq!(
        first["result"]["items"][0]["person"]["personId"],
        support::id(1)
    );
    assert_eq!(
        first["result"]["items"][0]["days"]
            .as_array()
            .ok_or("missing days")?
            .len(),
        7
    );
    assert_eq!(first["result"]["items"][0]["days"][0]["selectedCount"], 1);
    assert_eq!(
        first["result"]["items"][0]["days"][0]["selectedShifts"][0]["startsAt"]["offsetSeconds"],
        -14400
    );
    assert_eq!(
        first["result"]["items"][0]["days"][0]["selectedShifts"][0]["endsAt"]["offsetSeconds"],
        -18000
    );
    assert_eq!(first["result"]["items"][0]["days"][1]["selectedCount"], 0);
    let cursor: SetupContinuationV1 =
        serde_json::from_value(first["result"]["continuation"].clone())?;
    let second = view(
        &document,
        &solution,
        "calendarPeople",
        1,
        Some(cursor.clone()),
        context,
    )?;
    assert_eq!(
        second["result"]["items"][0]["person"]["personId"],
        support::id(20)
    );
    assert_eq!(second["result"]["items"][0]["days"][0]["selectedCount"], 0);
    assert!(second["result"]["continuation"].is_null());
    assert!(
        view(
            &document,
            &solution,
            "calendarShifts",
            1,
            Some(cursor),
            context
        )
        .is_err()
    );

    let shifts = view(&document, &solution, "calendarShifts", 1, None, context)?;
    assert_eq!(shifts["result"]["totalItems"], 2);
    assert_eq!(shifts["result"]["items"][0]["date"], "2026-11-01");
    assert_eq!(shifts["result"]["items"][0]["shift"]["selectedCount"], 1);
    assert_eq!(
        shifts["result"]["items"][0]["selectedPersonIds"],
        json!([support::id(1)])
    );
    let shift_cursor: SetupContinuationV1 =
        serde_json::from_value(shifts["result"]["continuation"].clone())?;
    let vacant = view(
        &document,
        &solution,
        "calendarShifts",
        1,
        Some(shift_cursor.clone()),
        context,
    )?;
    assert_eq!(
        vacant["result"]["items"][0]["shift"]["shiftId"],
        support::id(8)
    );
    assert_eq!(vacant["result"]["items"][0]["shift"]["selectedCount"], 0);
    assert_eq!(vacant["result"]["items"][0]["shift"]["margin"], -1);
    assert_eq!(vacant["result"]["items"][0]["selectedPersonIds"], json!([]));
    assert!(vacant["result"]["continuation"].is_null());
    let empty_window = view_parameters(
        &document,
        &solution,
        json!({"kind":"calendarShifts","startDate":"2026-11-04","limit":1}),
        None,
        context,
    )?;
    assert_eq!(empty_window["result"]["totalItems"], 0);
    assert_eq!(
        empty_window["result"]["window"],
        json!({"startDate":"2026-11-04","endDateExclusive":"2026-11-08"})
    );
    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"calendarShifts","startDate":"2026-11-04","limit":1}),
            Some(shift_cursor),
            context,
        )
        .is_err()
    );
    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"calendarShifts","startDate":"2026-11-08","limit":1}),
            None,
            context,
        )
        .is_err()
    );
    let same_day = view_parameters(
        &document,
        &solution,
        json!({"kind":"assignments","personId":support::id(1),"localDate":"2026-11-01"}),
        None,
        context,
    )?;
    assert_eq!(same_day["result"]["totalItems"], 1);
    assert_eq!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"assignments","personId":support::id(1),"localDate":"2026-11-02"}),
            None,
            context,
        )?["result"]["totalItems"],
        0
    );
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn dense_person_calendar_caps_samples_and_keeps_every_day_assignment_pageable() -> TestResult {
    let (mut document, mut solution) = source()?;
    document.settings.horizon.end = "2026-11-03T05:00:00Z".parse()?;
    for index in 0..5_u32 {
        let id = 30 + index;
        let mut shift = document
            .domain
            .entities
            .get(&support::id(8).parse()?)
            .ok_or("missing source shift")?
            .clone();
        shift["id"] = json!(support::id(id));
        let (date, local_start, local_end, utc_start, utc_end) = if index < 4 {
            (
                "2026-11-01",
                format!("{:02}:30:00", 3 + index),
                format!("{:02}:00:00", 4 + index),
                format!("{:02}:30:00Z", 8 + index),
                format!("{:02}:00:00Z", 9 + index),
            )
        } else {
            (
                "2026-11-02",
                "03:30:00".to_owned(),
                "04:00:00".to_owned(),
                "08:30:00Z".to_owned(),
                "09:00:00Z".to_owned(),
            )
        };
        shift["startsAt"] = json!({
            "instant":format!("{date}T{utc_start}"),
            "local":format!("{date}T{local_start}"),
            "offsetSeconds":-18000
        });
        shift["endsAt"] = json!({
            "instant":format!("{date}T{utc_end}"),
            "local":format!("{date}T{local_end}"),
            "offsetSeconds":-18000
        });
        document
            .domain
            .entities
            .insert(support::id(id).parse()?, shift);
        solution.assignments.push(selected_assignment(1, id)?);
    }
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [16; 32],
    };
    let calendar = view(&document, &solution, "calendarPeople", 50, None, context)?;
    assert_eq!(
        calendar["result"]["items"][0]["days"][0]["selectedCount"],
        5
    );
    assert_eq!(
        calendar["result"]["items"][0]["days"][0]["selectedShifts"]
            .as_array()
            .ok_or("missing bounded samples")?
            .len(),
        3
    );
    assert_eq!(
        calendar["result"]["items"][0]["days"][1]["selectedCount"],
        1
    );
    let first = view_parameters(
        &document,
        &solution,
        json!({"kind":"assignments","personId":support::id(1),"localDate":"2026-11-01","limit":2}),
        None,
        context,
    )?;
    assert_eq!(first["result"]["totalItems"], 5);
    let cursor: SetupContinuationV1 =
        serde_json::from_value(first["result"]["continuation"].clone())?;
    assert!(view_parameters(
        &document,
        &solution,
        json!({"kind":"assignments","personId":support::id(1),"localDate":"2026-11-02","limit":2}),
        Some(cursor.clone()),
        context,
    )
    .is_err());
    let second = view_parameters(
        &document,
        &solution,
        json!({"kind":"assignments","personId":support::id(1),"localDate":"2026-11-01","limit":2}),
        Some(cursor),
        context,
    )?;
    assert_eq!(
        second["result"]["items"]
            .as_array()
            .ok_or("missing assignment page")?
            .len(),
        2
    );
    assert!(second["result"]["continuation"].is_object());
    let next_day = view_parameters(
        &document,
        &solution,
        json!({"kind":"assignments","personId":support::id(1),"localDate":"2026-11-02","limit":2}),
        None,
        context,
    )?;
    assert_eq!(next_day["result"]["totalItems"], 1);
    assert_eq!(next_day["result"]["items"][0]["shiftId"], support::id(34));
    Ok(())
}

#[test]
fn health_rows_are_source_typed_with_exact_coverage_and_cursor_bounds() -> TestResult {
    let (mut document, solution) = source()?;
    document
        .domain
        .entities
        .get_mut(&support::id(8).parse()?)
        .ok_or("missing source shift")?["coverage"]["qualificationMinimums"] = json!([
        {
            "qualifications": {
                "allQualificationIds": [support::id(11)],
                "anyQualificationIds": []
            },
            "minimum": 1
        }
    ]);
    document.domain.entities.insert(
        support::id(30).parse()?,
        json!({
            "kind":"coverageRequirement",
            "id":support::id(30),
            "active":true,
            "scope":{"kind":"filter","assignmentTypeIds":[support::id(4)]},
            "coverage":{
                "kind":"atLeast",
                "minimum":2,
                "qualificationMinimums":[{
                    "qualifications":{
                        "allQualificationIds":[support::id(11)],
                        "anyQualificationIds":[]
                    },
                    "minimum":1
                }]
            }
        }),
    );
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [9; 32],
    };
    let first = view_parameters(
        &document,
        &solution,
        json!({"kind":"health","limit":2}),
        None,
        context,
    )?;
    assert_eq!(first["result"]["kind"], "health");
    assert!(first["result"]["totalItems"].as_u64().unwrap_or(0) >= 6);
    let rows = first["result"]["items"]
        .as_array()
        .ok_or("missing health rows")?;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| {
        row["units"] == "people"
            && row["threshold"]["denominator"] == "1"
            && row["observed"]["denominator"] == "1"
            && row["margin"]["denominator"] == "1"
    }));
    let cursor: SetupContinuationV1 =
        serde_json::from_value(first["result"]["continuation"].clone())?;
    assert_eq!(cursor.position["kind"], "health");
    let second = view_parameters(
        &document,
        &solution,
        json!({"kind":"health","limit":100}),
        Some(cursor.clone()),
        context,
    )?;
    let second_rows = second["result"]["items"]
        .as_array()
        .ok_or("missing second health page")?;
    assert!(
        second_rows
            .iter()
            .any(|row| row["coverageOwnerId"] == support::id(30))
    );
    let changed = SetupViewContext {
        query_fingerprint: [10; 32],
        ..context
    };
    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"health","limit":2}),
            Some(cursor),
            changed,
        )
        .is_err()
    );
    Ok(())
}
#[test]
fn health_preserves_both_staffing_limits_and_unmatched_coverage_rule() -> TestResult {
    let (mut document, solution) = source()?;
    document
        .domain
        .entities
        .get_mut(&support::id(8).parse()?)
        .ok_or("missing source shift")?["coverage"] = json!({
        "kind":"atLeast",
        "minimum":1,
        "maximumCount":2,
        "qualificationMinimums":[]
    });
    document.domain.rules.insert(
        support::id(50).parse()?,
        json!({
            "kind":"coverage",
            "id":support::id(50),
            "active":true,
            "strength":"required",
            "scope":{"people":{"kind":"all"},"weekdays":["monday"]}
        }),
    );
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [17; 32],
    };
    let health = view(&document, &solution, "health", 100, None, context)?;
    let rows = health["result"]["items"]
        .as_array()
        .ok_or("missing health rows")?;
    let headcount = |direction: &str| {
        rows.iter().find(|row| {
            row["check"] == "coverageHeadcount"
                && row["coverageOwnerId"] == support::id(8)
                && row["shiftId"] == support::id(8)
                && row["direction"] == direction
        })
    };
    let lower = headcount("minimum").ok_or("missing lower staffing limit")?;
    assert_eq!(lower["status"], "violated");
    assert_eq!(lower["margin"], json!({"numerator":"-1","denominator":"1"}));
    let upper = headcount("maximum").ok_or("missing upper staffing limit")?;
    assert_eq!(upper["status"], "satisfied");
    assert_eq!(
        upper["threshold"],
        json!({"numerator":"2","denominator":"1"})
    );
    assert_eq!(upper["margin"], json!({"numerator":"2","denominator":"1"}));
    let empty_rule = rows
        .iter()
        .find(|row| row["sourceRuleId"] == support::id(50))
        .ok_or("missing unmatched authored Coverage rule")?;
    assert_eq!(empty_rule["status"], "noApplicable");
    assert!(empty_rule["observed"].is_null());
    assert!(empty_rule["margin"].is_null());
    Ok(())
}

#[test]
fn rolling_hours_health_retains_both_start_and_intersection_contributors() -> TestResult {
    let (mut document, mut solution) = source()?;
    solution.assignments.push(selected_assignment(1, 8)?);
    for membership in ["startInstant", "intersection"] {
        document.domain.rules.insert(
            support::id(41).parse()?,
            json!({
                "kind":"maximumHours",
                "id":support::id(41),
                "active":true,
                "strength":"required",
                "scope":{"people":{"kind":"all"}},
                "bucketId":support::id(3),
                "window":{"kind":"rolling","durationMinutes":180,"membership":membership},
                "maximumMinutes":30
            }),
        );
        let health = view(
            &document,
            &solution,
            "health",
            100,
            None,
            SetupViewContext {
                revision: Revision::try_new(1)?,
                query_fingerprint: [19; 32],
            },
        )?;
        let rows = health["result"]["items"]
            .as_array()
            .ok_or("missing rolling hours rows")?;
        let window = rows
            .iter()
            .find(|row| {
                row["check"] == "maximumHours"
                    && row["sourceRuleId"] == support::id(41)
                    && row["affectedAssignmentCount"] == 2
            })
            .ok_or("missing shared rolling hours window")?;
        let retained = window["affectedAssignmentIds"]
            .as_array()
            .ok_or("missing rolling hour contributors")?;
        assert_eq!(retained.len(), 2, "{membership}");
        assert!(
            !window["affectedAssignmentsTruncated"]
                .as_bool()
                .ok_or("missing bound")?
        );
        for assignment in &solution.assignments {
            assert!(
                retained
                    .iter()
                    .any(|id| id.as_str() == Some(assignment.id.as_str())),
                "missing {} for {membership}",
                assignment.id,
            );
        }
    }
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn health_time_rows_keep_signed_dst_and_directed_scope_margins() -> TestResult {
    let (mut document, mut solution) = source()?;
    let mut second_location = document
        .domain
        .entities
        .get(&support::id(5).parse()?)
        .ok_or("missing source location")?
        .clone();
    second_location["id"] = json!(support::id(15));
    second_location["name"] = json!("South");
    second_location["transitions"] = json!([]);
    document
        .domain
        .entities
        .insert(support::id(15).parse()?, second_location);
    document
        .domain
        .entities
        .get_mut(&support::id(5).parse()?)
        .ok_or("missing source location")?["transitions"] =
        json!([{"locationId":support::id(15),"minutes":30}]);
    document
        .domain
        .entities
        .get_mut(&support::id(8).parse()?)
        .ok_or("missing source shift")?["locationId"] = json!(support::id(15));
    document.domain.rules.insert(
        support::id(40).parse()?,
        json!({
            "kind":"minimumRest",
            "id":support::id(40),
            "active":true,
            "strength":"required",
            "scope":{"people":{"kind":"all"}},
            "afterScope":{"people":{"kind":"all"}},
            "beforeScope":{"people":{"kind":"all"}},
            "minimumMinutes":120
        }),
    );
    document.domain.rules.insert(
        support::id(41).parse()?,
        json!({
            "kind":"maximumHours",
            "id":support::id(41),
            "active":true,
            "strength":"required",
            "scope":{"people":{"kind":"all"}},
            "bucketId":support::id(3),
            "window":{"kind":"calendar","calendarId":support::id(2),"membership":"reportingDate"},
            "maximumMinutes":30
        }),
    );
    document.domain.rules.insert(
        support::id(42).parse()?,
        json!({
            "kind":"transitionTime",
            "id":support::id(42),
            "active":true,
            "strength":"required",
            "scope":{"people":{"kind":"all"}},
            "locationIds":[support::id(5),support::id(15)]
        }),
    );
    document.domain.rules.insert(
        support::id(43).parse()?,
        json!({
            "kind":"minimumRest",
            "id":support::id(43),
            "active":true,
            "strength":"required",
            "scope":{"people":{"kind":"selected","personIds":[support::id(20)]}},
            "afterScope":{"people":{"kind":"selected","personIds":[support::id(20)]}},
            "beforeScope":{"people":{"kind":"selected","personIds":[support::id(20)]}},
            "minimumMinutes":15
        }),
    );
    solution.assignments.push(selected_assignment(1, 8)?);
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [11; 32],
    };
    let health = view(&document, &solution, "health", 100, None, context)?;
    let rows = health["result"]["items"]
        .as_array()
        .ok_or("missing health rows")?;
    let row = |check: &str, rule: &str| {
        rows.iter()
            .find(|value| value["check"] == check && value["sourceRuleId"] == rule)
            .ok_or("missing health row")
    };
    let rest = row("minimumRest", &support::id(40))?;
    assert_eq!(rest["status"], "violated");
    assert_eq!(
        rest["threshold"],
        json!({"numerator":"120","denominator":"1"})
    );
    assert_eq!(
        rest["observed"],
        json!({"numerator":"-60","denominator":"1"})
    );
    assert_eq!(
        rest["margin"],
        json!({"numerator":"-180","denominator":"1"})
    );
    let hours = row("maximumHours", &support::id(41))?;
    assert_eq!(hours["units"], "minutes");
    assert_eq!(
        hours["observed"],
        json!({"numerator":"120","denominator":"1"})
    );
    assert_eq!(
        hours["margin"],
        json!({"numerator":"-90","denominator":"1"})
    );
    let transition = row("transitionTime", &support::id(42))?;
    assert_eq!(transition["status"], "violated");
    assert_eq!(
        transition["margin"],
        json!({"numerator":"-90","denominator":"1"})
    );
    let no_applicable = row("minimumRest", &support::id(43))?;
    assert_eq!(no_applicable["status"], "noApplicable");
    assert!(no_applicable["observed"].is_null());
    assert!(no_applicable["margin"].is_null());
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[test]
fn timeline_pages_are_utc_chronological_and_reject_forged_or_mismatched_cursors() -> TestResult {
    let (document, solution) = timeline_source()?;
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [7; 32],
    };

    let first = view_parameters(
        &document,
        &solution,
        json!({"kind":"timeline","limit":1}),
        None,
        context,
    )?;
    assert_eq!(first["result"]["totalItems"], 5);
    assert_eq!(first["result"]["items"][0]["shiftId"], support::id(30));
    assert_eq!(first["result"]["items"][0]["personId"], support::id(20));
    assert_eq!(
        first["result"]["items"][0]["startsAt"]["instant"],
        "2026-11-01T04:30:00Z"
    );
    let first_cursor: SetupContinuationV1 =
        serde_json::from_value(first["result"]["continuation"].clone())?;
    assert_eq!(
        first_cursor.position,
        json!({"kind":"timeline","shiftId":support::id(30),"personId":support::id(20)})
    );

    let second = view_parameters(
        &document,
        &solution,
        json!({"kind":"timeline","limit":2}),
        Some(first_cursor.clone()),
        context,
    )?;
    assert_eq!(second["result"]["totalItems"], 5);
    assert_eq!(second["result"]["items"][0]["shiftId"], support::id(7));
    assert_eq!(second["result"]["items"][0]["personId"], support::id(1));
    assert_eq!(second["result"]["items"][1]["shiftId"], support::id(7));
    assert_eq!(second["result"]["items"][1]["personId"], support::id(20));
    assert_eq!(
        second["result"]["items"][0]["startsAt"]["instant"],
        "2026-11-01T05:30:00Z"
    );
    assert_eq!(
        second["result"]["items"][1]["startsAt"]["instant"],
        "2026-11-01T05:30:00Z"
    );
    assert_eq!(
        second["result"]["items"][0]["startsAt"]["local"],
        "2026-11-01T01:30:00"
    );
    assert_eq!(
        second["result"]["items"][0]["startsAt"]["offsetSeconds"],
        -14400
    );
    let second_cursor: SetupContinuationV1 =
        serde_json::from_value(second["result"]["continuation"].clone())?;
    assert_eq!(
        second_cursor.position,
        json!({"kind":"timeline","shiftId":support::id(7),"personId":support::id(20)})
    );

    let third = view_parameters(
        &document,
        &solution,
        json!({"kind":"timeline","limit":2}),
        Some(second_cursor),
        context,
    )?;
    assert_eq!(third["result"]["totalItems"], 5);
    assert_eq!(third["result"]["items"].as_array().map(Vec::len), Some(2));
    assert_eq!(third["result"]["items"][0]["shiftId"], support::id(31));
    assert_eq!(third["result"]["items"][1]["shiftId"], support::id(8));
    assert_eq!(
        third["result"]["items"][0]["startsAt"]["instant"],
        "2026-11-01T06:30:00Z"
    );
    assert_eq!(
        third["result"]["items"][0]["startsAt"]["local"],
        "2026-11-01T01:30:00"
    );
    assert_eq!(
        third["result"]["items"][0]["startsAt"]["offsetSeconds"],
        -18000
    );
    assert!(third["result"]["continuation"].is_null());

    let forged = SetupContinuationV1 {
        schema_version: 1,
        scenario_id: document.scenario_id,
        revision: Revision::try_new(1)?,
        query_fingerprint: [7; 32],
        position: json!({
            "kind":"timeline",
            "shiftId":support::id(30),
            "personId":support::id(1)
        }),
    };
    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"timeline","limit":1}),
            Some(forged),
            context,
        )
        .is_err()
    );
    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"timeline","personId":support::id(1),"limit":1}),
            Some(first_cursor.clone()),
            context,
        )
        .is_err()
    );
    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"timeline","locationId":support::id(15),"limit":1}),
            Some(first_cursor),
            context,
        )
        .is_err()
    );

    let person = view_parameters(
        &document,
        &solution,
        json!({"kind":"timeline","personId":support::id(1),"limit":100}),
        None,
        context,
    )?;
    assert_eq!(person["result"]["totalItems"], 2);
    assert_eq!(person["result"]["items"][0]["shiftId"], support::id(7));
    assert_eq!(person["result"]["items"][1]["shiftId"], support::id(31));

    let old_first = view(&document, &solution, "assignments", 1, None, context)?;
    assert_eq!(old_first["result"]["totalItems"], 5);
    assert_eq!(old_first["result"]["items"][0]["shiftId"], support::id(7));
    let old_cursor: SetupContinuationV1 =
        serde_json::from_value(old_first["result"]["continuation"].clone())?;
    assert_eq!(old_cursor.position["kind"], "assignment");
    let old_second = view(
        &document,
        &solution,
        "assignments",
        1,
        Some(old_cursor),
        context,
    )?;
    assert_eq!(old_second["result"]["items"][0]["shiftId"], support::id(31));
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[test]
fn location_filters_page_the_complete_original_shift_source() -> TestResult {
    let (document, solution) = timeline_source()?;
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [8; 32],
    };
    let first = view_parameters(
        &document,
        &solution,
        json!({"kind":"shifts","locationId":support::id(5),"limit":1}),
        None,
        context,
    )?;
    assert_eq!(first["result"]["totalItems"], 4);
    assert_eq!(first["result"]["items"][0]["shiftId"], support::id(30));
    let first_cursor: SetupContinuationV1 =
        serde_json::from_value(first["result"]["continuation"].clone())?;
    let second = view_parameters(
        &document,
        &solution,
        json!({"kind":"shifts","locationId":support::id(5),"limit":1}),
        Some(first_cursor),
        context,
    )?;
    assert_eq!(second["result"]["totalItems"], 4);
    assert_eq!(second["result"]["items"][0]["shiftId"], support::id(7));
    let second_cursor: SetupContinuationV1 =
        serde_json::from_value(second["result"]["continuation"].clone())?;
    let third = view_parameters(
        &document,
        &solution,
        json!({"kind":"shifts","locationId":support::id(5),"limit":1}),
        Some(second_cursor),
        context,
    )?;
    assert_eq!(third["result"]["totalItems"], 4);
    assert_eq!(third["result"]["items"][0]["shiftId"], support::id(31));
    let third_cursor: SetupContinuationV1 =
        serde_json::from_value(third["result"]["continuation"].clone())?;
    let fourth = view_parameters(
        &document,
        &solution,
        json!({"kind":"shifts","locationId":support::id(5),"limit":1}),
        Some(third_cursor),
        context,
    )?;
    assert_eq!(fourth["result"]["totalItems"], 4);
    assert_eq!(fourth["result"]["items"][0]["shiftId"], support::id(32));
    assert_eq!(fourth["result"]["items"][0]["selectedCount"], 0);
    assert!(fourth["result"]["continuation"].is_null());

    let filtered_timeline = view_parameters(
        &document,
        &solution,
        json!({"kind":"timeline","locationId":support::id(5),"limit":2}),
        None,
        context,
    )?;
    assert_eq!(filtered_timeline["result"]["totalItems"], 4);
    assert_eq!(
        filtered_timeline["result"]["items"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        filtered_timeline["result"]["items"][0]["shiftId"],
        support::id(30)
    );
    assert_eq!(
        filtered_timeline["result"]["items"][1]["shiftId"],
        support::id(7)
    );

    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"assignments","locationId":support::id(5),"limit":1}),
            None,
            context,
        )
        .is_err()
    );
    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"timeline","shiftId":support::id(7),"limit":1}),
            None,
            context,
        )
        .is_err()
    );
    assert!(
        view_parameters(
            &document,
            &solution,
            json!({"kind":"shifts","locationId":support::id(99),"limit":1}),
            None,
            context,
        )
        .is_err()
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[test]
fn fairness_distribution_preserves_weighted_targets_fractional_work_and_peer_membership()
-> TestResult {
    let (mut document, mut solution) = source()?;
    let person_two = document
        .domain
        .entities
        .get(&support::id(20).parse()?)
        .ok_or("missing second source person")?
        .clone();
    let mut ineligible = person_two;
    ineligible["eligibleAssignmentTypeIds"] = json!([]);
    ineligible["workloadWeight"] = json!({"numerator":2,"denominator":1});
    document
        .domain
        .entities
        .insert(support::id(20).parse()?, ineligible);
    let mut fractional = document
        .domain
        .entities
        .get(&support::id(8).parse()?)
        .ok_or("missing source shift")?
        .clone();
    fractional["id"] = json!(support::id(30));
    fractional["startsAt"] = json!({
        "instant":"2026-11-01T08:00:00Z",
        "local":"2026-11-01T03:00:00",
        "offsetSeconds":-18000
    });
    fractional["endsAt"] = json!({
        "instant":"2026-11-01T08:01:30Z",
        "local":"2026-11-01T03:01:30",
        "offsetSeconds":-18000
    });
    document
        .domain
        .entities
        .insert(support::id(30).parse()?, fractional);
    let mut score_policy = document
        .domain
        .entities
        .get(&support::id(9).parse()?)
        .ok_or("missing score policy")?
        .clone();
    score_policy["workloadPolicies"][support::id(10)]["targetMode"] =
        json!({"kind":"weightedEqualShare"});
    document
        .domain
        .entities
        .insert(support::id(9).parse()?, score_policy);
    document.domain.preferences.insert(
        support::id(21).parse()?,
        json!({
            "kind":"workloadBalance",
            "id":support::id(21),
            "active":true,
            "scope":{"people":{"kind":"all"}},
            "priority":"normal",
            "weight":10,
            "workloadPolicyId":support::id(10)
        }),
    );
    let identity = format!("{}.{}", support::id(1), support::id(30));
    solution.assignments.push(DomainAssignment {
        id: DomainAssignmentId::new(format!("official.workforce.assignment.{identity}"))?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new("official.workforce.assignment")?,
            id: DomainEntityId::new(identity)?,
        },
        value: AssignmentValue::Boolean(true),
        evidence: Vec::new(),
    });
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [1; 32],
    };
    let fairness = view(&document, &solution, "fairness", 100, None, context)?;
    assert_eq!(fairness["result"]["totalItems"], 2);
    let items = fairness["result"]["items"]
        .as_array()
        .ok_or("missing fairness rows")?;
    assert_eq!(items[0]["personId"], support::id(1));
    assert_eq!(
        items[0]["actual"],
        json!({"numerator":"123","denominator":"2"})
    );
    assert_eq!(
        items[0]["target"],
        json!({"numerator":"41","denominator":"2"})
    );
    assert_eq!(
        items[0]["deviation"],
        json!({"numerator":"41","denominator":"1"})
    );
    assert_eq!(
        items[0]["groupSpread"],
        json!({"numerator":"123","denominator":"2"})
    );
    assert_eq!(items[1]["personId"], support::id(20));
    assert_eq!(
        items[1]["target"],
        json!({"numerator":"41","denominator":"1"})
    );
    assert_eq!(
        items[1]["actual"],
        json!({"numerator":"0","denominator":"1"})
    );
    assert_eq!(
        items[1]["deviation"],
        json!({"numerator":"-41","denominator":"1"})
    );
    assert_eq!(items[0]["periodStartsAt"]["local"], "2026-11-01T00:00:00");
    assert_eq!(items[0]["measure"], "elapsedMinutes");
    assert_eq!(items[0]["units"], "minutes");
    let first_page = view(&document, &solution, "fairness", 1, None, context)?;
    assert_eq!(first_page["result"]["totalItems"], 2);
    assert_eq!(
        first_page["result"]["items"].as_array().map(Vec::len),
        Some(1)
    );
    let cursor: SetupContinuationV1 =
        serde_json::from_value(first_page["result"]["continuation"].clone())?;
    let second_page = view(&document, &solution, "fairness", 1, Some(cursor), context)?;
    assert_eq!(
        second_page["result"]["items"][0]["personId"],
        support::id(20)
    );
    assert!(second_page["result"]["continuation"].is_null());
    let (empty_document, empty_solution) = source()?;
    let empty = view(
        &empty_document,
        &empty_solution,
        "fairness",
        100,
        None,
        context,
    )?;
    assert_eq!(empty["result"]["totalItems"], 0);
    cancelled_view(&document, &solution)?;
    Ok(())
}

#[test]
fn fairness_view_unions_overlapping_work_and_clips_intersection_at_horizon_tail() -> TestResult {
    let (mut document, mut solution) = source()?;
    document.domain.preferences.insert(
        support::id(21).parse()?,
        json!({
            "kind":"workloadBalance",
            "id":support::id(21),
            "active":true,
            "scope":{"people":{"kind":"all"}},
            "priority":"normal",
            "weight":10,
            "workloadPolicyId":support::id(10)
        }),
    );
    let context = SetupViewContext {
        revision: Revision::try_new(1)?,
        query_fingerprint: [3; 32],
    };
    let identity = format!("{}.{}", support::id(1), support::id(8));
    let manual = DomainAssignment {
        id: DomainAssignmentId::new(format!("official.workforce.assignment.{identity}"))?,
        entity: DomainEntityRef {
            kind: DomainEntityKindId::new("official.workforce.assignment")?,
            id: DomainEntityId::new(identity)?,
        },
        value: AssignmentValue::Boolean(true),
        evidence: Vec::new(),
    };
    solution.assignments.push(manual.clone());
    let union = view(&document, &solution, "fairness", 100, None, context)?;
    assert_eq!(union["result"]["totalItems"], 2);
    assert_eq!(
        union["result"]["items"][0]["actual"],
        json!({"numerator":"120","denominator":"1"})
    );
    assert_eq!(
        union["result"]["items"][0]["groupSpread"],
        json!({"numerator":"120","denominator":"1"})
    );

    let manual_source = document
        .domain
        .entities
        .get_mut(&support::id(8).parse()?)
        .ok_or("missing manual shift")?;
    manual_source["startsAt"] = json!({
        "instant":"2026-11-02T04:30:00Z",
        "local":"2026-11-01T23:30:00",
        "offsetSeconds":-18000
    });
    manual_source["endsAt"] = json!({
        "instant":"2026-11-02T05:30:00Z",
        "local":"2026-11-02T00:30:00",
        "offsetSeconds":-18000
    });
    let policy = document
        .domain
        .entities
        .get_mut(&support::id(9).parse()?)
        .ok_or("missing score policy")?;
    policy["workloadPolicies"][support::id(10)]["membership"] = json!("intersection");
    policy["workloadPolicies"][support::id(10)]["targetMode"] = json!({"kind":"equalShare"});
    solution.assignments.clear();
    solution.assignments.push(manual);
    let clipped = view(&document, &solution, "fairness", 100, None, context)?;
    assert_eq!(clipped["result"]["totalItems"], 4);
    let rows = clipped["result"]["items"]
        .as_array()
        .ok_or("missing fairness rows")?;
    assert_eq!(rows[0]["personId"], support::id(1));
    assert_eq!(rows[0]["periodStartsAt"]["local"], "2026-11-01T00:00:00");
    assert_eq!(
        rows[0]["actual"],
        json!({"numerator":"30","denominator":"1"})
    );
    assert_eq!(rows[2]["personId"], support::id(1));
    assert_eq!(rows[2]["periodStartsAt"]["local"], "2026-11-02T00:00:00");
    assert_eq!(
        rows[2]["actual"],
        json!({"numerator":"30","denominator":"1"})
    );
    Ok(())
}

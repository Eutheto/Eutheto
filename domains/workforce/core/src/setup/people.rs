use super::{
    contracts::{
        AvailabilityOccurrenceV1, AvailabilityWindowParametersV1, EligibilityMatrixParametersV1,
        EligibilityMatrixV1, InstantIntervalV1, MAX_MATRIX_CELLS, MAX_MATRIX_PEOPLE,
        MAX_MATRIX_TYPES, ORDINARY_DATA_BYTES, WorkforceEntityKindV1, WorkforcePositionV1,
        WorkforceSetupViewDataV1,
    },
    entities::header,
    paging::{PageBuilder, ProjectionBudget, Result, invalid},
    work::check_dates,
};
use crate::{
    assignment_rules::{
        budget::OperationBudget,
        intervals::{availability_intervals, date_range},
        operation_error,
    },
    ids::AssignmentTypeId,
    model::WorkforceEntity,
    validation::WorkforceSchemas,
};
use eutheto_domain_api::{ContractJsonLimits, DomainPackError, SetupViewContext};
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::{EntityId, PersonId, ScenarioDocument};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn eligibility_matrix(
    document: &ScenarioDocument,
    parameters: &EligibilityMatrixParametersV1,
    position: Option<WorkforcePositionV1>,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    budget.visit()?;
    if position.is_some() {
        return Err(invalid(
            "/query/continuation",
            "eligibility matrix does not accept continuation",
        ));
    }
    if parameters.person_ids.len() > MAX_MATRIX_PEOPLE
        || parameters.assignment_type_ids.len() > MAX_MATRIX_TYPES
        || parameters
            .person_ids
            .len()
            .checked_mul(parameters.assignment_type_ids.len())
            .is_none_or(|cells| cells > MAX_MATRIX_CELLS)
    {
        return Err(invalid(
            "/query/parameters",
            "eligibility matrix exceeds its axis or cell bounds",
        ));
    }
    let mut columns = BTreeMap::new();
    for (column, &id) in parameters.assignment_type_ids.iter().enumerate() {
        budget.visit()?;
        if columns.insert(id, column).is_some() {
            return Err(invalid(
                "/query/parameters/assignmentTypeIds",
                "assignment type axis contains duplicates",
            ));
        }
        axis_record(
            document,
            id.as_entity_id(),
            WorkforceEntityKindV1::AssignmentType,
            "/query/parameters/assignmentTypeIds",
        )?;
    }
    let mut seen = BTreeSet::new();
    let mut configured_memberships = Vec::with_capacity(parameters.person_ids.len());
    for &id in &parameters.person_ids {
        budget.visit()?;
        if !seen.insert(id) {
            return Err(invalid(
                "/query/parameters/personIds",
                "person axis contains duplicates",
            ));
        }
        let record = axis_record(
            document,
            EntityId::from_uuid(id.as_uuid()),
            WorkforceEntityKindV1::Person,
            "/query/parameters/personIds",
        )?;
        let memberships = record
            .get("eligibleAssignmentTypeIds")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("/domain/entities", "person membership list is invalid"))?;
        let mut row = vec![false; columns.len()];
        // Scan each configured membership once, rather than once per requested type.
        // Only bounded requested columns are retained, even for a large source list.
        for membership in memberships {
            budget.visit()?;
            let membership = AssignmentTypeId::deserialize(membership).map_err(|_| {
                invalid("/domain/entities", "person membership identity is invalid")
            })?;
            if let Some(&column) = columns.get(&membership) {
                row[column] = true;
            }
        }
        for _ in &row {
            budget.visit()?;
        }
        configured_memberships.push(row);
    }
    Ok(WorkforceSetupViewDataV1::EligibilityMatrix(
        EligibilityMatrixV1 {
            person_ids: parameters.person_ids.clone(),
            assignment_type_ids: parameters.assignment_type_ids.clone(),
            configured_memberships,
        },
    ))
}

fn axis_record<'a>(
    document: &'a ScenarioDocument,
    id: EntityId,
    expected: WorkforceEntityKindV1,
    path: &str,
) -> Result<&'a Value> {
    let record = document
        .domain
        .entities
        .get(&id)
        .ok_or_else(|| invalid(path, "axis entity is absent"))?;
    if header(id, record)?.0 != expected {
        return Err(invalid(path, "axis entity has the wrong kind"));
    }
    Ok(record)
}

pub(super) fn availability_window(
    document: &ScenarioDocument,
    parameters: &AvailabilityWindowParametersV1,
    position: Option<WorkforcePositionV1>,
    context: SetupViewContext,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    budget.visit()?;
    check_dates(parameters.dates)?;
    let cursor = match position {
        None => None,
        Some(WorkforcePositionV1::Ordinal { next_ordinal }) => Some(next_ordinal),
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "availability requires a next ordinal",
            ));
        }
    };
    let mut page = PageBuilder::new(parameters.limit, ORDINARY_DATA_BYTES)?;
    let person_id = EntityId::from_uuid(parameters.person_id.as_uuid());
    axis_record(
        document,
        person_id,
        WorkforceEntityKindV1::Person,
        "/query/parameters/personId",
    )?;
    let mut authority =
        OperationBudget::analysis(Some(budget.control()), PlanningIrLimitsV1::DEFAULT);
    let query = date_range(parameters.dates, &document.settings, person_id)
        .map_err(|error| operation_error(&error))?;
    let schemas = WorkforceSchemas::load()?;
    let mut ordinal = 0_u32;
    // EntityId and AvailabilityId share UUID ordering. The interval authority supplies
    // sorted, deduplicated, fully clipped intervals within each record.
    for (&id, record) in &document.domain.entities {
        budget.visit()?;
        if header(id, record)?.0 != WorkforceEntityKindV1::Availability {
            continue;
        }
        let owner = PersonId::deserialize(
            record
                .get("personId")
                .ok_or_else(|| invalid("/domain/entities", "availability person is absent"))?,
        )
        .map_err(|_| invalid("/domain/entities", "availability person is invalid"))?;
        if owner != parameters.person_id {
            continue;
        }
        charge_record(record, 0, budget, &mut authority)?;
        let bytes = authority
            .measure(record)
            .map_err(|error| operation_error(&error))?;
        authority
            .reserve(1, 0, bytes)
            .map_err(|error| operation_error(&error))?;
        schemas
            .entities
            .validate(record, ContractJsonLimits::DEFAULT)?;
        let WorkforceEntity::Availability(availability) = WorkforceEntity::deserialize(record)
            .map_err(|_| invalid("/domain/entities", "availability record is invalid"))?
        else {
            return Err(invalid("/domain/entities", "availability kind is invalid"));
        };
        let resolved =
            availability_intervals(&availability, query, &document.settings, &mut authority)
                .map_err(|error| operation_error(&error))?;
        for interval in resolved.intervals {
            budget.visit()?;
            let next_ordinal = ordinal
                .checked_add(1)
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            page.observe(cursor.is_none_or(|cursor| ordinal >= cursor), || {
                Ok((
                    AvailabilityOccurrenceV1 {
                        ordinal,
                        availability_id: availability.id,
                        availability_kind: availability.availability_kind,
                        interval: InstantIntervalV1 {
                            start: interval.start,
                            end: interval.end,
                        },
                    },
                    WorkforcePositionV1::Ordinal { next_ordinal },
                ))
            })?;
            ordinal = next_ordinal;
        }
    }
    if cursor.is_some_and(|cursor| cursor > ordinal) {
        return Err(invalid(
            "/query/continuation/position",
            "continued availability occurrence is absent",
        ));
    }
    budget.control().check()?;
    page.finish(
        document.scenario_id,
        context,
        WorkforceSetupViewDataV1::AvailabilityWindow,
    )
}

// Bound source traversal and decoded retention before schema validation or allocation.
// The depth ceiling is the existing contract ceiling, not a new record format.
fn charge_record(
    value: &Value,
    depth: usize,
    projection: &mut ProjectionBudget<'_>,
    authority: &mut OperationBudget<'_>,
) -> Result<()> {
    projection.visit()?;
    if depth > ContractJsonLimits::DEFAULT.max_depth {
        return Err(DomainPackError::ResourceLimitExceeded);
    }
    authority
        .reserve(0, 1, 0)
        .map_err(|error| operation_error(&error))?;
    match value {
        Value::Array(values) => {
            for value in values {
                charge_record(value, depth + 1, projection, authority)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                charge_record(value, depth + 1, projection, authority)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::AvailabilityKind, test_support as support};
    use eutheto_types::{
        CancellationToken, DurationMillis, FixedMonotonicClock, OperationControl,
        ParentSolveBudget, Revision,
    };
    use serde_json::json;
    use std::{error::Error, sync::Arc, time::Duration};

    type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

    fn context() -> SetupViewContext {
        SetupViewContext {
            revision: Revision::INITIAL,
            query_fingerprint: [7; 32],
        }
    }

    #[test]
    fn matrix_preserves_unique_axis_order_and_configured_not_feasible_membership() -> TestResult {
        let mut document = support::fixture()?;
        let person_id: EntityId = support::id(1).parse()?;
        let type_id: EntityId = support::id(4).parse()?;
        let mut person = document
            .domain
            .entities
            .get(&person_id)
            .ok_or("person")?
            .clone();
        person["id"] = json!(support::id(12));
        person["eligibleAssignmentTypeIds"] = json!([support::id(13)]);
        document
            .domain
            .entities
            .insert(support::id(12).parse()?, person);
        let mut assignment_type = document
            .domain
            .entities
            .get(&type_id)
            .ok_or("type")?
            .clone();
        assignment_type["id"] = json!(support::id(13));
        document
            .domain
            .entities
            .insert(support::id(13).parse()?, assignment_type);
        // A missing required qualification must not change configured membership.
        document
            .domain
            .entities
            .get_mut(&person_id)
            .ok_or("person")?["qualificationGrants"] = json!([]);
        let parameters = EligibilityMatrixParametersV1 {
            person_ids: vec![support::id(12).parse()?, support::id(1).parse()?],
            assignment_type_ids: vec![support::id(4).parse()?, support::id(13).parse()?],
        };
        let control = OperationControl::Cancellation(CancellationToken::new());
        let WorkforceSetupViewDataV1::EligibilityMatrix(matrix) = eligibility_matrix(
            &document,
            &parameters,
            None,
            &mut ProjectionBudget::new(&control),
        )?
        else {
            return Err("wrong matrix result".into());
        };
        assert_eq!(matrix.person_ids, parameters.person_ids);
        assert_eq!(matrix.assignment_type_ids, parameters.assignment_type_ids);
        assert_eq!(
            matrix.configured_memberships,
            vec![vec![false, true], vec![true, false]]
        );

        for (people, types) in [
            (vec![1, 1], vec![4]),
            (vec![1], vec![4, 4]),
            (vec![99], vec![4]),
            (vec![4], vec![4]),
            (vec![1], vec![99]),
            (vec![1], vec![1]),
        ] {
            let invalid_axes = EligibilityMatrixParametersV1 {
                person_ids: people
                    .into_iter()
                    .map(|id| support::id(id).parse())
                    .collect::<std::result::Result<_, _>>()?,
                assignment_type_ids: types
                    .into_iter()
                    .map(|id| support::id(id).parse())
                    .collect::<std::result::Result<_, _>>()?,
            };
            assert!(matches!(
                eligibility_matrix(
                    &document,
                    &invalid_axes,
                    None,
                    &mut ProjectionBudget::new(&control)
                ),
                Err(DomainPackError::InvalidPayload { .. })
            ));
        }
        assert!(matches!(
            eligibility_matrix(
                &document,
                &parameters,
                Some(WorkforcePositionV1::Ordinal { next_ordinal: 1 }),
                &mut ProjectionBudget::new(&control)
            ),
            Err(DomainPackError::InvalidPayload { .. })
        ));
        Ok(())
    }

    #[test]
    fn matrix_charges_source_memberships_even_outside_requested_columns() -> TestResult {
        let mut document = support::fixture()?;
        let person: EntityId = support::id(1).parse()?;
        document.domain.entities.get_mut(&person).ok_or("person")?["eligibleAssignmentTypeIds"] =
            Value::Array(vec![
                json!(support::id(4));
                super::super::contracts::MAX_PROJECTION_VISITS as usize
            ]);
        let parameters = EligibilityMatrixParametersV1 {
            person_ids: vec![support::id(1).parse()?],
            assignment_type_ids: Vec::new(),
        };
        let control = OperationControl::Cancellation(CancellationToken::new());
        assert!(matches!(
            eligibility_matrix(
                &document,
                &parameters,
                None,
                &mut ProjectionBudget::new(&control)
            ),
            Err(DomainPackError::ResourceLimitExceeded)
        ));
        Ok(())
    }

    fn availability_document() -> TestResult<ScenarioDocument> {
        let mut document = support::fixture()?;
        document.settings.time_zone = "UTC".parse()?;
        let overnight = json!({
            "weekdays":["saturday"], "startTime":"23:00:00", "endTime":"02:00:00", "endDayOffset":1
        });
        let records = [
            json!({
                "kind":"availability", "id":support::id(40), "personId":support::id(1),
                "availabilityKind":"availableOnly", "source":"test", "note":"",
                "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
                "timeWindow":{"kind":"weekly","windows":[overnight.clone(), overnight, {
                    "weekdays":["sunday"], "startTime":"22:00:00", "endTime":"06:00:00", "endDayOffset":1
                }]}
            }),
            instant_record(41, "requestedTimeOff"),
            instant_record(42, "approvedTimeOff"),
            instant_record(43, "unavailable"),
        ];
        for record in records {
            let id = EntityId::deserialize(&record["id"])?;
            document.domain.entities.insert(id, record);
        }
        Ok(document)
    }

    fn instant_record(id: u32, kind: &str) -> Value {
        json!({
            "kind":"availability", "id":support::id(id), "personId":support::id(1),
            "availabilityKind":kind, "source":"test", "note":"",
            "effectiveRange":{"startDate":"2026-11-01","endDateExclusive":"2026-11-02"},
            "timeWindow":{"kind":"instant","startsAt":"2026-11-01T01:00:00Z","endsAt":"2026-11-01T03:00:00Z"}
        })
    }

    fn availability_parameters(limit: u16) -> TestResult<AvailabilityWindowParametersV1> {
        Ok(AvailabilityWindowParametersV1 {
            person_id: support::id(1).parse()?,
            dates: serde_json::from_value(json!({
                "startDate":"2026-11-01", "endDateExclusive":"2026-11-03"
            }))?,
            limit,
        })
    }

    #[test]
    fn availability_clips_deduplicates_and_pages_across_records_with_absolute_ordinals()
    -> TestResult {
        let document = availability_document()?;
        let control = OperationControl::Cancellation(CancellationToken::new());
        let parameters = availability_parameters(2)?;
        let WorkforceSetupViewDataV1::AvailabilityWindow(first) = availability_window(
            &document,
            &parameters,
            None,
            context(),
            &mut ProjectionBudget::new(&control),
        )?
        else {
            return Err("wrong availability result".into());
        };
        assert_eq!(first.total_items, 5);
        assert_eq!(
            serde_json::to_value(&first.items)?,
            json!([
                {"ordinal":0,"availabilityId":support::id(40),"availabilityKind":"availableOnly",
                    "interval":{"start":"2026-11-01T00:00:00Z","end":"2026-11-01T02:00:00Z"}},
                {"ordinal":1,"availabilityId":support::id(40),"availabilityKind":"availableOnly",
                    "interval":{"start":"2026-11-01T22:00:00Z","end":"2026-11-02T00:00:00Z"}}
            ])
        );
        let cursor = first.continuation.ok_or("missing continuation")?;
        let WorkforceSetupViewDataV1::AvailabilityWindow(rest) = availability_window(
            &document,
            &availability_parameters(200)?,
            Some(WorkforcePositionV1::deserialize(&cursor.position)?),
            context(),
            &mut ProjectionBudget::new(&control),
        )?
        else {
            return Err("wrong availability result".into());
        };
        assert_eq!(rest.total_items, 5);
        assert_eq!(
            rest.items
                .iter()
                .map(|row| (row.ordinal, row.availability_id, row.availability_kind))
                .collect::<Vec<_>>(),
            vec![
                (
                    2,
                    support::id(41).parse()?,
                    AvailabilityKind::RequestedTimeOff
                ),
                (
                    3,
                    support::id(42).parse()?,
                    AvailabilityKind::ApprovedTimeOff
                ),
                (4, support::id(43).parse()?, AvailabilityKind::Unavailable),
            ]
        );
        assert!(rest.continuation.is_none());
        let WorkforceSetupViewDataV1::AvailabilityWindow(restarted) = availability_window(
            &document,
            &parameters,
            Some(WorkforcePositionV1::Ordinal { next_ordinal: 0 }),
            context(),
            &mut ProjectionBudget::new(&control),
        )?
        else {
            return Err("wrong availability result".into());
        };
        assert_eq!(
            serde_json::to_value(restarted.items)?,
            serde_json::to_value(first.items)?
        );
        let WorkforceSetupViewDataV1::AvailabilityWindow(exhausted) = availability_window(
            &document,
            &parameters,
            Some(WorkforcePositionV1::Ordinal { next_ordinal: 5 }),
            context(),
            &mut ProjectionBudget::new(&control),
        )?
        else {
            return Err("wrong availability result".into());
        };
        assert_eq!(exhausted.total_items, 5);
        assert!(exhausted.items.is_empty());
        assert!(exhausted.continuation.is_none());
        for position in [
            WorkforcePositionV1::Ordinal { next_ordinal: 99 },
            WorkforcePositionV1::Person {
                person_id: parameters.person_id,
            },
        ] {
            assert!(matches!(
                availability_window(
                    &document,
                    &parameters,
                    Some(position),
                    context(),
                    &mut ProjectionBudget::new(&control)
                ),
                Err(DomainPackError::InvalidPayload { .. })
            ));
        }
        Ok(())
    }

    #[test]
    fn availability_preserves_real_cancellation_deadlines_and_temporal_errors() -> TestResult {
        let mut document = availability_document()?;
        let parameters = availability_parameters(2)?;
        let token = CancellationToken::new();
        let control = OperationControl::Cancellation(token.clone());
        token.cancel();
        assert!(matches!(
            availability_window(
                &document,
                &parameters,
                None,
                context(),
                &mut ProjectionBudget::new(&control)
            ),
            Err(DomainPackError::Cancelled)
        ));

        let clock = Arc::new(FixedMonotonicClock::new(Duration::ZERO));
        let parent = ParentSolveBudget::new(
            DurationMillis::new(100)?,
            clock.clone(),
            CancellationToken::new(),
        )?;
        let control = OperationControl::Solve(parent.phase_view());
        // A live solve control is supported; it must not be rejected as an unsupported mode.
        let WorkforceSetupViewDataV1::AvailabilityWindow(page) = availability_window(
            &document,
            &parameters,
            None,
            context(),
            &mut ProjectionBudget::new(&control),
        )?
        else {
            return Err("wrong availability result".into());
        };
        assert_eq!(page.total_items, 5);
        clock.advance(Duration::from_millis(100))?;
        assert!(matches!(
            availability_window(
                &document,
                &parameters,
                None,
                context(),
                &mut ProjectionBudget::new(&control)
            ),
            Err(DomainPackError::BudgetExpired)
        ));

        let control = OperationControl::Cancellation(CancellationToken::new());
        document.settings.time_zone = "America/New_York".parse()?;
        let id: EntityId = support::id(40).parse()?;
        document
            .domain
            .entities
            .get_mut(&id)
            .ok_or("availability")?["timeWindow"] = json!({
            "kind":"weekly","windows":[{
                "weekdays":["sunday"],"startTime":"01:15:00","endTime":"01:45:00","endDayOffset":0
            }]
        });
        assert!(matches!(
            availability_window(
                &document,
                &parameters,
                None,
                context(),
                &mut ProjectionBudget::new(&control)
            ),
            Err(DomainPackError::Contract(_))
        ));
        let mut invalid_dates = parameters;
        invalid_dates.dates.end_date_exclusive = invalid_dates.dates.start_date;
        assert!(matches!(
            availability_window(
                &document,
                &invalid_dates,
                None,
                context(),
                &mut ProjectionBudget::new(&control)
            ),
            Err(DomainPackError::InvalidPayload { .. })
        ));
        Ok(())
    }
}

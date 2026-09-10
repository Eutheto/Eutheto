use super::{
    contracts::{
        AssignmentInspectionParametersV1, AssignmentInspectionV1, InstantIntervalV1,
        ORDINARY_DATA_BYTES, PairRejectionV1, PersonSummaryV1, RejectionCauseV1, RuleClassV1,
        RuleScopeParametersV1, ScopeAxisV1, ScopeInspectionV1, ScopePartV1, ScopePopulationV1,
        WorkforcePositionV1, WorkforceSetupViewDataV1,
    },
    paging::{PageBuilder, ProjectionBudget, Result, invalid},
    work::shift_row,
};
use crate::{
    assignment_rules::{
        AssignmentRuleError, InstantInterval, RejectionCause,
        analysis::{
            analyze_with_budget,
            support::{person_scope, shift_scope},
        },
        budget::OperationBudget,
        input::AssignmentInput,
        operation_error,
    },
    model::{AssignmentPair, Scope, WorkforceDomainV1, WorkforceRule},
};
use eutheto_domain_api::{DomainPackError, SetupViewContext};
use eutheto_planning_ir::PlanningIrLimitsV1;
use eutheto_types::ScenarioDocument;

struct ScopeSelection<'a> {
    main: &'a Scope,
    sub: Option<&'a Scope>,
}

impl ScopeSelection<'_> {
    fn matches(
        &self,
        mut predicate: impl FnMut(&Scope) -> Result<bool, AssignmentRuleError>,
    ) -> Result<bool> {
        if !predicate(self.main).map_err(|error| operation_error(&error))? {
            return Ok(false);
        }
        self.sub.map_or(Ok(true), |sub| {
            predicate(sub).map_err(|error| operation_error(&error))
        })
    }
}

fn selected_scope<'a>(
    domain: &'a WorkforceDomainV1,
    parameters: &RuleScopeParametersV1,
) -> Result<ScopeSelection<'a>> {
    let missing = || {
        invalid(
            "/query/parameters/rule",
            "rule is absent from the requested class",
        )
    };
    let absent_part = || invalid("/query/parameters/part", "rule has no requested scope part");
    match parameters.rule.class {
        RuleClassV1::Required => {
            let rule = domain
                .rules
                .get(&parameters.rule.rule_id)
                .ok_or_else(missing)?;
            let main = rule.header().2;
            let sub = match (parameters.part, rule) {
                (ScopePartV1::Main, _) => None,
                (ScopePartV1::MinimumRestBefore, WorkforceRule::MinimumRest(rule)) => {
                    Some(&rule.before_scope)
                }
                (ScopePartV1::MinimumRestAfter, WorkforceRule::MinimumRest(rule)) => {
                    Some(&rule.after_scope)
                }
                _ => return Err(absent_part()),
            };
            Ok(ScopeSelection { main, sub })
        }
        RuleClassV1::Preference => {
            let preference = domain
                .preferences
                .get(&parameters.rule.rule_id)
                .ok_or_else(missing)?;
            match parameters.part {
                ScopePartV1::Main => Ok(ScopeSelection {
                    main: preference.header().2,
                    sub: None,
                }),
                ScopePartV1::MinimumRestBefore | ScopePartV1::MinimumRestAfter => {
                    Err(absent_part())
                }
            }
        }
    }
}

pub(super) fn rule_scope(
    document: &ScenarioDocument,
    parameters: &RuleScopeParametersV1,
    position: Option<WorkforcePositionV1>,
    context: SetupViewContext,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    let (person_cursor, shift_cursor) = match (parameters.axis, position) {
        (_, None) => (None, None),
        (ScopeAxisV1::People, Some(WorkforcePositionV1::Person { person_id })) => {
            (Some(person_id), None)
        }
        (ScopeAxisV1::Shifts, Some(WorkforcePositionV1::Shift { shift_id })) => {
            (None, Some(shift_id))
        }
        _ => {
            return Err(invalid(
                "/query/continuation/position",
                "scope continuation must match its population axis",
            ));
        }
    };
    let mut people_page = PageBuilder::new(parameters.limit, ORDINARY_DATA_BYTES)?;
    let mut shift_page = PageBuilder::new(parameters.limit, ORDINARY_DATA_BYTES)?;
    budget.visit()?;
    let mut authority =
        OperationBudget::analysis(Some(budget.control()), PlanningIrLimitsV1::DEFAULT);
    let input =
        AssignmentInput::new(document, &mut authority).map_err(|error| operation_error(&error))?;
    let scope = selected_scope(&input.domain, parameters)?;
    let mut people_count = 0_u32;
    let mut cursor_seen = person_cursor.is_none() && shift_cursor.is_none();
    for id in &input.people {
        budget.visit()?;
        let person = input
            .person(*id)
            .ok_or_else(|| invalid("/document", "resolved person is missing"))?;
        if !scope.matches(|part| person_scope(part, person, &mut authority))? {
            continue;
        }
        people_count = people_count
            .checked_add(1)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if matches!(parameters.axis, ScopeAxisV1::People) {
            cursor_seen |= person_cursor == Some(*id);
            people_page.observe(person_cursor.is_none_or(|cursor| *id > cursor), || {
                budget.visit()?;
                Ok((
                    PersonSummaryV1 {
                        person_id: *id,
                        name: person.name.clone(),
                    },
                    WorkforcePositionV1::Person { person_id: *id },
                ))
            })?;
        }
    }
    let (shift_count, shifts) =
        scoped_shifts(&input, &scope, parameters.axis, &mut authority, budget)?;
    let cartesian_pair_count = people_count
        .checked_mul(shift_count)
        .ok_or(DomainPackError::ResourceLimitExceeded)?;
    let wrap = |population| {
        WorkforceSetupViewDataV1::RuleScope(ScopeInspectionV1 {
            rule: parameters.rule,
            part: parameters.part,
            people_count,
            shift_count,
            cartesian_pair_count,
            population,
        })
    };
    match parameters.axis {
        ScopeAxisV1::People => {
            if !cursor_seen {
                return Err(invalid(
                    "/query/continuation/position",
                    "continuation person is absent from this scope",
                ));
            }
            people_page.finish(document.scenario_id, context, |page| {
                wrap(ScopePopulationV1::People(page))
            })
        }
        ScopeAxisV1::Shifts => {
            for shift in shifts {
                budget.visit()?;
                cursor_seen |= shift_cursor == Some(shift.id);
                shift_page.observe(shift_cursor.is_none_or(|cursor| shift.id > cursor), || {
                    budget.visit()?;
                    Ok((
                        shift_row(&input.domain, shift)?,
                        WorkforcePositionV1::Shift { shift_id: shift.id },
                    ))
                })?;
            }
            if !cursor_seen {
                return Err(invalid(
                    "/query/continuation/position",
                    "continuation shift is absent from this scope",
                ));
            }
            shift_page.finish(document.scenario_id, context, |page| {
                wrap(ScopePopulationV1::Shifts(page))
            })
        }
    }
}

// Only the shift population retains an ID-ordered borrowed index. The authority's
// chronological vector and its identity-to-position metadata remain unchanged.
fn scoped_shifts<'a>(
    input: &'a AssignmentInput,
    scope: &ScopeSelection<'_>,
    axis: ScopeAxisV1,
    authority: &mut OperationBudget<'_>,
    budget: &mut ProjectionBudget<'_>,
) -> Result<(u32, Vec<&'a crate::temporal::ResolvedShift>)> {
    let mut count = 0_u32;
    let mut shifts = Vec::new();
    for shift in &input.shifts {
        budget.visit()?;
        let metadata = input
            .metadata(shift)
            .map_err(|error| operation_error(&error))?;
        if !scope.matches(|part| shift_scope(part, shift, &metadata, authority))? {
            continue;
        }
        count = count
            .checked_add(1)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        if matches!(axis, ScopeAxisV1::Shifts) {
            authority
                .reserve(0, 1, size_of::<&crate::temporal::ResolvedShift>() as u64)
                .map_err(|error| operation_error(&error))?;
            shifts.push(shift);
        }
    }
    authority
        .sort_work(shifts.len())
        .map_err(|error| operation_error(&error))?;
    shifts.sort_unstable_by_key(|shift| shift.id);
    Ok((count, shifts))
}

pub(super) fn assignment_inspection(
    document: &ScenarioDocument,
    parameters: &AssignmentInspectionParametersV1,
    position: Option<WorkforcePositionV1>,
    context: SetupViewContext,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    let cursor = match position {
        None => None,
        Some(WorkforcePositionV1::Ordinal { next_ordinal }) => Some(next_ordinal),
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "assignment inspection requires a next ordinal",
            ));
        }
    };
    let mut page = PageBuilder::new(parameters.limit, ORDINARY_DATA_BYTES)?;
    budget.visit()?;
    let mut authority =
        OperationBudget::analysis(Some(budget.control()), PlanningIrLimitsV1::DEFAULT);
    let analysis = analyze_with_budget(document, &mut authority, PlanningIrLimitsV1::DEFAULT)
        .map_err(|error| operation_error(&error))?;
    let pair = AssignmentPair {
        person_id: parameters.person_id,
        shift_id: parameters.shift_id,
    };
    budget.visit()?;
    let candidate = analysis.candidates.binary_search(&pair).is_ok();
    let mut remaining_required_rule_count = 0_u32;
    for id in &analysis.obligations.remaining {
        budget.visit()?;
        if document.domain.rules.contains_key(id) {
            remaining_required_rule_count = remaining_required_rule_count
                .checked_add(1)
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
        }
    }
    let mut ordinal = 0_u32;
    for rejection in &analysis.rejections {
        budget.visit()?;
        if rejection.pair != pair {
            continue;
        }
        let next_ordinal = ordinal
            .checked_add(1)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        page.observe(cursor.is_none_or(|cursor| ordinal >= cursor), || {
            budget.visit()?;
            Ok((
                PairRejectionV1 {
                    ordinal,
                    binding_id: rejection.binding_id,
                    cause: rejection_cause(&rejection.cause),
                },
                WorkforcePositionV1::Ordinal { next_ordinal },
            ))
        })?;
        ordinal = next_ordinal;
    }
    // Existing analysis visits the complete person × active resolved shift product:
    // each pair is either retained as a candidate or has at least one rejection fact.
    // Thus this validates both identities without decoding/resolving the input twice.
    if !candidate && ordinal == 0 {
        return Err(invalid(
            "/query/parameters",
            "person or active planning-horizon shift is absent",
        ));
    }
    if cursor.is_some_and(|cursor| cursor > ordinal) {
        return Err(invalid(
            "/query/continuation/position",
            "rejection ordinal is beyond this pair's evidence",
        ));
    }
    page.finish(document.scenario_id, context, |rejections| {
        WorkforceSetupViewDataV1::AssignmentInspection(AssignmentInspectionV1 {
            person_id: pair.person_id,
            shift_id: pair.shift_id,
            candidate_in_implemented_assignment_graph: candidate,
            remaining_required_rule_count,
            rejections,
        })
    })
}

fn interval(value: InstantInterval) -> InstantIntervalV1 {
    InstantIntervalV1 {
        start: value.start,
        end: value.end,
    }
}

fn rejection_cause(cause: &RejectionCause) -> RejectionCauseV1 {
    match *cause {
        RejectionCause::OutsideActiveRange { allowed, outside } => {
            RejectionCauseV1::OutsideActiveRange {
                allowed: interval(allowed),
                outside: interval(outside),
            }
        }
        RejectionCause::AssignmentTypeNotAllowed { assignment_type_id } => {
            RejectionCauseV1::AssignmentTypeNotAllowed { assignment_type_id }
        }
        RejectionCause::QualificationExpression { assignment_type_id } => {
            RejectionCauseV1::QualificationExpression { assignment_type_id }
        }
        RejectionCause::Unavailable {
            availability_id,
            overlap,
        } => RejectionCauseV1::Unavailable {
            availability_id,
            overlap: interval(overlap),
        },
        RejectionCause::OutsideAvailableOnly {
            availability_id,
            uncovered,
        } => RejectionCauseV1::OutsideAvailableOnly {
            availability_id,
            uncovered: interval(uncovered),
        },
        RejectionCause::ApprovedTimeOff {
            availability_id,
            overlap,
        } => RejectionCauseV1::ApprovedTimeOff {
            availability_id,
            overlap: interval(overlap),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        setup::contracts::RuleReferenceV1,
        test_support::{fixture, id},
    };
    use eutheto_types::{CancellationToken, OperationControl, OverlapPolicy, Revision};
    use serde_json::json;
    use std::error::Error;

    type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

    fn document() -> TestResult<ScenarioDocument> {
        let mut document = fixture()?;
        document.settings.overlap_policy = OverlapPolicy::Earlier;
        document.domain.locked_assignments.clear();
        let mut second = document
            .domain
            .entities
            .get(&id(1).parse()?)
            .ok_or("person")?
            .clone();
        second["id"] = json!(id(12));
        second["name"] = json!("Ash");
        second["externalId"] = json!("staff-02");
        document.domain.entities.insert(id(12).parse()?, second);
        document.domain.rules.insert(
            id(20).parse()?,
            json!({
                "kind":"eligibility", "id":id(20), "active":true, "strength":"required",
                "scope":{"people":{"kind":"all"}},
            }),
        );
        Ok(document)
    }

    fn context() -> SetupViewContext {
        SetupViewContext {
            revision: Revision::INITIAL,
            query_fingerprint: [3; 32],
        }
    }

    fn scope(
        document: &ScenarioDocument,
        parameters: &RuleScopeParametersV1,
        position: Option<WorkforcePositionV1>,
    ) -> TestResult<ScopeInspectionV1> {
        let control = OperationControl::Cancellation(CancellationToken::new());
        match rule_scope(
            document,
            parameters,
            position,
            context(),
            &mut ProjectionBudget::new(&control),
        )? {
            WorkforceSetupViewDataV1::RuleScope(result) => Ok(result),
            _ => Err("wrong scope result".into()),
        }
    }

    fn inspect(
        document: &ScenarioDocument,
        person: u32,
        shift: u32,
        position: Option<WorkforcePositionV1>,
    ) -> TestResult<AssignmentInspectionV1> {
        let control = OperationControl::Cancellation(CancellationToken::new());
        let parameters = AssignmentInspectionParametersV1 {
            person_id: id(person).parse()?,
            shift_id: id(shift).parse()?,
            limit: 1,
        };
        match assignment_inspection(
            document,
            &parameters,
            position,
            context(),
            &mut ProjectionBudget::new(&control),
        )? {
            WorkforceSetupViewDataV1::AssignmentInspection(result) => Ok(result),
            _ => Err("wrong assignment result".into()),
        }
    }

    #[test]
    fn scope_counts_ignore_paging_and_candidate_exclusions() -> TestResult {
        let mut document = document()?;
        for person in [1, 12] {
            document
                .domain
                .entities
                .get_mut(&id(person).parse()?)
                .ok_or("person")?["eligibleAssignmentTypeIds"] = json!([]);
        }
        assert!(!inspect(&document, 1, 7, None)?.candidate_in_implemented_assignment_graph);
        let mut parameters = RuleScopeParametersV1 {
            rule: RuleReferenceV1 {
                class: RuleClassV1::Required,
                rule_id: id(20).parse()?,
            },
            part: ScopePartV1::Main,
            axis: ScopeAxisV1::People,
            limit: 1,
        };
        let first = scope(&document, &parameters, None)?;
        assert_eq!(
            (
                first.people_count,
                first.shift_count,
                first.cartesian_pair_count
            ),
            (2, 2, 4)
        );
        let ScopePopulationV1::People(page) = first.population else {
            return Err("people page".into());
        };
        assert_eq!(page.total_items, 2);
        assert_eq!(
            page.items
                .iter()
                .map(|row| row.person_id)
                .collect::<Vec<_>>(),
            vec![id(1).parse()?]
        );
        let position = serde_json::from_value(page.continuation.ok_or("first cursor")?.position)?;
        let second = scope(&document, &parameters, Some(position))?;
        assert_eq!(
            (
                second.people_count,
                second.shift_count,
                second.cartesian_pair_count
            ),
            (2, 2, 4)
        );
        let ScopePopulationV1::People(page) = second.population else {
            return Err("people page".into());
        };
        assert_eq!(page.total_items, 2);
        assert_eq!(
            page.items
                .iter()
                .map(|row| row.person_id)
                .collect::<Vec<_>>(),
            vec![id(12).parse()?]
        );
        assert!(page.continuation.is_none());
        parameters.axis = ScopeAxisV1::Shifts;
        let first = scope(&document, &parameters, None)?;
        let ScopePopulationV1::Shifts(page) = first.population else {
            return Err("shift page".into());
        };
        assert_eq!(page.total_items, 2);
        assert_eq!(
            page.items
                .iter()
                .map(|row| row.shift_id)
                .collect::<Vec<_>>(),
            vec![id(7).parse()?]
        );
        let position = serde_json::from_value(page.continuation.ok_or("shift cursor")?.position)?;
        let second = scope(&document, &parameters, Some(position))?;
        assert_eq!(
            (
                second.people_count,
                second.shift_count,
                second.cartesian_pair_count
            ),
            (2, 2, 4)
        );
        let ScopePopulationV1::Shifts(page) = second.population else {
            return Err("shift page".into());
        };
        assert_eq!(
            page.items
                .iter()
                .map(|row| row.shift_id)
                .collect::<Vec<_>>(),
            vec![id(8).parse()?]
        );
        assert!(page.continuation.is_none());
        Ok(())
    }

    #[test]
    fn scope_cursor_requires_an_existing_member_of_its_axis() -> TestResult {
        let document = document()?;
        let parameters = RuleScopeParametersV1 {
            rule: RuleReferenceV1 {
                class: RuleClassV1::Required,
                rule_id: id(20).parse()?,
            },
            part: ScopePartV1::Main,
            axis: ScopeAxisV1::Shifts,
            limit: 1,
        };
        assert!(
            scope(
                &document,
                &parameters,
                Some(WorkforcePositionV1::Person {
                    person_id: id(1).parse()?
                })
            )
            .is_err()
        );
        assert!(
            scope(
                &document,
                &parameters,
                Some(WorkforcePositionV1::Shift {
                    shift_id: id(99).parse()?
                })
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn minimum_rest_subscopes_intersect_main_people_and_shift_selections() -> TestResult {
        let mut document = document()?;
        document.domain.rules.insert(id(21).parse()?, json!({
            "kind":"minimumRest", "id":id(21), "active":true, "strength":"required",
            "scope":{"people":{"kind":"selected","personIds":[id(1)]},"categories":["absent"]},
            "beforeScope":{"people":{"kind":"selected","personIds":[id(12)]},"categories":["clinic"]},
            "afterScope":{"people":{"kind":"selected","personIds":[id(1)]},"categories":["clinic"]},
            "minimumMinutes":60
        }));
        for (part, expected) in [
            (ScopePartV1::MinimumRestBefore, (0, 0, 0)),
            (ScopePartV1::MinimumRestAfter, (1, 0, 0)),
        ] {
            let result = scope(
                &document,
                &RuleScopeParametersV1 {
                    rule: RuleReferenceV1 {
                        class: RuleClassV1::Required,
                        rule_id: id(21).parse()?,
                    },
                    part,
                    axis: ScopeAxisV1::People,
                    limit: 1,
                },
                None,
            )?;
            assert_eq!(
                (
                    result.people_count,
                    result.shift_count,
                    result.cartesian_pair_count
                ),
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn minimum_rest_projects_each_authored_scope_and_rejects_nonexistent_parts() -> TestResult {
        let mut document = document()?;
        document.domain.rules.insert(id(21).parse()?, json!({
            "kind":"minimumRest", "id":id(21), "active":true, "strength":"required",
            "scope":{"people":{"kind":"all"}},
            "beforeScope":{"people":{"kind":"selected","personIds":[id(1)]},"categories":["clinic"]},
            "afterScope":{"people":{"kind":"selected","personIds":[id(12)]},"categories":["absent"]},
            "minimumMinutes":60,
        }));
        let mut parameters = RuleScopeParametersV1 {
            rule: RuleReferenceV1 {
                class: RuleClassV1::Required,
                rule_id: id(21).parse()?,
            },
            part: ScopePartV1::MinimumRestBefore,
            axis: ScopeAxisV1::People,
            limit: 10,
        };
        let before = scope(&document, &parameters, None)?;
        assert_eq!(
            (
                before.people_count,
                before.shift_count,
                before.cartesian_pair_count
            ),
            (1, 2, 2)
        );
        let ScopePopulationV1::People(page) = before.population else {
            return Err("people page".into());
        };
        assert_eq!(
            page.items
                .iter()
                .map(|row| row.person_id)
                .collect::<Vec<_>>(),
            vec![id(1).parse()?]
        );
        parameters.part = ScopePartV1::MinimumRestAfter;
        let after = scope(&document, &parameters, None)?;
        assert_eq!(
            (
                after.people_count,
                after.shift_count,
                after.cartesian_pair_count
            ),
            (1, 0, 0)
        );
        let ScopePopulationV1::People(page) = after.population else {
            return Err("people page".into());
        };
        assert_eq!(
            page.items
                .iter()
                .map(|row| row.person_id)
                .collect::<Vec<_>>(),
            vec![id(12).parse()?]
        );
        assert!(
            scope(
                &document,
                &parameters,
                Some(WorkforcePositionV1::Person {
                    person_id: id(1).parse()?
                })
            )
            .is_err()
        );
        parameters.part = ScopePartV1::Main;
        let main = scope(&document, &parameters, None)?;
        assert_eq!(
            (
                main.people_count,
                main.shift_count,
                main.cartesian_pair_count
            ),
            (2, 2, 4)
        );
        parameters.rule.rule_id = id(20).parse()?;
        parameters.part = ScopePartV1::MinimumRestBefore;
        assert!(scope(&document, &parameters, None).is_err());
        parameters.rule.class = RuleClassV1::Preference;
        parameters.part = ScopePartV1::Main;
        assert!(scope(&document, &parameters, None).is_err());
        Ok(())
    }

    #[test]
    fn assignment_inspection_pages_existing_evidence_without_claiming_remaining_rules() -> TestResult
    {
        let mut document = document()?;
        document.domain.locked_assignments.insert(
            id(14).parse()?,
            json!({"id":id(14), "personId":id(12), "shiftId":id(7), "state":{"kind":"hard"}}),
        );
        assert_eq!(
            inspect(&document, 12, 7, None)?.remaining_required_rule_count,
            0
        );
        document.domain.rules.insert(
            id(21).parse()?,
            json!({
                "kind":"maximumConsecutive", "id":id(21), "active":true, "strength":"required",
                "scope":{"people":{"kind":"all"}}, "mode":{"kind":"workedDays"}, "maximum":1,
            }),
        );
        let person = document
            .domain
            .entities
            .get_mut(&id(1).parse()?)
            .ok_or("person")?;
        person["eligibleAssignmentTypeIds"] = json!([]);
        person["qualificationGrants"] = json!([]);
        let first = inspect(
            &document,
            1,
            7,
            Some(WorkforcePositionV1::Ordinal { next_ordinal: 0 }),
        )?;
        assert!(!first.candidate_in_implemented_assignment_graph);
        assert_eq!(first.remaining_required_rule_count, 1);
        assert_eq!(first.rejections.total_items, 2);
        let first_row = first.rejections.items.first().ok_or("first rejection")?;
        assert_eq!(first_row.ordinal, 0);
        assert_eq!(first_row.binding_id, id(20).parse()?);
        assert!(
            matches!(first_row.cause, RejectionCauseV1::AssignmentTypeNotAllowed { assignment_type_id } if assignment_type_id == id(4).parse()?)
        );
        let position = serde_json::from_value(
            first
                .rejections
                .continuation
                .ok_or("rejection cursor")?
                .position,
        )?;
        let second = inspect(&document, 1, 7, Some(position))?;
        assert!(!second.candidate_in_implemented_assignment_graph);
        assert_eq!(second.remaining_required_rule_count, 1);
        assert_eq!(second.rejections.total_items, 2);
        let second_row = second.rejections.items.first().ok_or("second rejection")?;
        assert_eq!(second_row.ordinal, 1);
        assert_eq!(second_row.binding_id, id(20).parse()?);
        assert!(
            matches!(second_row.cause, RejectionCauseV1::QualificationExpression { assignment_type_id } if assignment_type_id == id(4).parse()?)
        );
        assert!(second.rejections.continuation.is_none());
        let exhausted = inspect(
            &document,
            1,
            7,
            Some(WorkforcePositionV1::Ordinal { next_ordinal: 2 }),
        )?;
        assert_eq!(exhausted.rejections.total_items, 2);
        assert!(exhausted.rejections.items.is_empty());
        assert!(exhausted.rejections.continuation.is_none());
        assert!(
            inspect(
                &document,
                1,
                7,
                Some(WorkforcePositionV1::Ordinal { next_ordinal: 3 })
            )
            .is_err()
        );
        let candidate = inspect(
            &document,
            12,
            7,
            Some(WorkforcePositionV1::Ordinal { next_ordinal: 0 }),
        )?;
        assert!(candidate.candidate_in_implemented_assignment_graph);
        assert_eq!(candidate.remaining_required_rule_count, 1);
        assert_eq!(candidate.rejections.total_items, 0);
        assert!(candidate.rejections.items.is_empty());
        assert!(candidate.rejections.continuation.is_none());
        for (person, shift) in [(99, 7), (4, 7), (1, 99), (1, 6)] {
            assert!(inspect(&document, person, shift, None).is_err());
        }
        assert!(
            inspect(
                &document,
                1,
                7,
                Some(WorkforcePositionV1::Shift {
                    shift_id: id(7).parse()?
                })
            )
            .is_err()
        );
        Ok(())
    }
}

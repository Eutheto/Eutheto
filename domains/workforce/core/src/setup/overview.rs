use super::contracts::{
    EntityKindCountV1, RuleClassV1, WorkforceEntityKindV1, WorkforcePositionV1,
    WorkforceSetupFactsV1, WorkforceSetupViewDataV1,
};
use super::paging::{ProjectionBudget, Result, invalid};
use super::{entities, rules};
use crate::ids::AssignmentTypeId;
use crate::model::{AssignmentLock, DateRange, planning_dates};
use crate::validation::MAX_REFERENCE_ITEMS;
use eutheto_domain_api::DomainPackError;
use eutheto_types::ScenarioDocument;
use jiff::{Span, civil::Time};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

const ENTITY_KINDS: [WorkforceEntityKindV1; 13] = [
    WorkforceEntityKindV1::Person,
    WorkforceEntityKindV1::Qualification,
    WorkforceEntityKindV1::Team,
    WorkforceEntityKindV1::Location,
    WorkforceEntityKindV1::WorkloadBucket,
    WorkforceEntityKindV1::Calendar,
    WorkforceEntityKindV1::AssignmentType,
    WorkforceEntityKindV1::ShiftTemplate,
    WorkforceEntityKindV1::ShiftInstance,
    WorkforceEntityKindV1::Availability,
    WorkforceEntityKindV1::CoverageRequirement,
    WorkforceEntityKindV1::BaseSchedule,
    WorkforceEntityKindV1::ScorePolicy,
];

pub(super) fn facts(
    document: &ScenarioDocument,
    position: Option<WorkforcePositionV1>,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    if position.is_some() {
        return Err(invalid(
            "/query/continuation",
            "setup overview does not accept continuation",
        ));
    }
    let (planning_dates, initial_work_window) = date_windows(document, budget)?;
    let mut facts = WorkforceSetupFactsV1 {
        settings: document.settings.clone(),
        planning_dates,
        initial_work_window,
        entities: ENTITY_KINDS
            .into_iter()
            .map(|kind| EntityKindCountV1 { kind, count: 0 })
            .collect(),
        required_rules: 0,
        active_required_rules: 0,
        preferences: 0,
        active_preferences: 0,
        locked_assignments: 0,
        configured_type_memberships: 0,
    };
    for (&id, record) in &document.domain.entities {
        budget.visit()?;
        let (kind, _) = entities::header(id, record)?;
        let count = facts
            .entities
            .iter_mut()
            .find(|entry| entry.kind == kind)
            .ok_or_else(|| invalid("/domain/entities", "entity kind has no overview counter"))?;
        increment(&mut count.count)?;
        if kind == WorkforceEntityKindV1::Person {
            let memberships = record
                .get("eligibleAssignmentTypeIds")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    invalid(
                        "/domain/entities",
                        "person configured assignment type memberships are invalid",
                    )
                })?;
            if memberships.len() > MAX_REFERENCE_ITEMS {
                return Err(invalid(
                    "/domain/entities",
                    "person configured assignment type memberships exceed their bound",
                ));
            }
            let mut seen = BTreeSet::new();
            for membership in memberships {
                budget.visit()?;
                let id = AssignmentTypeId::deserialize(membership).map_err(|_| {
                    invalid(
                        "/domain/entities",
                        "configured assignment type identity is invalid",
                    )
                })?;
                if !seen.insert(id) {
                    return Err(invalid(
                        "/domain/entities",
                        "configured assignment type membership is duplicated",
                    ));
                }
                // Count stored membership, including unresolved references. This is not
                // an eligibility evaluation against dates, skills, activity or rules.
                increment(&mut facts.configured_type_memberships)?;
            }
        }
    }
    for (&id, record) in &document.domain.rules {
        budget.visit()?;
        let header = rules::header(id, RuleClassV1::Required, record)?;
        increment(&mut facts.required_rules)?;
        if header.active {
            increment(&mut facts.active_required_rules)?;
        }
    }
    for (&id, record) in &document.domain.preferences {
        budget.visit()?;
        let header = rules::header(id, RuleClassV1::Preference, record)?;
        increment(&mut facts.preferences)?;
        if header.active {
            increment(&mut facts.active_preferences)?;
        }
    }
    for (&id, record) in &document.domain.locked_assignments {
        budget.visit()?;
        let lock = AssignmentLock::deserialize(record).map_err(|_| {
            invalid(
                "/domain/lockedAssignments",
                "assignment lock has an invalid typed record",
            )
        })?;
        if lock.id != id {
            return Err(invalid(
                "/domain/lockedAssignments",
                "assignment lock identity does not match its map key",
            ));
        }
        // This is the stored map count, including explicit Unlocked records, not a
        // claim that hard/soft locks have executable compiler support.
        increment(&mut facts.locked_assignments)?;
    }
    budget.visit()?;
    Ok(WorkforceSetupViewDataV1::Overview(facts))
}

fn date_windows(
    document: &ScenarioDocument,
    budget: &mut ProjectionBudget<'_>,
) -> Result<(DateRange, DateRange)> {
    budget.visit()?;
    let dates = planning_dates(&document.settings)?;
    let end_date_exclusive = dates
        .last_date
        .checked_add(Span::new().days(1))
        .map_err(|_| {
            invalid(
                "/settings/horizon",
                "planning date boundary exceeds its range",
            )
        })?;
    let planning_dates = DateRange {
        start_date: dates.first_date,
        end_date_exclusive,
    };
    let days = dates
        .first_date
        .to_datetime(Time::MIN)
        .duration_until(end_date_exclusive.to_datetime(Time::MIN))
        .as_secs()
        / 86_400;
    let initial_work_window = DateRange {
        start_date: dates.first_date,
        end_date_exclusive: dates
            .first_date
            .checked_add(Span::new().days(days.min(7)))
            .map_err(|_| {
                invalid(
                    "/settings/horizon",
                    "work window boundary exceeds its range",
                )
            })?,
    };
    Ok((planning_dates, initial_work_window))
}

fn increment(count: &mut u32) -> Result<()> {
    *count = count
        .checked_add(1)
        .ok_or(DomainPackError::ResourceLimitExceeded)?;
    Ok(())
}

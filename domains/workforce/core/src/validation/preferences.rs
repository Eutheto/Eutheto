use super::{
    common::{Result, require, unique, weight},
    context::Context,
    time::time_window,
};
use crate::ids::AssignmentTypeId;
use crate::model::{
    AvailabilityKind, CalendarPeriod, ConsecutiveWorkMode, Coverage, PersonSelection,
    WorkforcePreference,
};

impl Context<'_> {
    pub(super) fn preference(&self, preference: &WorkforcePreference) -> Result {
        let (_, active, scope, _, preference_weight) = preference.header();
        self.scope(scope, active)?;
        weight(preference_weight, "weight")?;
        match preference {
            WorkforcePreference::Time {
                time_window: window,
                ..
            } => time_window(window)?,
            WorkforcePreference::AssignmentType {
                assignment_type_ids,
                ..
            } => {
                unique(assignment_type_ids, active, "assignmentTypeIds")?;
                for id in assignment_type_ids {
                    self.assignment_type(*id)?;
                }
            }
            WorkforcePreference::Location { location_ids, .. } => {
                unique(location_ids, active, "locationIds")?;
                for id in location_ids {
                    self.location(*id)?;
                }
            }
            WorkforcePreference::RequestedTimeOff {
                availability_ids, ..
            } => {
                unique(availability_ids, active, "availabilityIds")?;
                for id in availability_ids {
                    require(
                        self.availability(*id)?.availability_kind
                            == AvailabilityKind::RequestedTimeOff,
                        "availabilityIds",
                        "requested-time-off preferences require requested-time-off records",
                    )?;
                }
            }
            WorkforcePreference::AssignmentTarget { calendar_id, .. } => {
                self.calendar(*calendar_id)?;
            }
            WorkforcePreference::WorkloadBalance {
                workload_policy_id, ..
            } => {
                require(
                    matches!(scope.people, PersonSelection::All {}) && scope.team_ids.is_none(),
                    "scope",
                    "workload policy must be the sole population authority",
                )?;
                require(
                    self.score_policy.is_some_and(|score| {
                        score.workload_policies.contains_key(workload_policy_id)
                    }),
                    "workloadPolicyId",
                    "reference does not resolve to a workload policy",
                )?;
            }
            WorkforcePreference::ConsecutiveWork {
                mode,
                assignment_type_ids,
                ..
            } => {
                self.consecutive_preference(mode, assignment_type_ids, active)?;
            }
            WorkforcePreference::Adjacency {
                base_schedule_id, ..
            }
            | WorkforcePreference::BaseStability {
                base_schedule_id, ..
            } => {
                self.base_schedule(*base_schedule_id)?;
            }
            WorkforcePreference::TogetherSeparate { person_ids, .. } => {
                unique(person_ids, active, "personIds")?;
                for id in person_ids {
                    self.person(*id)?;
                }
            }
            WorkforcePreference::PreferredCoverage {
                coverage_requirement_ids,
                ..
            } => {
                unique(coverage_requirement_ids, active, "coverageRequirementIds")?;
                for id in coverage_requirement_ids {
                    require(
                        matches!(
                            self.coverage_requirement(*id)?.coverage,
                            Coverage::AtLeast {
                                preferred_count: Some(_),
                                ..
                            }
                        ),
                        "coverageRequirementIds",
                        "preferred coverage requires an explicit preferred count",
                    )?;
                }
            }
        }
        Ok(())
    }

    fn consecutive_preference(
        &self,
        mode: &ConsecutiveWorkMode,
        assignment_type_ids: &[AssignmentTypeId],
        active: bool,
    ) -> Result {
        unique(assignment_type_ids, active, "assignmentTypeIds")?;
        for id in assignment_type_ids {
            self.assignment_type(*id)?;
        }
        if let ConsecutiveWorkMode::Weekends {
            calendar_id,
            weekdays,
        } = mode
        {
            require(
                matches!(
                    self.calendar(*calendar_id)?.period,
                    CalendarPeriod::Week { .. }
                ),
                "calendarId",
                "weekend sequences require an anchored-week calendar",
            )?;
            unique(weekdays, active, "weekdays")?;
        }
        Ok(())
    }
}

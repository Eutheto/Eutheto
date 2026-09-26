use super::{
    common::{Result, prefix, require, token, unique},
    context::Context,
};
use crate::model::{WindowMembership, WorkWindow, WorkforceRule, WorkloadMeasurement};

impl Context<'_> {
    pub(super) fn rule(&self, rule: &WorkforceRule) -> Result {
        let (_, active, scope) = rule.header();
        prefix(self.scope(scope, active), "scope")?;
        match rule {
            WorkforceRule::Eligibility { .. }
            | WorkforceRule::Availability { .. }
            | WorkforceRule::Coverage { .. }
            | WorkforceRule::MaximumConsecutive { .. } => {}
            WorkforceRule::NoOverlap {
                compatible_category_pairs,
                ..
            } => {
                unique(compatible_category_pairs, false, "compatibleCategoryPairs")?;
                for (idx, pair) in compatible_category_pairs.iter().enumerate() {
                    prefix(
                        token(&pair.first_category, "firstCategory"),
                        format_args!("compatibleCategoryPairs.{idx}"),
                    )?;
                    prefix(
                        token(&pair.second_category, "secondCategory"),
                        format_args!("compatibleCategoryPairs.{idx}"),
                    )?;
                    prefix(
                        require(
                            pair.first_category <= pair.second_category,
                            "firstCategory",
                            "symmetric categories must be in lexical order",
                        ),
                        format_args!("compatibleCategoryPairs.{idx}"),
                    )?;
                }
            }
            WorkforceRule::MinimumRest(value) => {
                prefix(self.scope(&value.after_scope, active), "afterScope")?;
                prefix(self.scope(&value.before_scope, active), "beforeScope")?;
            }
            WorkforceRule::MaximumHours {
                bucket_id, window, ..
            } => {
                require(
                    self.bucket(*bucket_id)?.measurement != WorkloadMeasurement::AssignmentCount,
                    "bucketId",
                    "hours require a minute-measurement bucket",
                )?;
                match *window {
                    WorkWindow::Calendar {
                        calendar_id,
                        membership,
                    } => self.workload_membership(*bucket_id, calendar_id, membership)?,
                    WorkWindow::Rolling {
                        duration_minutes,
                        membership,
                    } => {
                        require(
                            duration_minutes > 0,
                            "durationMinutes",
                            "rolling window must have positive duration",
                        )?;
                        require(
                            membership != WindowMembership::ReportingDate,
                            "membership",
                            "reporting-date membership requires a calendar",
                        )?;
                        self.bucket_membership(*bucket_id, membership)?;
                    }
                }
            }
            WorkforceRule::MaximumAssignmentCount { calendar_id, .. } => {
                self.calendar(*calendar_id)?;
            }
            WorkforceRule::RequiredSkillMix {
                qualification_minimums,
                ..
            } => self.qualification_minimums(qualification_minimums)?,
            WorkforceRule::FixedAssignment {
                person_id,
                shift_id,
                ..
            } => {
                self.person(*person_id)?;
                self.shift(*shift_id)?;
            }
            WorkforceRule::MutualAssignmentRestriction { person_ids, .. } => {
                unique(person_ids, active, "personIds")?;
                for id in person_ids {
                    self.person(*id)?;
                }
            }
            WorkforceRule::TransitionTime { location_ids, .. } => {
                unique(location_ids, active, "locationIds")?;
                for id in location_ids {
                    self.location(*id)?;
                }
            }
        }
        Ok(())
    }
}

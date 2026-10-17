use super::{
    common::{Result, prefix, require, token, unique},
    context::Context,
};
use crate::model::{WindowMembership, WorkWindow, WorkforceRule, WorkloadMeasurement};

impl Context<'_> {
    #[allow(clippy::too_many_lines)]
    pub(super) fn rule(&self, rule: &WorkforceRule) -> Result {
        let (_, active, scope) = rule.header();
        prefix(self.scope(scope, active), "scope")?;
        match rule {
            WorkforceRule::Eligibility { .. }
            | WorkforceRule::Availability { .. }
            | WorkforceRule::Coverage { .. }
            | WorkforceRule::MaximumConsecutive { .. }
            | WorkforceRule::MaximumWorkedWeekends { .. } => {}
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
            WorkforceRule::WorkloadQuota(quota) => {
                require(
                    quota.maximum.is_none_or(|maximum| maximum >= quota.minimum),
                    "maximum",
                    "maximum must be greater than or equal to minimum",
                )?;
                match quota.measure {
                    crate::model::WorkloadQuotaMeasure::AssignmentCount {} => {
                        require(
                            quota.membership != WindowMembership::Intersection,
                            "membership",
                            "assignment-count quotas support reporting-date or start-instant membership",
                        )?;
                        self.calendar(quota.calendar_id)?;
                    }
                    crate::model::WorkloadQuotaMeasure::BucketMinutes { bucket_id } => {
                        let bucket = self.bucket(bucket_id)?;
                        require(
                            bucket.measurement != WorkloadMeasurement::AssignmentCount,
                            "bucketId",
                            "minute quotas require a minute-measurement bucket",
                        )?;
                        require(
                            bucket.overlapping_contribution
                                != crate::model::OverlappingContribution::Union
                                || bucket.measurement == WorkloadMeasurement::ElapsedMinutes,
                            "bucketId",
                            "union quotas require elapsed-minute measurement",
                        )?;
                        self.workload_membership(bucket_id, quota.calendar_id, quota.membership)?;
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
                if active {
                    self.fixed_target_scope(*person_id, *shift_id, scope)?;
                }
            }
            WorkforceRule::MutualAssignmentRestriction { person_ids, .. } => {
                unique(person_ids, false, "personIds")?;
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

use super::{
    common::{Result, bounded, note, require, tags, text, token, unique, weight},
    context::Context,
    time, validate_score_policy_shape,
};
use crate::ids::{AssignmentTypeId, LocationId, WorkCalendarId, WorkloadBucketId};
use crate::model::{
    ActiveRange, Availability, Coverage, LocationBehavior, OverlappingContribution, Person,
    QualificationExpression, QualificationMatch, QualificationMinimum, ShiftOrigin,
    WindowMembership, WorkforceEntity, WorkloadMeasurement,
};
use std::collections::BTreeSet;

impl Context<'_> {
    pub(super) fn entity(&self, entity: &WorkforceEntity) -> Result {
        match entity {
            WorkforceEntity::Person(person) => self.person_record(person),
            WorkforceEntity::Qualification(value) => {
                text(&value.name, "name")?;
                note(&value.description, "description")
            }
            WorkforceEntity::Team(value) => text(&value.name, "name"),
            WorkforceEntity::Location(value) => {
                text(&value.name, "name")?;
                bounded(&value.transitions, false, "transitions")?;
                let mut destinations = BTreeSet::new();
                for transition in &value.transitions {
                    require(
                        destinations.insert(transition.location_id),
                        "transitions",
                        "destinations must be unique",
                    )?;
                    self.location(transition.location_id)?;
                }
                Ok(())
            }
            WorkforceEntity::WorkloadBucket(value) => {
                text(&value.name, "name")?;
                require(
                    value.overlapping_contribution != OverlappingContribution::Union
                        || value.measurement == WorkloadMeasurement::ElapsedMinutes,
                    "overlappingContribution",
                    "union requires elapsed-minute measurement",
                )
            }
            WorkforceEntity::Calendar(value) => {
                text(&value.name, "name")?;
                time::calendar(&value.period)
            }
            WorkforceEntity::AssignmentType(value) => {
                text(&value.name, "name")?;
                token(&value.category, "category")?;
                require(
                    value.default_duration_minutes > 0,
                    "defaultDurationMinutes",
                    "duration must be positive",
                )?;
                self.qualification_expression(&value.qualifications)?;
                if let LocationBehavior::Fixed { location_id } = value.location_behavior {
                    self.location(location_id)?;
                }
                unique(&value.workload_bucket_ids, false, "workloadBucketIds")?;
                for id in &value.workload_bucket_ids {
                    self.bucket(*id)?;
                }
                Ok(())
            }
            WorkforceEntity::ShiftTemplate(value) => {
                text(&value.name, "name")?;
                self.shift_location(value.assignment_type_id, value.location_id)?;
                time::recurrence(&value.recurrence)?;
                time::timing(&value.timing)?;
                self.coverage(&value.coverage)?;
                tags(&value.tags, "tags")
            }
            WorkforceEntity::ShiftInstance(value) => {
                self.shift_location(value.assignment_type_id, value.location_id)?;
                self.resolved_time(&value.starts_at)?;
                self.resolved_time(&value.ends_at)?;
                require(
                    value.starts_at.instant < value.ends_at.instant,
                    "endsAt",
                    "shift instant interval must be increasing and nonempty",
                )?;
                if let ShiftOrigin::Detached { template_id, .. } = value.origin {
                    self.template(template_id)?;
                }
                self.coverage(&value.coverage)?;
                tags(&value.tags, "tags")
            }
            WorkforceEntity::Availability(value) => self.availability_record(value),
            WorkforceEntity::CoverageRequirement(value) => {
                self.shift_scope(&value.scope, value.active)?;
                self.coverage(&value.coverage)
            }
            WorkforceEntity::BaseSchedule(value) => {
                unique(&value.assignments, false, "assignments")?;
                for pair in &value.assignments {
                    self.person(pair.person_id)?;
                    self.shift(pair.shift_id)?;
                }
                Ok(())
            }
            WorkforceEntity::ScorePolicy(value) => validate_score_policy_shape(value),
        }
    }

    fn availability_record(&self, value: &Availability) -> Result {
        self.person(value.person_id)?;
        time::time_window(&value.time_window)?;
        time::date_range(value.effective_range)?;
        if let Some(ids) = &value.assignment_type_ids {
            unique(ids, true, "assignmentTypeIds")?;
            for id in ids {
                self.assignment_type(*id)?;
            }
        }
        if let Some(ids) = &value.location_ids {
            unique(ids, true, "locationIds")?;
            for id in ids {
                self.location(*id)?;
            }
        }
        note(&value.source, "source")?;
        note(&value.note, "note")
    }

    fn person_record(&self, person: &Person) -> Result {
        text(&person.name, "name")?;
        if let Some(external) = &person.external_id {
            text(external, "externalId")?;
        }
        if let ActiveRange::DateRange(range) = &person.active_range {
            time::date_range(*range)?;
        }
        weight(person.workload_weight.numerator, "workloadWeight.numerator")?;
        weight(
            person.workload_weight.denominator,
            "workloadWeight.denominator",
        )?;
        if let Some(target) = person.workload_target {
            self.workload_membership(target.bucket_id, target.calendar_id, target.membership)?;
        }
        if let Some(id) = person.home_location_id {
            self.location(id)?;
        }
        bounded(&person.qualification_grants, false, "qualificationGrants")?;
        let mut grants = BTreeSet::new();
        for grant in &person.qualification_grants {
            self.qualification(grant.qualification_id)?;
            require(
                grant
                    .effective_from
                    .zip(grant.expires_at)
                    .is_none_or(|(start, end)| start < end),
                "qualificationGrants",
                "effective qualification interval must be nonempty",
            )?;
            require(
                grants.insert((
                    grant.qualification_id,
                    grant.effective_from,
                    grant.expires_at,
                )),
                "qualificationGrants",
                "duplicate equivalent qualification grant",
            )?;
        }
        unique(
            &person.eligible_assignment_type_ids,
            false,
            "eligibleAssignmentTypeIds",
        )?;
        for id in &person.eligible_assignment_type_ids {
            self.assignment_type(*id)?;
        }
        unique(&person.team_ids, false, "teamIds")?;
        for id in &person.team_ids {
            self.team(*id)?;
        }
        tags(&person.tags, "tags")?;
        if let Some(display) = &person.display {
            if let Some(color) = &display.color {
                require(
                    color.len() == 7
                        && color.starts_with('#')
                        && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit),
                    "display.color",
                    "color must be an explicit six-digit RGB hexadecimal value",
                )?;
            }
            if let Some(initials) = &display.avatar_initials {
                require(
                    initials.len() <= 16,
                    "display.avatarInitials",
                    "avatar initials exceed their byte bound",
                )?;
                text(initials, "display.avatarInitials")?;
            }
        }
        Ok(())
    }

    fn shift_location(
        &self,
        assignment_type_id: AssignmentTypeId,
        location_id: Option<LocationId>,
    ) -> Result {
        let assignment_type = self.assignment_type(assignment_type_id)?;
        if let Some(id) = location_id {
            self.location(id)?;
        }
        require(
            match assignment_type.location_behavior {
                LocationBehavior::None {} => location_id.is_none(),
                LocationBehavior::Optional {} => true,
                LocationBehavior::Required {} => location_id.is_some(),
                LocationBehavior::Fixed { location_id: fixed } => location_id == Some(fixed),
            },
            "locationId",
            "shift location disagrees with assignment-type location behavior",
        )
    }

    pub(super) fn workload_membership(
        &self,
        bucket_id: WorkloadBucketId,
        calendar_id: WorkCalendarId,
        membership: WindowMembership,
    ) -> Result {
        self.calendar(calendar_id)?;
        self.bucket_membership(bucket_id, membership)
    }

    pub(super) fn bucket_membership(
        &self,
        id: WorkloadBucketId,
        membership: WindowMembership,
    ) -> Result {
        let bucket = self.bucket(id)?;
        require(
            membership != WindowMembership::Intersection
                || bucket.measurement == WorkloadMeasurement::ElapsedMinutes,
            "membership",
            "intersection requires an elapsed-minute bucket",
        )?;
        Ok(())
    }

    fn qualification_expression(&self, expression: &QualificationExpression) -> Result {
        match expression {
            QualificationExpression::Unconstrained {} => Ok(()),
            QualificationExpression::Matches(value) => self.qualification_match(value),
        }
    }

    fn qualification_match(&self, value: &QualificationMatch) -> Result {
        require(
            !value.all_qualification_ids.is_empty() || !value.any_qualification_ids.is_empty(),
            "qualifications",
            "empty qualification match must be explicit unconstrained",
        )?;
        for ids in [&value.all_qualification_ids, &value.any_qualification_ids] {
            unique(ids, false, "qualificationIds")?;
            for id in ids {
                self.qualification(*id)?;
            }
        }
        Ok(())
    }

    pub(super) fn qualification_minimums(&self, values: &[QualificationMinimum]) -> Result {
        bounded(values, false, "qualificationMinimums")?;
        for value in values {
            self.qualification_match(&value.qualifications)?;
        }
        Ok(())
    }

    fn coverage(&self, coverage: &Coverage) -> Result {
        match coverage {
            Coverage::Exact {
                qualification_minimums,
                ..
            } => self.qualification_minimums(qualification_minimums),
            Coverage::AtLeast {
                minimum,
                preferred_count,
                maximum_count,
                qualification_minimums,
            } => {
                require(
                    preferred_count.is_none_or(|preferred| *minimum <= preferred)
                        && maximum_count.is_none_or(|maximum| *minimum <= maximum)
                        && preferred_count
                            .zip(*maximum_count)
                            .is_none_or(|(preferred, maximum)| preferred <= maximum),
                    "coverage",
                    "coverage counts must be ordered minimum, preferred, maximum",
                )?;
                self.qualification_minimums(qualification_minimums)
            }
        }
    }
}

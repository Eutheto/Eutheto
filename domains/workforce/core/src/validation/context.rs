use super::MAX_REFERENCE_ITEMS;
use super::common::{Result, invalid, require};
use crate::ids::{
    AssignmentTypeId, AvailabilityId, CoverageRequirementId, LocationId, QualificationId, ShiftId,
    ShiftTemplateId, TeamId, WorkCalendarId, WorkloadBucketId,
};
use crate::model::{
    AssignmentType, Availability, BaseSchedule, CoverageRequirement, Location, Person,
    Qualification, ShiftOrigin, ShiftTemplate, Team, WorkCalendar, WorkforceDomainV1,
    WorkforceEntity, WorkforceScorePolicy, WorkloadBucket,
};
use eutheto_types::{EntityId, PersonId, ScenarioSettings};
use jiff::tz::TimeZone;
use std::collections::BTreeSet;

pub const MAX_TEMPLATE_OCCURRENCES: usize = 4096;
pub const MAX_DOCUMENT_OCCURRENCES: usize = 32_768;

pub(super) struct Context<'a> {
    pub domain: &'a WorkforceDomainV1,
    pub settings: &'a ScenarioSettings,
    pub zone: TimeZone,
    pub score_policy: Option<&'a WorkforceScorePolicy>,
    occurrences: BTreeSet<ShiftId>,
}

macro_rules! entity_lookup {
    ($method:ident, $id:ty, $variant:ident, $record:ty) => {
        pub(super) fn $method(&self, id: $id) -> Result<&$record> {
            match self.domain.entities.get(&id.as_entity_id()) {
                Some(WorkforceEntity::$variant(value)) => Ok(value),
                _ => Err(invalid(
                    stringify!($method),
                    "reference does not resolve to the expected entity kind",
                )),
            }
        }
    };
}

impl<'a> Context<'a> {
    pub(super) fn new(
        domain: &'a WorkforceDomainV1,
        settings: &'a ScenarioSettings,
    ) -> Result<Self> {
        require(
            domain.entities.len() <= MAX_REFERENCE_ITEMS
                && domain.rules.len() <= MAX_REFERENCE_ITEMS
                && domain.preferences.len() <= MAX_REFERENCE_ITEMS
                && domain.locked_assignments.len() <= MAX_REFERENCE_ITEMS,
            "domain",
            "too many records",
        )?;
        let mut occurrences = BTreeSet::new();
        let mut dates = BTreeSet::new();
        let mut score_policy = None;
        let mut has_base = false;
        for (id, entity) in &domain.entities {
            require(
                *id == entity.id(),
                "entities.id",
                "owned identity does not match its key",
            )?;
            match entity {
                WorkforceEntity::ScorePolicy(value) => {
                    require(
                        score_policy.is_none(),
                        "entities",
                        "only one score policy is allowed",
                    )?;
                    score_policy = Some(value);
                }
                WorkforceEntity::BaseSchedule(_) => {
                    require(!has_base, "entities", "only one base schedule is allowed")?;
                    has_base = true;
                }
                WorkforceEntity::ShiftTemplate(template) => {
                    require(
                        template.occurrence_identities.len() <= MAX_TEMPLATE_OCCURRENCES,
                        "occurrenceIdentities",
                        "too many template occurrence definitions",
                    )?;
                    for (shift_id, occurrence) in &template.occurrence_identities {
                        require(
                            *shift_id == occurrence.id,
                            "occurrenceIdentities.id",
                            "owned identity does not match its key",
                        )?;
                        require(
                            occurrences.insert(*shift_id),
                            "occurrenceIdentities",
                            "duplicate shift identity",
                        )?;
                        require(
                            dates.insert((template.id, occurrence.local_start_date)),
                            "occurrenceIdentities",
                            "duplicate template occurrence date",
                        )?;
                    }
                }
                WorkforceEntity::ShiftInstance(shift) => {
                    if let ShiftOrigin::Detached {
                        template_id,
                        occurrence_date,
                    } = shift.origin
                    {
                        require(
                            dates.insert((template_id, occurrence_date)),
                            "origin",
                            "duplicate template occurrence date",
                        )?;
                    }
                }
                _ => {}
            }
            require(
                dates.len() <= MAX_DOCUMENT_OCCURRENCES,
                "occurrenceIdentities",
                "too many document occurrence definitions",
            )?;
        }
        let zone = TimeZone::get(settings.time_zone.as_str())
            .map_err(|_| invalid("settings.timeZone", "unknown scenario time zone"))?;
        Ok(Self {
            domain,
            settings,
            zone,
            score_policy,
            occurrences,
        })
    }

    pub(super) fn person(&self, id: PersonId) -> Result<&Person> {
        match self.domain.entities.get(&EntityId::from_uuid(id.as_uuid())) {
            Some(WorkforceEntity::Person(person)) => Ok(person),
            _ => Err(invalid(
                "personId",
                "reference does not resolve to a person",
            )),
        }
    }

    pub(super) fn base_schedule(&self, id: EntityId) -> Result<&BaseSchedule> {
        match self.domain.entities.get(&id) {
            Some(WorkforceEntity::BaseSchedule(base)) => Ok(base),
            _ => Err(invalid(
                "baseScheduleId",
                "reference does not resolve to the base schedule",
            )),
        }
    }

    pub(super) fn shift(&self, id: ShiftId) -> Result {
        require(
            matches!(
                self.domain.entities.get(&id.as_entity_id()),
                Some(WorkforceEntity::ShiftInstance(_))
            ) || self.occurrences.contains(&id),
            "shiftId",
            "reference does not resolve to a stored shift or occurrence",
        )
    }

    entity_lookup!(qualification, QualificationId, Qualification, Qualification);
    entity_lookup!(location, LocationId, Location, Location);
    entity_lookup!(team, TeamId, Team, Team);
    entity_lookup!(bucket, WorkloadBucketId, WorkloadBucket, WorkloadBucket);
    entity_lookup!(calendar, WorkCalendarId, Calendar, WorkCalendar);
    entity_lookup!(
        assignment_type,
        AssignmentTypeId,
        AssignmentType,
        AssignmentType
    );
    entity_lookup!(template, ShiftTemplateId, ShiftTemplate, ShiftTemplate);
    entity_lookup!(availability, AvailabilityId, Availability, Availability);
    entity_lookup!(
        coverage_requirement,
        CoverageRequirementId,
        CoverageRequirement,
        CoverageRequirement
    );
}

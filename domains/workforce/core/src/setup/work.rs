use super::{
    contracts::{
        CoverageSummaryV1, DisplayDurationV1, GenerationReviewParametersV1, GenerationReviewV1,
        GenerationRowV1, MAX_WINDOW_DAYS, ORDINARY_DATA_BYTES, PREVIEW_DATA_BYTES,
        PriorUnresolvedShiftV1, PriorWorkShiftV1, ResolvedIntervalV1, ShiftChangeKindV1,
        ShiftOriginV1, TemporalIssueCodeV1, WorkDetailParametersV1, WorkShiftDetailV1,
        WorkShiftSourceV1, WorkShiftV1, WorkWindowParametersV1, WorkforcePositionV1,
        WorkforceSetupViewDataV1,
    },
    paging::{PageBuilder, ProjectionBudget, Result, invalid},
};
use crate::{
    ids::ShiftId,
    model::{
        Coverage, DateRange, ShiftInstance, ShiftTemplate, WorkforceDomainV1, WorkforceEntity,
    },
    temporal::{
        self, PriorShift, ResolvedShift, ResolvedShiftOrigin, ShiftChangeKind, TemporalError,
        TemporalIssueKind,
    },
    validation::validate_document_controlled,
};
use eutheto_domain_api::{DomainBatchCommand, DomainPackError, SetupViewContext};
use eutheto_types::ScenarioDocument;
use jiff::{
    SignedDuration,
    civil::{Date, Time},
};
use std::collections::BTreeMap;

pub(super) fn temporal_error(error: TemporalError) -> DomainPackError {
    match error {
        TemporalError::InvalidDocument(error) => error,
        TemporalError::Cancelled => DomainPackError::Cancelled,
        TemporalError::Issue(issue) => match issue.kind {
            TemporalIssueKind::OccurrenceLimit
            | TemporalIssueKind::OutputLimit
            | TemporalIssueKind::CalendarLimit => DomainPackError::ResourceLimitExceeded,
            _ => invalid(
                "/query",
                &format!("Workforce temporal resolution required: {:?}", issue.kind),
            ),
        },
    }
}

pub(super) fn check_dates(dates: DateRange) -> Result<()> {
    let days = dates
        .start_date
        .to_datetime(Time::MIN)
        .duration_until(dates.end_date_exclusive.to_datetime(Time::MIN))
        .as_secs()
        / 86_400;
    if !(1..=i64::from(MAX_WINDOW_DAYS)).contains(&days) {
        return Err(invalid(
            "/query/parameters/dates",
            "date window must contain 1 through 366 local days",
        ));
    }
    Ok(())
}

fn contains(dates: DateRange, date: Date) -> bool {
    dates.start_date <= date && date < dates.end_date_exclusive
}

pub(super) fn window(
    document: &ScenarioDocument,
    parameters: &WorkWindowParametersV1,
    position: Option<WorkforcePositionV1>,
    context: SetupViewContext,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    check_dates(parameters.dates)?;
    let cursor = match position {
        None => None,
        Some(WorkforcePositionV1::TimedShift {
            starts_at,
            shift_id,
        }) => Some((starts_at, shift_id)),
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "work window requires a timed shift position",
            ));
        }
    };
    let mut page = PageBuilder::new(parameters.limit, ORDINARY_DATA_BYTES)?;
    budget.visit()?;
    let domain = validate_document_controlled(document, Some(budget.control()))?;
    let shifts = temporal::resolve_validated_shifts_in_range(
        &domain,
        &document.settings,
        Some(parameters.dates),
        &mut |_| budget.visit().map_err(TemporalError::from),
    )
    .map_err(temporal_error)?;
    let mut cursor_seen = cursor.is_none();
    for shift in shifts {
        budget.visit()?;
        let key = (shift.interval.starts_at.instant, shift.id);
        cursor_seen |= cursor == Some(key);
        page.observe(cursor.is_none_or(|cursor| key > cursor), || {
            Ok((
                shift_row(&domain, &shift)?,
                WorkforcePositionV1::TimedShift {
                    starts_at: key.0,
                    shift_id: key.1,
                },
            ))
        })?;
    }
    if !cursor_seen {
        return Err(invalid(
            "/query/continuation/position",
            "continuation shift is absent from this window",
        ));
    }
    page.finish(
        document.scenario_id,
        context,
        WorkforceSetupViewDataV1::WorkWindow,
    )
}

pub(super) fn detail(
    document: &ScenarioDocument,
    parameters: &WorkDetailParametersV1,
    position: Option<WorkforcePositionV1>,
    budget: &mut ProjectionBudget<'_>,
) -> Result<WorkforceSetupViewDataV1> {
    if position.is_some() {
        return Err(invalid(
            "/query/continuation",
            "work detail does not accept continuation",
        ));
    }
    budget.visit()?;
    let domain = validate_document_controlled(document, Some(budget.control()))?;
    let shift = temporal::resolve_validated_shift(
        &domain,
        &document.settings,
        parameters.shift_id,
        &mut |_| budget.visit().map_err(TemporalError::from),
    )
    .map_err(temporal_error)?
    .ok_or_else(|| {
        invalid(
            "/query/parameters/shiftId",
            "shift is not active in this planning horizon",
        )
    })?;
    budget.visit()?;
    let source = match source(&domain, &shift)? {
        Source::Template(template) => WorkShiftSourceV1::Template(template.clone()),
        Source::Instance(instance) => WorkShiftSourceV1::Instance(instance.clone()),
    };
    Ok(WorkforceSetupViewDataV1::WorkDetail(Box::new(
        WorkShiftDetailV1 {
            shift: shift_row(&domain, &shift)?,
            source,
        },
    )))
}

type Joined = (
    Option<PriorShift>,
    Option<ResolvedShift>,
    Option<ShiftChangeKind>,
);
pub(super) fn generation_review(
    original: &ScenarioDocument,
    prospective: &ScenarioDocument,
    parameters: &GenerationReviewParametersV1,
    position: Option<WorkforcePositionV1>,
    context: SetupViewContext,
    budget: &mut ProjectionBudget<'_>,
) -> Result<(WorkforceSetupViewDataV1, Option<DomainBatchCommand>)> {
    if let Some(dates) = parameters.dates {
        check_dates(dates)?;
    }
    let cursor = match position {
        None => None,
        Some(WorkforcePositionV1::Shift { shift_id }) => Some(shift_id),
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "generation review requires a shift position",
            ));
        }
    };
    let mut page = PageBuilder::new(parameters.limit, PREVIEW_DATA_BYTES)?;
    budget.visit()?;
    let control = budget.control();
    let (preview, before_domain, after_domain) =
        temporal::preview_generation_checked(original, prospective, control, &mut || {
            budget.visit().map_err(TemporalError::from)
        })
        .map_err(temporal_error)?;
    budget.visit()?;
    let mut joined: BTreeMap<ShiftId, Joined> = BTreeMap::new();
    for prior in preview.before {
        budget.visit()?;
        joined.entry(prior.id()).or_default().0 = Some(prior);
    }
    for after in preview.after {
        budget.visit()?;
        joined.entry(after.id).or_default().1 = Some(after);
    }
    let (mut total_added, mut total_changed, mut total_removed) = (0u32, 0u32, 0u32);
    for change in preview.changes {
        budget.visit()?;
        let count = match change.kind {
            ShiftChangeKind::Added => &mut total_added,
            ShiftChangeKind::Changed => &mut total_changed,
            ShiftChangeKind::Removed => &mut total_removed,
        };
        *count = count
            .checked_add(1)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        joined.entry(change.id).or_default().2 = Some(change.kind);
    }
    let mut cursor_seen = cursor.is_none();
    for (shift_id, (before, after, change)) in joined {
        budget.visit()?;
        if parameters.changes_only && change.is_none() {
            continue;
        }
        if let Some(dates) = parameters.dates {
            let before_matches = before
                .map(|prior| prior_date(&before_domain, prior))
                .transpose()?
                .is_some_and(|date| contains(dates, date));
            let after_matches = after.is_some_and(|shift| {
                contains(dates, shift.interval.starts_at.local.as_datetime().date())
            });
            if !before_matches && !after_matches {
                continue;
            }
        }
        cursor_seen |= cursor == Some(shift_id);
        page.observe(cursor.is_none_or(|cursor| shift_id > cursor), || {
            let before = before
                .map(|prior| prior_row(&before_domain, prior))
                .transpose()?;
            let after = after
                .as_ref()
                .map(|shift| shift_row(&after_domain, shift))
                .transpose()?;
            Ok((
                GenerationRowV1 {
                    shift_id,
                    before,
                    after,
                    change: change.map(change_row),
                },
                WorkforcePositionV1::Shift { shift_id },
            ))
        })?;
    }
    if !cursor_seen {
        return Err(invalid(
            "/query/continuation/position",
            "continuation shift is absent from this filtered review",
        ));
    }
    let reconciliation_required = preview.reconciliation.is_some();
    let result = page.finish(prospective.scenario_id, context, |page| {
        WorkforceSetupViewDataV1::GenerationReview(GenerationReviewV1 {
            prospective_hash: preview.prospective_hash,
            reconciliation_required,
            total_added,
            total_changed,
            total_removed,
            page,
        })
    })?;
    Ok((result, preview.reconciliation))
}

fn prior_row(domain: &WorkforceDomainV1, prior: PriorShift) -> Result<PriorWorkShiftV1> {
    match prior {
        PriorShift::Resolved(shift) => {
            shift_row(domain, &shift).map(|row| PriorWorkShiftV1::Resolved(Box::new(row)))
        }
        PriorShift::Unresolved { origin, issue, .. } => {
            Ok(PriorWorkShiftV1::Unresolved(PriorUnresolvedShiftV1 {
                origin: origin_row(origin),
                issue: issue_row(issue),
            }))
        }
    }
}

enum Source<'a> {
    Template(&'a ShiftTemplate),
    Instance(&'a ShiftInstance),
}

fn source<'a>(domain: &'a WorkforceDomainV1, shift: &ResolvedShift) -> Result<Source<'a>> {
    match shift.origin {
        ResolvedShiftOrigin::Generated { template_id, .. } => {
            match domain.entities.get(&template_id.as_entity_id()) {
                Some(WorkforceEntity::ShiftTemplate(template)) => Ok(Source::Template(template)),
                _ => Err(invalid(
                    "/domain/entities",
                    "generated shift source template is missing",
                )),
            }
        }
        ResolvedShiftOrigin::Detached { .. } | ResolvedShiftOrigin::Manual => {
            match domain.entities.get(&shift.id.as_entity_id()) {
                Some(WorkforceEntity::ShiftInstance(instance)) => Ok(Source::Instance(instance)),
                _ => Err(invalid(
                    "/domain/entities",
                    "stored shift source instance is missing",
                )),
            }
        }
    }
}

pub(super) fn shift_row(domain: &WorkforceDomainV1, shift: &ResolvedShift) -> Result<WorkShiftV1> {
    let (assignment_type_id, location_id, coverage, template_name) = match source(domain, shift)? {
        Source::Template(template) => (
            template.assignment_type_id,
            template.location_id,
            &template.coverage,
            Some(template.name.clone()),
        ),
        Source::Instance(instance) => (
            instance.assignment_type_id,
            instance.location_id,
            &instance.coverage,
            None,
        ),
    };
    let assignment_type_name = match domain.entities.get(&assignment_type_id.as_entity_id()) {
        Some(WorkforceEntity::AssignmentType(value)) => value.name.clone(),
        _ => {
            return Err(invalid(
                "/domain/entities",
                "shift assignment type is missing",
            ));
        }
    };
    let location_name = location_id
        .map(|id| match domain.entities.get(&id.as_entity_id()) {
            Some(WorkforceEntity::Location(value)) => Ok(value.name.clone()),
            _ => Err(invalid("/domain/entities", "shift location is missing")),
        })
        .transpose()?;
    let (minimum, preferred, maximum, qualification_minimums) = match coverage {
        Coverage::Exact {
            count,
            qualification_minimums,
        } => (*count, None, Some(*count), qualification_minimums),
        Coverage::AtLeast {
            minimum,
            preferred_count,
            maximum_count,
            qualification_minimums,
        } => (
            *minimum,
            *preferred_count,
            *maximum_count,
            qualification_minimums,
        ),
    };
    Ok(WorkShiftV1 {
        shift_id: shift.id,
        origin: origin_row(shift.origin),
        assignment_type_id,
        assignment_type_name,
        template_name,
        location_id,
        location_name,
        coverage: CoverageSummaryV1 {
            minimum,
            preferred,
            maximum,
            qualification_minimum_count: u32::try_from(qualification_minimums.len())
                .map_err(|_| DomainPackError::ResourceLimitExceeded)?,
        },
        interval: ResolvedIntervalV1 {
            starts_at: shift.interval.starts_at,
            ends_at: shift.interval.ends_at,
        },
        reporting_date: shift.reporting_date,
        elapsed: duration_row(shift.interval.elapsed_duration()),
        scheduled: duration_row(shift.interval.scheduled_duration()),
    })
}

fn prior_date(domain: &WorkforceDomainV1, prior: PriorShift) -> Result<Date> {
    match prior {
        PriorShift::Resolved(shift) => Ok(shift.interval.starts_at.local.as_datetime().date()),
        PriorShift::Unresolved {
            origin:
                ResolvedShiftOrigin::Generated {
                    occurrence_date, ..
                },
            ..
        } => Ok(occurrence_date),
        PriorShift::Unresolved { id, .. } => match domain.entities.get(&id.as_entity_id()) {
            Some(WorkforceEntity::ShiftInstance(instance)) => {
                Ok(instance.starts_at.local.as_datetime().date())
            }
            _ => Err(invalid(
                "/domain/entities",
                "unresolved prior shift source is missing",
            )),
        },
    }
}

fn duration_row(duration: SignedDuration) -> DisplayDurationV1 {
    DisplayDurationV1 {
        seconds: duration.as_secs().to_string(),
        nanoseconds: duration.subsec_nanos(),
    }
}

fn origin_row(origin: ResolvedShiftOrigin) -> ShiftOriginV1 {
    match origin {
        ResolvedShiftOrigin::Generated {
            template_id,
            occurrence_date,
        } => ShiftOriginV1::Generated {
            template_id,
            occurrence_date,
        },
        ResolvedShiftOrigin::Detached {
            template_id,
            occurrence_date,
        } => ShiftOriginV1::Detached {
            template_id,
            occurrence_date,
        },
        ResolvedShiftOrigin::Manual => ShiftOriginV1::Manual,
    }
}

fn change_row(kind: ShiftChangeKind) -> ShiftChangeKindV1 {
    match kind {
        ShiftChangeKind::Added => ShiftChangeKindV1::Added,
        ShiftChangeKind::Changed => ShiftChangeKindV1::Changed,
        ShiftChangeKind::Removed => ShiftChangeKindV1::Removed,
    }
}

fn issue_row(issue: TemporalIssueKind) -> TemporalIssueCodeV1 {
    match issue {
        TemporalIssueKind::Resolution(kind) => TemporalIssueCodeV1::Resolution(kind),
        TemporalIssueKind::DateOverflow => TemporalIssueCodeV1::DateOverflow,
        TemporalIssueKind::InvalidInterval => TemporalIssueCodeV1::InvalidInterval,
        TemporalIssueKind::OutsideHorizon => TemporalIssueCodeV1::OutsideHorizon,
        TemporalIssueKind::UnreconciledIdentity => TemporalIssueCodeV1::UnreconciledIdentity,
        TemporalIssueKind::IdentityCollision => TemporalIssueCodeV1::IdentityCollision,
        TemporalIssueKind::IdentityTransition => TemporalIssueCodeV1::IdentityTransition,
        TemporalIssueKind::OccurrenceLimit => TemporalIssueCodeV1::OccurrenceLimit,
        TemporalIssueKind::OutputLimit => TemporalIssueCodeV1::OutputLimit,
        TemporalIssueKind::CalendarLimit => TemporalIssueCodeV1::CalendarLimit,
        TemporalIssueKind::CalendarOverlap => TemporalIssueCodeV1::CalendarOverlap,
        TemporalIssueKind::CalendarOrder => TemporalIssueCodeV1::CalendarOrder,
        TemporalIssueKind::AmbiguousReportingDate => TemporalIssueCodeV1::AmbiguousReportingDate,
        TemporalIssueKind::UnknownCalendar => TemporalIssueCodeV1::UnknownCalendar,
        TemporalIssueKind::InvalidQuery => TemporalIssueCodeV1::InvalidQuery,
        TemporalIssueKind::DifferentScenario => TemporalIssueCodeV1::DifferentScenario,
    }
}

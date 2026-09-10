use super::{
    MAX_RESOLVED_SHIFTS, ResolvedInterval, ResolvedShift, ResolvedShiftOrigin, TemporalError,
    TemporalIssueKind, check_cancelled, issue, resolve_shift_timing,
};
use crate::{
    ids::{ShiftId, ShiftTemplateId},
    model::{
        DateRange, PlanningDates, ReportingAttribution, ShiftInstance, ShiftOrigin, ShiftTemplate,
        Weekday, WorkforceDomainV1, WorkforceEntity, planning_dates,
    },
    validation::{MAX_DOCUMENT_OCCURRENCES, MAX_TEMPLATE_OCCURRENCES, validate_document},
};
use eutheto_types::{CancellationToken, ScenarioDocument, ScenarioSettings};
use jiff::{Span, civil::Date};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(super) struct OccurrenceOwner {
    pub id: ShiftId,
    pub detached: bool,
}

pub(super) type Owners = BTreeMap<(ShiftTemplateId, Date), OccurrenceOwner>;

/// Logical work and fixed-size retention boundaries for a validated resolution.
/// Public temporal callers check cancellation; rule operations also charge their budget.
#[derive(Clone, Copy)]
pub(crate) enum ResolutionStep {
    Inspect,
    RetainRecurrenceKey,
    Sort(usize),
    RetainOwner,
    RetainSpec,
    RetainShift,
}

/// The caller has already validated global identity and template/date uniqueness.
pub(super) fn owners<E>(
    domain: &WorkforceDomainV1,
    checkpoint: &mut impl FnMut(ResolutionStep) -> Result<(), E>,
) -> Result<Owners, E> {
    let mut owners = BTreeMap::new();
    for entity in domain.entities.values() {
        checkpoint(ResolutionStep::Inspect)?;
        match entity {
            WorkforceEntity::ShiftTemplate(template) => {
                for occurrence in template.occurrence_identities.values() {
                    checkpoint(ResolutionStep::RetainOwner)?;
                    owners.insert(
                        (template.id, occurrence.local_start_date),
                        OccurrenceOwner {
                            id: occurrence.id,
                            detached: false,
                        },
                    );
                }
            }
            WorkforceEntity::ShiftInstance(shift) => {
                if let ShiftOrigin::Detached {
                    template_id,
                    occurrence_date,
                } = shift.origin
                {
                    checkpoint(ResolutionStep::RetainOwner)?;
                    owners.insert(
                        (template_id, occurrence_date),
                        OccurrenceOwner {
                            id: shift.id,
                            detached: true,
                        },
                    );
                }
            }
            _ => {}
        }
    }
    Ok(owners)
}

#[derive(Clone, Copy)]
pub(super) enum ShiftSpec<'a> {
    Generated {
        template: &'a ShiftTemplate,
        date: Date,
        id: Option<ShiftId>,
    },
    Stored(&'a ShiftInstance),
}

impl ShiftSpec<'_> {
    pub fn id(self) -> Option<ShiftId> {
        match self {
            Self::Generated { id, .. } => id,
            Self::Stored(shift) => Some(shift.id),
        }
    }

    pub fn origin(self) -> ResolvedShiftOrigin {
        match self {
            Self::Generated { template, date, .. } => ResolvedShiftOrigin::Generated {
                template_id: template.id,
                occurrence_date: date,
            },
            Self::Stored(shift) => match shift.origin {
                ShiftOrigin::Manual {} => ResolvedShiftOrigin::Manual,
                ShiftOrigin::Detached {
                    template_id,
                    occurrence_date,
                } => ResolvedShiftOrigin::Detached {
                    template_id,
                    occurrence_date,
                },
            },
        }
    }

    pub fn resolve(
        self,
        settings: &ScenarioSettings,
        dates: PlanningDates,
    ) -> Result<ResolvedShift, TemporalError> {
        let (id, interval, attribution) = match self {
            Self::Generated { template, date, id } => {
                let entity_id = template.id.as_entity_id();
                let id = id.ok_or_else(|| {
                    issue(
                        TemporalIssueKind::UnreconciledIdentity,
                        Some(entity_id),
                        Some(date),
                    )
                })?;
                let interval = resolve_shift_timing(template.timing, date, settings, entity_id)?;
                // The resolved offset gives the actual local date without another zone lookup.
                // Generated membership is civil-date based even at repeated midnights.
                let offset = jiff::tz::Offset::from_seconds(interval.starts_at.offset_seconds)
                    .map_err(|_| {
                        issue(
                            TemporalIssueKind::InvalidInterval,
                            Some(entity_id),
                            Some(date),
                        )
                    })?;
                let actual_date = interval
                    .starts_at
                    .instant
                    .as_timestamp()
                    .to_zoned(jiff::tz::TimeZone::fixed(offset))
                    .date();
                if actual_date < dates.first_date || actual_date > dates.last_date {
                    return Err(issue(
                        TemporalIssueKind::OutsideHorizon,
                        Some(entity_id),
                        Some(date),
                    ));
                }
                (id, interval, template.reporting_attribution)
            }
            Self::Stored(shift) => (
                shift.id,
                ResolvedInterval {
                    starts_at: shift.starts_at,
                    ends_at: shift.ends_at,
                },
                shift.reporting_attribution,
            ),
        };
        let reporting_date = match attribution {
            ReportingAttribution::StartLocalDate => interval.starts_at.local.as_datetime().date(),
            ReportingAttribution::EndLocalDate => interval.ends_at.local.as_datetime().date(),
        };
        Ok(ResolvedShift {
            id,
            origin: self.origin(),
            interval,
            reporting_date,
        })
    }
}

fn contains_start(settings: &ScenarioSettings, interval: ResolvedInterval) -> bool {
    settings.horizon.start <= interval.starts_at.instant
        && interval.starts_at.instant < settings.horizon.end
}

pub(crate) fn weekday(date: Date) -> Weekday {
    match date.weekday() {
        jiff::civil::Weekday::Monday => Weekday::Monday,
        jiff::civil::Weekday::Tuesday => Weekday::Tuesday,
        jiff::civil::Weekday::Wednesday => Weekday::Wednesday,
        jiff::civil::Weekday::Thursday => Weekday::Thursday,
        jiff::civil::Weekday::Friday => Weekday::Friday,
        jiff::civil::Weekday::Saturday => Weekday::Saturday,
        jiff::civil::Weekday::Sunday => Weekday::Sunday,
    }
}

/// A previous draft need not have identities for every intent. Enumerate its known
/// definitions directly so a narrower prospective horizon can repair an oversized draft.
pub(super) fn collect_prior_specs<'a>(
    domain: &'a WorkforceDomainV1,
    settings: &ScenarioSettings,
    checkpoint: &mut impl FnMut() -> Result<(), TemporalError>,
) -> Result<Vec<ShiftSpec<'a>>, TemporalError> {
    let dates = planning_dates(settings)?;
    let mut specs = Vec::new();
    for entity in domain.entities.values() {
        checkpoint()?;
        match entity {
            WorkforceEntity::ShiftTemplate(template) => {
                let mut excluded = BTreeSet::new();
                for date in &template.recurrence.excluded_dates {
                    checkpoint()?;
                    excluded.insert(*date);
                }
                for occurrence in template.occurrence_identities.values() {
                    checkpoint()?;
                    let date = occurrence.local_start_date;
                    if dates.first_date <= date
                        && date <= dates.last_date
                        && template.recurrence.effective_range.start_date <= date
                        && date < template.recurrence.effective_range.end_date_exclusive
                        && template.recurrence.weekdays.contains(&weekday(date))
                        && !excluded.contains(&date)
                    {
                        push_spec(
                            &mut specs,
                            ShiftSpec::Generated {
                                template,
                                date,
                                id: Some(occurrence.id),
                            },
                        )?;
                    }
                }
            }
            WorkforceEntity::ShiftInstance(shift)
                if contains_start(
                    settings,
                    ResolvedInterval {
                        starts_at: shift.starts_at,
                        ends_at: shift.ends_at,
                    },
                ) =>
            {
                push_spec(&mut specs, ShiftSpec::Stored(shift))?;
            }
            _ => {}
        }
    }
    Ok(specs)
}

/// Enumerates only active definitions/intents, keeping metadata borrowed once from the model.
/// Missing identity is allowed here solely so review can propose an ordinary reconciliation.
pub(super) fn collect_specs<'a, E: From<TemporalError>>(
    domain: &'a WorkforceDomainV1,
    settings: &ScenarioSettings,
    owners: &Owners,
    checkpoint: &mut impl FnMut(ResolutionStep) -> Result<(), E>,
) -> Result<Vec<ShiftSpec<'a>>, E> {
    collect_specs_in_range(domain, settings, owners, None, checkpoint)
}

fn collect_specs_in_range<'a, E: From<TemporalError>>(
    domain: &'a WorkforceDomainV1,
    settings: &ScenarioSettings,
    owners: &Owners,
    range: Option<DateRange>,
    checkpoint: &mut impl FnMut(ResolutionStep) -> Result<(), E>,
) -> Result<Vec<ShiftSpec<'a>>, E> {
    let dates = planning_dates(settings).map_err(TemporalError::from)?;
    let first_date = range.map_or(dates.first_date, |range| {
        dates.first_date.max(range.start_date)
    });
    let last_date = range.map_or(Ok(dates.last_date), |range| {
        range
            .end_date_exclusive
            .checked_sub(Span::new().days(1))
            .map(|last| dates.last_date.min(last))
            .map_err(|_| issue(TemporalIssueKind::InvalidQuery, None, None))
    })?;
    let mut specs = Vec::new();
    let mut generated = 0;
    for entity in domain.entities.values() {
        checkpoint(ResolutionStep::Inspect)?;
        match entity {
            WorkforceEntity::ShiftTemplate(template) => {
                let recurrence = &template.recurrence;
                let mut date = first_date.max(recurrence.effective_range.start_date);
                let mut weekdays = BTreeSet::new();
                for weekday in &recurrence.weekdays {
                    checkpoint(ResolutionStep::RetainRecurrenceKey)?;
                    weekdays.insert(*weekday);
                }
                let mut excluded = BTreeSet::new();
                for date in &recurrence.excluded_dates {
                    checkpoint(ResolutionStep::RetainRecurrenceKey)?;
                    excluded.insert(*date);
                }
                let mut count = 0;
                while date <= last_date && date < recurrence.effective_range.end_date_exclusive {
                    checkpoint(ResolutionStep::Inspect)?;
                    let owner = owners.get(&(template.id, date));
                    if weekdays.contains(&weekday(date))
                        && !excluded.contains(&date)
                        && !owner.is_some_and(|owner| owner.detached)
                    {
                        if count == MAX_TEMPLATE_OCCURRENCES
                            || generated == MAX_DOCUMENT_OCCURRENCES
                        {
                            return Err(issue(
                                TemporalIssueKind::OccurrenceLimit,
                                Some(template.id.as_entity_id()),
                                Some(date),
                            )
                            .into());
                        }
                        checkpoint(ResolutionStep::RetainSpec)?;
                        push_spec(
                            &mut specs,
                            ShiftSpec::Generated {
                                template,
                                date,
                                id: owner.map(|owner| owner.id),
                            },
                        )?;
                        count += 1;
                        generated += 1;
                    }
                    if date == last_date {
                        break;
                    }
                    date = date.checked_add(Span::new().days(1)).map_err(|_| {
                        issue(
                            TemporalIssueKind::DateOverflow,
                            Some(template.id.as_entity_id()),
                            Some(date),
                        )
                    })?;
                }
            }
            WorkforceEntity::ShiftInstance(shift) => {
                let interval = ResolvedInterval {
                    starts_at: shift.starts_at,
                    ends_at: shift.ends_at,
                };
                let local_date = shift.starts_at.local.as_datetime().date();
                if contains_start(settings, interval)
                    && range.is_none_or(|range| {
                        range.start_date <= local_date && local_date < range.end_date_exclusive
                    })
                {
                    checkpoint(ResolutionStep::RetainSpec)?;
                    push_spec(&mut specs, ShiftSpec::Stored(shift))?;
                }
            }
            _ => {}
        }
    }
    Ok(specs)
}

fn push_spec<'a>(specs: &mut Vec<ShiftSpec<'a>>, spec: ShiftSpec<'a>) -> Result<(), TemporalError> {
    if specs.len() == MAX_RESOLVED_SHIFTS {
        return Err(issue(TemporalIssueKind::OutputLimit, None, None));
    }
    specs.push(spec);
    Ok(())
}

/// Resolves in-horizon shifts only after every generated identity is reconciled.
/// Does not mint IDs, mutate state, establish feasibility, or reinterpret stored fold choices.
///
/// # Errors
/// Rejects invalid documents, unresolved policies/identities, temporal overflow, limits and cancellation.
pub fn resolve_shifts(
    document: &ScenarioDocument,
    cancellation: &CancellationToken,
) -> Result<Vec<ResolvedShift>, TemporalError> {
    check_cancelled(cancellation)?;
    let domain = validate_document(document)?;
    resolve_validated_shifts(&domain, &document.settings, &mut |_| {
        check_cancelled(cancellation)
    })
}

/// Reuses a previously validated model without decoding or manufacturing cancellation state.
pub(crate) fn resolve_validated_shifts<E: From<TemporalError>>(
    domain: &WorkforceDomainV1,
    settings: &ScenarioSettings,
    checkpoint: &mut impl FnMut(ResolutionStep) -> Result<(), E>,
) -> Result<Vec<ResolvedShift>, E> {
    resolve_validated_shifts_in_range(domain, settings, None, checkpoint)
}

/// Intersects recurrence enumeration with original local start dates before resolving.
/// Endpoint horizon checks still use the entire planning horizon, not this display range.
pub(crate) fn resolve_validated_shifts_in_range<E: From<TemporalError>>(
    domain: &WorkforceDomainV1,
    settings: &ScenarioSettings,
    range: Option<DateRange>,
    checkpoint: &mut impl FnMut(ResolutionStep) -> Result<(), E>,
) -> Result<Vec<ResolvedShift>, E> {
    checkpoint(ResolutionStep::Inspect)?;
    let dates = planning_dates(settings).map_err(TemporalError::from)?;
    if range.is_some_and(|range| range.start_date >= range.end_date_exclusive) {
        return Err(issue(TemporalIssueKind::InvalidQuery, None, None).into());
    }
    let owners = owners(domain, checkpoint)?;
    let specs = collect_specs_in_range(domain, settings, &owners, range, checkpoint)?;
    let mut resolved = Vec::new();
    for spec in specs {
        checkpoint(ResolutionStep::Inspect)?;
        let shift = spec.resolve(settings, dates)?;
        checkpoint(ResolutionStep::RetainShift)?;
        resolved.push(shift);
    }
    checkpoint(ResolutionStep::Sort(resolved.len()))?;
    resolved.sort_unstable_by_key(|shift| (shift.interval.starts_at.instant, shift.id));
    checkpoint(ResolutionStep::Inspect)?;
    Ok(resolved)
}

/// Resolves only the requested retained identity; dormant definitions are not work.
pub(crate) fn resolve_validated_shift<E: From<TemporalError>>(
    domain: &WorkforceDomainV1,
    settings: &ScenarioSettings,
    id: ShiftId,
    checkpoint: &mut impl FnMut(ResolutionStep) -> Result<(), E>,
) -> Result<Option<ResolvedShift>, E> {
    let dates = planning_dates(settings).map_err(TemporalError::from)?;
    let mut local_date = None;
    for entity in domain.entities.values() {
        checkpoint(ResolutionStep::Inspect)?;
        match entity {
            WorkforceEntity::ShiftInstance(shift) if shift.id == id => {
                let spec = ShiftSpec::Stored(shift);
                let resolved = spec.resolve(settings, dates)?;
                return Ok(contains_start(settings, resolved.interval).then_some(resolved));
            }
            WorkforceEntity::ShiftTemplate(template) => {
                if let Some(occurrence) = template.occurrence_identities.get(&id) {
                    local_date = Some(occurrence.local_start_date);
                }
            }
            _ => {}
        }
    }
    let Some(start_date) = local_date else {
        return Ok(None);
    };
    let end_date_exclusive = start_date.checked_add(Span::new().days(1)).map_err(|_| {
        issue(
            TemporalIssueKind::DateOverflow,
            Some(id.as_entity_id()),
            Some(start_date),
        )
    })?;
    let owners = owners(domain, checkpoint)?;
    let specs = collect_specs_in_range(
        domain,
        settings,
        &owners,
        Some(DateRange {
            start_date,
            end_date_exclusive,
        }),
        checkpoint,
    )?;
    for spec in specs {
        checkpoint(ResolutionStep::Inspect)?;
        if spec.id() == Some(id) {
            return spec.resolve(settings, dates).map(Some).map_err(Into::into);
        }
    }
    Ok(None)
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use crate::model::{Coverage, OccurrenceIdentity, Recurrence, ShiftTiming};
    use eutheto_types::{GapPolicy, Horizon, OverlapPolicy, UnitSystem};
    use jiff::civil::Time;

    #[test]
    fn range_resolves_original_gap_date_without_generating_unrelated_dates()
    -> Result<(), Box<dyn std::error::Error>> {
        let date = Date::new(2011, 12, 30)?;
        let id: ShiftId = "018f7b40-a000-7000-8000-000000000008".parse()?;
        let template = ShiftTemplate {
            id: "018f7b40-a000-7000-8000-000000000006".parse()?,
            name: "Skipped civil day".to_owned(),
            assignment_type_id: "018f7b40-a000-7000-8000-000000000007".parse()?,
            location_id: None,
            recurrence: Recurrence {
                weekdays: vec![Weekday::Thursday, Weekday::Friday, Weekday::Saturday],
                effective_range: DateRange {
                    start_date: Date::new(2011, 12, 29)?,
                    end_date_exclusive: Date::new(2012, 1, 2)?,
                },
                excluded_dates: Vec::new(),
            },
            timing: ShiftTiming::ElapsedDuration {
                start_time: Time::new(9, 0, 0, 0)?,
                duration_minutes: 60,
            },
            coverage: Coverage::Exact {
                count: 1,
                qualification_minimums: Vec::new(),
            },
            tags: Vec::new(),
            reporting_attribution: ReportingAttribution::StartLocalDate,
            occurrence_identities: BTreeMap::from([(
                id,
                OccurrenceIdentity {
                    id,
                    local_start_date: date,
                },
            )]),
        };
        let mut domain = WorkforceDomainV1::default();
        domain.entities.insert(
            template.id.as_entity_id(),
            WorkforceEntity::ShiftTemplate(template),
        );
        let settings = ScenarioSettings {
            time_zone: "Pacific/Apia".parse()?,
            locale: "en-US".parse()?,
            units: UnitSystem::Metric,
            horizon: Horizon {
                start: "2011-12-29T00:00:00-10:00".parse()?,
                end: "2012-01-02T00:00:00+14:00".parse()?,
            },
            gap_policy: GapPolicy::MoveForward,
            overlap_policy: OverlapPolicy::Earlier,
        };
        let range = DateRange {
            start_date: date,
            end_date_exclusive: Date::new(2011, 12, 31)?,
        };
        let shifts =
            resolve_validated_shifts_in_range(&domain, &settings, Some(range), &mut |_| {
                Ok::<_, TemporalError>(())
            })?;
        assert_eq!(
            shifts.iter().map(|shift| shift.id).collect::<Vec<_>>(),
            vec![id]
        );
        assert_eq!(
            shifts[0].interval.starts_at.local.as_datetime().date(),
            date
        );
        assert_eq!(
            shifts[0]
                .interval
                .starts_at
                .instant
                .as_timestamp()
                .to_zoned(jiff::tz::TimeZone::get("Pacific/Apia")?)
                .date(),
            Date::new(2011, 12, 31)?,
        );
        assert!(matches!(
            resolve_validated_shifts(&domain, &settings, &mut |_| Ok::<_, TemporalError>(())),
            Err(TemporalError::Issue(super::super::TemporalIssue {
                kind: TemporalIssueKind::UnreconciledIdentity,
                ..
            })),
        ));
        assert_eq!(
            resolve_validated_shift(&domain, &settings, id, &mut |_| Ok::<_, TemporalError>(()))?,
            Some(shifts[0]),
        );
        Ok(())
    }
}

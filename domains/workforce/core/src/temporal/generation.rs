use super::{
    MAX_RESOLVED_SHIFTS, ResolvedInterval, ResolvedShift, ResolvedShiftOrigin, TemporalError,
    TemporalIssueKind, check_cancelled, issue, resolve_shift_timing,
};
use crate::{
    ids::{ShiftId, ShiftTemplateId},
    model::{
        PlanningDates, ReportingAttribution, ShiftInstance, ShiftOrigin, ShiftTemplate, Weekday,
        WorkforceDomainV1, WorkforceEntity, planning_dates,
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

/// The caller has already validated global identity and template/date uniqueness.
pub(super) fn owners(domain: &WorkforceDomainV1) -> Owners {
    let mut owners = BTreeMap::new();
    for entity in domain.entities.values() {
        match entity {
            WorkforceEntity::ShiftTemplate(template) => {
                for occurrence in template.occurrence_identities.values() {
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
    owners
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

fn weekday(date: Date) -> Weekday {
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
    cancellation: &CancellationToken,
) -> Result<Vec<ShiftSpec<'a>>, TemporalError> {
    let dates = planning_dates(settings)?;
    let mut specs = Vec::new();
    for entity in domain.entities.values() {
        check_cancelled(cancellation)?;
        match entity {
            WorkforceEntity::ShiftTemplate(template) => {
                let excluded: BTreeSet<_> =
                    template.recurrence.excluded_dates.iter().copied().collect();
                for occurrence in template.occurrence_identities.values() {
                    check_cancelled(cancellation)?;
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
pub(super) fn collect_specs<'a>(
    domain: &'a WorkforceDomainV1,
    settings: &ScenarioSettings,
    owners: &Owners,
    cancellation: &CancellationToken,
) -> Result<Vec<ShiftSpec<'a>>, TemporalError> {
    let dates = planning_dates(settings)?;
    let mut specs = Vec::new();
    let mut generated = 0;
    for entity in domain.entities.values() {
        check_cancelled(cancellation)?;
        match entity {
            WorkforceEntity::ShiftTemplate(template) => {
                let recurrence = &template.recurrence;
                let mut date = dates.first_date.max(recurrence.effective_range.start_date);
                let weekdays: BTreeSet<_> = recurrence.weekdays.iter().copied().collect();
                let excluded: BTreeSet<_> = recurrence.excluded_dates.iter().copied().collect();
                let mut count = 0;
                while date <= dates.last_date
                    && date < recurrence.effective_range.end_date_exclusive
                {
                    check_cancelled(cancellation)?;
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
                            ));
                        }
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
                    if date == dates.last_date {
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
                if contains_start(settings, interval) {
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
    let dates = planning_dates(&document.settings)?;
    let owners = owners(&domain);
    let specs = collect_specs(&domain, &document.settings, &owners, cancellation)?;
    let mut resolved = Vec::with_capacity(specs.len());
    for spec in specs {
        check_cancelled(cancellation)?;
        resolved.push(spec.resolve(&document.settings, dates)?);
    }
    resolved.sort_unstable_by_key(|shift| (shift.interval.starts_at.instant, shift.id));
    check_cancelled(cancellation)?;
    Ok(resolved)
}

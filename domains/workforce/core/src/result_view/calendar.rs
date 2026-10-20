use super::{
    AssignmentPair, BTreeMap, Date, DateRange, DomainPackError, LocationId, OperationControl,
    PageKind, Person, PersonId, PersonRow, ResolvedLocalTime, ResolvedShift, ResultData,
    ResultEnvelope, ResultPosition, Serialize, ShiftId, ShiftMetadata, ShiftRow, Span,
    coverage_values, fit_page, invalid, planning_dates,
};

const SAMPLE_LIMIT: usize = 3;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CalendarAssignmentSample {
    assignment_id: String,
    shift_id: ShiftId,
    starts_at: ResolvedLocalTime,
    ends_at: ResolvedLocalTime,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersonCalendarDay {
    date: Date,
    selected_count: u32,
    selected_shifts: Vec<CalendarAssignmentSample>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PersonCalendarRow {
    person: PersonRow,
    days: Vec<PersonCalendarDay>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ShiftCalendarRow {
    shift: ShiftRow,
    date: Date,
    selected_person_ids: Vec<PersonId>,
}

fn source_window(
    document: &eutheto_types::ScenarioDocument,
    requested_start: Option<Date>,
) -> Result<(DateRange, DateRange), DomainPackError> {
    let dates = planning_dates(&document.settings)?;
    let end_date_exclusive = dates
        .last_date
        .checked_add(Span::new().days(1))
        .map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    let start_date = requested_start.unwrap_or(dates.first_date);
    if start_date < dates.first_date || start_date >= end_date_exclusive {
        return Err(invalid(
            "/query/parameters/startDate",
            "calendar startDate is outside the accepted source planning dates",
        ));
    }
    let end = start_date
        .checked_add(Span::new().days(7))
        .unwrap_or(end_date_exclusive);
    Ok((
        DateRange {
            start_date: dates.first_date,
            end_date_exclusive,
        },
        DateRange {
            start_date,
            end_date_exclusive: end.min(end_date_exclusive),
        },
    ))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn build_people_calendar<'doc>(
    people: &BTreeMap<PersonId, &Person>,
    selected_by_person: &BTreeMap<PersonId, u32>,
    selected: &BTreeMap<AssignmentPair, &str>,
    resolved: &[ResolvedShift],
    start_date: Option<Date>,
    cursor: Option<&ResultPosition>,
    limit: usize,
    document: &'doc eutheto_types::ScenarioDocument,
    context: eutheto_domain_api::SetupViewContext,
    control: &OperationControl,
) -> Result<ResultEnvelope<'doc>, DomainPackError> {
    let (planning_dates, window) = source_window(document, start_date)?;
    let cursor_id = match cursor {
        None => None,
        Some(ResultPosition::CalendarPerson { person_id }) if people.contains_key(person_id) => {
            Some(*person_id)
        }
        Some(ResultPosition::CalendarPerson { .. }) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor person is not in accepted source",
            ));
        }
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor kind does not match person calendar",
            ));
        }
    };
    let total = u32::try_from(people.len()).map_err(|_| DomainPackError::ResourceLimitExceeded)?;
    let mut rows = Vec::new();
    let mut row_index = BTreeMap::new();
    let mut has_more = false;
    for (&person_id, person) in people {
        control.check()?;
        if cursor_id.is_some_and(|id| person_id <= id) {
            continue;
        }
        if rows.len() == limit {
            has_more = true;
            break;
        }
        let mut days = Vec::new();
        let mut date = window.start_date;
        while date < window.end_date_exclusive {
            control.check()?;
            days.push(PersonCalendarDay {
                date,
                selected_count: 0,
                selected_shifts: Vec::new(),
            });
            date = date
                .checked_add(Span::new().days(1))
                .map_err(|_| DomainPackError::ResourceLimitExceeded)?;
        }
        row_index.insert(person_id, rows.len());
        rows.push((
            PersonCalendarRow {
                person: PersonRow {
                    person_id,
                    name: person.name.clone(),
                    selected_count: selected_by_person.get(&person_id).copied().unwrap_or(0),
                },
                days,
            },
            ResultPosition::CalendarPerson { person_id },
        ));
    }
    if !rows.is_empty() {
        let resolved_by_id: BTreeMap<ShiftId, &ResolvedShift> =
            resolved.iter().map(|shift| (shift.id, shift)).collect();
        for (pair, assignment_id) in selected {
            control.check()?;
            let Some(&index) = row_index.get(&pair.person_id) else {
                continue;
            };
            let shift = resolved_by_id
                .get(&pair.shift_id)
                .ok_or_else(|| invalid("/domain", "selected shift source is missing"))?;
            let date = shift.interval.starts_at.local.as_datetime().date();
            let Some(day) = rows[index].0.days.iter_mut().find(|day| day.date == date) else {
                continue;
            };
            day.selected_count = day
                .selected_count
                .checked_add(1)
                .ok_or(DomainPackError::ResourceLimitExceeded)?;
            if day.selected_shifts.len() < SAMPLE_LIMIT {
                day.selected_shifts.push(CalendarAssignmentSample {
                    assignment_id: (*assignment_id).to_owned(),
                    shift_id: pair.shift_id,
                    starts_at: shift.interval.starts_at,
                    ends_at: shift.interval.ends_at,
                });
            }
        }
    }
    let (items, continuation) = fit_page(
        rows,
        total,
        has_more,
        document.scenario_id,
        context,
        &document.settings.time_zone,
        PageKind::CalendarPeople {
            planning_dates,
            window,
        },
    )?;
    Ok(ResultEnvelope {
        schema_version: 1,
        time_zone: &document.settings.time_zone,
        result: ResultData::CalendarPeople {
            planning_dates,
            window,
            total_items: total,
            items,
            continuation,
        },
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn build_shift_calendar<'doc>(
    resolved: &[ResolvedShift],
    shifts: &BTreeMap<ShiftId, ShiftMetadata<'_>>,
    assignment_types: &BTreeMap<crate::ids::AssignmentTypeId, &crate::model::AssignmentType>,
    locations: &BTreeMap<LocationId, &crate::model::Location>,
    selected_by_shift: &BTreeMap<ShiftId, u32>,
    selected: &BTreeMap<AssignmentPair, &str>,
    location_filter: Option<LocationId>,
    start_date: Option<Date>,
    cursor: Option<&ResultPosition>,
    limit: usize,
    document: &'doc eutheto_types::ScenarioDocument,
    context: eutheto_domain_api::SetupViewContext,
    control: &OperationControl,
) -> Result<ResultEnvelope<'doc>, DomainPackError> {
    let (planning_dates, window) = source_window(document, start_date)?;
    let cursor_key = match cursor {
        None => None,
        Some(ResultPosition::CalendarShift { shift_id }) => {
            let shift = resolved
                .iter()
                .find(|shift| shift.id == *shift_id)
                .ok_or_else(|| {
                    invalid(
                        "/query/continuation/position",
                        "cursor shift is not in accepted source",
                    )
                })?;
            let metadata = shifts
                .get(shift_id)
                .ok_or_else(|| invalid("/domain", "resolved shift metadata is missing"))?;
            let date = shift.interval.starts_at.local.as_datetime().date();
            if date < window.start_date
                || date >= window.end_date_exclusive
                || location_filter.is_some_and(|id| metadata.location_id != Some(id))
            {
                return Err(invalid(
                    "/query/continuation/position",
                    "cursor shift does not match the calendar window and location",
                ));
            }
            Some((shift.interval.starts_at.instant, *shift_id))
        }
        Some(_) => {
            return Err(invalid(
                "/query/continuation/position",
                "cursor kind does not match shift calendar",
            ));
        }
    };
    let mut rows = Vec::new();
    let mut row_index = BTreeMap::new();
    let mut total = 0_u32;
    let mut has_more = false;
    for shift in resolved {
        control.check()?;
        let date = shift.interval.starts_at.local.as_datetime().date();
        if date < window.start_date || date >= window.end_date_exclusive {
            continue;
        }
        let metadata = shifts
            .get(&shift.id)
            .ok_or_else(|| invalid("/domain", "resolved shift metadata is missing"))?;
        if location_filter.is_some_and(|id| metadata.location_id != Some(id)) {
            continue;
        }
        total = total
            .checked_add(1)
            .ok_or(DomainPackError::ResourceLimitExceeded)?;
        let key = (shift.interval.starts_at.instant, shift.id);
        if cursor_key.is_some_and(|cursor_key| key <= cursor_key) {
            continue;
        }
        if rows.len() == limit {
            has_more = true;
            continue;
        }
        let selected_count = selected_by_shift.get(&shift.id).copied().unwrap_or(0);
        let (required, minimum, preferred, maximum, margin) =
            coverage_values(metadata.coverage, selected_count);
        row_index.insert(shift.id, rows.len());
        rows.push((
            ShiftCalendarRow {
                shift: ShiftRow {
                    shift_id: shift.id,
                    assignment_type_id: metadata.assignment_type_id,
                    assignment_type_name: assignment_types
                        .get(&metadata.assignment_type_id)
                        .map(|value| value.name.clone()),
                    location_id: metadata.location_id,
                    location_name: metadata
                        .location_id
                        .and_then(|id| locations.get(&id).map(|value| value.name.clone())),
                    starts_at: shift.interval.starts_at,
                    ends_at: shift.interval.ends_at,
                    required,
                    minimum,
                    preferred,
                    maximum,
                    selected_count,
                    margin,
                },
                date,
                selected_person_ids: Vec::new(),
            },
            ResultPosition::CalendarShift { shift_id: shift.id },
        ));
    }
    for pair in selected.keys() {
        control.check()?;
        if let Some(&index) = row_index.get(&pair.shift_id) {
            let ids = &mut rows[index].0.selected_person_ids;
            if ids.len() < SAMPLE_LIMIT {
                ids.push(pair.person_id);
            }
        }
    }
    let (items, continuation) = fit_page(
        rows,
        total,
        has_more,
        document.scenario_id,
        context,
        &document.settings.time_zone,
        PageKind::CalendarShifts {
            planning_dates,
            window,
        },
    )?;
    Ok(ResultEnvelope {
        schema_version: 1,
        time_zone: &document.settings.time_zone,
        result: ResultData::CalendarShifts {
            planning_dates,
            window,
            total_items: total,
            items,
            continuation,
        },
    })
}

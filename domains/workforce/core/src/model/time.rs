use crate::ids::{ShiftTemplateId, WorkCalendarId};
use eutheto_domain_api::DomainPackError;
use eutheto_types::{LocalWallTime, Rfc3339Timestamp, ScenarioSettings};
use jiff::{
    Span,
    civil::{Date, Time},
    tz::TimeZone,
};
use serde::{Deserialize, Serialize};

/// Inclusive local shift-start dates derived from the host envelope, never stored twice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanningDates {
    pub first_date: Date,
    pub last_date: Date,
}

/// Derives the planning dates from increasing scenario-zone midnight boundaries.
///
/// # Errors
///
/// Rejects a reversed, empty, non-midnight, or unresolvable host horizon.
pub fn planning_dates(settings: &ScenarioSettings) -> Result<PlanningDates, DomainPackError> {
    let invalid = || DomainPackError::InvalidPayload {
        path: "settings.horizon".to_owned(),
        message: "workforce horizon must increase between scenario-zone midnight boundaries"
            .to_owned(),
    };
    let zone = TimeZone::get(settings.time_zone.as_str()).map_err(|_| invalid())?;
    let start = settings.horizon.start.as_timestamp().to_zoned(zone.clone());
    let end = settings.horizon.end.as_timestamp().to_zoned(zone);
    if start.time() != Time::MIN
        || end.time() != Time::MIN
        || settings.horizon.start >= settings.horizon.end
        || start.date() >= end.date()
    {
        return Err(invalid());
    }
    Ok(PlanningDates {
        first_date: start.date(),
        last_date: end
            .date()
            .checked_sub(Span::new().days(1))
            .map_err(|_| invalid())?,
    })
}

/// A half-open range of local dates, resolved only in the scenario's zone.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DateRange {
    pub start_date: Date,
    pub end_date_exclusive: Date,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ActiveRange {
    Always {},
    DateRange(DateRange),
}

/// Stable weekday names; ordering is Monday through Sunday, independent of locale.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalInterval {
    pub starts_at: LocalWallTime,
    pub ends_at: LocalWallTime,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkCalendar {
    pub id: WorkCalendarId,
    pub name: String,
    pub period: CalendarPeriod,
}

/// Explicit local reporting boundaries; expansion belongs to WF-002.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CalendarPeriod {
    Day {
        start_time: Time,
    },
    Week {
        anchor_date: Date,
        start_time: Time,
    },
    PayPeriod {
        anchor_date: Date,
        start_time: Time,
        length_days: u16,
    },
    Custom {
        intervals: Vec<LocalInterval>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recurrence {
    pub weekdays: Vec<Weekday>,
    pub effective_range: DateRange,
    pub excluded_dates: Vec<Date>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ShiftTiming {
    LocalWindow {
        start_time: Time,
        end_time: Time,
        end_day_offset: u8,
    },
    ElapsedDuration {
        start_time: Time,
        duration_minutes: u32,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReportingAttribution {
    StartLocalDate,
    EndLocalDate,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ShiftOrigin {
    Manual {},
    Detached {
        template_id: ShiftTemplateId,
        occurrence_date: Date,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeeklyWindow {
    pub weekdays: Vec<Weekday>,
    pub start_time: Time,
    pub end_time: Time,
    pub end_day_offset: u8,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TimeWindow {
    Instant {
        starts_at: Rfc3339Timestamp,
        ends_at: Rfc3339Timestamp,
    },
    Weekly {
        windows: Vec<WeeklyWindow>,
    },
}

/// Whole-quantity membership differs from clipped elapsed interval membership.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowMembership {
    ReportingDate,
    StartInstant,
    Intersection,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkWindow {
    Calendar {
        calendar_id: WorkCalendarId,
        membership: WindowMembership,
    },
    Rolling {
        duration_minutes: u32,
        membership: WindowMembership,
    },
}

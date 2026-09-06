//! Bounded operation-local Workforce time calculations, without persistence or solver authority.
//! Public results are Rust values, not another stored format or an IPC approval token.

mod calendar;
mod generation;
mod review;
pub use calendar::{reporting_window, resolve_calendar};
pub use generation::resolve_shifts;
pub use review::preview_generation;

mod types;
pub use types::*;

use crate::model::ShiftTiming;
use eutheto_types::{
    CancellationToken, EntityId, LocalWallTime, ResolvedLocalTime, Rfc3339Timestamp,
    ScenarioSettings,
};
use jiff::{
    SignedDuration, Span,
    civil::{Date, DateTime},
    tz::TimeZone,
};

pub(crate) fn check_cancelled(cancellation: &CancellationToken) -> Result<(), TemporalError> {
    if cancellation.is_cancelled() {
        Err(TemporalError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) fn issue(
    kind: TemporalIssueKind,
    entity_id: Option<EntityId>,
    local_date: Option<Date>,
) -> TemporalError {
    TemporalError::Issue(TemporalIssue {
        kind,
        entity_id,
        local_date,
    })
}

pub(crate) fn resolve_endpoint(
    local: DateTime,
    settings: &ScenarioSettings,
    entity_id: EntityId,
) -> Result<ResolvedLocalTime, TemporalError> {
    eutheto_types::resolve_local_time(
        LocalWallTime::from_datetime(local),
        &settings.time_zone,
        settings.gap_policy,
        settings.overlap_policy,
    )
    .map_err(|error| {
        issue(
            TemporalIssueKind::Resolution(error.kind),
            Some(entity_id),
            Some(local.date()),
        )
    })
}

pub(crate) fn checked_interval(
    starts_at: ResolvedLocalTime,
    ends_at: ResolvedLocalTime,
    entity_id: EntityId,
) -> Result<ResolvedInterval, TemporalError> {
    if starts_at.instant >= ends_at.instant {
        return Err(issue(
            TemporalIssueKind::InvalidInterval,
            Some(entity_id),
            Some(starts_at.local.as_datetime().date()),
        ));
    }
    Ok(ResolvedInterval { starts_at, ends_at })
}

pub(crate) fn resolve_interval(
    start: DateTime,
    end: DateTime,
    settings: &ScenarioSettings,
    entity_id: EntityId,
) -> Result<ResolvedInterval, TemporalError> {
    checked_interval(
        resolve_endpoint(start, settings, entity_id)?,
        resolve_endpoint(end, settings, entity_id)?,
        entity_id,
    )
}

pub(crate) fn resolve_shift_timing(
    timing: ShiftTiming,
    date: Date,
    settings: &ScenarioSettings,
    entity_id: EntityId,
) -> Result<ResolvedInterval, TemporalError> {
    let overflow = || issue(TemporalIssueKind::DateOverflow, Some(entity_id), Some(date));
    match timing {
        ShiftTiming::LocalWindow {
            start_time,
            end_time,
            end_day_offset,
        } => {
            let end_date = date
                .checked_add(Span::new().days(i64::from(end_day_offset)))
                .map_err(|_| overflow())?;
            resolve_interval(
                date.to_datetime(start_time),
                end_date.to_datetime(end_time),
                settings,
                entity_id,
            )
        }
        ShiftTiming::ElapsedDuration {
            start_time,
            duration_minutes,
        } => {
            let start = resolve_endpoint(date.to_datetime(start_time), settings, entity_id)?;
            // u32 minutes fit in SignedDuration's i64-second representation without narrowing.
            let instant = start
                .instant
                .as_timestamp()
                .checked_add(SignedDuration::from_mins(i64::from(duration_minutes)))
                .map_err(|_| overflow())?;
            let zone = TimeZone::get(settings.time_zone.as_str()).map_err(|_| overflow())?;
            let end = instant.to_zoned(zone);
            checked_interval(
                start,
                ResolvedLocalTime {
                    instant: Rfc3339Timestamp::from_timestamp(instant),
                    local: LocalWallTime::from_datetime(end.datetime()),
                    offset_seconds: end.offset().seconds(),
                },
                entity_id,
            )
        }
    }
}

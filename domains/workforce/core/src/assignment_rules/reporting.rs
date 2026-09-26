use super::{
    AssignmentConstructionIssue, AssignmentRuleError,
    budget::{OperationBudget, count},
    input::AssignmentInput,
};
use crate::ids::WorkCalendarId;
use crate::{
    model::WorkforceEntity,
    temporal::{CalendarWindow, reporting_window_for_calendar},
};
use eutheto_types::{CancellationToken, OperationControl, Rfc3339Timestamp, ScenarioDocument};
use jiff::civil::Date;
use serde::Serialize;
use std::collections::BTreeMap;

/// Canonical identity for one reporting-date-owned calendar period.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(super) struct ReportingPeriod {
    pub calendar_id: WorkCalendarId,
    pub start: Rfc3339Timestamp,
    pub end: Rfc3339Timestamp,
}

impl ReportingPeriod {
    fn from_window(window: CalendarWindow) -> Self {
        Self {
            calendar_id: window.calendar_id,
            start: window.interval.starts_at.instant,
            end: window.interval.ends_at.instant,
        }
    }
}

/// Operation-local cache of distinct reporting-date lookups. The source evaluator and compiler
/// each own a fresh cache; neither consumes the other's derived periods or candidate data.
pub(super) struct ReportingWindows {
    windows: BTreeMap<(WorkCalendarId, Date), Option<ReportingPeriod>>,
}

impl ReportingWindows {
    pub(super) fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
        }
    }

    pub(super) fn resolve(
        &mut self,
        document: &ScenarioDocument,
        input: &AssignmentInput,
        calendar_id: WorkCalendarId,
        date: Date,
        budget: &mut OperationBudget<'_>,
    ) -> Result<Option<ReportingPeriod>, AssignmentRuleError> {
        budget.check()?;
        let key = (calendar_id, date);
        if let Some(period) = self.windows.get(&key) {
            return Ok(*period);
        }
        let Some(WorkforceEntity::Calendar(calendar)) =
            input.domain.entities.get(&calendar_id.as_entity_id())
        else {
            return Err(AssignmentRuleError::InvalidConstruction(
                AssignmentConstructionIssue::InvalidRecord,
            ));
        };
        let cancellation = match budget.control() {
            Some(OperationControl::Cancellation(token)) => token.clone(),
            _ => CancellationToken::new(),
        };
        budget.step()?;
        let interval_count = match &calendar.period {
            crate::model::CalendarPeriod::Custom { intervals } => count(intervals.len())?,
            _ => 1,
        };
        // Temporal resolution checks its own cancellation token; charge the bounded source scan
        // here as well so custom calendars cannot hide work from the shared operation budget.
        budget.steps(interval_count)?;
        let period =
            reporting_window_for_calendar(document, calendar_id, calendar, date, &cancellation)?
                .map(ReportingPeriod::from_window);
        budget.check()?;
        // This retained cache is bounded by the operation's own record/item/byte ceilings.
        budget.reserve(1, 1, 128)?;
        self.windows.insert(key, period);
        Ok(period)
    }
}

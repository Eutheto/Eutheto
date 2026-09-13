use super::{EuthetoApp, store_error, validation_error};
use eutheto_types::{AppError, CommandId, CommandSource, Revision, Rfc3339Timestamp, ScenarioId};
use serde::{Deserialize, Serialize};

pub use eutheto_store::{HISTORY_PAGE_MAX_ENTRIES, HISTORY_SUMMARY_MAX_BYTES};

/// Current metadata history wire schema.
pub const HISTORY_API_SCHEMA_VERSION: u32 = 1;
/// Native history request admission budget, before typed decoding.
pub const HISTORY_PAGE_MAX_REQUEST_BYTES: usize = 4 * 1024;
/// Native history response encoding budget, including JSON escaping.
pub const HISTORY_PAGE_MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Descending keyset cursor bound to one immutable scenario revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryPageContinuationV1 {
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub before_sequence: u64,
}

/// Requests a bounded projection, never the journal's command or actor payloads.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryPageRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub limit: u32,
    pub continuation: Option<HistoryPageContinuationV1>,
}

impl HistoryPageRequestV1 {
    /// Validates fixed-size request semantics before entering the store actor queue.
    ///
    /// # Errors
    ///
    /// Rejects unsupported versions, invalid limits, unsafe sequence integers, and
    /// continuation identities or revisions that differ from the request context.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != HISTORY_API_SCHEMA_VERSION {
            return Err(validation_error(
                "history.schema_version_unsupported",
                "/schemaVersion",
                "The history request schema version is not supported.",
            ));
        }
        if !(1..=HISTORY_PAGE_MAX_ENTRIES).contains(&self.limit) {
            return Err(validation_error(
                "history.limit_invalid",
                "/limit",
                "The history page limit must be between 1 and 100.",
            ));
        }
        if let Some(continuation) = &self.continuation
            && (continuation.scenario_id != self.scenario_id
                || continuation.revision != self.expected_revision
                || continuation.before_sequence == 0
                || Revision::try_new(continuation.before_sequence).is_err())
        {
            return Err(validation_error(
                "history.continuation_invalid",
                "/continuation",
                "The history continuation must match the requested scenario and revision and contain a valid sequence.",
            ));
        }
        Ok(())
    }
}

/// Bounded recorded metadata for one command on the retained journal branch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryEntrySummaryDtoV1 {
    pub id: CommandId,
    pub revision_before: Revision,
    pub revision_after: Revision,
    pub source: CommandSource,
    /// Null means the recorded summary exceeded the UTF-8 display byte limit.
    pub summary: Option<String>,
    pub created_at: Rfc3339Timestamp,
    pub history_sequence: u64,
    pub branch_generation: u64,
    pub applied: bool,
}

/// Metadata and undo/redo availability captured in the same read transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryPageDtoV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub revision: Revision,
    pub entries: Vec<HistoryEntrySummaryDtoV1>,
    pub continuation: Option<HistoryPageContinuationV1>,
    pub undo_available: bool,
    pub redo_available: bool,
}

impl EuthetoApp {
    /// Reads only a bounded, revision-consistent journal metadata page.
    ///
    /// # Errors
    ///
    /// Returns validation, cancellation, not-found, conflict, or storage errors.
    pub async fn history_page(
        &self,
        request: HistoryPageRequestV1,
    ) -> Result<HistoryPageDtoV1, AppError> {
        request.validate()?;
        self.check_cancelled()?;
        let page = self
            .store
            .history_page(
                request.scenario_id,
                request.expected_revision,
                request.limit,
                request.continuation.map(|cursor| cursor.before_sequence),
            )
            .await
            .map_err(store_error)?;
        Ok(HistoryPageDtoV1 {
            schema_version: HISTORY_API_SCHEMA_VERSION,
            scenario_id: page.scenario_id,
            revision: page.revision,
            entries: page
                .entries
                .into_iter()
                .map(|entry| HistoryEntrySummaryDtoV1 {
                    id: entry.id,
                    revision_before: entry.revision_before,
                    revision_after: entry.revision_after,
                    source: entry.source,
                    summary: entry.summary,
                    created_at: entry.created_at,
                    history_sequence: entry.history_sequence,
                    branch_generation: entry.branch_generation,
                    applied: entry.applied,
                })
                .collect(),
            continuation: page.next_before_sequence.map(|before_sequence| {
                HistoryPageContinuationV1 {
                    scenario_id: page.scenario_id,
                    revision: page.revision,
                    before_sequence,
                }
            }),
            undo_available: page.undo_available,
            redo_available: page.redo_available,
        })
    }
}

//! Native reader custody for ephemeral Workforce CSV reviews and ordinary durable commands.

use super::{
    EuthetoApp, MAX_PENDING_PREVIEW_BYTES, MAX_PENDING_PREVIEWS, PendingPortablePreview,
    pending_tree_memory_charge, preview_total_bytes, protocol_error, store_error,
};
pub use eutheto_domain_api::bounded_json_size;
use eutheto_types::{
    ActorRef, AppError, CancellationToken, CommandBatch, CommandEnvelope, CommandId, CommandSource,
    RequestId, Revision, ScenarioCommand, ScenarioId,
};
// Public CSV DTO fields and admission limits let native clients avoid a direct pack dependency.
use eutheto_workforce::people_csv::{
    self as csv, CsvError, CsvErrorCode, MAX_CSV_REJECTED_REPORT_BYTES, MAX_CSV_REJECTED_ROWS,
    MAX_CSV_REVIEW_BYTES, PeopleCsvDetection, PeopleImportDisposition, RejectedRow,
};
pub use eutheto_workforce::people_csv::{
    MAX_CSV_DECISION_BYTES, MAX_CSV_DECISIONS, MAX_CSV_MAPPING_BYTES, MAX_CSV_SOURCE_BYTES,
    PeopleCsvMapping, PeopleImportPreview, RowDecision,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read},
    mem::size_of_val,
    sync::Arc,
};

/// Version of the native application's serialized CSV requests and receipts.
pub const PEOPLE_CSV_API_SCHEMA_VERSION: u32 = 1;

/// Cloneable signal; cancelling it affects only this operation and its descendants.
#[derive(Clone, Debug)]
pub struct PeopleCsvCancellation {
    token: CancellationToken,
}

impl PeopleCsvCancellation {
    /// Requests cooperative cancellation, without claiming rollback or interrupting a system call.
    pub fn cancel(&self) {
        self.token.cancel();
    }

    /// Reports a request, not the eventual commit outcome.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

/// Caller-owned cancel-on-drop guard. Move this into exactly one operation future.
///
/// Dropping an apply future abandons its receipt but does not abort its finite owner.
/// Reconcile the caller's known `CommandId` through ordinary durable history; absence
/// while the owner is still running is not proof of rollback.
#[derive(Debug)]
pub struct PeopleCsvOperation {
    token: CancellationToken,
}

impl PeopleCsvOperation {
    /// Creates a cancel-on-drop child of a caller-owned operation.
    ///
    /// Parent cancellation reaches CSV work; dropping this guard never cancels
    /// the parent or sibling operations.
    #[must_use]
    pub fn child_of(parent: &CancellationToken) -> Self {
        Self {
            token: parent.child(),
        }
    }

    /// Returns a signal without sharing ownership of this guard.
    #[must_use]
    pub fn cancellation(&self) -> PeopleCsvCancellation {
        PeopleCsvCancellation {
            token: self.token.clone(),
        }
    }
}

impl Drop for PeopleCsvOperation {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

/// Explicit native review policy. The source reader is a separate, nonserialized argument.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeopleCsvPreviewRequestV1 {
    pub schema_version: u32,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub mapping: PeopleCsvMapping,
    pub decisions: Vec<RowDecision>,
}

/// Bounded proposal, not apply authority. Approval must be supplied separately on apply.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeopleCsvPreviewDtoV1 {
    pub schema_version: u32,
    pub preview_id: RequestId,
    pub preview: PeopleImportPreview,
}

/// A known ordinary command identity and independently approved digest.
/// There is intentionally no caller-provided document, batch, mapping or decision field.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeopleCsvApplyRequestV1 {
    pub schema_version: u32,
    pub preview_id: RequestId,
    pub scenario_id: ScenarioId,
    pub expected_revision: Revision,
    pub approved_digest: String,
    pub command_id: CommandId,
    pub request_id: RequestId,
    pub actor: ActorRef,
    pub source: CommandSource,
    pub truncate_redo: bool,
}

/// Safe report data remains ephemeral, including after a durable command commits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeopleCsvRejectedRowsDtoV1 {
    pub schema_version: u32,
    pub preview_id: RequestId,
    pub disposition: PeopleImportDisposition,
    pub consumed: bool,
    pub rejected_rows: Vec<RejectedRow>,
}

/// A no-op has no command/history identity and does not advance the revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PeopleCsvApplyOutcomeV1 {
    Applied {
        command_id: CommandId,
        revision: Revision,
    },
    NoChanges {
        revision: Revision,
    },
}

/// Returned only after the finite owner has settled the ordinary command outcome.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeopleCsvApplyDtoV1 {
    pub schema_version: u32,
    pub outcome: PeopleCsvApplyOutcomeV1,
    pub report: PeopleCsvRejectedRowsDtoV1,
}

struct ReviewAuthority {
    review: Box<[u8]>,
    digest: Box<str>,
}

#[derive(Clone)]
pub(super) struct PendingPeopleCsvPreview {
    authority: Option<Arc<ReviewAuthority>>,
    disposition: PeopleImportDisposition,
    rejected_rows: Arc<[RejectedRow]>,
    consumed: bool,
}

impl PendingPeopleCsvPreview {
    pub(super) fn retained_bytes(&self) -> usize {
        // Exact-sized payloads, plus conservative allocation and existing tree-node charges.
        pending_tree_memory_charge(1, size_of::<(RequestId, PendingPortablePreview)>())
            .unwrap_or(usize::MAX)
            .saturating_add(size_of_val(self.rejected_rows.as_ref()))
            .saturating_add(2 * size_of::<usize>())
            .saturating_add(self.authority.as_ref().map_or(0, |authority| {
                size_of::<ReviewAuthority>()
                    + 2 * size_of::<usize>()
                    + authority.review.len()
                    + authority.digest.len()
            }))
    }

    fn report(&self, preview_id: RequestId) -> PeopleCsvRejectedRowsDtoV1 {
        PeopleCsvRejectedRowsDtoV1 {
            schema_version: PEOPLE_CSV_API_SCHEMA_VERSION,
            preview_id,
            disposition: self.disposition,
            consumed: self.consumed,
            rejected_rows: self.rejected_rows.to_vec(),
        }
    }

    fn from_preview(preview: &PeopleImportPreview) -> Result<Self, AppError> {
        if preview.rejected_rows.len() > MAX_CSV_REJECTED_ROWS {
            return Err(csv_error_code(CsvErrorCode::RejectedReportLimit));
        }
        bounded_json_size(&preview.rejected_rows, MAX_CSV_REJECTED_REPORT_BYTES)
            .map_err(|_| csv_error_code(CsvErrorCode::RejectedReportLimit))?;
        let authority = match (&preview.review, &preview.approval_digest) {
            (Some(review), Some(digest)) => {
                bounded_json_size(review, MAX_CSV_REVIEW_BYTES)
                    .map_err(|_| csv_error_code(CsvErrorCode::ReviewLimit))?;
                let bytes = serde_json::to_vec(review)
                    .map_err(|_| csv_error_code(CsvErrorCode::InvalidReview))?;
                Some(Arc::new(ReviewAuthority {
                    review: bytes.into_boxed_slice(),
                    digest: digest.clone().into_boxed_str(),
                }))
            }
            (None, None) if preview.disposition == PeopleImportDisposition::Blocked => None,
            _ => return Err(csv_error_code(CsvErrorCode::InvalidReview)),
        };
        Ok(Self {
            authority,
            disposition: preview.disposition,
            rejected_rows: Arc::from(preview.rejected_rows.as_slice()),
            consumed: false,
        })
    }
}

impl EuthetoApp {
    /// Creates an app-rooted operation guard; pass it by value to one native CSV operation.
    #[must_use]
    pub fn people_csv_operation(&self) -> PeopleCsvOperation {
        PeopleCsvOperation::child_of(&self.cancellation)
    }

    /// Reads at most the source cap plus one overflow byte on the blocking pool.
    /// The reader is trusted native custody, not evidence of a filesystem grant.
    ///
    /// # Errors
    /// Returns safe CSV input/limit/cancellation codes or a safe task failure.
    pub async fn detect_people_csv<R: Read + Send + 'static>(
        &self,
        mut input: R,
        operation: PeopleCsvOperation,
    ) -> Result<PeopleCsvDetection, AppError> {
        let token = operation.token.clone();
        tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                check_cancelled(&token)?;
                let available = (MAX_CSV_SOURCE_BYTES + 1 - bytes.len()).min(chunk.len());
                let count = match input.read(&mut chunk[..available]) {
                    Ok(count) => count,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err(csv_error_code(CsvErrorCode::Io)),
                };
                check_cancelled(&token)?;
                if bytes.len() + count > MAX_CSV_SOURCE_BYTES {
                    return Err(csv_error_code(CsvErrorCode::SourceByteLimit));
                }
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            csv::detect_people_csv(&bytes, &token).map_err(csv_error)
        })
        .await
        .map_err(super::join_error)?
    }

    /// Streams a preview against the current registered Workforce document and revision.
    /// Nothing is written to scenario state, journal or history.
    ///
    /// # Errors
    /// Rejects stale/wrong-pack state, invalid policy/input, cancellation and retention limits.
    pub async fn preview_people_csv<R: Read + Send + 'static>(
        &self,
        request: PeopleCsvPreviewRequestV1,
        mut input: R,
        operation: PeopleCsvOperation,
    ) -> Result<PeopleCsvPreviewDtoV1, AppError> {
        ensure_version(request.schema_version)?;
        check_cancelled(&operation.token)?;
        let project = self
            .csv_project(request.scenario_id, request.expected_revision)
            .await?;
        let token = operation.token.clone();
        // Pure preview validates all mapping/decision bounds before cloning or serializing them.
        let preview = tokio::task::spawn_blocking(move || {
            csv::preview_people_csv(
                &project.document,
                request.expected_revision,
                &mut input,
                &request.mapping,
                &request.decisions,
                &token,
            )
            .map_err(csv_error)
        })
        .await
        .map_err(super::join_error)??;
        let pending = PendingPeopleCsvPreview::from_preview(&preview)?;
        let retained_bytes = pending.retained_bytes();
        if retained_bytes > MAX_PENDING_PREVIEW_BYTES {
            return Err(csv_error_code(CsvErrorCode::PreviewLimit));
        }
        let mut previews = self.previews.lock().await;
        check_cancelled(&operation.token)?;
        let preview_id = self
            .next_preview_id(&previews)?
            .ok_or_else(|| csv_protocol(CsvHostError::PreviewIdUnavailable))?;
        check_cancelled(&operation.token)?;
        while previews.len() >= MAX_PENDING_PREVIEWS
            || preview_total_bytes(&previews).saturating_add(retained_bytes)
                > MAX_PENDING_PREVIEW_BYTES
        {
            let Some(oldest) = previews.keys().next().copied() else {
                break;
            };
            previews.remove(&oldest);
        }
        previews.insert(preview_id, PendingPortablePreview::PeopleCsv(pending));
        Ok(PeopleCsvPreviewDtoV1 {
            schema_version: PEOPLE_CSV_API_SCHEMA_VERSION,
            preview_id,
            preview,
        })
    }

    /// Rebuilds a retained strict review against fresh input, then uses ordinary command authority.
    ///
    /// A single detached-on-drop owner completes rebuild, transaction, ordinary events and
    /// best-effort consumption. Cancellation after the store's final check loses to commit;
    /// successful commit is never replaced by cancellation or a missing-cache error.
    ///
    /// # Errors
    /// Rejects missing/wrong-kind/consumed/blocked reviews, mismatched approval, changed source,
    /// stale revisions, invalid requests, cancellation before commit and ordinary command failures.
    pub async fn apply_people_csv_import<R: Read + Send + 'static>(
        &self,
        request: PeopleCsvApplyRequestV1,
        input: R,
        operation: PeopleCsvOperation,
    ) -> Result<PeopleCsvApplyDtoV1, AppError> {
        validate_apply_request(&request)?;
        let app = self.clone();
        let token = operation.token.clone();
        // Do not select on cancellation or abort this task. The caller alone owns the guard.
        tokio::spawn(async move { app.apply_people_csv_owned(request, input, token).await })
            .await
            .map_err(super::join_error)?
    }

    async fn apply_people_csv_owned<R: Read + Send + 'static>(
        &self,
        request: PeopleCsvApplyRequestV1,
        mut input: R,
        token: CancellationToken,
    ) -> Result<PeopleCsvApplyDtoV1, AppError> {
        check_cancelled(&token)?;
        let pending = self.pending_people_csv(request.preview_id).await?;
        if pending.consumed {
            return Err(csv_protocol(CsvHostError::PreviewConsumed));
        }
        if pending.disposition == PeopleImportDisposition::Blocked {
            return Err(csv_protocol(CsvHostError::PreviewBlocked));
        }
        let authority = pending
            .authority
            .as_ref()
            .ok_or_else(|| csv_protocol(CsvHostError::PreviewConsumed))?;
        if authority.digest.as_ref() != request.approved_digest {
            return Err(csv_protocol(CsvHostError::ApprovalMismatch));
        }
        let project = self
            .csv_project(request.scenario_id, request.expected_revision)
            .await?;
        let authority = Arc::clone(authority);
        let rebuild_token = token.clone();
        let approved_digest = request.approved_digest;
        let rebuilt = tokio::task::spawn_blocking(move || {
            let review = csv::decode_people_csv_review(&authority.review).map_err(csv_error)?;
            csv::rebuild_review(
                &project.document,
                project.summary.revision,
                &mut input,
                &review,
                &approved_digest,
                &rebuild_token,
            )
            .map_err(csv_error)
        })
        .await
        .map_err(super::join_error)??;
        check_cancelled(&token)?;
        let outcome = match rebuilt.batch {
            Some(batch) => {
                let envelope = CommandEnvelope {
                    command_id: request.command_id,
                    scenario_id: request.scenario_id,
                    expected_revision: request.expected_revision,
                    actor: request.actor,
                    source: request.source,
                    command: ScenarioCommand::ApplyBatch(CommandBatch {
                        label: batch.label,
                        commands: batch
                            .commands
                            .into_iter()
                            .map(ScenarioCommand::ApplyDomainCommand)
                            .collect(),
                    }),
                };
                let result = self
                    .apply_scenario(request.request_id, envelope, request.truncate_redo, token)
                    .await?;
                PeopleCsvApplyOutcomeV1::Applied {
                    command_id: request.command_id,
                    revision: result.new_revision,
                }
            }
            None if rebuilt.disposition == PeopleImportDisposition::NoChanges => {
                // Even an approved no-op must observe the actual current revision, not just
                // its pre-rebuild snapshot. There is deliberately no empty write transaction.
                let lock = self.scenario_lock(request.scenario_id).await;
                let _guard = lock.lock().await;
                self.csv_project(request.scenario_id, request.expected_revision)
                    .await?;
                self.consume_no_change_review(request.preview_id, &pending, &token)
                    .await?;
                PeopleCsvApplyOutcomeV1::NoChanges {
                    revision: request.expected_revision,
                }
            }
            None => return Err(csv_error_code(CsvErrorCode::InvalidReview)),
        };
        // A durable commit cannot be turned into failure by cache eviction or discard.
        if matches!(outcome, PeopleCsvApplyOutcomeV1::Applied { .. }) {
            let mut previews = self.previews.lock().await;
            if let Some(PendingPortablePreview::PeopleCsv(retained)) =
                previews.get_mut(&request.preview_id)
                && retained
                    .authority
                    .as_ref()
                    .zip(pending.authority.as_ref())
                    .is_some_and(|(current, used)| Arc::ptr_eq(current, used))
            {
                retained.authority = None;
                retained.consumed = true;
            }
        }
        let mut report = pending.report(request.preview_id);
        report.consumed = true;
        Ok(PeopleCsvApplyDtoV1 {
            schema_version: PEOPLE_CSV_API_SCHEMA_VERSION,
            outcome,
            report,
        })
    }

    async fn consume_no_change_review(
        &self,
        preview_id: RequestId,
        expected: &PendingPeopleCsvPreview,
        token: &CancellationToken,
    ) -> Result<(), AppError> {
        let mut previews = self.previews.lock().await;
        check_cancelled(token)?;
        let retained = match previews.get_mut(&preview_id) {
            Some(PendingPortablePreview::PeopleCsv(preview)) => preview,
            Some(_) => return Err(csv_protocol(CsvHostError::PreviewKindMismatch)),
            None => return Err(csv_protocol(CsvHostError::PreviewUnavailable)),
        };
        if retained.consumed {
            return Err(csv_protocol(CsvHostError::PreviewConsumed));
        }
        if !retained
            .authority
            .as_ref()
            .zip(expected.authority.as_ref())
            .is_some_and(|(current, used)| Arc::ptr_eq(current, used))
        {
            return Err(csv_protocol(CsvHostError::PreviewUnavailable));
        }
        retained.authority = None;
        retained.consumed = true;
        Ok(())
    }

    /// Retrieves safe bounded rows, including from consumed/blocked previews.
    ///
    /// # Errors
    /// `people_csv.preview_unavailable` explicitly includes eviction, discard and restart:
    /// reports are ephemeral and cannot be reconstructed from durable history.
    pub async fn people_csv_rejected_rows(
        &self,
        preview_id: RequestId,
    ) -> Result<PeopleCsvRejectedRowsDtoV1, AppError> {
        Ok(self
            .pending_people_csv(preview_id)
            .await?
            .report(preview_id))
    }

    /// Releases only CSV cache data; it does not cancel an in-flight operation or claim rollback.
    ///
    /// # Errors
    /// Returns unavailable or wrong-kind without removing any other kind of preview.
    pub async fn discard_people_csv_preview(&self, preview_id: RequestId) -> Result<(), AppError> {
        let mut previews = self.previews.lock().await;
        match previews.get(&preview_id) {
            Some(PendingPortablePreview::PeopleCsv(_)) => {}
            Some(_) => return Err(csv_protocol(CsvHostError::PreviewKindMismatch)),
            None => return Err(csv_protocol(CsvHostError::PreviewUnavailable)),
        }
        previews.remove(&preview_id);
        Ok(())
    }

    async fn pending_people_csv(
        &self,
        preview_id: RequestId,
    ) -> Result<PendingPeopleCsvPreview, AppError> {
        match self.previews.lock().await.get(&preview_id) {
            Some(PendingPortablePreview::PeopleCsv(preview)) => Ok(preview.clone()),
            Some(_) => Err(csv_protocol(CsvHostError::PreviewKindMismatch)),
            None => Err(csv_protocol(CsvHostError::PreviewUnavailable)),
        }
    }

    async fn csv_project(
        &self,
        scenario_id: ScenarioId,
        revision: Revision,
    ) -> Result<eutheto_store::StoredProject, AppError> {
        let project = self
            .store
            .get_project(scenario_id)
            .await
            .map_err(store_error)?;
        if project.summary.revision != revision {
            return Err(AppError::Conflict {
                expected_revision: revision,
                actual_revision: project.summary.revision,
            });
        }
        if project.document.domain_pack.id.as_str() != "official.workforce"
            || super::available_pack(&project.document, &self.pack_registry).is_none()
        {
            return Err(csv_error_code(CsvErrorCode::InvalidCurrentDocument));
        }
        Ok(project)
    }
}

fn check_cancelled(token: &CancellationToken) -> Result<(), AppError> {
    if token.is_cancelled() {
        Err(csv_error_code(CsvErrorCode::Cancelled))
    } else {
        Ok(())
    }
}

fn ensure_version(version: u32) -> Result<(), AppError> {
    if version == PEOPLE_CSV_API_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(csv_error_code(CsvErrorCode::UnsupportedVersion))
    }
}

fn validate_apply_request(request: &PeopleCsvApplyRequestV1) -> Result<(), AppError> {
    ensure_version(request.schema_version)?;
    if request.approved_digest.len() != 64
        || !request
            .approved_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !super::valid_command_actor(&request.actor)
        || request.command_id.as_uuid().get_version_num() != 7
        || request.request_id.as_uuid().get_version_num() != 7
        || request.scenario_id.as_uuid().get_version_num() != 7
        || request.preview_id.as_uuid().get_version_num() != 7
    {
        return Err(csv_protocol(CsvHostError::InvalidRequest));
    }
    Ok(())
}

fn csv_error_code(code: CsvErrorCode) -> AppError {
    csv_error(CsvError { code, record: None })
}

fn csv_error(error: CsvError) -> AppError {
    if error.code == CsvErrorCode::Cancelled {
        return store_error(eutheto_store::StoreError::OperationCancelled);
    }
    let code = csv_code(error.code);
    let message = csv_message(error.code);
    if let Some(record) = error.record {
        super::validation_error(code, &format!("/peopleCsv/records/{record}"), message)
    } else {
        protocol_error(code, message, false)
    }
}

fn csv_code(code: CsvErrorCode) -> &'static str {
    match code {
        CsvErrorCode::Io => "people_csv.io",
        CsvErrorCode::Cancelled => "operation.cancelled",
        CsvErrorCode::UnsupportedEncoding => "people_csv.unsupportedEncoding",
        CsvErrorCode::InvalidUtf8 => "people_csv.invalidUtf8",
        CsvErrorCode::BinaryControl => "people_csv.binaryControl",
        CsvErrorCode::SourceByteLimit => "people_csv.sourceByteLimit",
        CsvErrorCode::CellLimit => "people_csv.cellLimit",
        CsvErrorCode::RecordByteLimit => "people_csv.recordByteLimit",
        CsvErrorCode::ColumnLimit => "people_csv.columnLimit",
        CsvErrorCode::LogicalRecordLimit => "people_csv.logicalRecordLimit",
        CsvErrorCode::ParserNoProgress => "people_csv.parserNoProgress",
        CsvErrorCode::DetectionLimit => "people_csv.detectionLimit",
        CsvErrorCode::InvalidMapping => "people_csv.invalidMapping",
        CsvErrorCode::MappingLimit => "people_csv.mappingLimit",
        CsvErrorCode::HeaderMismatch => "people_csv.headerMismatch",
        CsvErrorCode::DataRecordLimit => "people_csv.dataRecordLimit",
        CsvErrorCode::InvalidDecision => "people_csv.invalidDecision",
        CsvErrorCode::DecisionLimit => "people_csv.decisionLimit",
        CsvErrorCode::MutationLimit => "people_csv.mutationLimit",
        CsvErrorCode::RejectedReportLimit => "people_csv.rejectedReportLimit",
        CsvErrorCode::ValidationReportLimit => "people_csv.validationReportLimit",
        CsvErrorCode::PreviewLimit => "people_csv.previewLimit",
        CsvErrorCode::ReviewLimit => "people_csv.reviewLimit",
        CsvErrorCode::UnsupportedVersion => "people_csv.unsupportedVersion",
        CsvErrorCode::InvalidCurrentDocument => "people_csv.invalidCurrentDocument",
        CsvErrorCode::InvalidReview => "people_csv.invalidReview",
        CsvErrorCode::StaleReview => "people_csv.staleReview",
        CsvErrorCode::InvalidBatch => "people_csv.invalidBatch",
    }
}

fn csv_message(code: CsvErrorCode) -> &'static str {
    match code {
        CsvErrorCode::Io => "The native CSV reader could not read the source.",
        CsvErrorCode::Cancelled => "The operation was cancelled.",
        CsvErrorCode::UnsupportedEncoding => "The CSV source must use supported UTF-8 encoding.",
        CsvErrorCode::InvalidUtf8 => "The CSV source contains invalid UTF-8.",
        CsvErrorCode::BinaryControl => {
            "The CSV source contains disallowed binary control characters."
        }
        CsvErrorCode::SourceByteLimit => "The CSV source exceeds the source byte limit.",
        CsvErrorCode::CellLimit => "A CSV cell exceeds the cell byte limit.",
        CsvErrorCode::RecordByteLimit => "A CSV logical record exceeds the record byte limit.",
        CsvErrorCode::ColumnLimit => "A CSV logical record exceeds the column limit.",
        CsvErrorCode::LogicalRecordLimit => "The CSV source exceeds the logical record limit.",
        CsvErrorCode::ParserNoProgress => "The CSV parser could not make progress.",
        CsvErrorCode::DetectionLimit => "The CSV detection response exceeds its safe output limit.",
        CsvErrorCode::InvalidMapping => "The explicit CSV mapping is invalid.",
        CsvErrorCode::MappingLimit => "The CSV mapping exceeds its bounded policy limits.",
        CsvErrorCode::HeaderMismatch => "The CSV header does not match the explicit mapping.",
        CsvErrorCode::DataRecordLimit => "The CSV source exceeds the data record limit.",
        CsvErrorCode::InvalidDecision => "An explicit CSV identity decision is invalid.",
        CsvErrorCode::DecisionLimit => "The CSV identity decisions exceed their policy limits.",
        CsvErrorCode::MutationLimit => "The proposed CSV import exceeds the mutation limit.",
        CsvErrorCode::RejectedReportLimit => {
            "The rejected-row report exceeds its safe output limit."
        }
        CsvErrorCode::ValidationReportLimit => {
            "The CSV validation report exceeds its safe output limit."
        }
        CsvErrorCode::PreviewLimit => "The CSV preview exceeds its safe output or retention limit.",
        CsvErrorCode::ReviewLimit => "The CSV review exceeds its byte limit.",
        CsvErrorCode::UnsupportedVersion => "The CSV request or review version is unsupported.",
        CsvErrorCode::InvalidCurrentDocument => {
            "The current scenario is not a valid registered Workforce document."
        }
        CsvErrorCode::InvalidReview => "The CSV review is invalid and requires a fresh preview.",
        CsvErrorCode::StaleReview => {
            "The source, scenario, or review binding changed; a fresh preview and approval are required."
        }
        CsvErrorCode::InvalidBatch => {
            "The proposed CSV batch violates the ordinary command contract."
        }
    }
}

#[derive(Clone, Copy)]
enum CsvHostError {
    PreviewIdUnavailable,
    PreviewConsumed,
    PreviewBlocked,
    ApprovalMismatch,
    PreviewKindMismatch,
    PreviewUnavailable,
    InvalidRequest,
}

fn csv_protocol(error: CsvHostError) -> AppError {
    let (code, message) = match error {
        CsvHostError::PreviewIdUnavailable => (
            "people_csv.preview_id_unavailable",
            "A unique CSV preview identity could not be allocated.",
        ),
        CsvHostError::PreviewConsumed => (
            "people_csv.preview_consumed",
            "This CSV review has already been consumed and cannot authorize another apply.",
        ),
        CsvHostError::PreviewBlocked => (
            "people_csv.preview_blocked",
            "This CSV preview is blocked; resolve its review findings and preview again.",
        ),
        CsvHostError::ApprovalMismatch => (
            "people_csv.approval_mismatch",
            "The independently approved digest does not match the retained CSV review.",
        ),
        CsvHostError::PreviewKindMismatch => (
            "people_csv.preview_kind_mismatch",
            "This preview belongs to a different capability and cannot be used for people CSV.",
        ),
        CsvHostError::PreviewUnavailable => (
            "people_csv.preview_unavailable",
            "The CSV preview is unavailable. Reports are ephemeral and unavailable after eviction, discard or restart.",
        ),
        CsvHostError::InvalidRequest => (
            "people_csv.invalid_request",
            "The CSV apply request contains an invalid identity, digest or actor field.",
        ),
    };
    protocol_error(code, message, false)
}

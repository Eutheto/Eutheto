//! Native CSV ownership. Core cache availability is deliberately a separate lifetime.

use crate::{ApiError, boundary_error};
use eutheto_core::EuthetoApp;
use eutheto_types::{
    ApiErrorDto, CancellationToken, IdGenerator, OperationId, RequestId, Revision, ScenarioId,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read},
    sync::{Arc, Mutex, MutexGuard},
};

pub(crate) const MAX_SOURCE_BYTES: usize = eutheto_core::MAX_CSV_SOURCE_BYTES;
const MAX_SOURCES: usize = 3;
const MAX_PREVIEWS: usize = 3;
const MAX_ID_ATTEMPTS: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub(crate) struct SourceId(pub RequestId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Creator {
    pub operation_id: OperationId,
    pub request_id: RequestId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum SourceTarget {
    Source {
        source_id: SourceId,
    },
    Creator {
        operation_id: OperationId,
        request_id: RequestId,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum PreviewTarget {
    Preview {
        preview_id: RequestId,
    },
    Creator {
        operation_id: OperationId,
        request_id: RequestId,
    },
}

struct SourceEntry {
    owner: String,
    scenario: ScenarioId,
    creator: Creator,
    bytes: Option<Arc<Vec<u8>>>,
    closing: bool,
    active: usize,
}

struct PreviewEntry {
    owner: String,
    scenario: ScenarioId,
    source: SourceId,
    revision: Revision,
    preview_id: Option<RequestId>,
    closing: bool,
    active: usize,
    finalizing: bool,
}

#[derive(Default)]
struct State {
    sources: BTreeMap<SourceId, SourceEntry>,
    previews: BTreeMap<Creator, PreviewEntry>,
    closed_windows: BTreeSet<String>,
    shutdown: bool,
}

pub(crate) struct CsvCustody {
    app: EuthetoApp,
    ids: Arc<dyn IdGenerator>,
    state: Mutex<State>,
}

impl CsvCustody {
    pub fn new(app: EuthetoApp, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            app,
            ids,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // No external work executes under this lock. Recovering lets lifecycle drops
        // finish retiring resources even if an unrelated caller panicked.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn reserve_source(
        self: &Arc<Self>,
        owner: &str,
        scenario: ScenarioId,
        creator: Creator,
        cancellation: CancellationToken,
    ) -> Result<SourceReservation, ApiError> {
        let mut state = self.state();
        if state.shutdown || state.closed_windows.contains(owner) || cancellation.is_cancelled() {
            return Err(cancelled().into());
        }
        if state.sources.values().any(|entry| entry.creator == creator) {
            return Err(creator_collision().into());
        }
        if state.sources.len() >= MAX_SOURCES {
            return Err(capacity().into());
        }
        let mut selected = None;
        for _ in 0..MAX_ID_ATTEMPTS {
            let id = SourceId(RequestId::new(self.ids.as_ref()).map_err(|_| id_unavailable())?);
            if !state.sources.contains_key(&id) {
                selected = Some(id);
                break;
            }
        }
        let id = selected.ok_or_else(id_unavailable)?;
        state.sources.insert(
            id,
            SourceEntry {
                owner: owner.to_owned(),
                scenario,
                creator,
                bytes: None,
                closing: false,
                active: 1,
            },
        );
        Ok(SourceReservation {
            custody: Arc::clone(self),
            id,
            cancellation,
            published: false,
        })
    }

    pub fn source(
        self: &Arc<Self>,
        owner: &str,
        scenario: ScenarioId,
        id: SourceId,
    ) -> Result<SourceUse, ApiError> {
        let mut state = self.state();
        let entry = state.sources.get_mut(&id).ok_or_else(source_unavailable)?;
        if entry.owner != owner || entry.scenario != scenario || entry.closing {
            return Err(source_unavailable().into());
        }
        let bytes = Arc::clone(entry.bytes.as_ref().ok_or_else(source_unavailable)?);
        entry.active += 1;
        Ok(SourceUse {
            lease: Arc::new(SourceLease {
                custody: Arc::clone(self),
                id,
                bytes,
            }),
        })
    }

    pub fn close_source(
        self: &Arc<Self>,
        owner: &str,
        scenario: ScenarioId,
        target: &SourceTarget,
    ) {
        let mut state = self.state();
        let id = state.sources.iter().find_map(|(id, entry)| {
            let matches = match target {
                SourceTarget::Source { source_id } => id == source_id,
                SourceTarget::Creator {
                    operation_id,
                    request_id,
                } => {
                    entry.creator
                        == Creator {
                            operation_id: *operation_id,
                            request_id: *request_id,
                        }
                }
            };
            (matches && entry.owner == owner && entry.scenario == scenario).then_some(*id)
        });
        if let Some(id) = id
            && let Some(entry) = state.sources.get_mut(&id)
        {
            entry.closing = true;
            if entry.active == 0 {
                state.sources.remove(&id);
            }
        }
    }

    fn release_source(&self, id: SourceId, failed: bool) {
        let mut state = self.state();
        if let Some(entry) = state.sources.get_mut(&id) {
            entry.closing |= failed;
            entry.active -= 1;
            if entry.closing && entry.active == 0 {
                state.sources.remove(&id);
            }
        }
    }

    pub fn reserve_preview(
        self: &Arc<Self>,
        source: &SourceUse,
        revision: Revision,
        creator: Creator,
        cancellation: CancellationToken,
    ) -> Result<PreviewReservation, ApiError> {
        if !Arc::ptr_eq(self, &source.lease.custody) {
            return Err(source_unavailable().into());
        }
        let mut state = self.state();
        if state.shutdown || cancellation.is_cancelled() {
            return Err(cancelled().into());
        }
        let entry = state
            .sources
            .get(&source.source_id())
            .ok_or_else(source_unavailable)?;
        if entry.closing {
            return Err(source_unavailable().into());
        }
        let owner = entry.owner.clone();
        let scenario = entry.scenario;
        if state.previews.contains_key(&creator) {
            return Err(creator_collision().into());
        }
        if state.previews.len() >= MAX_PREVIEWS {
            return Err(capacity().into());
        }
        state.previews.insert(
            creator,
            PreviewEntry {
                owner,
                scenario,
                source: source.source_id(),
                revision,
                preview_id: None,
                closing: false,
                active: 1,
                finalizing: false,
            },
        );
        Ok(PreviewReservation {
            custody: Arc::clone(self),
            creator,
            cancellation,
            published: false,
        })
    }

    pub fn preview_for_apply(
        self: &Arc<Self>,
        owner: &str,
        scenario: ScenarioId,
        source: SourceId,
        revision: Revision,
        preview_id: RequestId,
    ) -> Result<PreviewUse, ApiError> {
        self.preview_use(owner, scenario, preview_id, Some((source, revision)))
    }

    pub fn preview_for_report(
        self: &Arc<Self>,
        owner: &str,
        scenario: ScenarioId,
        preview_id: RequestId,
    ) -> Result<PreviewUse, ApiError> {
        self.preview_use(owner, scenario, preview_id, None)
    }

    fn preview_use(
        self: &Arc<Self>,
        owner: &str,
        scenario: ScenarioId,
        preview_id: RequestId,
        apply: Option<(SourceId, Revision)>,
    ) -> Result<PreviewUse, ApiError> {
        let mut state = self.state();
        let (creator, entry) = state
            .previews
            .iter_mut()
            .find(|(_, entry)| entry.preview_id == Some(preview_id))
            .ok_or_else(preview_unavailable)?;
        if entry.owner != owner || entry.scenario != scenario || entry.closing {
            return Err(preview_unavailable().into());
        }
        if let Some((source, revision)) = apply
            && (entry.source != source || entry.revision != revision)
        {
            return Err(preview_unavailable().into());
        }
        entry.active += 1;
        Ok(PreviewUse {
            custody: Arc::clone(self),
            creator: *creator,
            preview_id,
        })
    }

    pub fn discard_preview(
        self: &Arc<Self>,
        owner: &str,
        scenario: ScenarioId,
        target: &PreviewTarget,
    ) {
        let mut state = self.state();
        for (creator, entry) in &mut state.previews {
            let matches = match target {
                PreviewTarget::Preview { preview_id } => entry.preview_id == Some(*preview_id),
                PreviewTarget::Creator {
                    operation_id,
                    request_id,
                } => {
                    *creator
                        == Creator {
                            operation_id: *operation_id,
                            request_id: *request_id,
                        }
                }
            };
            if matches && entry.owner == owner && entry.scenario == scenario {
                entry.closing = true;
            }
        }
        drop(state);
        self.finalize_ready();
    }

    fn release_preview(self: &Arc<Self>, creator: Creator, failed: bool) {
        {
            let mut state = self.state();
            if let Some(entry) = state.previews.get_mut(&creator) {
                entry.closing |= failed;
                entry.active -= 1;
            }
        }
        self.finalize_ready();
    }

    fn finalize_ready(self: &Arc<Self>) {
        // At most three entries; select one at a time without allocating a job list.
        loop {
            let ready = {
                let mut state = self.state();
                let ready = state.previews.iter().find_map(|(creator, entry)| {
                    (entry.closing && entry.active == 0 && !entry.finalizing)
                        .then_some((*creator, entry.preview_id))
                });
                match ready {
                    Some((creator, Some(_))) => {
                        if let Some(entry) = state.previews.get_mut(&creator) {
                            entry.finalizing = true;
                        }
                    }
                    Some((creator, None)) => {
                        state.previews.remove(&creator);
                    }
                    None => {}
                }
                ready
            };
            match ready {
                Some((creator, Some(preview_id))) => {
                    let custody = Arc::clone(self);
                    // Never owned by the lifecycle invoke future: abandoning its response
                    // cannot abort discard. Core errors (including eviction/wrong kind)
                    // do not retain a native slot and never trigger a retry.
                    tauri::async_runtime::spawn(async move {
                        let _ = custody.app.discard_people_csv_preview(preview_id).await;
                        custody.state().previews.remove(&creator);
                    });
                }
                Some((_, None)) => {}
                None => return,
            }
        }
    }

    pub fn close_window(self: &Arc<Self>, owner: &str) {
        {
            let mut state = self.state();
            state.closed_windows.insert(owner.to_owned());
            for entry in state
                .sources
                .values_mut()
                .filter(|entry| entry.owner == owner)
            {
                entry.closing = true;
            }
            state
                .sources
                .retain(|_, entry| !entry.closing || entry.active != 0);
            for entry in state
                .previews
                .values_mut()
                .filter(|entry| entry.owner == owner)
            {
                entry.closing = true;
            }
        }
        self.finalize_ready();
    }

    pub fn shutdown(self: &Arc<Self>) {
        {
            let mut state = self.state();
            state.shutdown = true;
            for entry in state.sources.values_mut() {
                entry.closing = true;
            }
            state.sources.retain(|_, entry| entry.active != 0);
            for entry in state.previews.values_mut() {
                entry.closing = true;
            }
        }
        self.finalize_ready();
    }
}

pub(crate) struct SourceReservation {
    custody: Arc<CsvCustody>,
    id: SourceId,
    cancellation: CancellationToken,
    published: bool,
}

impl SourceReservation {
    pub fn publish(mut self, bytes: Vec<u8>) -> Result<SourceId, ApiError> {
        if bytes.len() > MAX_SOURCE_BYTES {
            return Err(source_too_large().into());
        }
        {
            let mut state = self.custody.state();
            let entry = state
                .sources
                .get_mut(&self.id)
                .ok_or_else(source_unavailable)?;
            if entry.closing || self.cancellation.is_cancelled() {
                return Err(cancelled().into());
            }
            entry.bytes = Some(Arc::new(bytes));
            self.published = true;
        }
        Ok(self.id)
    }
}

impl Drop for SourceReservation {
    fn drop(&mut self) {
        self.custody.release_source(self.id, !self.published);
    }
}

struct SourceLease {
    custody: Arc<CsvCustody>,
    id: SourceId,
    bytes: Arc<Vec<u8>>,
}
impl Drop for SourceLease {
    fn drop(&mut self) {
        self.custody.release_source(self.id, false);
    }
}

pub(crate) struct SourceUse {
    lease: Arc<SourceLease>,
}
impl SourceUse {
    pub fn source_id(&self) -> SourceId {
        self.lease.id
    }
    pub fn reader(&self) -> SourceReader {
        SourceReader {
            lease: Arc::clone(&self.lease),
            position: 0,
        }
    }
}

pub(crate) struct SourceReader {
    lease: Arc<SourceLease>,
    position: usize,
}
impl Read for SourceReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = &self.lease.bytes[self.position..];
        let count = remaining.len().min(buffer.len());
        buffer[..count].copy_from_slice(&remaining[..count]);
        self.position += count;
        Ok(count)
    }
}

pub(crate) struct PreviewReservation {
    custody: Arc<CsvCustody>,
    creator: Creator,
    cancellation: CancellationToken,
    published: bool,
}
impl PreviewReservation {
    pub fn publish(mut self, preview_id: RequestId) -> Result<RequestId, ApiError> {
        {
            let mut state = self.custody.state();
            // A core ID already bound elsewhere is not this reservation's authority to discard.
            if state
                .previews
                .values()
                .any(|entry| entry.preview_id == Some(preview_id))
            {
                return Err(preview_unavailable().into());
            }
            let entry = state
                .previews
                .get_mut(&self.creator)
                .ok_or_else(preview_unavailable)?;
            entry.preview_id = Some(preview_id);
            if entry.closing || self.cancellation.is_cancelled() {
                return Err(cancelled().into());
            }
            self.published = true;
        }
        Ok(preview_id)
    }
}
impl Drop for PreviewReservation {
    fn drop(&mut self) {
        self.custody.release_preview(self.creator, !self.published);
    }
}

pub(crate) struct PreviewUse {
    custody: Arc<CsvCustody>,
    creator: Creator,
    preview_id: RequestId,
}
impl PreviewUse {
    pub fn preview_id(&self) -> RequestId {
        self.preview_id
    }
}
impl Drop for PreviewUse {
    fn drop(&mut self) {
        self.custody.release_preview(self.creator, false);
    }
}

fn capacity() -> ApiErrorDto {
    boundary_error(
        "people_csv.native_capacity",
        "Native CSV capacity is unavailable.",
        None,
    )
}
fn creator_collision() -> ApiErrorDto {
    boundary_error(
        "people_csv.creator_in_use",
        "This CSV creator is already in use.",
        None,
    )
}
fn id_unavailable() -> ApiErrorDto {
    boundary_error(
        "people_csv.id_unavailable",
        "A native CSV identifier is unavailable.",
        None,
    )
}
fn cancelled() -> ApiErrorDto {
    boundary_error("operation.cancelled", "The operation was cancelled.", None)
}
fn source_unavailable() -> ApiErrorDto {
    boundary_error(
        "people_csv.source_unavailable",
        "This CSV source is unavailable.",
        None,
    )
}
fn preview_unavailable() -> ApiErrorDto {
    boundary_error(
        "people_csv.preview_unavailable",
        "This CSV preview is unavailable.",
        None,
    )
}
fn source_too_large() -> ApiErrorDto {
    boundary_error(
        "people_csv.source_too_large",
        "The CSV source exceeds the supported size.",
        None,
    )
}

#[cfg(test)]
#[path = "custody_tests.rs"]
mod tests;

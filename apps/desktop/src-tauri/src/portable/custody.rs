//! Concrete custody for core-backed reviews and native prepared archives.

use crate::{ApiError, PreparedPortableKind, PreparedPortableOutput, boundary_error};
use eutheto_core::{AppCommand, EuthetoApp};
use eutheto_export::PORTABLE_LIMITS;
use eutheto_types::{ApiErrorDto, CancellationToken, OperationId, RequestId, Revision, ScenarioId};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, MutexGuard},
};

const MAX_REVIEWS_PER_GROUP: usize = 3;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Creator {
    pub operation_id: OperationId,
    pub request_id: RequestId,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum PreviewTarget {
    Preview {
        preview_id: RequestId,
    },
    Creator {
        operation_id: OperationId,
        request_id: RequestId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReviewKind {
    Import,
    Restore,
    Unopened,
    ScenarioExport,
    Backup,
}

impl ReviewKind {
    const fn is_prepared(self) -> bool {
        matches!(self, Self::ScenarioExport | Self::Backup)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReviewBinding {
    Import {
        library_revision: Revision,
    },
    Restore {
        library_revision: Revision,
    },
    Unopened,
    ScenarioExport {
        scenario_id: ScenarioId,
        scenario_revision: Revision,
        library_revision: Revision,
    },
    Backup {
        library_revision: Revision,
    },
}

impl ReviewBinding {
    const fn kind(self) -> ReviewKind {
        match self {
            Self::Import { .. } => ReviewKind::Import,
            Self::Restore { .. } => ReviewKind::Restore,
            Self::Unopened => ReviewKind::Unopened,
            Self::ScenarioExport { .. } => ReviewKind::ScenarioExport,
            Self::Backup { .. } => ReviewKind::Backup,
        }
    }
}

struct Binding {
    preview_id: RequestId,
    reviewed: ReviewBinding,
}

struct Entry {
    owner: String,
    kind: ReviewKind,
    binding: Option<Binding>,
    output: Option<PreparedPortableOutput>,
    charged_bytes: usize,
    active: bool,
    closing: bool,
    finalizing: bool,
}

#[derive(Default)]
struct State {
    previews: BTreeMap<Creator, Entry>,
    closed_windows: BTreeSet<String>,
    shutdown: bool,
}

pub(crate) struct PortableCustody {
    app: EuthetoApp,
    state: Mutex<State>,
}

impl PortableCustody {
    pub fn new(app: EuthetoApp) -> Self {
        Self {
            app,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // As in settings custody, cleanup must retire authority after a poisoned lock.
        // No external work executes while this lock is held.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn reserve(
        self: &Arc<Self>,
        owner: &str,
        creator: Creator,
        kind: ReviewKind,
        cancellation: CancellationToken,
    ) -> Result<PreviewReservation, ApiError> {
        let mut state = self.state();
        if state.shutdown || state.closed_windows.contains(owner) || cancellation.is_cancelled() {
            return Err(crate::operations::cancelled().into());
        }
        if state.previews.contains_key(&creator) {
            return Err(unavailable().into());
        }
        let count = state
            .previews
            .values()
            .filter(|entry| entry.kind.is_prepared() == kind.is_prepared())
            .count();
        if count >= MAX_REVIEWS_PER_GROUP {
            return Err(boundary_error(
                "portable.preview_capacity",
                "Close an existing portable review of this kind before creating another.",
                None,
            )
            .into());
        }
        state.previews.insert(
            creator,
            Entry {
                owner: owner.to_owned(),
                kind,
                binding: None,
                output: None,
                charged_bytes: 0,
                active: true,
                closing: false,
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

    pub(super) fn acquire(
        self: &Arc<Self>,
        owner: &str,
        preview_id: RequestId,
        reviewed: ReviewBinding,
    ) -> Result<PreviewUse, ApiError> {
        let mut state = self.state();
        if state.shutdown || state.closed_windows.contains(owner) {
            return Err(unavailable().into());
        }
        let (creator, entry) = state
            .previews
            .iter_mut()
            .find(|(_, entry)| {
                entry.owner == owner
                    && entry
                        .binding
                        .as_ref()
                        .is_some_and(|binding| binding.preview_id == preview_id)
            })
            .ok_or_else(unavailable)?;
        if entry.closing || entry.active {
            return Err(unavailable().into());
        }
        if !entry
            .binding
            .as_ref()
            .is_some_and(|binding| binding.reviewed == reviewed)
        {
            return Err(boundary_error(
                "portable.preview_binding_mismatch",
                "The portable request does not match the retained review.",
                None,
            )
            .into());
        }
        entry.active = true;
        Ok(PreviewUse {
            custody: Arc::clone(self),
            creator: *creator,
            preview_id,
            reviewed,
            output: entry.output.take(),
            released: false,
        })
    }

    pub(super) fn discard(self: &Arc<Self>, owner: &str, target: &PreviewTarget) {
        {
            let mut state = self.state();
            for (creator, entry) in &mut state.previews {
                let selected = match target {
                    PreviewTarget::Preview { preview_id } => entry
                        .binding
                        .as_ref()
                        .is_some_and(|binding| binding.preview_id == *preview_id),
                    PreviewTarget::Creator {
                        operation_id,
                        request_id,
                    } => creator.operation_id == *operation_id && creator.request_id == *request_id,
                };
                if entry.owner == owner && selected {
                    entry.closing = true;
                }
            }
        }
        self.finalize_ready();
    }

    fn release(self: &Arc<Self>, creator: Creator, close: bool) -> bool {
        let retained = {
            let mut state = self.state();
            state.previews.get_mut(&creator).is_some_and(|entry| {
                entry.active = false;
                entry.closing |= close;
                !entry.closing
            })
        };
        self.finalize_ready();
        retained
    }

    fn finalize_ready(self: &Arc<Self>) {
        loop {
            let ready = {
                let mut state = self.state();
                let ready = state.previews.iter().find_map(|(creator, entry)| {
                    (entry.closing && !entry.active && !entry.finalizing).then_some((
                        *creator,
                        entry
                            .binding
                            .as_ref()
                            .filter(|binding| !binding.reviewed.kind().is_prepared())
                            .map(|binding| binding.preview_id),
                    ))
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
                    // Keep the slot charged until real core cleanup settles, even after shutdown.
                    tauri::async_runtime::spawn(async move {
                        let _ = custody
                            .app
                            .execute(AppCommand::CancelPortablePreview { preview_id })
                            .await;
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
            for entry in state.previews.values_mut() {
                entry.closing = true;
            }
        }
        self.finalize_ready();
    }
}

pub(super) struct PreviewReservation {
    custody: Arc<PortableCustody>,
    creator: Creator,
    cancellation: CancellationToken,
    published: bool,
}

impl PreviewReservation {
    pub(super) fn publish_core(
        self,
        preview_id: RequestId,
        reviewed: ReviewBinding,
    ) -> Result<(), ApiError> {
        self.publish(preview_id, reviewed, None)
    }

    pub(super) fn publish_prepared(
        self,
        preview_id: RequestId,
        output: PreparedPortableOutput,
    ) -> Result<(), ApiError> {
        let reviewed = match &output.kind {
            PreparedPortableKind::Scenario {
                scenario_id,
                revision,
                library_revision,
                ..
            } => ReviewBinding::ScenarioExport {
                scenario_id: *scenario_id,
                scenario_revision: *revision,
                library_revision: *library_revision,
            },
            PreparedPortableKind::Backup {
                library_revision, ..
            } => ReviewBinding::Backup {
                library_revision: *library_revision,
            },
        };
        self.publish(preview_id, reviewed, Some(output))
    }

    fn publish(
        mut self,
        preview_id: RequestId,
        reviewed: ReviewBinding,
        output: Option<PreparedPortableOutput>,
    ) -> Result<(), ApiError> {
        let mut state = self.custody.state();
        let duplicate = state.previews.values().any(|entry| {
            entry
                .binding
                .as_ref()
                .is_some_and(|binding| binding.preview_id == preview_id)
        });
        let charged_bytes = output.as_ref().map_or(0, |output| output.bytes.len());
        let total = state
            .previews
            .values()
            .try_fold(charged_bytes, |total, entry| {
                total.checked_add(entry.charged_bytes)
            });
        let within_budget = total.is_some_and(|total| {
            u64::try_from(total).is_ok_and(|total| total <= PORTABLE_LIMITS.max_archive_bytes)
        });
        let entry = state
            .previews
            .get_mut(&self.creator)
            .ok_or_else(unavailable)?;
        // Active creation cannot be removed by teardown. Bind the real result before
        // checking failure paths so reservation drop can discard any core capability.
        entry.binding = Some(Binding {
            preview_id,
            reviewed,
        });
        entry.output = output;
        entry.charged_bytes = charged_bytes;
        if duplicate
            || entry.kind != reviewed.kind()
            || reviewed.kind().is_prepared() != entry.output.is_some()
        {
            return Err(unavailable().into());
        }
        if !within_budget {
            return Err(boundary_error(
                "portable.preview_too_large",
                "The prepared portable reviews exceed the retained archive-byte limit.",
                None,
            )
            .into());
        }
        if entry.closing || self.cancellation.is_cancelled() {
            return Err(crate::operations::cancelled().into());
        }
        entry.active = false;
        self.published = true;
        Ok(())
    }
}

impl Drop for PreviewReservation {
    fn drop(&mut self) {
        if !self.published {
            self.custody.release(self.creator, true);
        }
    }
}

pub(super) struct PreviewUse {
    custody: Arc<PortableCustody>,
    creator: Creator,
    preview_id: RequestId,
    reviewed: ReviewBinding,
    output: Option<PreparedPortableOutput>,
    released: bool,
}

impl PreviewUse {
    pub(super) fn take_output(&mut self) -> Result<PreparedPortableOutput, ApiError> {
        self.output.take().ok_or_else(|| unavailable().into())
    }

    pub(super) async fn retain_restore_retry(mut self, cancellation: &CancellationToken) -> bool {
        let retained = if let ReviewBinding::Restore { library_revision } = self.reviewed {
            self.custody
                .app
                .portable_restore_retry_is_retained(self.preview_id, library_revision)
                .await
                && !cancellation.is_cancelled()
        } else {
            false
        };
        let retained = self.custody.release(self.creator, !retained);
        self.released = true;
        retained
    }
}

impl Drop for PreviewUse {
    fn drop(&mut self) {
        if !self.released {
            self.custody.release(self.creator, true);
        }
    }
}

fn unavailable() -> ApiErrorDto {
    boundary_error(
        "portable.preview_not_found",
        "The portable review is unavailable or belongs to another window.",
        None,
    )
}

#[cfg(test)]
#[path = "custody_tests.rs"]
mod tests;

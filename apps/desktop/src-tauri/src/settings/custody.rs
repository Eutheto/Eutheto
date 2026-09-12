//! Window-owned settings reviews; native ownership does not imply core-cache availability.

use crate::{ApiError, boundary_error};
use eutheto_core::EuthetoApp;
use eutheto_types::{
    CancellationToken, OperationId, RequestId, Revision, SettingsImportPreviewDtoV1,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, MutexGuard},
};

const MAX_PREVIEWS: usize = 3;

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

struct Binding {
    preview_id: RequestId,
    approval_sha256: String,
    library_revision: Revision,
}

struct Entry {
    owner: String,
    binding: Option<Binding>,
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

pub(crate) struct SettingsCustody {
    app: EuthetoApp,
    state: Mutex<State>,
}

impl SettingsCustody {
    pub fn new(app: EuthetoApp) -> Self {
        Self {
            app,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // Like CSV custody, drops must finish retiring authority after a poisoned lock.
        // No external work executes while this lock is held.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn reserve(
        self: &Arc<Self>,
        owner: &str,
        creator: Creator,
        cancellation: CancellationToken,
    ) -> Result<PreviewReservation, ApiError> {
        let mut state = self.state();
        if state.shutdown || state.closed_windows.contains(owner) || cancellation.is_cancelled() {
            return Err(cancelled().into());
        }
        if state.previews.contains_key(&creator) {
            return Err(unavailable().into());
        }
        if state.previews.len() >= MAX_PREVIEWS {
            return Err(boundary_error(
                "settings.preview_capacity",
                "Close an existing settings review before creating another.",
                None,
            )
            .into());
        }
        state.previews.insert(
            creator,
            Entry {
                owner: owner.to_owned(),
                binding: None,
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

    pub(super) fn acquire_apply(
        self: &Arc<Self>,
        owner: &str,
        preview_id: RequestId,
        approval_sha256: &str,
        library_revision: Revision,
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
        let binding = entry.binding.as_ref().ok_or_else(unavailable)?;
        if binding.approval_sha256 != approval_sha256
            || binding.library_revision != library_revision
        {
            return Err(boundary_error(
                "settings.approval_mismatch",
                "The settings approval does not match the retained review.",
                None,
            )
            .into());
        }
        entry.active = true;
        Ok(PreviewUse {
            custody: Arc::clone(self),
            creator: *creator,
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

    fn release(self: &Arc<Self>, creator: Creator, close: bool) {
        {
            let mut state = self.state();
            if let Some(entry) = state.previews.get_mut(&creator) {
                entry.active = false;
                entry.closing |= close;
            }
        }
        self.finalize_ready();
    }

    fn finalize_ready(self: &Arc<Self>) {
        loop {
            let ready = {
                let mut state = self.state();
                let ready = state.previews.iter().find_map(|(creator, entry)| {
                    (entry.closing && !entry.active && !entry.finalizing).then_some((
                        *creator,
                        entry.binding.as_ref().map(|binding| binding.preview_id),
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
                    // The slot remains occupied until core discard settles, independent of
                    // a dropped lifecycle invoke response. Never retry or resurrect authority.
                    tauri::async_runtime::spawn(async move {
                        custody
                            .app
                            .discard_nonsecret_settings_preview(preview_id)
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
    custody: Arc<SettingsCustody>,
    creator: Creator,
    cancellation: CancellationToken,
    published: bool,
}

impl PreviewReservation {
    pub fn publish(mut self, preview: &SettingsImportPreviewDtoV1) -> Result<(), ApiError> {
        {
            let mut state = self.custody.state();
            if state.previews.values().any(|entry| {
                entry
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.preview_id == preview.preview_id)
            }) {
                return Err(unavailable().into());
            }
            let entry = state
                .previews
                .get_mut(&self.creator)
                .ok_or_else(unavailable)?;
            entry.binding = Some(Binding {
                preview_id: preview.preview_id,
                approval_sha256: preview.approval_sha256.clone(),
                library_revision: preview.library_revision,
            });
            if entry.closing || self.cancellation.is_cancelled() {
                return Err(cancelled().into());
            }
            self.published = true;
        }
        Ok(())
    }
}

impl Drop for PreviewReservation {
    fn drop(&mut self) {
        self.custody.release(self.creator, !self.published);
    }
}

pub(super) struct PreviewUse {
    custody: Arc<SettingsCustody>,
    creator: Creator,
}

impl Drop for PreviewUse {
    fn drop(&mut self) {
        self.custody.release(self.creator, true);
    }
}

fn unavailable() -> eutheto_types::ApiErrorDto {
    boundary_error(
        "settings.preview_unavailable",
        "The settings review is no longer available.",
        None,
    )
}

fn cancelled() -> eutheto_types::ApiErrorDto {
    boundary_error("operation.cancelled", "The operation was cancelled.", None)
}

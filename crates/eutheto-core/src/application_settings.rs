//! Bounded standalone settings snapshots and one-use reviewed replacement authority.

use super::{
    EuthetoApp, MAX_PENDING_PREVIEW_BYTES, MAX_PENDING_PREVIEWS, PendingPortablePreview,
    join_error, operation_interrupted, pending_tree_memory_charge, preview_total_bytes,
    protocol_error, store_error, validate_app_setting, validation_error,
};
use eutheto_store::{AppSetting, AppSettingsSnapshot};
use eutheto_types::{
    APPLICATION_SETTINGS_SCHEMA_VERSION, AppError, ApplicationSettingEntryV1,
    ApplicationSettingsFormat, CancellationToken, MAX_APPLICATION_SETTINGS_BYTES,
    NonsecretSettingsDocumentV1, OperationControl, PortableJsonLimits, RequestId, Revision,
    SettingsChangeV1, SettingsImportApplyDtoV1, SettingsImportApplyRequestV1,
    SettingsImportPreviewDtoV1, validate_nonsecret_portable_json_bytes,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

const SETTING_KEYS: &[&str] = &["appearance", "locale", "units"];
const SETTINGS_JSON_LIMITS: PortableJsonLimits = PortableJsonLimits {
    max_depth: 8,
    max_string_bytes: 4096,
    max_collection_items: 64,
};
const APPROVAL_DOMAIN: &[u8] = b"eutheto/application-settings-approval/v1\0";

/// Validated immutable document and its canonical size at the captured library revision.
#[derive(Debug)]
pub struct SettingsExportSnapshot {
    pub document: NonsecretSettingsDocumentV1,
    pub byte_count: usize,
    pub library_revision: Revision,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsAuthority {
    format: ApplicationSettingsFormat,
    schema_version: u32,
    preview_id: RequestId,
    source_sha256: String,
    library_revision: Revision,
    before: BTreeMap<String, ApplicationSettingEntryV1>,
    after: BTreeMap<String, ApplicationSettingEntryV1>,
}

#[derive(Clone)]
pub(super) struct PendingSettingsPreview {
    library_revision: Revision,
    approval_sha256: Box<str>,
    authority: Arc<[u8]>,
}

impl PendingSettingsPreview {
    pub(super) fn retained_bytes(&self) -> usize {
        pending_tree_memory_charge(1, size_of::<(RequestId, PendingPortablePreview)>())
            .and_then(|charge| charge.checked_add(size_of::<Self>()))
            .and_then(|charge| charge.checked_add(2 * size_of::<usize>()))
            .and_then(|charge| charge.checked_add(self.authority.len()))
            .and_then(|charge| charge.checked_add(self.approval_sha256.len()))
            .unwrap_or(usize::MAX)
    }
}

impl EuthetoApp {
    /// Captures and serializes only supported portable nonsecret settings.
    ///
    /// # Errors
    /// Rejects cancellation, invalid stored settings and any entry excluded by
    /// portable policy. Export never silently omits an unportable setting.
    pub async fn export_nonsecret_settings(
        &self,
        cancellation: CancellationToken,
    ) -> Result<SettingsExportSnapshot, AppError> {
        self.check_cancelled()?;
        check_cancelled(&cancellation)?;
        let snapshot = self
            .store
            .settings_snapshot(SETTING_KEYS)
            .await
            .map_err(store_error)?;
        check_cancelled(&cancellation)?;
        let library_revision = snapshot.library_revision;
        let settings = portable_entries(snapshot.settings);
        let (document, byte_count) = tokio::task::spawn_blocking(move || {
            validate_settings(&settings)?;
            let document = NonsecretSettingsDocumentV1 {
                format: ApplicationSettingsFormat::EuthetoApplicationSettings,
                schema_version: APPLICATION_SETTINGS_SCHEMA_VERSION,
                settings,
            };
            let bytes = canonical(&document)?;
            validate_document_bytes(&bytes)?;
            Ok::<_, AppError>((document, bytes.len()))
        })
        .await
        .map_err(join_error)??;
        check_cancelled(&cancellation)?;
        Ok(SettingsExportSnapshot {
            document,
            byte_count,
            library_revision,
        })
    }

    /// Retains an immutable, bounded replacement review from native-captured bytes.
    ///
    /// # Errors
    /// Rejects unsupported versions/keys, malformed or over-budget source,
    /// invalid settings, cancellation and unavailable preview capacity/identities.
    pub async fn preview_nonsecret_settings(
        &self,
        bytes: Vec<u8>,
        cancellation: CancellationToken,
    ) -> Result<SettingsImportPreviewDtoV1, AppError> {
        self.check_cancelled()?;
        check_cancelled(&cancellation)?;
        if bytes.len() > MAX_APPLICATION_SETTINGS_BYTES {
            return Err(settings_error(
                "settings.document_too_large",
                "The settings document exceeds the supported size.",
            ));
        }
        let (document, source_sha256) = tokio::task::spawn_blocking(move || {
            validate_document_bytes(&bytes)?;
            let document: NonsecretSettingsDocumentV1 =
                serde_json::from_slice(&bytes).map_err(|_| {
                    settings_error(
                        "settings.document_invalid",
                        "The settings document is invalid.",
                    )
                })?;
            ensure_version(document.schema_version)?;
            validate_settings(&document.settings)?;
            Ok::<_, AppError>((document, eutheto_export::sha256_hex(&bytes)))
        })
        .await
        .map_err(join_error)??;
        check_cancelled(&cancellation)?;
        let snapshot = self
            .store
            .settings_snapshot(SETTING_KEYS)
            .await
            .map_err(store_error)?;
        let before = portable_entries(snapshot.settings);
        validate_settings(&before)?;
        let changes = SETTING_KEYS
            .iter()
            .filter_map(|key| {
                let previous = before.get(*key);
                let next = document.settings.get(*key);
                (previous != next).then(|| SettingsChangeV1 {
                    key: (*key).to_owned(),
                    before: previous.cloned(),
                    after: next.cloned(),
                })
            })
            .collect();
        let mut previews = self.previews.lock().await;
        check_cancelled(&cancellation)?;
        let preview_id = self
            .next_preview_id(&previews)?
            .ok_or_else(Self::preview_id_unavailable)?;
        let authority = canonical(&SettingsAuthority {
            format: document.format,
            schema_version: document.schema_version,
            preview_id,
            source_sha256: source_sha256.clone(),
            library_revision: snapshot.library_revision,
            before,
            after: document.settings,
        })?;
        let mut digest_input = Vec::with_capacity(APPROVAL_DOMAIN.len() + authority.len());
        digest_input.extend_from_slice(APPROVAL_DOMAIN);
        digest_input.extend_from_slice(&authority);
        let approval_sha256 = eutheto_export::sha256_hex(&digest_input);
        let pending = PendingSettingsPreview {
            library_revision: snapshot.library_revision,
            approval_sha256: approval_sha256.clone().into_boxed_str(),
            authority: authority.into(),
        };
        let retained_bytes = pending.retained_bytes();
        if retained_bytes > MAX_PENDING_PREVIEW_BYTES {
            return Err(settings_error(
                "settings.preview_too_large",
                "The settings review exceeds the retained preview limit.",
            ));
        }
        check_cancelled(&cancellation)?;
        while previews.len() >= MAX_PENDING_PREVIEWS
            || preview_total_bytes(&previews)
                .checked_add(retained_bytes)
                .is_none_or(|total| total > MAX_PENDING_PREVIEW_BYTES)
        {
            let Some(oldest) = previews.keys().next().copied() else {
                break;
            };
            previews.remove(&oldest);
        }
        previews.insert(preview_id, PendingPortablePreview::Settings(pending));
        Ok(SettingsImportPreviewDtoV1 {
            schema_version: APPLICATION_SETTINGS_SCHEMA_VERSION,
            preview_id,
            source_sha256,
            approval_sha256,
            library_revision: snapshot.library_revision,
            changes,
        })
    }

    /// Applies one approved preview through a finite owner that outlives its caller.
    ///
    /// Pass an app-rooted operation token. Abandonment cancels a child, never the
    /// parent. A committed transaction and its notification survive abandonment.
    ///
    /// # Errors
    /// Rejects malformed approval, missing/wrong-kind authority, stale state,
    /// cancellation before commit and atomic persistence failures. An owned apply
    /// consumes its preview even on failure; wrong approval does not consume it.
    pub async fn apply_nonsecret_settings(
        &self,
        request: SettingsImportApplyRequestV1,
        cancellation: CancellationToken,
    ) -> Result<SettingsImportApplyDtoV1, AppError> {
        ensure_version(request.schema_version)?;
        self.check_cancelled()?;
        if request.approval_sha256.len() != 64
            || !request
                .approval_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(settings_error(
                "settings.approval_mismatch",
                "The settings approval does not match the retained review.",
            ));
        }
        let token = cancellation.child();
        let _guard = super::setup::CancelOnDrop(token.clone());
        let app = self.clone();
        tokio::spawn(async move { app.apply_nonsecret_settings_owned(request, token).await })
            .await
            .map_err(join_error)?
    }

    async fn apply_nonsecret_settings_owned(
        &self,
        request: SettingsImportApplyRequestV1,
        cancellation: CancellationToken,
    ) -> Result<SettingsImportApplyDtoV1, AppError> {
        check_cancelled(&cancellation)?;
        let pending = {
            let mut previews = self.previews.lock().await;
            check_cancelled(&cancellation)?;
            let Some(PendingPortablePreview::Settings(pending)) = previews.get(&request.preview_id)
            else {
                return Err(settings_error(
                    "settings.preview_unavailable",
                    "The settings review is no longer available.",
                ));
            };
            if pending.approval_sha256.as_ref() != request.approval_sha256 {
                return Err(settings_error(
                    "settings.approval_mismatch",
                    "The settings approval does not match the retained review.",
                ));
            }
            if pending.library_revision != request.expected_library_revision {
                return Err(AppError::Conflict {
                    expected_revision: request.expected_library_revision,
                    actual_revision: pending.library_revision,
                });
            }
            let Some(PendingPortablePreview::Settings(pending)) =
                previews.remove(&request.preview_id)
            else {
                return Err(settings_error(
                    "settings.preview_unavailable",
                    "The settings review is no longer available.",
                ));
            };
            pending
        };
        let authority: SettingsAuthority =
            serde_json::from_slice(&pending.authority).map_err(|_| {
                settings_error(
                    "settings.preview_invalid",
                    "The retained settings review is invalid.",
                )
            })?;
        let committed = self
            .store
            .replace_settings(
                SETTING_KEYS,
                AppSettingsSnapshot {
                    library_revision: authority.library_revision,
                    settings: stored_entries(authority.before),
                },
                stored_entries(authority.after),
                cancellation,
            )
            .await
            .map_err(store_error)?;
        if committed.changed {
            self.publish_app_notification(
                request.request_id,
                "settings.updated",
                "Application settings changed.",
            );
        }
        Ok(SettingsImportApplyDtoV1 {
            schema_version: APPLICATION_SETTINGS_SCHEMA_VERSION,
            preview_id: request.preview_id,
            library_revision: committed.library_revision,
            changed: committed.changed,
        })
    }

    /// Idempotently discards only settings authority, never another preview kind.
    pub async fn discard_nonsecret_settings_preview(&self, preview_id: RequestId) {
        let mut previews = self.previews.lock().await;
        if matches!(
            previews.get(&preview_id),
            Some(PendingPortablePreview::Settings(_))
        ) {
            previews.remove(&preview_id);
        }
    }
}

fn portable_entries(
    settings: BTreeMap<String, AppSetting<Value>>,
) -> BTreeMap<String, ApplicationSettingEntryV1> {
    settings
        .into_iter()
        .map(|(key, entry)| {
            (
                key,
                ApplicationSettingEntryV1 {
                    value: entry.value,
                    updated_at: entry.updated_at,
                },
            )
        })
        .collect()
}

fn stored_entries(
    settings: BTreeMap<String, ApplicationSettingEntryV1>,
) -> BTreeMap<String, AppSetting<Value>> {
    settings
        .into_iter()
        .map(|(key, entry)| {
            (
                key,
                AppSetting {
                    value: entry.value,
                    updated_at: entry.updated_at,
                },
            )
        })
        .collect()
}

fn validate_settings(
    settings: &BTreeMap<String, ApplicationSettingEntryV1>,
) -> Result<(), AppError> {
    for (key, entry) in settings {
        validate_app_setting(key, &entry.value)?;
    }
    Ok(())
}

fn validate_document_bytes(bytes: &[u8]) -> Result<(), AppError> {
    if bytes.len() > MAX_APPLICATION_SETTINGS_BYTES {
        return Err(settings_error(
            "settings.document_too_large",
            "The settings document exceeds the supported size.",
        ));
    }
    validate_nonsecret_portable_json_bytes(bytes, &SETTINGS_JSON_LIMITS).map_err(|_| {
        validation_error(
            "settings.document_not_portable",
            "/settings",
            "The settings document is malformed or outside the nonsecret portable policy.",
        )
    })
}

fn ensure_version(version: u32) -> Result<(), AppError> {
    if version != APPLICATION_SETTINGS_SCHEMA_VERSION {
        return Err(settings_error(
            "settings.version_unsupported",
            "The settings document or operation version is not supported.",
        ));
    }
    Ok(())
}

fn canonical(value: &impl Serialize) -> Result<Vec<u8>, AppError> {
    let bytes = eutheto_export::canonical_json(value).map_err(|_| {
        settings_error(
            "settings.serialization_failed",
            "The settings snapshot could not be serialized.",
        )
    })?;
    if bytes.len() > MAX_APPLICATION_SETTINGS_BYTES {
        return Err(settings_error(
            "settings.document_too_large",
            "The settings document exceeds the supported size.",
        ));
    }
    Ok(bytes)
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), AppError> {
    OperationControl::Cancellation(cancellation.clone())
        .check()
        .map_err(operation_interrupted)
}

fn settings_error(code: &str, message: &str) -> AppError {
    protocol_error(code, message, false)
}

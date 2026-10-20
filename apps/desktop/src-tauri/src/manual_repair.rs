//! Window-owned manual edits; Core owns source custody, whole-selection review and commit.
use super::operations::{OperationClaim, OperationContextV1, OperationPhaseV1, OperationPurposeV1};
use super::{
    ApiError, DesktopState, SolutionApiResult, boundary_error, map_app_error, setup_boundary,
};
use eutheto_core::{
    MAX_MANUAL_EDIT_REQUEST_BYTES, MAX_MANUAL_EDIT_RESPONSE_BYTES, ManualEditIntentV1,
    ManualEditReadV1, ManualEditRequestV1, ManualEditResultV1, ManualEditReviewV1,
    ManualProtectionV1, bounded_json_size,
};
use eutheto_types::{ApiErrorDto, CommandResult, DurationMillis, OperationControl, OperationId};
use serde::Serialize;
use serde_json::Value;
use tauri::State;

#[derive(Clone, Copy)]
enum Action {
    Edit,
    Lock,
    Unlock,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum ManualResponse {
    Read { read: ManualEditReadV1 },
    Review { review: ManualEditReviewV1 },
    Applied { applied: Applied },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Applied {
    schema_version: u32,
    command_result: CommandResult,
    source_admission_failure: Option<ApiErrorDto>,
}

fn decode(request: Option<Value>) -> Result<(OperationId, ManualEditRequestV1), ApiError> {
    let mut request = request.ok_or_else(|| {
        boundary_error(
            "manual.request_missing",
            "A manual edit request is required.",
            None,
        )
    })?;
    bounded_json_size(&request, MAX_MANUAL_EDIT_REQUEST_BYTES).map_err(|_| {
        boundary_error(
            "manual.request_too_large",
            "The manual edit exceeds its native request limit.",
            None,
        )
    })?;
    if request
        .get("intent")
        .and_then(|intent| intent.get("protectionChanges"))
        .and_then(Value::as_array)
        .is_some_and(|changes| changes.len() > 2)
    {
        return Err(boundary_error(
            "manual.protection_limit",
            "A manual edit allows at most two protection changes.",
            Some("/intent/protectionChanges"),
        )
        .into());
    }
    let operation_id = request
        .as_object_mut()
        .and_then(|object| object.remove("operationId"));
    let operation_id = setup_boundary::decode(operation_id)?;
    // Core's closed tagged DTO rejects every other unknown field; the removed ID only owns IPC.
    let request = setup_boundary::decode(Some(request))?;
    Ok((operation_id, request))
}

fn require_action(request: &ManualEditRequestV1, action: Action) -> Result<(), ApiError> {
    if matches!(action, Action::Edit) {
        return Ok(());
    }
    let intent = match request {
        ManualEditRequestV1::Review { intent, .. } | ManualEditRequestV1::Apply { intent, .. } => {
            intent
        }
        ManualEditRequestV1::Read { .. } => {
            return Err(boundary_error(
                "manual.action_mismatch",
                "Lock actions require a protection review or apply request.",
                None,
            )
            .into());
        }
    };
    let valid = matches!(
        (action, intent),
        (
            Action::Lock,
            ManualEditIntentV1::Protection {
                protection: ManualProtectionV1::Hard | ManualProtectionV1::Soft { .. },
                ..
            }
        ) | (
            Action::Unlock,
            ManualEditIntentV1::Protection {
                protection: ManualProtectionV1::Unlocked,
                ..
            }
        )
    );
    if valid {
        Ok(())
    } else {
        Err(boundary_error(
            "manual.action_mismatch",
            "This command does not accept the requested protection change.",
            Some("/intent"),
        )
        .into())
    }
}

// Keep owned read/review and commit-aware settlement visible in one boundary flow.
#[allow(clippy::too_many_lines)]
async fn run<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
    action: Action,
) -> SolutionApiResult {
    let (operation_id, request) = decode(request)?;
    require_action(&request, action)?;
    let (request_id, scenario_id, expected_revision, purpose) = match &request {
        ManualEditRequestV1::Read {
            request_id,
            scenario_id,
            expected_revision,
            ..
        } => (
            *request_id,
            *scenario_id,
            *expected_revision,
            OperationPurposeV1::ManualRepairRead,
        ),
        ManualEditRequestV1::Review {
            request_id,
            scenario_id,
            expected_revision,
            ..
        } => (
            *request_id,
            *scenario_id,
            *expected_revision,
            OperationPurposeV1::ManualRepairReview,
        ),
        ManualEditRequestV1::Apply {
            request_id,
            scenario_id,
            expected_revision,
            ..
        } => (
            *request_id,
            *scenario_id,
            *expected_revision,
            OperationPurposeV1::ManualRepairApply,
        ),
    };
    let applying = matches!(purpose, OperationPurposeV1::ManualRepairApply);
    let phase = if applying {
        OperationPhaseV1::ApplyingPreview
    } else {
        OperationPhaseV1::Validating
    };
    let app = state.app.clone();
    let claim = OperationClaim {
        operation_id,
        request_id,
        purpose: Some(purpose),
        context: OperationContextV1::Scenario {
            scenario_id,
            expected_revision: Some(expected_revision),
        },
    };
    let time_limit = DurationMillis::new(30_000).map_err(|_| {
        boundary_error(
            "manual.budget_invalid",
            "The native manual edit budget is invalid.",
            None,
        )
    })?;
    let work = move |request, mut execution: super::operations::OperationExecution| async move {
        let (budget, _) = execution.take_solve_admission().ok_or_else(|| {
            boundary_error(
                "manual.budget_missing",
                "The manual edit owner has no admitted budget.",
                None,
            )
        })?;
        let control = OperationControl::Solve(budget.phase_view());
        let result = app
            .manual_edit(request, control.clone())
            .await
            .map_err(|error| Box::new(map_app_error(error)))?;
        let (revision, response, committed) = match result {
            ManualEditResultV1::Read { read } => {
                (read.source.revision, ManualResponse::Read { read }, false)
            }
            ManualEditResultV1::Review { review } => {
                execution.preparing_response();
                return tauri::async_runtime::spawn_blocking(move || {
                    review_response(request_id, review, &control)
                })
                .await
                .map_err(|_| {
                    Box::new(boundary_error(
                        "operation.execution_failed",
                        "The native operation could not finish safely.",
                        None,
                    ))
                })?;
            }
            ManualEditResultV1::Applied { applied } => (
                applied.command_result.new_revision,
                ManualResponse::Applied {
                    applied: Applied {
                        schema_version: applied.schema_version,
                        command_result: applied.command_result,
                        source_admission_failure: applied
                            .source_admission_failure
                            .map(map_app_error),
                    },
                },
                true,
            ),
        };
        if !committed {
            control.check().map_err(|error| {
                Box::new(match error {
                    eutheto_types::OperationInterruption::Cancelled => {
                        super::operations::cancelled()
                    }
                    eutheto_types::OperationInterruption::DeadlineExceeded => {
                        super::operations::deadline_exceeded()
                    }
                })
            })?;
        }
        let cancellation = if committed {
            None
        } else {
            Some(execution.cancellation())
        };
        setup_boundary::finish(
            &mut execution,
            request_id,
            Some(revision),
            response,
            MAX_MANUAL_EDIT_RESPONSE_BYTES,
            cancellation,
        )
        .await
    };
    if applying {
        state
            .operations
            .run_with_mutation_budget(
                window.label(),
                claim,
                None,
                phase,
                (request, None),
                time_limit,
                work,
            )
            .await
    } else {
        state
            .operations
            .run_with_bounded_budget(
                window.label(),
                claim,
                None,
                phase,
                (request, None),
                time_limit,
                work,
            )
            .await
    }
}

fn review_response(
    request_id: eutheto_types::RequestId,
    review: ManualEditReviewV1,
    control: &OperationControl,
) -> SolutionApiResult {
    // Verification facts use the solution wire contract: every signed integer is a string.
    super::solution_response_controlled(
        request_id,
        review.source.revision,
        ManualResponse::Review { review },
        Some(setup_boundary::response_wire_limit(
            MAX_MANUAL_EDIT_RESPONSE_BYTES,
        )),
        Some(control),
    )
}

#[tauri::command]
pub(super) async fn solution_create_repair_request<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    run(window, state, request, Action::Edit).await
}

#[tauri::command]
pub(super) async fn solution_lock_assignment<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    run(window, state, request, Action::Lock).await
}

#[tauri::command]
pub(super) async fn solution_unlock_assignment<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    run(window, state, request, Action::Unlock).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn review_response_uses_lossless_verification_facts() -> Result<(), Box<dyn std::error::Error>>
    {
        let id = "018f47f2-e880-7000-8000-000000000001";
        let digest = "a".repeat(64);
        let review = serde_json::from_value(json!({
            "schemaVersion": 1,
            "source": {
                "scenarioId": id, "revision": 1, "documentHash": digest,
                "original": {
                    "entity": {
                        "kind": "official.workforce.base-schedule",
                        "id": format!("official.workforce.{id}")
                    },
                    "pairContentHash": digest,
                    "acceptedResult": { "solutionId": id, "resultChecksum": digest },
                    "sourceScenarioId": id, "sourceRevision": 0,
                    "sourceDocumentHash": digest, "sourceRunId": id,
                    "sourceRunInputChecksum": digest, "sourceRunManifestChecksum": digest
                },
                "currentManualId": null, "currentManualChecksum": null,
                "selectedCount": 1, "protectionCount": 0,
                "authoredOptimizeUnavailableReason": null
            },
            "intent": {
                "kind": "protection", "personId": id, "shiftId": id,
                "protection": { "kind": "hard" }
            },
            "changes": [],
            "evaluations": [{
                "evaluation": {
                    "ruleId": id, "satisfied": false, "affectedEntities": [],
                    "messageKey": "official.workforce.evaluation.summary",
                    "expected": {
                        "official.workforce.fact.zero": { "type": "integer", "value": 0 },
                        "official.workforce.fact.minimum": { "type": "integer", "value": i64::MIN },
                        "official.workforce.fact.maximum": { "type": "integer", "value": i64::MAX }
                    },
                    "observed": {}, "evidence": []
                },
                "ownerPaths": [format!("/domain/rules/{id}")]
            }],
            "disposition": "invalid", "approvalDigest": null
        }))?;
        let cancellation = eutheto_types::CancellationToken::new();
        let response = review_response(
            id.parse()?,
            review,
            &OperationControl::Cancellation(cancellation),
        )
        .map_err(|error| format!("review encoding failed: {}", error.code))?;
        let tauri::ipc::InvokeResponseBody::Json(body) = tauri::ipc::IpcResponse::body(response)?
        else {
            return Err("manual review was not JSON".into());
        };
        let wire: Value = serde_json::from_str(&body)?;
        let facts = &wire["result"]["review"]["evaluations"][0]["evaluation"]["expected"];
        assert_eq!(facts["official.workforce.fact.zero"]["value"], "0");
        assert_eq!(
            facts["official.workforce.fact.minimum"]["value"],
            i64::MIN.to_string()
        );
        assert_eq!(
            facts["official.workforce.fact.maximum"]["value"],
            i64::MAX.to_string()
        );
        assert_eq!(wire["result"]["review"]["source"]["revision"], 1);
        Ok(())
    }

    fn review() -> Value {
        json!({
            "mode": "review", "schemaVersion": 1,
            "requestId": "018f47f2-e880-7000-8000-000000000001",
            "operationId": "018f47f2-e880-7000-8000-000000000002",
            "scenarioId": "018f47f2-e880-7000-8000-000000000003",
            "expectedRevision": 4,
            "intent": {
                "kind": "protection",
                "personId": "018f47f2-e880-7000-8000-000000000004",
                "shiftId": "018f47f2-e880-7000-8000-000000000005",
                "protection": { "kind": "hard" }
            }
        })
    }

    #[test]
    fn convenience_commands_cannot_change_selection_or_reverse_protection() -> Result<(), String> {
        let (_, request) = decode(Some(review())).map_err(|error| format!("{error:?}"))?;
        assert!(require_action(&request, Action::Lock).is_ok());
        assert!(require_action(&request, Action::Unlock).is_err());
        let mut removal = review();
        removal["intent"]["kind"] = json!("remove");
        removal["intent"]
            .as_object_mut()
            .ok_or("expected intent object")?
            .remove("protection");
        removal["intent"]["protectionChanges"] = json!([]);
        let (_, request) = decode(Some(removal)).map_err(|error| format!("{error:?}"))?;
        assert!(require_action(&request, Action::Edit).is_ok());
        assert!(require_action(&request, Action::Lock).is_err());
        assert!(require_action(&request, Action::Unlock).is_err());
        Ok(())
    }

    #[test]
    fn request_rejects_unknown_authority_and_oversized_protection_before_decode()
    -> Result<(), String> {
        let mut unknown = review();
        unknown["accepted"] = json!(true);
        assert!(decode(Some(unknown)).is_err());
        let mut oversized = review();
        oversized["intent"]["protectionChanges"] = json!([null, null, null]);
        let error = decode(Some(oversized))
            .err()
            .ok_or("oversized transitions were accepted")?;
        assert_eq!(error.code, "manual.protection_limit");
        let mut missing_owner = review();
        missing_owner
            .as_object_mut()
            .ok_or("expected request object")?
            .remove("operationId");
        assert!(decode(Some(missing_owner)).is_err());
        Ok(())
    }
}

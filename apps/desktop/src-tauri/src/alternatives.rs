//! Versioned desktop ownership for the Core Alternatives operation.

use super::operations::{
    AlternativesGroupDtoV1, OperationClaim, OperationContextV1, OperationControlRequestV1,
    OperationPhaseV1, OperationPurposeV1,
};
use super::{
    ApiError, DesktopState, SOLVE_RESPONSE_MAX_BYTES, SolutionApiResult, boundary_error,
    decode_solve_request, map_stored_solve_failure, require_solve_runtime, setup_boundary,
    solve_current_revision, solve_options,
};
use eutheto_core::WorkforceAlternativesRequest;
use eutheto_types::{OperationId, RequestId, Revision, ScenarioId, SolveMode};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tauri::State;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartRequest {
    schema_version: u32,
    request_id: RequestId,
    operation_id: OperationId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    mode: SolveMode,
    #[serde(default)]
    baseline_source: eutheto_core::StoredBaselineSourceV1,
    #[serde(default)]
    maximum_time_milliseconds: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GetRequest {
    schema_version: u32,
    request_id: RequestId,
    scenario_id: ScenarioId,
    expected_revision: Revision,
    #[serde(default)]
    group_request_id: Option<RequestId>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupStatus {
    schema_version: u32,
    scenario_id: ScenarioId,
    scenario_revision: Revision,
    current_revision: Revision,
    group: Option<AlternativesGroupDtoV1>,
}

fn require_version(version: u32) -> Result<(), ApiError> {
    if version == 1 {
        return Ok(());
    }
    Err(boundary_error(
        "solve.alternatives_unsupported_schema",
        "This Alternatives request version is not supported.",
        Some("/schemaVersion"),
    )
    .into())
}

#[tauri::command]
#[allow(clippy::too_many_lines)]
pub(super) async fn alternatives_start<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: StartRequest = decode_solve_request(request)?;
    require_version(request.schema_version)?;
    let core_request = WorkforceAlternativesRequest {
        request_id: request.request_id,
        scenario_id: request.scenario_id,
        expected_revision: request.expected_revision,
        baseline_source: request.baseline_source,
        options: solve_options(request.mode, request.maximum_time_milliseconds)?,
    };
    let claim = OperationClaim {
        operation_id: request.operation_id,
        request_id: request.request_id,
        purpose: Some(OperationPurposeV1::SolveAlternatives),
        context: OperationContextV1::Scenario {
            scenario_id: request.scenario_id,
            expected_revision: Some(request.expected_revision),
        },
    };
    if let Some(group) =
        state
            .operations
            .register_alternatives(window.label(), &claim, core_request.clone())?
    {
        // A redundant prepare token is not the existing group's owner.
        if group.cancel_operation_id != Some(request.operation_id) {
            state.operations.release(
                window.label(),
                &OperationControlRequestV1 {
                    schema_version: 1,
                    request_id: request.request_id,
                    operation_id: request.operation_id,
                },
            )?;
        }
        let revision = solve_current_revision(&state.app, request.scenario_id).await?;
        return setup_boundary::encode_response(
            request.request_id,
            Some(revision),
            GroupStatus {
                schema_version: 1,
                scenario_id: request.scenario_id,
                scenario_revision: request.expected_revision,
                current_revision: revision,
                group: Some(group),
            },
            SOLVE_RESPONSE_MAX_BYTES,
            None,
        )
        .await;
    }
    let app = state.app.clone();
    let operations = Arc::clone(&state.operations);
    let owner = window.label().to_owned();
    let time_limit = core_request.options.time_limit_milliseconds;
    state
        .operations
        .run_with_solve_budget(
            window.label(),
            claim,
            None,
            OperationPhaseV1::Validating,
            (core_request, None),
            time_limit,
            move |core_request, mut execution| async move {
                let (budget, started_at) = execution.take_solve_admission().ok_or_else(|| {
                    boundary_error(
                        "solve.budget_missing",
                        "The bounded Alternatives owner was not admitted safely.",
                        None,
                    )
                })?;
                require_solve_runtime(&app)?;
                let observer = Arc::new(
                    move |snapshot: &eutheto_core::WorkforceAlternativesSnapshotV1| {
                        // Observation failure cannot undo a durable accepted member or create a retry.
                        let _ =
                            operations.observe_alternatives(&owner, request.operation_id, snapshot);
                    },
                );
                let snapshot = app
                    .solve_workforce_alternatives(core_request, budget, started_at, observer)
                    .await
                    .map_err(|failure| Box::new(map_stored_solve_failure(failure.failure)))?;
                let revision = solve_current_revision(&app, request.scenario_id).await?;
                setup_boundary::finish(
                    &mut execution,
                    request.request_id,
                    Some(revision),
                    GroupStatus {
                        schema_version: 1,
                        scenario_id: request.scenario_id,
                        scenario_revision: request.expected_revision,
                        current_revision: revision,
                        group: Some(AlternativesGroupDtoV1 {
                            schema_version: 1,
                            request_id: request.request_id,
                            scenario_id: request.scenario_id,
                            scenario_revision: request.expected_revision,
                            baseline_source: request.baseline_source,
                            cancel_operation_id: None,
                            settled: true,
                            snapshot: Some(snapshot),
                        }),
                    },
                    SOLVE_RESPONSE_MAX_BYTES,
                    None,
                )
                .await
            },
        )
        .await
}

#[tauri::command]
pub(super) async fn alternatives_get_group<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: State<'_, DesktopState>,
    request: Option<Value>,
) -> SolutionApiResult {
    let request: GetRequest = decode_solve_request(request)?;
    require_version(request.schema_version)?;
    let revision = solve_current_revision(&state.app, request.scenario_id).await?;
    let group = state.operations.alternatives_group(
        window.label(),
        request.scenario_id,
        request.expected_revision,
        request.group_request_id,
    )?;
    setup_boundary::encode_response(
        request.request_id,
        Some(revision),
        GroupStatus {
            schema_version: 1,
            scenario_id: request.scenario_id,
            scenario_revision: request.expected_revision,
            current_revision: revision,
            group,
        },
        SOLVE_RESPONSE_MAX_BYTES,
        None,
    )
    .await
}

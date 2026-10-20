use eutheto_domain_ir::{
    OptimizationDirection, PortableAcceptedResult, RunInput, RunManifest, RunNativeAttempt,
    RunNativeAttemptTermination, RunNativeObjective, RunNativeObservations, RunNativeOutcome,
    RunNativeRouterReason, RunNativeRuntime, RunNativeTerminationReason, ScoreLevelId, blake3_hex,
};
use eutheto_types::SolveStatus;
use std::error::Error;

fn source() -> Result<RunInput, Box<dyn Error>> {
    Ok(
        PortableAcceptedResult::from_json(include_bytes!("fixtures/portable_accepted_v2.json"))?
            .run_input,
    )
}

fn bound_only(input: &RunInput) -> Result<RunNativeObservations, Box<dyn Error>> {
    Ok(RunNativeObservations::new(
        input,
        vec![RunNativeObjective {
            level_id: ScoreLevelId::new("official.test.score.cost")?,
            direction: OptimizationDirection::Minimize,
            lower_bound: 0,
            upper_bound: 10,
        }],
        SolveStatus::NoSolutionWithinLimit,
        RunNativeRouterReason::BackendTerminated,
        Some(RunNativeAttempt {
            backend_id: input.backend_id.clone(),
            backend_version: input.backend_version.clone(),
            adapter_version: input.adapter_version.clone(),
            solve_fingerprint: blake3_hex(b"routed-fingerprint-syntax-only"),
            invocation_index: Some(1),
            remaining_at_dispatch_milliseconds: input.solve_options.time_limit_milliseconds,
            backend_limit_milliseconds: input.solve_options.time_limit_milliseconds,
            termination: RunNativeAttemptTermination::BackendOutcome(
                RunNativeTerminationReason::TimeLimit,
            ),
            backend_failure_code: None,
            approved_worker_manifest_sha256: None,
            outcome: Some(RunNativeOutcome {
                reason: RunNativeTerminationReason::TimeLimit,
                objective_values: None,
                best_bound_values: Some(vec![3]),
                runtime: None,
            }),
        }),
    )?)
}

#[test]
fn compact_bounds_remain_independent_and_reject_invalid_claims() -> Result<(), Box<dyn Error>> {
    let input = source()?;
    let retained = bound_only(&input)?;
    retained.validate_for(&input)?;
    let decoded: RunNativeObservations = serde_json::from_slice(&serde_json::to_vec(&retained)?)?;
    let native = decoded
        .attempt
        .as_ref()
        .and_then(|attempt| attempt.outcome.as_ref())
        .ok_or("missing bound-only native outcome")?;
    assert_eq!(native.objective_values, None);
    assert_eq!(native.best_bound_values.as_deref(), Some([3].as_slice()));
    assert_eq!(native.runtime, None);
    for invalid_values in [vec![], vec![3, 3], vec![11], vec![-1]] {
        let mut invalid = retained.clone();
        invalid
            .attempt
            .as_mut()
            .and_then(|attempt| attempt.outcome.as_mut())
            .ok_or("native outcome")?
            .best_bound_values = Some(invalid_values);
        assert!(invalid.validate_for(&input).is_err());
    }
    let mut contradictory = retained.clone();
    contradictory
        .attempt
        .as_mut()
        .and_then(|attempt| attempt.outcome.as_mut())
        .ok_or("native outcome")?
        .objective_values = Some(vec![2]);
    assert!(contradictory.validate_for(&input).is_err());
    contradictory.objectives[0].direction = OptimizationDirection::Maximize;
    contradictory.validate_for(&input)?;

    let mut preflight = retained.clone();
    preflight.attempt.as_mut().ok_or("attempt")?.termination =
        RunNativeAttemptTermination::PreflightRejected {
            code: "solver.preflight".to_owned(),
        };
    assert!(preflight.validate_for(&input).is_err());
    let mut detached = retained.clone();
    detached.model_hash = blake3_hex(b"other-model");
    assert!(detached.validate_for(&input).is_err());
    detached = retained.clone();
    detached.solve_options_hash = blake3_hex(b"other-options");
    assert!(detached.validate_for(&input).is_err());
    detached = retained.clone();
    detached.attempt.as_mut().ok_or("attempt")?.adapter_version = "other.1".to_owned();
    assert!(detached.validate_for(&input).is_err());
    Ok(())
}

#[test]
fn router_time_limit_without_worker_outcome_roundtrips() -> Result<(), Box<dyn Error>> {
    let input = source()?;
    let mut retained = bound_only(&input)?;
    retained.attempt.as_mut().ok_or("attempt")?.outcome = None;
    retained
        .attempt
        .as_mut()
        .ok_or("attempt")?
        .backend_failure_code = Some("tests.headless.failure".to_owned());
    retained.validate_for(&input)?;
    let decoded: RunNativeObservations = serde_json::from_slice(&serde_json::to_vec(&retained)?)?;
    decoded.validate_for(&input)?;
    assert_eq!(decoded, retained);
    assert!(decoded.attempt.as_ref().ok_or("attempt")?.outcome.is_none());
    for invocation in [None, Some(2)] {
        let mut invalid = retained.clone();
        invalid.attempt.as_mut().ok_or("attempt")?.invocation_index = invocation;
        assert!(invalid.validate_for(&input).is_err());
    }
    for reason in [
        RunNativeTerminationReason::OptimalityClaimed,
        RunNativeTerminationReason::CandidateFound,
        RunNativeTerminationReason::InfeasibilityClaimed,
        RunNativeTerminationReason::UnboundedClaimed,
        RunNativeTerminationReason::SolutionLimit,
        RunNativeTerminationReason::Cancelled,
        RunNativeTerminationReason::InvalidModel,
        RunNativeTerminationReason::Unavailable,
        RunNativeTerminationReason::Failed,
    ] {
        let mut invalid = retained.clone();
        invalid.attempt.as_mut().ok_or("attempt")?.termination =
            RunNativeAttemptTermination::BackendOutcome(reason);
        assert!(invalid.validate_for(&input).is_err());
    }
    let mut invalid = retained.clone();
    invalid.terminal_status = SolveStatus::Infeasible;
    assert!(invalid.validate_for(&input).is_err());
    invalid = retained.clone();
    invalid.terminal_reason = RunNativeRouterReason::CandidateVerified;
    assert!(invalid.validate_for(&input).is_err());
    invalid = retained.clone();
    invalid.schema_version += 1;
    assert!(invalid.validate_for(&input).is_err());
    invalid = bound_only(&input)?;
    invalid
        .attempt
        .as_mut()
        .ok_or("attempt")?
        .outcome
        .as_mut()
        .ok_or("outcome")?
        .reason = RunNativeTerminationReason::Cancelled;
    assert!(invalid.validate_for(&input).is_err());
    Ok(())
}

#[test]
fn compact_runtime_requires_terminal_digest_and_exact_input_identity() -> Result<(), Box<dyn Error>>
{
    let input = source()?;
    let mut retained = bound_only(&input)?;
    let runtime = RunNativeRuntime {
        worker_version: input.worker_version.clone(),
        engine_version: input.solver_version.clone(),
        protocol_major: input.protocol_major,
        protocol_minor: input.protocol_minor,
        model_fingerprint_sha256: "a".repeat(64),
        applied_parameters_sha256: "b".repeat(64),
    };
    retained
        .attempt
        .as_mut()
        .and_then(|attempt| attempt.outcome.as_mut())
        .ok_or("native outcome")?
        .runtime = Some(runtime);
    retained.validate_for(&input)?;
    let mut wire = serde_json::to_value(&retained)?;
    wire["attempt"]["outcome"]["runtime"]["appliedParametersSha256"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<RunNativeObservations>(wire).is_err());
    retained
        .attempt
        .as_mut()
        .and_then(|attempt| attempt.outcome.as_mut())
        .and_then(|outcome| outcome.runtime.as_mut())
        .ok_or("runtime")?
        .worker_version = "other.1".to_owned();
    assert!(retained.validate_for(&input).is_err());
    Ok(())
}

#[test]
fn manifest_claims_reject_null_old_envelopes_and_resealed_input_substitution()
-> Result<(), Box<dyn Error>> {
    let historical =
        PortableAcceptedResult::from_json(include_bytes!("fixtures/portable_accepted_v2.json"))?;
    let observations = bound_only(&historical.run_input)?;
    assert!(
        historical
            .run_manifest
            .clone()
            .with_native_observations(Some(observations.clone()))
            .is_err()
    );
    let mut current = historical.run_manifest.clone();
    current.schema_version = eutheto_domain_ir::RUN_MANIFEST_SCHEMA_VERSION;
    let current = current.with_native_observations(Some(observations))?;
    let roundtrip = RunManifest::from_json(&serde_json::to_vec(&current)?)?;
    roundtrip.validate_native_input_binding(&historical.run_input)?;

    let mut null = serde_json::to_value(&current)?;
    null["nativeObservations"] = serde_json::Value::Null;
    assert!(RunManifest::from_json(&serde_json::to_vec(&null)?).is_err());
    let mut old = serde_json::to_value(&current)?;
    old["schemaVersion"] = 1.into();
    assert!(RunManifest::from_json(&serde_json::to_vec(&old)?).is_err());
    old["schemaVersion"] = 3.into();
    assert!(RunManifest::from_json(&serde_json::to_vec(&old)?).is_err());

    let accepted = PortableAcceptedResult::new(
        historical.run_input.clone(),
        current.clone(),
        historical.accepted_result.clone(),
        historical.evidence.clone(),
    )?;
    accepted.validate()?;
    let mut old_wrapper = serde_json::to_value(&accepted)?;
    old_wrapper["schemaVersion"] = 2.into();
    assert!(PortableAcceptedResult::from_json(&serde_json::to_vec(&old_wrapper)?).is_err());

    let mut detached = serde_json::to_value(&current)?;
    detached["nativeObservations"]["modelHash"] = blake3_hex(b"substituted-native-model").into();
    detached
        .as_object_mut()
        .ok_or("manifest object")?
        .remove("checksum");
    let checksum = blake3_hex(&serde_json::to_vec(&detached)?);
    detached["checksum"] = checksum.into();
    let detached = RunManifest::from_json(&serde_json::to_vec(&detached)?)?;
    assert!(
        detached
            .validate_native_input_binding(&historical.run_input)
            .is_err()
    );
    assert!(
        PortableAcceptedResult::new(
            historical.run_input,
            detached,
            historical.accepted_result,
            historical.evidence,
        )
        .is_err()
    );
    Ok(())
}

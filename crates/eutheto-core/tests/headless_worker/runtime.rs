use super::*;

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved real worker; full practitioner completion calibration"]
async fn practitioner_balanced_finishes_with_an_accepted_native_terminal() -> TestResult {
    let (service, runtime) = real_service().await?;
    let mut solve_options = options()?;
    solve_options.time_limit_milliseconds = DurationMillis::new(5_000)?;
    solve_options.collect_intermediate_solutions = true;
    solve_options.explanation_mode = ExplanationMode::Standard;
    solve_options.resource_limits = ResourceLimits {
        max_entities: 100_000,
        max_rules: 100_000,
        max_variables: 100_000,
        max_constraints: 500_000,
    };
    let operation = service
        .begin_solve(
            RequestId::new(&SystemIdGenerator)?,
            solve_options,
            CancellationToken::new(),
        )
        .map_err(boxed)?;
    let inspected = service
        .decode_scenario(
            include_bytes!("../../../../domains/workforce/fixtures/v2/practitioner-workflow.json"),
            &operation.control(),
        )
        .map_err(boxed)?;
    let prepared = operation.prepare(inspected.scenario).map_err(boxed)?;
    let outcome = prepared.execute(&mut Progress).await.map_err(boxed)?;
    assert_bound_runtime(&outcome, &runtime)?;
    let accepted = outcome
        .accepted_result()
        .ok_or("no accepted terminal result; a checkpoint is insufficient")?;
    assert!(accepted.verification.accepted);
    assert!(matches!(
        outcome.manifest().outcome,
        RunTerminalOutcomeV1::Accepted { .. }
    ));
    let backend = outcome.execution.attempts[0]
        .outcome
        .as_ref()
        .ok_or("missing completed backend outcome")?;
    assert!(matches!(
        backend.termination,
        BackendTerminationReason::CandidateFound | BackendTerminationReason::OptimalityClaimed
    ));
    let execution = backend
        .evidence
        .execution
        .as_ref()
        .ok_or("missing admitted native execution")?;
    let statistics = execution
        .worker_statistics
        .as_ref()
        .ok_or("missing completed native statistics")?;
    let digest = execution
        .reproducibility
        .applied_parameters_sha256
        .as_ref()
        .ok_or("missing completed native parameters digest")?;
    assert_eq!(digest.len(), 64);
    assert!(statistics.wall_time_milliseconds.is_some());
    assert!(
        !execution
            .reproducibility
            .applied_parameters
            .stop_after_first_feasible
    );
    assert!(
        execution
            .reproducibility
            .applied_parameters
            .emit_intermediate_solutions
    );
    eprintln!(
        "{}",
        json!({
            "case": "practitioner_balanced_complete",
            "terminal": outcome.manifest().outcome,
            "elapsedMilliseconds": outcome.manifest().elapsed_milliseconds,
            "phaseTimings": outcome.manifest().phase_timings,
            "backendTermination": backend.termination,
        "nativeOutcome": outcome.manifest().native_observations.as_ref().and_then(|observations| observations.attempt.as_ref()).and_then(|attempt| attempt.outcome.as_ref()),
            "backendLimitMilliseconds": backend.evidence.backend_limit_milliseconds,
            "backendElapsedMilliseconds": backend.evidence.elapsed_milliseconds,
            "appliedParameters": execution.reproducibility.applied_parameters,
            "appliedParametersSha256": digest,
            "workerWallMilliseconds": statistics.wall_time_milliseconds,
        })
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved real worker; no worker launch"]
async fn real_worker_wrappers_delegate_completion_reserve() -> TestResult {
    let (registry, runtime) = real_registry().await?;
    let inner = Arc::clone(
        registry
            .get(runtime.backend_id())
            .ok_or("missing real worker")?,
    );
    let expected = inner.completion_reserve();
    assert_ne!(expected, DurationMillis::ZERO);
    let faulted = FaultedRealWorker {
        inner: Arc::clone(&inner),
        fault: WorkerFault::FailAfterRealOutcome,
        invocations: Arc::new(AtomicUsize::new(0)),
    };
    let retaining = RetainingContradictoryWorker {
        inner,
        retained: Arc::new(Mutex::new(None)),
        invocations: Arc::new(AtomicUsize::new(0)),
    };
    assert_eq!(faulted.completion_reserve(), expected);
    assert_eq!(retaining.completion_reserve(), expected);
    Ok(())
}

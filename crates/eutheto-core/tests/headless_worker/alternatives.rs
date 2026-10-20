use super::*;
use eutheto_core::{
    NativeObjectiveCapture, WorkforceAlternativeEquivalenceV1, WorkforceAlternativeMemberStatusV1,
    WorkforceAlternativeProfileV1, WorkforceAlternativesRequest, WorkforceAlternativesSnapshotV1,
    WorkforceAlternativesStatusV1, WorkforceAlternativesStopV1,
};
use eutheto_types::{FixedMonotonicClock, ParentSolveBudget};

async fn adopt_native_base(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    base: &StoredSolveOutcome,
) -> TestResult<Revision> {
    let base_result = base.portable_result().ok_or("missing native base result")?;
    let AppCommandResult::ScenarioCommand(adoption) = app
        .execute(AppCommand::AdoptBaseSchedule(
            eutheto_core::SolutionAdoptBaseRequestV1 {
                schema_version: eutheto_core::SOLUTION_ADOPT_BASE_SCHEMA_VERSION,
                request_id: RequestId::new(&SystemIdGenerator)?,
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                result: AcceptedResultRefV1::from_result(&base_result.accepted_result)?,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Captured Alternatives regression".to_owned(),
                },
                truncate_redo: false,
            },
        ))
        .await
        .map_err(boxed)?
    else {
        return Err("base adoption omitted receipt".into());
    };
    Ok(adoption.new_revision)
}

fn member_request(
    scenario_id: ScenarioId,
    capture: &NativeObjectiveCapture,
    options: &SolveOptions,
    profile: &str,
) -> TestResult<StoredSolveRequest> {
    Ok(StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Some(capture.revision()),
        options: options.clone(),
        objective_selection: Some(
            capture.selection(profile.parse()?, RunObjectiveOperation::Optimize),
        ),
    })
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_members_share_one_capture_parent_and_recorded_profiles() -> TestResult {
    let original = snapshot(1)?;
    let scenario_id = original.document.scenario_id;
    let (_directory, store, base, runtime, app) =
        stored_real_run(original, ExplanationMode::None).await?;
    let revision = adopt_native_base(&app, scenario_id, &base).await?;
    let source_before = store.get_project(scenario_id).await?;
    let mut common_options = options()?;
    common_options.backend = BackendSelection::Specific(runtime.backend_id().clone());
    let parent = ParentSolveBudget::new(
        common_options.time_limit_milliseconds,
        Arc::new(SystemMonotonicClock::new()),
        CancellationToken::new(),
    )?;
    let started_at = SystemClock.now();
    let capture = app
        .capture_objective_alternatives_source(
            scenario_id,
            Some(revision),
            eutheto_core::StoredBaselineSourceV1::Accepted,
            OperationControl::Solve(parent.phase_view()),
            started_at,
        )
        .await
        .map_err(boxed)?;
    assert_eq!(capture.revision(), revision);
    let bindings = Arc::new(Mutex::new(Vec::new()));
    let first_profile = "official.workforce.objective.balanced";
    for invalid_count in [0, 5, u8::MAX] {
        let request = member_request(scenario_id, &capture, &common_options, first_profile)?;
        let request_id = request.request_id;
        let observed = Arc::clone(&bindings);
        let refusal = app
            .solve_stored_with_budget_observer_in_capture(
                request,
                &mut Progress,
                parent.clone(),
                started_at,
                Arc::new(move |run_id| {
                    observed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(run_id);
                }),
                &capture,
                invalid_count,
            )
            .await;
        assert!(matches!(refusal, Err(StoredSolveFailure::BeforeStart(_))));
        assert!(store.load_solve_run_by_request(request_id).await?.is_none());
    }
    assert!(
        bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    let profiles = [
        (first_profile, 4),
        ("official.workforce.objective.preference-focused", 3),
        ("official.workforce.objective.minimal-changes", 2),
        ("official.workforce.objective.fairness-first", 1),
    ];
    let mut capture_binding = None;
    let mut previous_elapsed = DurationMillis::ZERO;
    for (index, (profile, remaining_members)) in profiles.into_iter().enumerate() {
        let request = member_request(scenario_id, &capture, &common_options, profile)?;
        let request_id = request.request_id;
        let observed = Arc::clone(&bindings);
        let outcome = app
            .solve_stored_with_budget_observer_in_capture(
                request,
                &mut Progress,
                parent.clone(),
                started_at,
                Arc::new(move |run_id| {
                    observed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(run_id);
                }),
                &capture,
                remaining_members,
            )
            .await
            .map_err(boxed)?;
        assert!(!outcome.reused);
        assert_stored_bound_runtime(&outcome, &runtime)?;
        let portable = outcome
            .portable_result()
            .ok_or("member omitted accepted result")?;
        assert_eq!(outcome.accepted_result_id(), Some(portable.result_id));
        let input = &portable.run_input;
        assert_eq!(input.request_id, request_id);
        assert_eq!(input.snapshot_id, capture.snapshot_id());
        assert_eq!(input.scenario_revision, revision.value());
        assert_eq!(input.solve_options, common_options);
        let context = input
            .objective_context
            .as_ref()
            .ok_or("missing profile context")?;
        assert_eq!(context.profile_id.as_str(), profile);
        assert_eq!(context.semantic_version, 1);
        assert_eq!(context.operation, RunObjectiveOperation::Optimize);
        let baseline = context
            .baseline
            .as_deref()
            .ok_or("member omitted captured baseline")?;
        let binding = (
            input.snapshot_document_hash.clone(),
            input.snapshot_created_at,
            baseline.clone(),
        );
        if let Some(expected) = &capture_binding {
            assert_eq!(&binding, expected);
        } else {
            capture_binding = Some(binding);
        }
        assert_eq!(portable.run_manifest.started_at, started_at);
        assert!(
            input.snapshot_created_at <= started_at,
            "a shared source must retain the original operation's snapshot/start chronology",
        );
        let elapsed = portable
            .run_manifest
            .elapsed_milliseconds
            .ok_or("missing elapsed")?;
        assert!(elapsed >= previous_elapsed);
        assert!(elapsed <= common_options.time_limit_milliseconds);
        previous_elapsed = elapsed;
        let recorded = store
            .load_solve_run_by_request(request_id)
            .await?
            .ok_or("missing member run")?;
        assert_eq!(recorded.input, *input);
        assert_eq!(recorded.manifest.as_ref(), Some(&portable.run_manifest));
        {
            let observed = bindings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(observed.len(), index + 1);
            assert_eq!(observed[index], input.run_id);
        }
        let AppQueryResult::SolutionVerification(verified) = app
            .query(AppQuery::SolutionVerify(SolutionVerifyRequestV1 {
                schema_version: SOLUTION_API_SCHEMA_VERSION,
                scenario_id,
                solution_id: portable.result_id,
            }))
            .await
            .map_err(boxed)?
        else {
            return Err("fresh member verification omitted native report".into());
        };
        assert_eq!(verified.verification, portable.accepted_result.verification);
    }
    let source_after = store.get_project(scenario_id).await?;
    assert_eq!(
        source_after.summary.revision,
        source_before.summary.revision
    );
    assert_eq!(source_after.document, source_before.document);

    // Spend the worker share only after durable admission. A zero share still owns a real run.
    let clock = Arc::new(FixedMonotonicClock::default());
    let zero_parent = ParentSolveBudget::new(
        common_options.time_limit_milliseconds,
        clock.clone(),
        CancellationToken::new(),
    )?;
    let request = member_request(scenario_id, &capture, &common_options, first_profile)?;
    let request_id = request.request_id;
    let zero_bindings = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&zero_bindings);
    let dispatch_clock = Arc::clone(&clock);
    let zero_started_at = SystemClock.now();
    let zero = app
        .solve_stored_with_budget_observer_in_capture(
            request,
            &mut Progress,
            zero_parent.clone(),
            zero_started_at,
            Arc::new(move |run_id| {
                observed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(run_id);
                assert!(dispatch_clock.set(Duration::from_millis(59_100)).is_ok());
            }),
            &capture,
            4,
        )
        .await
        .map_err(boxed)?;
    assert!(!zero.reused);
    assert!(zero.accepted_result_id().is_none());
    assert!(zero.portable_result().is_none());
    assert_eq!(
        zero.report
            .as_ref()
            .ok_or("missing zero-share report")?
            .execution
            .invocation_count,
        0
    );
    assert_eq!(
        zero_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[zero.state().input.run_id],
    );
    let manifest = zero
        .state()
        .manifest
        .as_ref()
        .ok_or("zero-share run was not terminalized")?;
    assert_eq!(manifest.started_at, zero_started_at);
    assert_eq!(
        manifest.elapsed_milliseconds,
        Some(DurationMillis::new(59_100)?)
    );
    assert_eq!(
        manifest.outcome,
        RunTerminalOutcomeV1::NoResult {
            status: SolveStatus::NoSolutionWithinLimit
        }
    );
    let recorded = store
        .load_solve_run_by_request(request_id)
        .await?
        .ok_or("bound zero-share run missing")?;
    assert_eq!(&recorded, zero.state());
    assert!(!zero_parent.is_expired());
    assert_eq!(zero_parent.snapshot().remaining_milliseconds.value(), 900);
    Ok(())
}

async fn coordinator_fixture() -> TestResult<(
    tempfile::TempDir,
    Arc<SqliteScenarioStore>,
    EuthetoApp,
    WorkforceAlternativesRequest,
)> {
    let original = snapshot(1)?;
    let scenario_id = original.document.scenario_id;
    let (directory, store, base, runtime, app) =
        stored_real_run(original, ExplanationMode::None).await?;
    let revision = adopt_native_base(&app, scenario_id, &base).await?;
    let mut options = options()?;
    options.backend = BackendSelection::Specific(runtime.backend_id().clone());
    let request = WorkforceAlternativesRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: revision,
        baseline_source: eutheto_core::StoredBaselineSourceV1::Accepted,
        options,
    };
    Ok((directory, store, app, request))
}

fn group_budget(
    request: &WorkforceAlternativesRequest,
    cancellation: CancellationToken,
) -> TestResult<ParentSolveBudget> {
    Ok(ParentSolveBudget::new(
        request.options.time_limit_milliseconds,
        Arc::new(SystemMonotonicClock::new()),
        cancellation,
    )?)
}

fn assert_callback_transitions(
    observations: &[WorkforceAlternativesSnapshotV1],
    bound_counts: [usize; 4],
    finished_counts: [usize; 4],
) {
    assert!(!observations.is_empty());
    for index in 0..4 {
        let bound = observations
            .windows(2)
            .filter(|pair| {
                pair[0].members[index].run_id.is_none() && pair[1].members[index].run_id.is_some()
            })
            .count();
        let finished = observations
            .windows(2)
            .filter(|pair| {
                pair[0].members[index].status != WorkforceAlternativeMemberStatusV1::Finished
                    && pair[1].members[index].status == WorkforceAlternativeMemberStatusV1::Finished
            })
            .count();
        assert_eq!(
            bound, bound_counts[index],
            "member {index} native binding count"
        );
        assert_eq!(
            finished, finished_counts[index],
            "member {index} terminal callback count"
        );
        for observed in observations {
            if observed.members[index].run_id.is_some() {
                assert!(observed.members[..index].iter().all(|member| {
                    member.status == WorkforceAlternativeMemberStatusV1::Finished
                }));
            }
        }
    }
}

async fn assert_saved_member(
    app: &EuthetoApp,
    store: &SqliteScenarioStore,
    request: &WorkforceAlternativesRequest,
    snapshot: &WorkforceAlternativesSnapshotV1,
    index: usize,
) -> TestResult<PortableAcceptedResult> {
    let member = &snapshot.members[index];
    let request_id = member.request_id.ok_or("member omitted stable request")?;
    let run = store
        .load_solve_run_by_request(request_id)
        .await?
        .ok_or("member run missing")?;
    assert_eq!(member.run_id, Some(run.input.run_id));
    assert_eq!(
        run.input.snapshot_id,
        snapshot.capture_id.ok_or("group omitted capture")?
    );
    assert_eq!(
        run.input.scenario_revision,
        request.expected_revision.value()
    );
    assert_eq!(run.input.solve_options, request.options);
    let context = run
        .input
        .objective_context
        .as_ref()
        .ok_or("missing native profile")?;
    assert_eq!(context.profile_id.as_str(), member.profile.objective_id());
    assert_eq!(context.operation, RunObjectiveOperation::Optimize);
    assert!(context.baseline.is_some());
    let manifest = run.manifest.as_ref().ok_or("member manifest missing")?;
    assert_eq!(manifest.started_at, snapshot.started_at);
    assert!(
        run.input.snapshot_created_at <= snapshot.started_at,
        "capturing after group admission must not move the snapshot anchor past its common start",
    );
    assert_eq!(member.outcome.as_ref(), Some(&manifest.outcome));
    let native = manifest
        .native_observations
        .as_ref()
        .ok_or("missing admitted native observations")?;
    let attempt = native
        .attempt
        .as_ref()
        .ok_or("accepted member omitted actual worker attempt")?;
    assert_eq!(attempt.invocation_index, Some(1));
    assert_eq!(attempt.backend_id, run.input.backend_id);
    assert!(attempt.approved_worker_manifest_sha256.is_some());
    let runtime = attempt
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.runtime.as_ref())
        .ok_or("accepted member omitted protocol-admitted runtime evidence")?;
    assert_eq!(runtime.worker_version, run.input.worker_version);
    assert_eq!(runtime.engine_version, run.input.solver_version);
    let remaining_members = u64::try_from(4 - index)?;
    let ordinary_reserve = (request.options.time_limit_milliseconds.value() / 4).clamp(1, 1_000);
    assert!(
        attempt.backend_limit_milliseconds.value()
            <= attempt
                .remaining_at_dispatch_milliseconds
                .value()
                .saturating_sub(ordinary_reserve)
                / remaining_members
    );
    let RunTerminalOutcomeV1::Accepted { solution_id, .. } = manifest.outcome else {
        return Err("native member was not accepted".into());
    };
    let stored = store.load_accepted_result(solution_id).await?;
    assert_eq!(stored.portable.run_input, run.input);
    assert_eq!(&stored.portable.run_manifest, manifest);
    let AppQueryResult::SolutionVerification(verified) = app
        .query(AppQuery::SolutionVerify(SolutionVerifyRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id: request.scenario_id,
            solution_id,
        }))
        .await
        .map_err(boxed)?
    else {
        return Err("member verification omitted native report".into());
    };
    assert_eq!(
        verified.verification,
        stored.portable.accepted_result.verification
    );
    Ok(stored.portable)
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_coordinator_suppresses_equivalent_profiles_without_replaying_history()
-> TestResult {
    let (_directory, store, app, request) = coordinator_fixture().await?;
    let source_before = store.get_project(request.scenario_id).await?;
    let observations = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&observations);
    let started_at = SystemClock.now();
    let result = app
        .solve_workforce_alternatives(
            request.clone(),
            group_budget(&request, CancellationToken::new())?,
            started_at,
            Arc::new(move |snapshot| {
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(snapshot.clone());
            }),
        )
        .await
        .map_err(boxed)?;
    assert_eq!(result.status, WorkforceAlternativesStatusV1::Completed);
    assert_eq!(result.stop_reason, None);
    assert_eq!(result.started_at, started_at);
    assert_eq!(result.members[0].request_id, Some(request.request_id));
    let mut capture_binding = None;
    let mut unique_requests = BTreeSet::new();
    let mut previous_elapsed = DurationMillis::ZERO;
    for (index, member) in result.members.iter().enumerate() {
        assert_eq!(member.profile, WorkforceAlternativeProfileV1::ORDER[index]);
        assert_eq!(member.status, WorkforceAlternativeMemberStatusV1::Finished);
        assert_eq!(member.warning_code, None);
        assert!(unique_requests.insert(member.request_id.ok_or("missing request")?));
        assert_eq!(
            member.equivalence,
            Some(if index == 0 {
                WorkforceAlternativeEquivalenceV1::NotSuppressed
            } else {
                WorkforceAlternativeEquivalenceV1::Equivalent {
                    profile: WorkforceAlternativeProfileV1::Balanced,
                }
            })
        );
        let portable =
            Box::pin(assert_saved_member(&app, &store, &request, &result, index)).await?;
        let binding = (
            portable.run_input.snapshot_document_hash.clone(),
            portable.run_input.snapshot_created_at,
            portable
                .run_input
                .objective_context
                .as_ref()
                .and_then(|context| context.baseline.clone()),
        );
        if let Some(expected) = &capture_binding {
            assert_eq!(&binding, expected);
        } else {
            capture_binding = Some(binding);
        }
        let elapsed = portable
            .run_manifest
            .elapsed_milliseconds
            .ok_or("missing elapsed")?;
        assert!(elapsed >= previous_elapsed);
        assert!(elapsed <= request.options.time_limit_milliseconds);
        previous_elapsed = elapsed;
    }
    {
        let observed = observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_callback_transitions(&observed, [1; 4], [1; 4]);
        assert_eq!(observed.last(), Some(&result));
        assert!(
            observed
                .iter()
                .filter_map(|snapshot| snapshot.capture_id)
                .all(|id| Some(id) == result.capture_id)
        );
    }
    let source_after = store.get_project(request.scenario_id).await?;
    assert_eq!(source_before.document, source_after.document);
    assert_eq!(
        source_before.summary.revision,
        source_after.summary.revision
    );
    let history_before_replay = store.history(request.scenario_id).await?;
    let replay_observations = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&replay_observations);
    let replay = match app
        .solve_workforce_alternatives(
            request.clone(),
            group_budget(&request, CancellationToken::new())?,
            SystemClock.now(),
            Arc::new(move |snapshot| {
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(snapshot.clone());
            }),
        )
        .await
    {
        Err(failure) => failure,
        Ok(snapshot) => {
            return Err(format!("durable request replayed its group: {snapshot:?}").into());
        }
    };
    assert_eq!(
        replay.snapshot.stop_reason,
        Some(WorkforceAlternativesStopV1::SummaryUnavailable)
    );
    assert!(replay.snapshot.capture_id.is_none());
    assert!(
        replay
            .snapshot
            .members
            .iter()
            .all(|member| member.request_id.is_none() && member.run_id.is_none())
    );
    {
        let observed = replay_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(observed.len(), 2);
        assert_callback_transitions(&observed, [0; 4], [0; 4]);
    }
    assert_eq!(
        store.history(request.scenario_id).await?,
        history_before_replay
    );
    for index in 0..4 {
        Box::pin(assert_saved_member(&app, &store, &request, &result, index)).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn actual_worker_coordinator_cancel_preserves_first_acceptance_and_skips_later_requests()
-> TestResult {
    let (_directory, store, app, request) = coordinator_fixture().await?;
    let source_before = store.get_project(request.scenario_id).await?;
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&observations);
    let result = app
        .solve_workforce_alternatives(
            request.clone(),
            group_budget(&request, cancellation)?,
            SystemClock.now(),
            Arc::new(move |snapshot| {
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(snapshot.clone());
                if snapshot.members[0].status == WorkforceAlternativeMemberStatusV1::Finished {
                    cancel.cancel();
                }
            }),
        )
        .await
        .map_err(boxed)?;
    assert_eq!(result.status, WorkforceAlternativesStatusV1::Stopped);
    assert_eq!(
        result.stop_reason,
        Some(WorkforceAlternativesStopV1::Cancelled)
    );
    Box::pin(assert_saved_member(&app, &store, &request, &result, 0)).await?;
    for member in &result.members[1..] {
        assert_eq!(member.status, WorkforceAlternativeMemberStatusV1::Skipped);
        assert!(member.run_id.is_none());
        assert!(
            store
                .load_solve_run_by_request(member.request_id.ok_or("missing stable request")?)
                .await?
                .is_none()
        );
    }
    {
        let observed = observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_callback_transitions(&observed, [1, 0, 0, 0], [1, 0, 0, 0]);
    }
    let source_after = store.get_project(request.scenario_id).await?;
    assert_eq!(source_before.document, source_after.document);
    assert_eq!(
        source_before.summary.revision,
        source_after.summary.revision
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_coordinator_source_change_stops_admission_without_recapture() -> TestResult {
    let (_directory, store, app, request) = coordinator_fixture().await?;
    let source_before = store.get_project(request.scenario_id).await?;
    let changed = serde_json::to_value(snapshot(2)?.document)?;
    let replacement = changed["domain"]["entities"][id(8)].clone();
    let edit_app = app.clone();
    let scenario_id = request.scenario_id;
    let revision = request.expected_revision;
    let observations = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&observations);
    let edit_result = Arc::new(Mutex::new(None));
    let recorded_edit = Arc::clone(&edit_result);
    let edited = Arc::new(AtomicUsize::new(0));
    let recorded_edits = Arc::clone(&edited);
    let failure = match app
        .solve_workforce_alternatives(
            request.clone(),
            group_budget(&request, CancellationToken::new())?,
            SystemClock.now(),
            Arc::new(move |snapshot| {
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(snapshot.clone());
                if snapshot.members[0].status == WorkforceAlternativeMemberStatusV1::Finished
                    && recorded_edits
                        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    // The native source command must commit before this terminal callback returns.
                    let receipt = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(edit_manual_source(
                            &edit_app,
                            scenario_id,
                            revision,
                            eutheto_workforce::commands::UPDATE_ENTITY,
                            json!({"entity": replacement}),
                        ))
                    })
                    .map_err(|error| error.to_string());
                    *recorded_edit
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(receipt);
                }
            }),
        )
        .await
    {
        Err(failure) => failure,
        Ok(snapshot) => {
            return Err(format!("source change did not stop admissions: {snapshot:?}").into());
        }
    };
    let receipt = edit_result
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .ok_or("terminal callback did not edit source")??;
    assert_eq!(edited.load(Ordering::SeqCst), 1);
    assert!(receipt.new_revision > request.expected_revision);
    assert_eq!(
        failure.snapshot.stop_reason,
        Some(WorkforceAlternativesStopV1::SourceChanged)
    );
    assert_eq!(
        failure.snapshot.scenario_revision,
        request.expected_revision
    );
    Box::pin(assert_saved_member(
        &app,
        &store,
        &request,
        &failure.snapshot,
        0,
    ))
    .await?;
    assert!(matches!(
        failure.failure,
        StoredSolveFailure::BeforeStart(AppError::Conflict { .. })
    ));
    for (index, member) in failure.snapshot.members.iter().enumerate().skip(1) {
        assert_eq!(
            member.status,
            if index == 1 {
                WorkforceAlternativeMemberStatusV1::Failed
            } else {
                WorkforceAlternativeMemberStatusV1::Skipped
            }
        );
        assert!(member.run_id.is_none());
        assert!(
            store
                .load_solve_run_by_request(member.request_id.ok_or("missing stable request")?)
                .await?
                .is_none()
        );
    }
    {
        let observed = observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_callback_transitions(&observed, [1, 0, 0, 0], [1, 0, 0, 0]);
        assert!(
            observed
                .iter()
                .filter_map(|snapshot| snapshot.capture_id)
                .all(|id| Some(id) == failure.snapshot.capture_id)
        );
    }
    let source_after = store.get_project(request.scenario_id).await?;
    assert_eq!(source_after.summary.revision, receipt.new_revision);
    assert_ne!(source_after.document, source_before.document);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn missing_external_source_never_falls_back_to_an_adopted_accepted_base() -> TestResult {
    let (_directory, store, app, group_request) = coordinator_fixture().await?;
    let before = store.get_project(group_request.scenario_id).await?;
    assert!(
        before
            .document
            .domain
            .entities
            .values()
            .any(|entity| entity["kind"] == "baseSchedule")
    );
    let request = StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id: group_request.scenario_id,
        expected_revision: Some(group_request.expected_revision),
        options: group_request.options.clone(),
        objective_selection: Some(StoredObjectiveSelectionV1 {
            schema_version: 1,
            profile_id: "official.workforce.objective.minimal-changes".parse()?,
            semantic_version: 1,
            operation: RunObjectiveOperation::Repair,
            baseline_source: eutheto_core::StoredBaselineSourceV1::External {
                entity_id: id(50).parse()?,
            },
            alternatives_capture_id: None,
        }),
    };
    assert!(matches!(
        &app.solve_stored(request.clone(), &mut Progress).await,
        Err(StoredSolveFailure::BeforeStart(AppError::Protocol(error)))
            if error.code == "solve.external_baseline_invalid"
    ));
    assert!(
        store
            .load_solve_run_by_request(request.request_id)
            .await?
            .is_none()
    );
    assert!(matches!(
        &app.capture_objective_alternatives_source(
            request.scenario_id,
            request.expected_revision,
            request.objective_selection.as_ref().ok_or("missing source selector")?.baseline_source,
            OperationControl::Solve(group_budget(&group_request, CancellationToken::new())?.phase_view()),
            SystemClock.now(),
        ).await,
        Err(AppError::Protocol(error)) if error.code == "solve.external_baseline_invalid"
    ));
    assert_eq!(
        store.get_project(request.scenario_id).await?.document,
        before.document
    );
    assert_eq!(
        store
            .get_project(request.scenario_id)
            .await?
            .summary
            .revision,
        before.summary.revision,
    );
    Ok(())
}

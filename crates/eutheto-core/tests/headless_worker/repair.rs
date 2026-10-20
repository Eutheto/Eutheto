use super::*;
use eutheto_core::RepairSource;
use eutheto_export::{
    ApplicationMetadata, BackupSections, ScenarioExportSnapshot, assemble_scenario_export,
};
use eutheto_types::ParentSolveBudget;
use std::collections::BTreeMap;

fn repair_request(scenario_id: ScenarioId, revision: Revision) -> TestResult<StoredSolveRequest> {
    Ok(StoredSolveRequest {
        request_id: RequestId::new(&SystemIdGenerator)?,
        scenario_id,
        expected_revision: Some(revision),
        options: options()?,
        objective_selection: None,
    })
}

async fn explicit_repair(
    app: &EuthetoApp,
    request: StoredSolveRequest,
    source: RepairSource,
) -> Result<StoredSolveOutcome, StoredSolveFailure> {
    let budget = ParentSolveBudget::new(
        request.options.time_limit_milliseconds,
        Arc::new(SystemMonotonicClock::new()),
        CancellationToken::new(),
    )
    .map_err(|_| {
        StoredSolveFailure::BeforeStart(AppError::Internal {
            incident_id: request.request_id,
        })
    })?;
    app.solve_stored_repair_with_budget(request, source, &mut Progress, budget, SystemClock.now())
        .await
}

async fn scenario_state(
    store: &SqliteScenarioStore,
    scenario_id: ScenarioId,
) -> TestResult<(Revision, ScenarioDocument, Vec<eutheto_store::HistoryEntry>)> {
    let project = store.get_project(scenario_id).await?;
    Ok((
        project.summary.revision,
        project.document,
        store.history(scenario_id).await?,
    ))
}

async fn assert_refused_without_run(
    app: &EuthetoApp,
    store: &SqliteScenarioStore,
    scenario_id: ScenarioId,
    source: RepairSource,
) -> TestResult {
    let before = scenario_state(store, scenario_id).await?;
    let request = repair_request(scenario_id, before.0)?;
    let request_id = request.request_id;
    let refusal = explicit_repair(app, request, source).await;
    assert!(
        matches!(refusal, Err(StoredSolveFailure::BeforeStart(_))),
        "repair source must be refused before durable admission: {refusal:?}"
    );
    assert!(store.load_solve_run_by_request(request_id).await?.is_none());
    assert_eq!(scenario_state(store, scenario_id).await?, before);
    Ok(())
}

fn headless_service(registry: SolverRegistry) -> TestResult<HeadlessService> {
    HeadlessService::with_solver_registry(
        Arc::new(SystemClock),
        Arc::new(SystemMonotonicClock::new()),
        Arc::new(SystemIdGenerator),
        registry,
    )
    .map_err(boxed)
}

fn operation(service: &HeadlessService) -> TestResult<eutheto_core::SolveOperation> {
    service
        .begin_solve(
            RequestId::new(&SystemIdGenerator)?,
            options()?,
            CancellationToken::new(),
        )
        .map_err(boxed)
}

fn baseline(result: &PortableAcceptedResult) -> TestResult<&eutheto_domain_ir::RunBaselineBinding> {
    result
        .run_input
        .objective_context
        .as_ref()
        .and_then(|context| context.baseline.as_deref())
        .ok_or_else(|| "explicit repair omitted its recorded baseline".into())
}

fn assert_pair_change(result: &PortableAcceptedResult, expected: i64) -> TestResult {
    let score = &result.accepted_result.verification.score;
    assert_eq!(score.feasibility, 0);
    let level = score
        .levels
        .iter()
        .find(|level| level.level_id.as_str() == "official.workforce.objective.pair_change")
        .ok_or("repair omitted authoritative pair-change score")?;
    assert_eq!(level.value, expected);
    Ok(())
}

fn repair_bundle(
    current: ScenarioSnapshotV1,
    history: Vec<ScenarioSnapshotV1>,
    result: &PortableAcceptedResult,
) -> TestResult<Vec<u8>> {
    Ok(assemble_scenario_export(
        &ScenarioExportSnapshot {
            bundle_id: id(800).parse()?,
            created_at: "2026-09-07T12:00:00Z".to_owned(),
            application: ApplicationMetadata {
                name: "Eutheto".to_owned(),
                version: "0.1.0".to_owned(),
            },
            title: "Explicit repair source regression".to_owned(),
            scenario: current,
            scenario_revisions: history,
            sections: BackupSections {
                results: BTreeMap::from([(
                    result.result_id.to_string(),
                    serde_json::to_value(result)?,
                )]),
                ..BackupSections::default()
            },
            nonsemantic_extensions: BTreeSet::new(),
            manifest_extensions: BTreeMap::new(),
        },
        &|document| {
            eutheto_workforce::portable::export_portable(document)
                .map_err(|error| eutheto_export::ExportError::InvalidModel(error.to_string()))
        },
    )?)
}

async fn replace_manual_person(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    revision: Revision,
    incumbent: PersonId,
    target: PersonId,
) -> TestResult<Revision> {
    let intent = eutheto_core::ManualEditIntentV1::Replace {
        incumbent_person_id: incumbent,
        target_person_id: target,
        shift_id: id(8).parse()?,
        protection_changes: Vec::new(),
    };
    let review = review_manual(app, scenario_id, revision, intent.clone()).await?;
    assert_eq!(
        review.disposition,
        eutheto_core::ManualEditDispositionV1::Valid
    );
    let applied = apply_reviewed_manual(
        app,
        scenario_id,
        revision,
        intent,
        review
            .approval_digest
            .ok_or("valid manual review omitted digest")?,
        false,
    )
    .await?;
    Ok(applied.new_revision)
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn explicit_stored_repair_retries_keep_recorded_manual_and_conflict_on_different_original()
-> TestResult {
    let original = comparison_snapshot()?;
    let scenario_id = original.document.scenario_id;
    let (_directory, store, a_outcome, runtime, app) = stored_real_run_with_options(
        original.clone(),
        options()?,
        Some(authored_optimize_selection()?),
    )
    .await?;
    let a_id = a_outcome.accepted_result_id().ok_or("missing A")?;
    let a = store.load_accepted_result(a_id).await?;
    let a_bytes = serde_json::to_vec(&a.portable)?;
    let a_person = selected_person(&a.portable.accepted_result)?;
    assert_eq!(a_person, id(0).parse()?);
    let AppCommandResult::ScenarioCommand(adoption) = app
        .execute(AppCommand::AdoptBaseSchedule(
            eutheto_core::SolutionAdoptBaseRequestV1 {
                schema_version: eutheto_core::SOLUTION_ADOPT_BASE_SCHEMA_VERSION,
                request_id: RequestId::new(&SystemIdGenerator)?,
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                result: AcceptedResultRefV1::from_result(&a.portable.accepted_result)?,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Explicit repair regression".to_owned(),
                },
                truncate_redo: false,
            },
        ))
        .await
        .map_err(boxed)?
    else {
        return Err("adoption omitted receipt".into());
    };
    let manual_revision = replace_manual_person(
        &app,
        scenario_id,
        adoption.new_revision,
        a_person,
        id(1).parse()?,
    )
    .await?;
    let before = scenario_state(&store, scenario_id).await?;
    let imported_b = repair_bundle(
        ScenarioSnapshotV1::current(manual_revision, before.1.clone(), BTreeSet::new()),
        vec![original],
        &a.portable,
    )?;
    let rejected_import = operation(&app.headless_service())?
        .prepare_bundle_repair(&imported_b, RepairSource::Json(a_bytes.clone()));
    let Err(rejected_import) = rejected_import else {
        return Err("exported native B gained receipt authority through a file".into());
    };
    assert!(matches!(
        &rejected_import.error,
        AppError::Protocol(error) if error.code == "solve.repair_manual_custody_unavailable"
    ));
    let request = repair_request(scenario_id, manual_revision)?;
    let first = explicit_repair(&app, request.clone(), RepairSource::ResultId(a_id))
        .await
        .map_err(boxed)?;
    assert_stored_bound_runtime(&first, &runtime)?;
    assert!(!first.reused);
    let c = store
        .load_accepted_result(first.accepted_result_id().ok_or("missing C")?)
        .await?;
    assert_eq!(
        selected_person(&c.portable.accepted_result)?,
        id(1).parse()?
    );
    assert_pair_change(&c.portable, 0)?;
    let recorded = baseline(&c.portable)?;
    assert!(matches!(
        recorded.selection,
        eutheto_domain_ir::RunBaselineSelection::ManualSelection { .. }
    ));
    assert_eq!(
        recorded.original.accepted_result,
        AcceptedResultRefV1::from_result(&a.portable.accepted_result)?
    );
    assert!(recorded.captured_original.is_none());
    assert_eq!(
        c.portable.run_input.scenario_revision,
        manual_revision.value()
    );
    assert_eq!(scenario_state(&store, scenario_id).await?, before);
    let recorded_input = store.load_solve_input(first.state().input.run_id).await?;
    let from_file = explicit_repair(
        &app,
        repair_request(scenario_id, manual_revision)?,
        RepairSource::Json(a_bytes.clone()),
    )
    .await
    .map_err(boxed)?;
    let file_c = store
        .load_accepted_result(from_file.accepted_result_id().ok_or("missing file C")?)
        .await?;
    assert_eq!(baseline(&file_c.portable)?, recorded);
    assert_pair_change(&file_c.portable, 0)?;
    assert_eq!(scenario_state(&store, scenario_id).await?, before);

    // A later native B must not replace the B generation already recorded for this request.
    let later_revision =
        replace_manual_person(&app, scenario_id, manual_revision, id(1).parse()?, a_person).await?;
    let later = scenario_state(&store, scenario_id).await?;
    for source in [
        RepairSource::ResultId(a_id),
        RepairSource::Json(serde_json::to_vec_pretty(&a.portable)?),
    ] {
        let retry = explicit_repair(&app, request.clone(), source)
            .await
            .map_err(boxed)?;
        assert!(retry.reused);
        assert!(retry.report.is_none());
        assert_eq!(retry.state(), first.state());
        assert_eq!(
            store.load_solve_input(first.state().input.run_id).await?,
            recorded_input
        );
        assert_eq!(scenario_state(&store, scenario_id).await?, later);
    }
    let other = solve_real_again(
        &app,
        scenario_id,
        later_revision,
        options()?,
        authored_optimize_selection()?,
    )
    .await?;
    let other_id = other.accepted_result_id().ok_or("missing different A")?;
    assert_ne!(other_id, a_id);
    let other_portable = store.load_accepted_result(other_id).await?.portable;
    for source in [
        RepairSource::ResultId(other_id),
        RepairSource::Json(serde_json::to_vec(&other_portable)?),
    ] {
        assert!(matches!(
            explicit_repair(&app, request.clone(), source).await,
            Err(StoredSolveFailure::BeforeStart(AppError::Validation(_)))
        ));
        assert_eq!(
            store.load_solve_input(first.state().input.run_id).await?,
            recorded_input
        );
    }
    // A new request also cannot silently throw away the incompatible admitted manual ancestry.
    assert_refused_without_run(&app, &store, scenario_id, RepairSource::ResultId(other_id)).await?;
    let changed =
        reseal_untrusted_source_metadata(&a, UntrustedMetadataControl::UnknownCompilerVersion)?;
    assert!(matches!(
        explicit_repair(
            &app,
            request.clone(),
            RepairSource::Json(serde_json::to_vec(&changed)?)
        )
        .await,
        Err(StoredSolveFailure::BeforeStart(_))
    ));
    assert_eq!(
        store.load_solve_input(first.state().input.run_id).await?,
        recorded_input
    );
    assert_eq!(scenario_state(&store, scenario_id).await?, later);
    assert_eq!(
        serde_json::to_vec(&store.load_accepted_result(a_id).await?.portable)?,
        a_bytes
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn explicit_stored_repair_native_id_and_file_have_identical_baselines_without_adoption()
-> TestResult {
    let original = comparison_snapshot()?;
    let scenario_id = original.document.scenario_id;
    let (_directory, store, outcome, runtime, app) =
        stored_real_run_with_options(original, options()?, Some(authored_optimize_selection()?))
            .await?;
    let a_id = outcome.accepted_result_id().ok_or("missing A")?;
    let a = store.load_accepted_result(a_id).await?;
    let a_bytes = serde_json::to_vec(&a.portable)?;
    let mut person = store
        .get_project(scenario_id)
        .await?
        .document
        .domain
        .entities[&id(0).parse()?]
        .clone();
    person["eligibleAssignmentTypeIds"] = json!([]);
    let changed = edit_manual_source(
        &app,
        scenario_id,
        Revision::INITIAL,
        eutheto_workforce::commands::UPDATE_ENTITY,
        json!({"entity":person}),
    )
    .await?;
    let before = scenario_state(&store, scenario_id).await?;
    let mut results = Vec::new();
    for source in [
        RepairSource::ResultId(a_id),
        RepairSource::Json(a_bytes.clone()),
    ] {
        let repair = explicit_repair(
            &app,
            repair_request(scenario_id, changed.new_revision)?,
            source,
        )
        .await
        .map_err(boxed)?;
        assert_stored_bound_runtime(&repair, &runtime)?;
        let result = store
            .load_accepted_result(repair.accepted_result_id().ok_or("missing C")?)
            .await?
            .portable;
        assert_eq!(selected_person(&result.accepted_result)?, id(1).parse()?);
        assert_pair_change(&result, 2)?;
        let context = result
            .run_input
            .objective_context
            .as_ref()
            .ok_or("missing context")?;
        assert_eq!(context.schema_version, 2);
        assert_eq!(context.operation, RunObjectiveOperation::Repair);
        assert!(baseline(&result)?.captured_original.is_some());
        let current = ScenarioSnapshotV1::current(before.0, before.1.clone(), BTreeSet::new());
        app.headless_service()
            .verify_result(
                &current,
                &result,
                &OperationControl::Cancellation(CancellationToken::new()),
            )
            .map_err(boxed)?;
        results.push(result);
        assert_eq!(scenario_state(&store, scenario_id).await?, before);
    }
    assert_eq!(baseline(&results[0])?, baseline(&results[1])?);
    assert_eq!(
        results[0].accepted_result.verification.score,
        results[1].accepted_result.verification.score
    );
    assert_eq!(
        results[0].run_input.model_hash,
        results[1].run_input.model_hash
    );
    assert_eq!(
        serde_json::to_vec(&store.load_accepted_result(a_id).await?.portable)?,
        a_bytes
    );
    assert!(!before.1.domain.entities.values().any(|entity| matches!(
        entity["kind"].as_str(),
        Some("baseSchedule" | "manualSelection")
    )));
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn explicit_stored_repair_refuses_missing_foreign_and_conflicting_native_sources()
-> TestResult {
    let original = comparison_snapshot()?;
    let scenario_id = original.document.scenario_id;
    let (_directory, store, outcome, _runtime, app) = stored_real_run_with_options(
        original.clone(),
        options()?,
        Some(authored_optimize_selection()?),
    )
    .await?;
    let a_id = outcome.accepted_result_id().ok_or("missing A")?;
    let a = store.load_accepted_result(a_id).await?;
    let before_results = store.list_accepted_results(scenario_id).await?;
    assert_refused_without_run(
        &app,
        &store,
        scenario_id,
        RepairSource::ResultId(SolutionId::new(&SystemIdGenerator)?),
    )
    .await?;
    let foreign_id = match app
        .execute(AppCommand::DuplicateProject {
            request_id: RequestId::new(&SystemIdGenerator)?,
            source_id: scenario_id,
            expected_revision: Revision::INITIAL,
            title: "Foreign repair source".to_owned(),
        })
        .await
        .map_err(boxed)?
    {
        AppCommandResult::Project(project) => project.scenario_id,
        other => return Err(format!("unexpected duplicate: {other:?}").into()),
    };
    for source in [
        RepairSource::ResultId(a_id),
        RepairSource::Json(serde_json::to_vec(&a.portable)?),
    ] {
        assert_refused_without_run(&app, &store, foreign_id, source).await?;
    }
    let conflicting =
        reseal_untrusted_source_metadata(&a, UntrustedMetadataControl::UnknownCompilerVersion)?;
    assert_refused_without_run(
        &app,
        &store,
        scenario_id,
        RepairSource::Json(serde_json::to_vec(&conflicting)?),
    )
    .await?;
    assert_eq!(
        store.list_accepted_results(scenario_id).await?,
        before_results
    );
    assert_eq!(store.load_accepted_result(a_id).await?.portable, a.portable);
    Ok(())
}

pub(super) async fn assert_quarantined_checkpoint_repair_refused(
    app: &EuthetoApp,
    store: &SqliteScenarioStore,
    checkpoint: &VerifiedCheckpoint,
    started_at: Rfc3339Timestamp,
) -> TestResult {
    // A checksum-valid portable claim about the earlier feasible checkpoint cannot
    // override the actual native alarm, even when durable quarantine rolled back.
    let accepted = &checkpoint.accepted_result;
    let manifest = RunManifest::new(
        checkpoint.run_input.run_id,
        checkpoint.run_input.checksum.clone(),
        RunTerminalOutcomeV1::Accepted {
            status: SolveStatus::Feasible,
            solution_id: accepted.solution.solution_id,
            accepted_result_checksum: accepted.checksum.clone(),
            verification_checksum: accepted.verification.checksum.clone(),
        },
        started_at,
        checkpoint.accepted_at,
        Some(checkpoint.verified_after_milliseconds),
        Some(checkpoint.observed_after_milliseconds),
        Some(checkpoint.verified_after_milliseconds),
        eutheto_domain_ir::RunPhaseTimingsV1::default(),
        Vec::new(),
    )?;
    let portable = PortableAcceptedResult::new(
        checkpoint.run_input.clone(),
        manifest,
        accepted.clone(),
        checkpoint.evidence.clone(),
    )?;
    let scenario_id = portable.scenario_id;
    let before = scenario_state(store, scenario_id).await?;
    app.headless_service()
        .verify_result(
            &ScenarioSnapshotV1::current(before.0, before.1.clone(), BTreeSet::new()),
            &portable,
            &OperationControl::Cancellation(CancellationToken::new()),
        )
        .map_err(boxed)?;
    for source in [
        RepairSource::ResultId(portable.result_id),
        RepairSource::Json(serde_json::to_vec(&portable)?),
    ] {
        let request = repair_request(scenario_id, before.0)?;
        let request_id = request.request_id;
        let Err(StoredSolveFailure::BeforeStart(AppError::Verification(failure))) =
            explicit_repair(app, request, source).await
        else {
            return Err("repair lost the original native correctness alarm".into());
        };
        let known = failure
            .known_alarm
            .ok_or("repair omitted known native alarm")?;
        assert_eq!(known.run_id(), checkpoint.run_input.run_id);
        assert_eq!(known.input_checksum(), checkpoint.run_input.checksum);
        assert_eq!(
            known.terminalization(),
            eutheto_types::VerificationAlarmTerminalization::Unconfirmed
        );
        assert!(store.load_solve_run_by_request(request_id).await?.is_none());
        assert_eq!(scenario_state(store, scenario_id).await?, before);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
async fn explicit_stored_repair_external_original_is_verified_without_native_source_installation()
-> TestResult {
    let original = comparison_snapshot()?;
    let scenario_id = original.document.scenario_id;
    let (_directory, store, native, _runtime, app) = stored_real_run_with_options(
        original.clone(),
        options()?,
        Some(authored_optimize_selection()?),
    )
    .await?;
    let (registry, _) = real_registry().await?;
    let service = headless_service(registry)?;
    let external = operation(&service)?
        .prepare(original.clone())
        .map_err(boxed)?
        .execute(&mut Progress)
        .await
        .map_err(boxed)?
        .into_portable()
        .map_err(boxed)?
        .ok_or("missing external A")?;
    assert_ne!(
        external.result_id,
        native.accepted_result_id().ok_or("missing native A")?
    );
    let external_bytes = serde_json::to_vec(&external)?;
    let before = scenario_state(&store, scenario_id).await?;
    let result = explicit_repair(
        &app,
        repair_request(scenario_id, Revision::INITIAL)?,
        RepairSource::Json(external_bytes.clone()),
    )
    .await
    .map_err(boxed)?;
    let c = store
        .load_accepted_result(result.accepted_result_id().ok_or("missing C")?)
        .await?;
    assert_eq!(
        baseline(&c.portable)?.original.accepted_result,
        AcceptedResultRefV1::from_result(&external.accepted_result)?
    );
    assert_pair_change(&c.portable, 0)?;
    assert!(
        store
            .load_accepted_result(external.result_id)
            .await
            .is_err()
    );
    assert!(
        store
            .load_solve_run_by_request(external.run_input.request_id)
            .await?
            .is_none()
    );
    assert_eq!(store.list_accepted_results(scenario_id).await?.len(), 2);
    assert_eq!(scenario_state(&store, scenario_id).await?, before);
    assert_eq!(serde_json::to_vec(&external)?, external_bytes);

    // A new result identity cannot turn an already-known native run identity into an import.
    let native_source = store
        .load_accepted_result(native.accepted_result_id().ok_or("missing native A")?)
        .await?
        .portable;
    let input = &native_source.run_input;
    let manifest = &native_source.run_manifest;
    let collision_manifest = RunManifest::new(
        input.run_id,
        input.checksum.clone(),
        external.run_manifest.outcome.clone(),
        manifest.started_at,
        manifest.finished_at,
        manifest.elapsed_milliseconds,
        manifest.first_incumbent_milliseconds,
        manifest.first_verified_feasible_milliseconds,
        manifest.phase_timings,
        manifest.verification_warnings.clone(),
    )?
    .with_native_observations(manifest.native_observations.clone())?;
    let collision = PortableAcceptedResult::new(
        input.clone(),
        collision_manifest,
        external.accepted_result.clone(),
        external.evidence.clone(),
    )?;
    service
        .verify_result(
            &original,
            &collision,
            &OperationControl::Cancellation(CancellationToken::new()),
        )
        .map_err(boxed)?;
    assert_refused_without_run(
        &app,
        &store,
        scenario_id,
        RepairSource::Json(serde_json::to_vec(&collision)?),
    )
    .await?;
    assert_eq!(store.list_accepted_results(scenario_id).await?.len(), 2);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn explicit_file_repair_requires_exact_historical_original_and_bundle_id_file_equivalence()
-> TestResult {
    let original = comparison_snapshot()?;
    let scenario_id = original.document.scenario_id;
    let (_directory, store, outcome, _runtime, app) = stored_real_run_with_options(
        original.clone(),
        options()?,
        Some(authored_optimize_selection()?),
    )
    .await?;
    let a = store
        .load_accepted_result(outcome.accepted_result_id().ok_or("missing A")?)
        .await?
        .portable;
    let a_bytes = serde_json::to_vec(&a)?;
    let mut person = original.document.domain.entities[&id(0).parse()?].clone();
    person["eligibleAssignmentTypeIds"] = json!([]);
    let change = edit_manual_source(
        &app,
        scenario_id,
        Revision::INITIAL,
        eutheto_workforce::commands::UPDATE_ENTITY,
        json!({"entity":person}),
    )
    .await?;
    let before = scenario_state(&store, scenario_id).await?;
    let current =
        ScenarioSnapshotV1::current(change.new_revision, before.1.clone(), BTreeSet::new());
    let input_bytes = serde_json::to_vec(&current)?;
    let (registry, _) = real_registry().await?;
    let service = headless_service(registry)?;
    let unchanged = operation(&service)?
        .prepare_repair(original.clone(), RepairSource::Json(a_bytes.clone()))
        .map_err(boxed)?
        .execute(&mut Progress)
        .await
        .map_err(boxed)?
        .into_portable()
        .map_err(boxed)?
        .ok_or("missing unchanged file repair")?;
    assert_pair_change(&unchanged, 0)?;
    service
        .verify_result(
            &original,
            &unchanged,
            &OperationControl::Cancellation(CancellationToken::new()),
        )
        .map_err(boxed)?;
    let missing_history =
        operation(&service)?.prepare_repair(current.clone(), RepairSource::Json(a_bytes.clone()));
    let Err(missing_history) = missing_history else {
        return Err("changed standalone input inferred missing original".into());
    };
    assert!(matches!(
        &missing_history.error,
        AppError::Protocol(error) if error.code == "solve.repair_original_revision_required"
    ));
    let mut wrong_original = original.clone();
    wrong_original
        .document
        .domain
        .entities
        .get_mut(&id(1).parse()?)
        .ok_or("missing person")?["name"] = json!("not the recorded original");
    assert!(
        operation(&service)?
            .prepare_repair(wrong_original, RepairSource::Json(a_bytes.clone()))
            .is_err()
    );
    let mut foreign = current.clone();
    foreign.document.scenario_id = ScenarioId::new(&SystemIdGenerator)?;
    assert!(
        operation(&service)?
            .prepare_repair(foreign, RepairSource::Json(a_bytes.clone()))
            .is_err()
    );
    assert!(
        operation(&service)?
            .prepare_repair(original.clone(), RepairSource::ResultId(a.result_id))
            .is_err()
    );
    let bundle = repair_bundle(current.clone(), vec![original], &a)?;
    let bundle_bytes = bundle.clone();
    assert!(
        operation(&service)?
            .prepare_bundle_repair(
                &bundle,
                RepairSource::ResultId(SolutionId::new(&SystemIdGenerator)?)
            )
            .is_err()
    );
    let mut results = Vec::new();
    for source in [
        RepairSource::ResultId(a.result_id),
        RepairSource::Json(a_bytes.clone()),
    ] {
        let repaired = operation(&service)?
            .prepare_bundle_repair(&bundle, source)
            .map_err(boxed)?
            .execute(&mut Progress)
            .await
            .map_err(boxed)?
            .into_portable()
            .map_err(boxed)?
            .ok_or("missing file C")?;
        service
            .verify_result(
                &current,
                &repaired,
                &OperationControl::Cancellation(CancellationToken::new()),
            )
            .map_err(boxed)?;
        assert_eq!(selected_person(&repaired.accepted_result)?, id(1).parse()?);
        assert_pair_change(&repaired, 2)?;
        assert_eq!(
            repaired.run_input.scenario_revision,
            change.new_revision.value()
        );
        assert!(baseline(&repaired)?.captured_original.is_some());
        results.push(repaired);
    }
    assert_eq!(baseline(&results[0])?, baseline(&results[1])?);
    assert_eq!(
        results[0].run_input.model_hash,
        results[1].run_input.model_hash
    );
    assert_eq!(
        results[0].accepted_result.verification.score,
        results[1].accepted_result.verification.score
    );
    assert_eq!(serde_json::to_vec(&current)?, input_bytes);
    assert_eq!(bundle, bundle_bytes);
    assert_eq!(
        serde_json::to_vec(&store.load_accepted_result(a.result_id).await?.portable)?,
        a_bytes
    );
    assert_eq!(scenario_state(&store, scenario_id).await?, before);
    assert_eq!(store.list_accepted_results(scenario_id).await?.len(), 1);
    Ok(())
}

#[test]
fn explicit_file_repair_refuses_checksum_valid_imported_manual_without_native_custody() -> TestResult
{
    let service = HeadlessService::new(
        Arc::new(SystemClock),
        Arc::new(SystemMonotonicClock::new()),
        Arc::new(SystemIdGenerator),
    )
    .map_err(boxed)?;
    let mut input = comparison_snapshot()?;
    let manual = attach_unadmitted_manual_selection(&mut input.document)?;
    assert_eq!(
        manual.checksum,
        eutheto_workforce::assignment_rules::workforce_manual_selection_checksum(&manual, None)?
    );
    let before = serde_json::to_vec(&input)?;
    let refused = operation(&service)?.prepare_repair(
        input.clone(),
        RepairSource::ResultId(manual.original.accepted_result.solution_id),
    );
    let Err(refused) = refused else {
        return Err("public manual checksum granted native receipt authority".into());
    };
    assert!(matches!(
        &refused.error,
        AppError::Protocol(error) if error.code == "solve.repair_manual_custody_unavailable"
    ));
    assert_eq!(serde_json::to_vec(&input)?, before);
    Ok(())
}

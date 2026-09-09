use std::collections::BTreeMap;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use eutheto_core::{
    HeadlessCompilationReport, HeadlessService, HeadlessSolveOutcome, SolvePreparationFailure,
};
use eutheto_domain_api::{ContractJsonLimits, ShareResultOptions, ValidatedContractSchema};
use eutheto_domain_ir::{
    AcceptedResult, AssignmentValue, PortableAcceptedResultV2, RunInputV1, RunPhaseTimingsV1,
    RunTerminalOutcomeV1, VerificationContextV1, VerificationReport,
};
use eutheto_planning_ir::{PlanningIrLimitsV1, ProvenanceParameter};
use eutheto_solver_api::{BackendTerminationReason, ProgressSink, SolveProgressEvent};
use eutheto_solver_ortools::{VerifiedWorkerArtifact, registry_with_ortools};
use eutheto_types::{
    BackendSelection, CancellationToken, DurationMillis, ExplanationMode, PreservationPolicy,
    ReproducibilityMode, RequestId, ResourceLimits, ScenarioSnapshotV1, SolveOptions, SolveStatus,
    SystemClock, SystemIdGenerator, SystemMonotonicClock, WorkerThreadPolicy,
};
use eutheto_workforce::assignment_rules::build_workforce_share_result;
#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::io::AsyncReadExt;

use crate::contract::{
    AcceptanceMeasurements, BackendMeasurements, BenchmarkEvidence, BenchmarkSample,
    CORPUS_VERSION, CaseId, EVIDENCE_FORMAT, ExpectedDisposition, FORMAT_VERSION, LoadedCorpus,
    LoadedFixture, MAX_EVIDENCE_BYTES, MeasurementProfile, ModelMeasurements, RunnerIdentity,
    RunnerProcessState, SampleBatch, TimingAggregate, TimingMeasurements,
};
use crate::generation::{app, error_code};
use crate::{files, generation};

const EXECUTABLE_LIMIT: u64 = 512 * 1024 * 1024;
const RANK_LEVEL: &str = "official.workforce.objective.assignment.rank";
const SAMPLE_GUARD: Duration = Duration::from_mins(6);
const CLEANUP_GRACE: Duration = Duration::from_secs(10);

fn check_cancellation(cancellation: &CancellationToken) -> Result<()> {
    ensure!(
        !cancellation.is_cancelled(),
        "headless corpus operation cancelled"
    );
    Ok(())
}

struct DiscardProgress;
impl ProgressSink for DiscardProgress {
    fn emit(&mut self, _: SolveProgressEvent) -> Result<(), eutheto_solver_api::OutputError> {
        Ok(())
    }
}

fn options() -> Result<SolveOptions> {
    let profile = MeasurementProfile::REVIEWED;
    let collection = u32::try_from(eutheto_export::PORTABLE_LIMITS.max_collection_items)?;
    Ok(SolveOptions {
        backend: BackendSelection::Auto,
        mode: profile.mode,
        time_limit_milliseconds: DurationMillis::new(profile.budget_milliseconds)?,
        memory_limit_bytes: None,
        worker_threads: WorkerThreadPolicy::Exact(profile.worker_threads),
        random_seed: profile.random_seed,
        solution_limit: None,
        stop_after_first_feasible: profile.stop_after_first_feasible,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::None,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: collection,
            max_rules: collection,
            max_variables: PlanningIrLimitsV1::DEFAULT.max_variables,
            max_constraints: PlanningIrLimitsV1::DEFAULT.max_constraints,
        },
    })
}

pub(crate) async fn sample(
    root: &Path,
    artifact_root: &Path,
    manifest_sha256: &str,
    id: CaseId,
    post_warmup: bool,
    output: &Path,
    cancellation: &CancellationToken,
) -> Result<()> {
    check_cancellation(cancellation)?;
    let corpus = generation::load(root)?;
    let fixture = corpus
        .fixtures
        .iter()
        .find(|case| case.manifest.id == id)
        .context("missing measured case")?;
    let artifact =
        VerifiedWorkerArtifact::verify(artifact_root, files::digest(manifest_sha256)?).await?;
    let service = app(HeadlessService::with_solver_registry(
        Arc::new(SystemClock),
        Arc::new(SystemMonotonicClock::new()),
        Arc::new(SystemIdGenerator),
        registry_with_ortools(artifact)?,
    ))?;
    let state = if post_warmup {
        RunnerProcessState::PostWarmup
    } else {
        RunnerProcessState::FirstOperation
    };
    let profile = corpus.manifest.profile;
    if post_warmup {
        for _ in 0..profile.warmup_runs {
            Box::pin(measure(root, &service, fixture, state, 0, cancellation)).await?;
        }
    }
    let count = if post_warmup {
        profile.post_warmup_samples
    } else {
        profile.first_operation_samples
    };
    let mut samples = Vec::with_capacity(count.into());
    for ordinal in 0..count {
        samples.push(
            Box::pin(measure(
                root,
                &service,
                fixture,
                state,
                ordinal,
                cancellation,
            ))
            .await?,
        );
    }
    let batch = SampleBatch {
        schema_version: FORMAT_VERSION,
        corpus_sha256: corpus.manifest_sha256,
        worker_manifest_sha256: manifest_sha256.to_owned(),
        runner_sha256: files::hash_file(&std::env::current_exe()?, EXECUTABLE_LIMIT)?,
        samples,
    };
    check_cancellation(cancellation)?;
    publish(output, &batch)
}

async fn measure(
    root: &Path,
    service: &HeadlessService,
    fixture: &LoadedFixture,
    state: RunnerProcessState,
    ordinal: u16,
    cancellation: &CancellationToken,
) -> Result<BenchmarkSample> {
    check_cancellation(cancellation)?;
    let attempt_started = Instant::now();
    let operation = app(service.begin_solve(
        RequestId::new(&SystemIdGenerator)?,
        options()?,
        cancellation.clone(),
    ))?;
    // Preflight is not acquisition for this solve: re-read and hash under its one original budget.
    let control = operation.control();
    let path = files::contained(root, &fixture.manifest.input.path)?;
    let bytes = files::read_controlled(
        &path,
        eutheto_export::PORTABLE_LIMITS.max_json_bytes,
        &control,
    )?;
    ensure!(
        files::sha256(&bytes) == fixture.manifest.input.sha256
            && u64::try_from(bytes.len())? == fixture.manifest.input.bytes,
        "measured fixture changed since corpus validation"
    );
    let snapshot = app(service.decode_scenario(&bytes, &control))?.scenario;
    let prepared = match operation.prepare(snapshot) {
        Ok(prepared) => prepared,
        Err(failure) => {
            return preparation_failure(fixture, state, ordinal, &failure, attempt_started);
        }
    };
    ensure!(
        matches!(
            fixture.expected.disposition,
            ExpectedDisposition::Accepted | ExpectedDisposition::Infeasible
        ),
        "preservation/pressure unexpectedly compiled"
    );
    let model = model_measurements(fixture, prepared.compilation())?;
    let outcome = app(prepared.execute(&mut DiscardProgress).await)?;
    measured_outcome(service, fixture, state, ordinal, model, outcome)
}

fn measured_outcome(
    service: &HeadlessService,
    fixture: &LoadedFixture,
    state: RunnerProcessState,
    ordinal: u16,
    model: ModelMeasurements,
    outcome: HeadlessSolveOutcome,
) -> Result<BenchmarkSample> {
    ensure!(
        outcome.correctness_alarm().is_none(),
        "independent verification alarm"
    );
    ensure!(
        outcome.execution.invocation_count == 1 && outcome.execution.attempts.len() == 1,
        "expected one manifest-bound backend invocation"
    );
    let backend = backend_measurements(&outcome)?;
    let provisional_manifest = outcome
        .accepted_result()
        .is_none()
        .then(|| outcome.manifest().clone());
    // Materialization rechecks admission; a prior candidate is never promoted after failure here.
    let portable = app(outcome.into_portable())?;
    let manifest = match portable.as_ref() {
        Some(portable) => &portable.run_manifest,
        None => provisional_manifest
            .as_ref()
            .context("final admission lost its manifest")?,
    };
    let (RunTerminalOutcomeV1::Accepted { status, .. } | RunTerminalOutcomeV1::NoResult { status }) =
        manifest.outcome
    else {
        anyhow::bail!("unexpected nonterminal/alarmed run manifest");
    };
    let acceptance = match (fixture.expected.disposition, portable.as_ref()) {
        (ExpectedDisposition::Accepted, Some(portable)) => {
            ensure!(
                matches!(status, SolveStatus::Optimal | SolveStatus::Feasible),
                "accepted result has nonaccepted status"
            );
            Some(check_accepted(
                service,
                fixture,
                &fixture.snapshot,
                portable,
            )?)
        }
        (ExpectedDisposition::Infeasible, None) => {
            ensure!(
                status == SolveStatus::Infeasible && backend.termination == "infeasibilityClaimed",
                "contradiction did not reach actual backend infeasibility"
            );
            None
        }
        _ => anyhow::bail!("unexpected final acceptance disposition"),
    };
    let timings = phase_measurements(manifest, acceptance.is_some())?;
    Ok(BenchmarkSample {
        case_id: fixture.manifest.id,
        runner_process_state: state,
        ordinal,
        status,
        preparation_error_code: None,
        model: Some(model),
        timings,
        backend: Some(backend),
        acceptance,
    })
}

fn model_measurements(
    fixture: &LoadedFixture,
    report: &HeadlessCompilationReport,
) -> Result<ModelMeasurements> {
    let envelope = fixture
        .expected
        .model_envelope
        .context("executable case lacks model bounds")?;
    ensure!(
        (u64::from(envelope.minimum_variables)..=u64::from(envelope.maximum_variables))
            .contains(&report.summary.variable_count)
            && report.summary.constraint_count <= u64::from(envelope.maximum_constraints),
        "compiled model exceeded reviewed envelope"
    );
    let mut counts = BTreeMap::new();
    for (key, value) in &report.compile_metadata {
        let ProvenanceParameter::Integer(value) = value else {
            anyhow::bail!("unexpected nonnumeric compile metadata");
        };
        counts.insert(key.as_str().to_owned(), u64::try_from(*value)?);
    }
    ensure!(
        counts.len() == 7,
        "required compiler filtering stages unavailable"
    );
    Ok(ModelMeasurements {
        variables: report.summary.variable_count,
        constraints: report.summary.constraint_count,
        canonical_ir_blake3: report.summary.canonical_ir_hash.clone(),
        domain_filter_counts: counts,
    })
}

fn preparation_failure(
    fixture: &LoadedFixture,
    state: RunnerProcessState,
    ordinal: u16,
    failure: &SolvePreparationFailure,
    started: Instant,
) -> Result<BenchmarkSample> {
    let code = error_code(&failure.error);
    match fixture.expected.disposition {
        ExpectedDisposition::CompileRejected => ensure!(
            failure.status == SolveStatus::InvalidModel && code == "solve.compilation_failed",
            "full document did not preserve its unsupported boundary"
        ),
        ExpectedDisposition::ResourceLimit => ensure!(
            failure.status == SolveStatus::NoSolutionWithinLimit
                && code == "operation.resource_limit",
            "pressure did not preserve typed resource refusal"
        ),
        _ => anyhow::bail!(
            "required executable case failed preparation: {}: {code}",
            fixture.manifest.id.slug()
        ),
    }
    let mut timings = TimingMeasurements::default();
    timings
        .milliseconds
        .insert("preparationAttempt".to_owned(), elapsed(started)?);
    for metric in [
        "compile",
        "backend",
        "projection",
        "structuralValidation",
        "scoreRecomputation",
        "requiredRuleVerification",
        "independentAcceptance",
        "parentTotal",
        "firstIncumbent",
        "firstVerifiedFeasible",
        "evidenceMaterialization",
    ] {
        timings
            .unavailable
            .insert(metric.to_owned(), "preparationDidNotComplete".to_owned());
    }
    Ok(BenchmarkSample {
        case_id: fixture.manifest.id,
        runner_process_state: state,
        ordinal,
        status: failure.status,
        preparation_error_code: Some(code.to_owned()),
        model: None,
        timings,
        backend: None,
        acceptance: None,
    })
}

fn elapsed(start: Instant) -> Result<u64> {
    Ok(u64::try_from(start.elapsed().as_millis())?)
}

fn timing(
    measurements: &mut TimingMeasurements,
    name: &str,
    value: Option<DurationMillis>,
    reason: &str,
) {
    if let Some(value) = value {
        measurements
            .milliseconds
            .insert(name.to_owned(), value.value());
    } else {
        measurements
            .unavailable
            .insert(name.to_owned(), reason.to_owned());
    }
}

fn phase_measurements(
    manifest: &eutheto_domain_ir::RunManifestV1,
    accepted: bool,
) -> Result<TimingMeasurements> {
    let mut result = TimingMeasurements::default();
    let phase = manifest.phase_timings;
    for (name, value) in [
        ("compile", phase.compile_milliseconds),
        ("backend", phase.backend_milliseconds),
        ("projection", phase.projection_milliseconds),
        (
            "structuralValidation",
            phase.structural_validation_milliseconds,
        ),
        ("scoreRecomputation", phase.score_recomputation_milliseconds),
        (
            "requiredRuleVerification",
            phase.required_rule_verification_milliseconds,
        ),
        (
            "evidenceMaterialization",
            phase.evidence_persistence_milliseconds,
        ),
        ("parentTotal", manifest.elapsed_milliseconds),
        ("firstIncumbent", manifest.first_incumbent_milliseconds),
        (
            "firstVerifiedFeasible",
            manifest.first_verified_feasible_milliseconds,
        ),
    ] {
        timing(
            &mut result,
            name,
            value,
            if accepted {
                "coreDidNotExposeStage"
            } else {
                "noAcceptedCandidate"
            },
        );
    }
    if let Some(total) = acceptance_duration(phase)? {
        result
            .milliseconds
            .insert("independentAcceptance".to_owned(), total);
    } else {
        result.unavailable.insert(
            "independentAcceptance".to_owned(),
            "noCompleteAcceptanceStages".to_owned(),
        );
    }
    if accepted {
        for required in [
            "compile",
            "backend",
            "projection",
            "structuralValidation",
            "scoreRecomputation",
            "requiredRuleVerification",
            "independentAcceptance",
            "parentTotal",
            "firstIncumbent",
            "firstVerifiedFeasible",
            "evidenceMaterialization",
        ] {
            ensure!(
                result.milliseconds.contains_key(required),
                "accepted sample lacks required timing stage: {required}"
            );
        }
        ensure!(
            result.milliseconds["firstIncumbent"] <= result.milliseconds["firstVerifiedFeasible"]
                && result.milliseconds["firstVerifiedFeasible"]
                    <= result.milliseconds["parentTotal"],
            "acceptance milestones are inconsistent"
        );
    }
    Ok(result)
}

fn acceptance_duration(phase: RunPhaseTimingsV1) -> Result<Option<u64>> {
    let mut total = 0_u64;
    for value in [
        phase.projection_milliseconds,
        phase.structural_validation_milliseconds,
        phase.score_recomputation_milliseconds,
        phase.required_rule_verification_milliseconds,
    ] {
        let Some(value) = value else {
            return Ok(None);
        };
        total = total
            .checked_add(value.value())
            .context("acceptance duration overflow")?;
    }
    Ok(Some(total))
}

fn backend_measurements(outcome: &HeadlessSolveOutcome) -> Result<BackendMeasurements> {
    let attempt = &outcome.execution.attempts[0];
    let backend = attempt.outcome.as_ref().with_context(|| {
        format!(
            "backend terminal evidence unavailable: {:?}, {:?}, code {:?}",
            outcome.execution.terminal_status, attempt.termination, attempt.backend_failure_code,
        )
    })?;
    let execution = backend
        .evidence
        .execution
        .as_ref()
        .context("worker execution evidence unavailable")?;
    let reproducibility = &execution.reproducibility;
    let profile = MeasurementProfile::REVIEWED;
    ensure!(
        reproducibility.applied_parameters.worker_threads == u32::from(profile.worker_threads)
            && i64::from(reproducibility.applied_parameters.random_seed)
                == i64::try_from(profile.random_seed)?
            && reproducibility.applied_parameters.log_search_progress
                == (outcome.compilation.summary.objective_level_count > 0)
            && reproducibility
                .applied_parameters
                .deterministic_test_profile,
        "worker did not apply deterministic profile"
    );
    ensure!(
        reproducibility.applied_parameters.stop_after_first_feasible
            == profile.stop_after_first_feasible,
        "worker stopping policy differs from measured profile"
    );
    ensure!(
        backend.model_hash == outcome.run_input().model_hash,
        "backend model binding mismatch"
    );
    let mut timings = TimingMeasurements::default();
    let observed = &execution.timings;
    for (name, value) in [
        (
            "translationSerialization",
            Some(observed.translation_serialization_milliseconds),
        ),
        ("workerStartup", observed.worker_startup_milliseconds),
        ("handshake", observed.handshake_milliseconds),
        ("solver", observed.solver_milliseconds),
        ("protocolDecode", observed.protocol_decode_milliseconds),
    ] {
        timing(&mut timings, name, value, "backendDidNotExposeStage");
    }
    let versions = backend_versions(outcome.run_input());
    let termination = serde_json::to_value(backend.termination)?
        .as_str()
        .context("backend termination was not a string")?
        .to_owned();
    Ok(BackendMeasurements {
        backend_id: backend.backend_id.to_string(),
        termination,
        versions,
        timings,
        remaining_at_dispatch_milliseconds: backend
            .evidence
            .remaining_at_dispatch_milliseconds
            .value(),
        backend_limit_milliseconds: backend.evidence.backend_limit_milliseconds.value(),
        translated_variables: execution.model_counts.translated_variable_count,
        translated_constraints: execution.model_counts.translated_constraint_count,
        applied_parameters_sha256: reproducibility
            .applied_parameters_sha256
            .clone()
            .context("worker parameters hash unavailable")?,
        model_fingerprint_sha256: reproducibility.model_fingerprint_sha256.clone(),
        backend_objective_values: backend
            .evidence
            .objective
            .as_ref()
            .map(|objective| objective.objective_values.clone()),
        backend_best_bound_values: backend
            .evidence
            .objective
            .as_ref()
            .and_then(|objective| objective.best_bound_values.clone()),
        // Headless currently returns no original-domain conflict mapping, matching the CLI contract.
        infeasibility_core: (backend.termination == BackendTerminationReason::InfeasibilityClaimed)
            .then(|| "unavailable:conflictNotReturned".to_owned()),
    })
}

fn backend_versions(input: &RunInputV1) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("backend".to_owned(), input.backend_version.clone()),
        ("adapter".to_owned(), input.adapter_version.clone()),
        ("worker".to_owned(), input.worker_version.clone()),
        ("solver".to_owned(), input.solver_version.clone()),
        ("compiler".to_owned(), input.compiler_version.clone()),
        ("application".to_owned(), input.application_version.clone()),
        ("protocolMajor".to_owned(), input.protocol_major.to_string()),
        ("protocolMinor".to_owned(), input.protocol_minor.to_string()),
        (
            "domainSchema".to_owned(),
            input.pack_schema_version.to_string(),
        ),
        (
            "planningIrSchema".to_owned(),
            input.planning_ir_schema_version.to_string(),
        ),
    ])
}

fn check_accepted(
    service: &HeadlessService,
    fixture: &LoadedFixture,
    snapshot: &ScenarioSnapshotV1,
    portable: &PortableAcceptedResultV2,
) -> Result<AcceptanceMeasurements> {
    let bytes = serde_json::to_vec(portable)?;
    let reopened = PortableAcceptedResultV2::from_json(&bytes)?;
    let verified = app(service.verify_result(snapshot, &reopened, &files::control()))?;
    ensure!(
        verified.accepted && verified.score == portable.accepted_result.verification.score,
        "fresh source verification rejected acceptance/score"
    );
    let selected = portable
        .accepted_result
        .solution
        .assignments
        .iter()
        .filter(|assignment| assignment.value == AssignmentValue::Boolean(true))
        .count();
    ensure!(
        Some(u32::try_from(selected)?) == fixture.expected.selected_assignments,
        "coverage assignment count differs from original-domain expectation"
    );
    let expected = fixture
        .expected
        .score
        .context("accepted fixture lacks score expectation")?;
    ensure!(
        verified.score.feasibility == expected.feasibility && verified.score.levels.len() == 1,
        "authoritative score meaning changed"
    );
    let rank = &verified.score.levels[0];
    ensure!(
        rank.level_id.as_str() == RANK_LEVEL
            && (expected.minimum_stable_rank..=expected.maximum_stable_rank).contains(&rank.value),
        "authoritative stable-rank score outside semantic bounds"
    );
    let mut consumer_checks = vec![
        "portableRoundtrip".to_owned(),
        "freshOriginalRevisionVerification".to_owned(),
        "authoritativeScore".to_owned(),
    ];
    if fixture.manifest.id == CaseId::ClinicTiny {
        share_checks(service, snapshot, portable)?;
        consumer_checks.push("sharePrivacyAndAuthorityBoundaries".to_owned());
    }
    Ok(AcceptanceMeasurements {
        portable_sha256: files::sha256(&bytes),
        selected_assignments: u32::try_from(selected)?,
        feasibility: verified.score.feasibility,
        stable_rank: rank.value,
        consumer_checks,
    })
}

fn share_checks(
    service: &HeadlessService,
    snapshot: &ScenarioSnapshotV1,
    portable: &PortableAcceptedResultV2,
) -> Result<()> {
    app(service.verify_result(snapshot, portable, &files::control()))?;
    let accepted = &portable.accepted_result;
    let schema = ValidatedContractSchema::new(serde_json::from_str(include_str!(
        "../../../schemas/generated/workforce.share-result.schema.json"
    ))?)?;
    for include in [false, true] {
        let shared = build_workforce_share_result(
            &snapshot.document,
            accepted,
            ShareResultOptions {
                include_evidence_references: include,
            },
        )?;
        schema.validate(&shared.payload, ContractJsonLimits::DEFAULT)?;
        ensure!(
            shared.payload["scenarioRevision"].as_u64() == Some(snapshot.revision.value())
                && shared.payload["acceptedResultChecksum"].as_str()
                    == Some(accepted.checksum.as_str()),
            "share source/revision binding changed"
        );
        ensure!(
            shared.payload.get("evidenceReferences").is_some() == include,
            "share privacy choice ignored"
        );
        let assignments = shared.payload["assignments"]
            .as_array()
            .context("share assignments absent")?;
        ensure!(
            assignments
                .iter()
                .all(|assignment| assignment
                    .as_object()
                    .is_some_and(|fields| fields.len() == 3
                        && ["assignmentId", "personId", "shiftId"]
                            .iter()
                            .all(|key| fields.contains_key(*key)))),
            "share payload exposed more than identities"
        );
    }
    let mut wrong_revision = snapshot.clone();
    wrong_revision.revision = wrong_revision.revision.checked_next()?;
    ensure!(
        service
            .verify_result(&wrong_revision, portable, &files::control())
            .is_err(),
        "same document with different external revision accepted"
    );
    let mut unaccepted = accepted.clone();
    unaccepted.verification.accepted = false;
    ensure!(
        build_workforce_share_result(
            &snapshot.document,
            &unaccepted,
            ShareResultOptions {
                include_evidence_references: false
            }
        )
        .is_err(),
        "unaccepted share admitted"
    );
    let forged = forged_score(accepted)?;
    ensure!(
        build_workforce_share_result(
            &snapshot.document,
            &forged,
            ShareResultOptions {
                include_evidence_references: false
            }
        )
        .is_err(),
        "resealed forged score admitted to share"
    );
    let mut changed = snapshot.document.clone();
    changed.metadata.title.push_str(" changed");
    ensure!(
        build_workforce_share_result(
            &changed,
            accepted,
            ShareResultOptions {
                include_evidence_references: false
            }
        )
        .is_err(),
        "mismatched source admitted to share"
    );
    Ok(())
}

fn forged_score(accepted: &AcceptedResult) -> Result<AcceptedResult> {
    let report = &accepted.verification;
    let context = VerificationContextV1::new(
        report.scenario_id,
        report.evaluated_revision,
        report.document_hash.clone(),
        report.planning_model_hash.clone(),
        accepted.solution.canonical_hash()?,
        report.verification_scope_checksum.clone(),
    )?;
    let mut score = report.score.clone();
    let rank = score.levels.first_mut().context("missing score level")?;
    rank.value = rank.value.checked_add(1).context("forged score overflow")?;
    let report = VerificationReport::new(
        &context,
        report.required_rule_results.clone(),
        score,
        report.warnings.clone(),
        report.metrics.clone(),
    )?;
    Ok(AcceptedResult::new(accepted.solution.clone(), report)?)
}

pub(crate) async fn run(
    root: &Path,
    artifact_root: &Path,
    manifest_sha256: &str,
    output: &Path,
    cancellation: &CancellationToken,
) -> Result<()> {
    check_cancellation(cancellation)?;
    generation::generate(root, true)?;
    let corpus = generation::load(root)?;
    let source = generation::identity(root)?;
    let executable = std::env::current_exe()?;
    let executable_sha256 = files::hash_file(&executable, EXECUTABLE_LIMIT)?;
    files::digest(manifest_sha256)?;
    let artifact_root = artifact_root.canonicalize()?;
    let mut scratch_builder = tempfile::Builder::new();
    scratch_builder.prefix("workforce-evidence-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        scratch_builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let scratch = scratch_builder.tempdir()?;
    let runner = SampleRunner {
        root,
        artifact_root: &artifact_root,
        manifest_sha256,
        executable: &executable,
        executable_sha256: &executable_sha256,
        corpus: &corpus,
        cancellation,
    };
    let mut samples = Vec::new();
    for id in CaseId::ALL {
        for post_warmup in [false, true] {
            let child_output =
                scratch
                    .path()
                    .join(format!("{}-{}.json", id.slug(), u8::from(post_warmup)));
            samples.extend(runner.collect(id, post_warmup, &child_output).await?);
        }
    }
    let checkout_commit = git_text(root, &["rev-parse", "HEAD"], cancellation)
        .await?
        .trim()
        .to_owned();
    ensure!(
        (checkout_commit.len() == 40 || checkout_commit.len() == 64)
            && checkout_commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid checkout identity"
    );
    let checkout_dirty = !git_text(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
        cancellation,
    )
    .await?
    .is_empty();
    let evidence = BenchmarkEvidence {
        format: EVIDENCE_FORMAT.to_owned(),
        schema_version: FORMAT_VERSION,
        corpus_version: CORPUS_VERSION,
        corpus_sha256: corpus.manifest_sha256,
        worker_manifest_sha256: manifest_sha256.to_owned(),
        runner: RunnerIdentity {
            source_sha256: source.source_sha256,
            executable_sha256,
            target: env!("EUTHETO_BENCH_TARGET").to_owned(),
            rustc: env!("EUTHETO_BENCH_RUSTC").to_owned(),
            build_profile: env!("EUTHETO_BENCH_PROFILE").to_owned(),
            runner_class: if std::env::var("GITHUB_ACTIONS").is_ok_and(|value| value == "true") {
                "githubActions"
            } else {
                "local"
            }
            .to_owned(),
            logical_parallelism: std::thread::available_parallelism()
                .ok()
                .and_then(|value| u32::try_from(value.get()).ok()),
            checkout_commit,
            checkout_dirty,
        },
        profile: corpus.manifest.profile,
        worker_lifecycle: "spawnPerSolve".to_owned(),
        os_cache_state: "unmanaged".to_owned(),
        enrichment_state: "notApplicable".to_owned(),
        evidence_class: "synthetic-observations-not-release-baseline".to_owned(),
        aggregation_method: "raw samples; ascending integer median; min/max".to_owned(),
        variance_policy: "report observed range; no Phase12 timing threshold".to_owned(),
        aggregates: aggregate(&samples)?,
        samples,
    };
    check_cancellation(cancellation)?;
    publish(output, &evidence)
}

struct SampleRunner<'a> {
    root: &'a Path,
    artifact_root: &'a Path,
    manifest_sha256: &'a str,
    executable: &'a Path,
    executable_sha256: &'a str,
    corpus: &'a LoadedCorpus,
    cancellation: &'a CancellationToken,
}

impl SampleRunner<'_> {
    async fn collect(
        &self,
        id: CaseId,
        post_warmup: bool,
        output: &Path,
    ) -> Result<Vec<BenchmarkSample>> {
        check_cancellation(self.cancellation)?;
        let mut command = CommandWrap::with_new(self.executable.as_os_str(), |command| {
            command
                .arg("--repository")
                .arg(self.root)
                .arg("sample")
                .arg("--artifact-root")
                .arg(self.artifact_root)
                .arg("--manifest-sha256")
                .arg(self.manifest_sha256)
                .arg("--case")
                .arg(id.slug())
                .arg("--output")
                .arg(output)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                // Child diagnostics are not evidence and may contain private scenario data.
                .stderr(Stdio::null());
            if post_warmup {
                command.arg("--post-warmup");
            }
        });
        command.wrap(KillOnDrop);
        #[cfg(unix)]
        command.wrap(ProcessGroup::leader());
        #[cfg(windows)]
        command.wrap(JobObject);
        let mut child = command.spawn()?;
        supervise_sample(child.as_mut(), self.cancellation).await?;
        check_cancellation(self.cancellation)?;
        let batch =
            serde_json::from_slice(&files::read_bounded(output, MAX_EVIDENCE_BYTES as u64)?)?;
        self.validate_batch(&batch, id, post_warmup)?;
        Ok(batch.samples)
    }

    fn validate_batch(&self, batch: &SampleBatch, id: CaseId, post_warmup: bool) -> Result<()> {
        let state = if post_warmup {
            RunnerProcessState::PostWarmup
        } else {
            RunnerProcessState::FirstOperation
        };
        let count = if post_warmup {
            self.corpus.manifest.profile.post_warmup_samples
        } else {
            self.corpus.manifest.profile.first_operation_samples
        };
        ensure!(
            batch.schema_version == FORMAT_VERSION
                && batch.corpus_sha256 == self.corpus.manifest_sha256
                && batch.worker_manifest_sha256 == self.manifest_sha256
                && batch.runner_sha256 == self.executable_sha256
                && batch.samples.len() == usize::from(count),
            "child evidence identity/count mismatch"
        );
        for (ordinal, sample) in batch.samples.iter().enumerate() {
            ensure!(
                sample.case_id == id
                    && sample.runner_process_state == state
                    && usize::from(sample.ordinal) == ordinal,
                "child sample identity/order mismatch"
            );
        }
        Ok(())
    }
}

async fn supervise_sample(
    child: &mut dyn ChildWrapper,
    cancellation: &CancellationToken,
) -> Result<()> {
    let started = Instant::now();
    let result = loop {
        if cancellation.is_cancelled() {
            break Err("headless corpus sample cancelled");
        }
        if started.elapsed() >= SAMPLE_GUARD {
            break Err("corpus sample supervisory limit exceeded");
        }
        // These commands have exactly one child wrapper (group/job; KillOnDrop
        // configures Command only). Poll its native child without consuming job
        // completion notifications or cancelling a group/job wait midway.
        match child.inner_mut().try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => tokio::time::sleep(Duration::from_millis(25)).await,
            Err(_) => break Err("corpus sample wait failed"),
        }
    };
    let status = match result {
        Ok(status) => status,
        Err(reason) => {
            cancel_sample(child).await?;
            anyhow::bail!("{reason}");
        }
    };
    #[cfg(unix)]
    {
        finish_sample(status)
    }
    #[cfg(windows)]
    {
        finish_sample(child, status).await
    }
}

#[cfg(unix)]
async fn cancel_sample(child: &mut dyn ChildWrapper) -> Result<()> {
    // The production worker owns a separate PGID. Only the wrapper can cooperatively
    // cancel and reap it; killing this outer group is not whole-session containment.
    let _ = child.signal(tokio::signal::unix::SignalKind::interrupt().as_raw_value());
    if let Ok(Ok(status)) = tokio::time::timeout(CLEANUP_GRACE, child.wait()).await
        && matches!(status.code(), Some(0 | 1))
    {
        return Ok(());
    }
    force_sample_cleanup(child).await;
    anyhow::bail!(
        "headless sample cleanup-unconfirmed: wrapper unresponsive or aborted; worker cleanup cannot be confirmed"
    )
}

#[cfg(unix)]
async fn force_sample_cleanup(child: &mut dyn ChildWrapper) {
    // Signal only an observed live, unreaped wrapper; never reuse a reaped PGID.
    // Best effort only: an independently grouped worker can survive this.
    if matches!(child.inner_mut().try_wait(), Ok(None)) {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(CLEANUP_GRACE, child.wait()).await;
    }
}

#[cfg(unix)]
fn finish_sample(status: ExitStatus) -> Result<()> {
    // Main returns only 0/1 after awaiting work; other codes include unwound panics.
    if !matches!(status.code(), Some(0 | 1)) {
        anyhow::bail!(
            "headless sample cleanup-unconfirmed: wrapper aborted; worker cleanup cannot be confirmed"
        );
    }
    ensure!(status.success(), "headless corpus sample failed");
    Ok(())
}

#[cfg(windows)]
async fn cancel_sample(child: &mut dyn ChildWrapper) -> Result<()> {
    // Tokio JobObject + KillOnDrop owns a kill-on-close job; explicitly terminate
    // it even if the wrapper has exited, then wait while its handles remain owned.
    let killed = child.start_kill();
    let waited = tokio::time::timeout(CLEANUP_GRACE, child.wait()).await;
    ensure!(
        killed.is_ok() && matches!(waited, Ok(Ok(_))),
        "headless sample cleanup-unconfirmed: owned job termination/wait failed"
    );
    Ok(())
}

#[cfg(windows)]
async fn finish_sample(child: &mut dyn ChildWrapper, status: ExitStatus) -> Result<()> {
    cancel_sample(child).await?;
    ensure!(status.success(), "headless corpus sample failed");
    Ok(())
}

fn aggregate(samples: &[BenchmarkSample]) -> Result<Vec<TimingAggregate>> {
    let mut groups: BTreeMap<(CaseId, RunnerProcessState, String), Vec<u64>> = BTreeMap::new();
    for sample in samples {
        for (metric, value) in &sample.timings.milliseconds {
            groups
                .entry((sample.case_id, sample.runner_process_state, metric.clone()))
                .or_default()
                .push(*value);
        }
    }
    groups
        .into_iter()
        .map(|((case_id, runner_process_state, metric), mut values)| {
            values.sort_unstable();
            let minimum = *values.first().context("empty timing group")?;
            let maximum = *values.last().context("empty timing group")?;
            Ok(TimingAggregate {
                case_id,
                runner_process_state,
                metric,
                samples: u16::try_from(values.len())?,
                minimum_milliseconds: minimum,
                median_milliseconds: values[values.len() / 2],
                maximum_milliseconds: maximum,
            })
        })
        .collect()
}

async fn git_text(
    root: &Path,
    arguments: &[&str],
    cancellation: &CancellationToken,
) -> Result<String> {
    check_cancellation(cancellation)?;
    let mut child = tokio::process::Command::new("git")
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().context("git output pipe absent")?;
    let read = async move {
        let mut bytes = Vec::new();
        stdout.take(65_537).read_to_end(&mut bytes).await?;
        Ok::<_, std::io::Error>(bytes)
    };
    let (bytes, status) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::try_join!(read, child.wait())
    })
    .await
    .context("checkout observation timed out")??;
    ensure!(
        status.success() && bytes.len() <= 65_536,
        "checkout observation failed or exceeded bound"
    );
    Ok(String::from_utf8(bytes)?)
}

fn publish(output: &Path, value: &impl serde::Serialize) -> Result<()> {
    files::publish_evidence(output, value)
}

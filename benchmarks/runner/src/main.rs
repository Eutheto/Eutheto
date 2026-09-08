#![forbid(unsafe_code)]

use std::error::Error;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use eutheto_planning_ir::{
    PLANNING_IR_SCHEMA_VERSION, PlanningIrLimitsV1, PlanningProblem, PlanningProblemSummary,
    summarize,
};
use eutheto_protocol::checked_in_policy;
use eutheto_solver_api::{
    BackendAppliedParameterEvidence, BackendModelCountEvidence, BackendOutputSink,
    BackendTerminationReason, BackendTimingEvidence, BackendWorkerStatistics, BoundedBackendOutput,
    OutputError, ProgressSink, SolveProgressEvent, SolveRequest, SolverApiLimits, SolverBackend,
    validate_outcome,
};
use eutheto_solver_ortools::{
    ORTOOLS_ADAPTER_VERSION, ORTOOLS_BACKEND_ID, ORTOOLS_VERSION, VerifiedWorkerArtifact,
    registry_with_ortools,
};
use eutheto_types::{
    BackendSelection, CancellationToken, DurationMillis, ExplanationMode, ParentSolveBudget,
    PreservationPolicy, ReproducibilityMode, ResourceLimits, SolveMode, SolveOptions,
    SystemMonotonicClock, WorkerThreadPolicy,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

const CORPUS_FORMAT: &str = "eutheto/planning-ir-benchmark-corpus";
const CORPUS_SCHEMA_VERSION: u32 = 1;
const CORPUS_VERSION: u32 = 1;
const CORPUS_PROVENANCE_SOURCE: &str = "benchmarks/corpus/solver/phase03-primitives.json";
const CORPUS_PROVENANCE_METHOD: &str = "checked-in-hand-authored-planning-ir";
const CORPUS_LICENSE: &str = "Apache-2.0";
const EVIDENCE_FORMAT: &str = "eutheto/phase03-benchmark-evidence";
const EVIDENCE_SCHEMA_VERSION: u32 = 1;
const MAX_CORPUS_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CASE_IR_BYTES: u64 = 256 * 1024;
const MAX_CASES: usize = 32;
const MAX_CASE_ID_BYTES: usize = 160;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const WALL_BUDGET_MILLISECONDS: u64 = 2_000;
const WORKER_IDENTITY: &str = "eutheto-ortools-worker";
const WORKER_VERSION: &str = "0.1.0";
const ENGINE_IDENTITY: &str = "google.or-tools-cp-sat";
const PROTOCOL_IDENTITY: &str = "eutheto.worker";
const PROTOCOL_WIRE_VERSION: u32 = 1;

const IR_LIMITS: PlanningIrLimitsV1 = PlanningIrLimitsV1 {
    max_ir_bytes: MAX_CASE_IR_BYTES,
    max_variables: 128,
    max_constraints: 256,
    max_assumptions: 32,
    max_objective_levels: 4,
    max_objective_terms: 64,
    max_provenance_records: 256,
    max_provenance_depth: 8,
    max_parameters_per_record: 16,
    max_parameter_text_bytes: 256,
    max_entity_refs_per_record: 64,
    max_projections: 128,
    max_projection_expression_depth: 8,
    max_domain_ranges: 16,
    max_refs_per_node: 128,
    max_total_refs: 2_048,
    max_table_rows: 256,
    max_table_arity: 16,
    max_table_cells: 2_048,
    max_intervals_per_global: 128,
    max_enforcement_literals: 32,
    max_tags: 16,
    max_component_nodes: 384,
    max_component_edges: 4_096,
    max_id_bytes: 160,
    max_metadata_text_bytes: 1_024,
    max_abs_coefficient: 1_000_000,
    max_abs_value: 1_000_000_000,
};

const OUTPUT_LIMITS: SolverApiLimits = SolverApiLimits {
    max_candidates: 1,
    max_candidate_assignments: 128,
    max_progress_events: 256,
    max_diagnostic_lines: 32,
    max_evidence_refs_per_candidate: 16,
};

type RunnerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug)]
struct Cli {
    artifact_root: PathBuf,
    manifest_sha256: [u8; 32],
    corpus: PathBuf,
    output: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Corpus {
    format: String,
    schema_version: u32,
    #[serde(rename = "corpusVersion")]
    version: u32,
    planning_ir_schema_version: u32,
    provenance: CorpusProvenance,
    license: CorpusLicense,
    cases: Vec<CorpusCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CorpusProvenance {
    source: String,
    method: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CorpusLicense {
    spdx_expression: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CorpusCase {
    id: String,
    problem: PlanningProblem,
    expected: ExpectedEvidence,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedEvidence {
    raw_termination: BackendTerminationReason,
    raw_candidate_count: u32,
    raw_objective_values: Option<Vec<i64>>,
    raw_best_bound_values: Option<Vec<i64>>,
    model_counts: BackendModelCountEvidence,
}

struct ValidatedCorpus {
    cases: Vec<ValidatedCase>,
}

struct ValidatedCase {
    id: String,
    problem: PlanningProblem,
    summary: PlanningProblemSummary,
    expected: ExpectedEvidence,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkEvidence {
    format: &'static str,
    schema_version: u32,
    evidence_status: &'static str,
    independent_verification_performed: bool,
    manifest_sha256: String,
    corpus_sha256: String,
    corpus: CorpusIdentity,
    identities: ExecutionIdentities,
    fixed_solve_options: SolveOptions,
    cases: Vec<CaseEvidence>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CorpusIdentity {
    format: &'static str,
    schema_version: u32,
    #[serde(rename = "corpusVersion")]
    version: u32,
    planning_ir_schema_version: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionIdentities {
    target: TargetIdentity,
    runner: VersionedIdentity,
    backend: VersionedIdentity,
    adapter: VersionedIdentity,
    worker: VersionedIdentity,
    engine: VersionedIdentity,
    protocol: ProtocolIdentity,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetIdentity {
    triple: &'static str,
    architecture: &'static str,
    operating_system: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionedIdentity {
    id: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProtocolIdentity {
    id: &'static str,
    major: u32,
    minor: u32,
    wire_version: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CaseEvidence {
    id: String,
    evidence_status: &'static str,
    canonical_ir_blake3: String,
    solve_fingerprint_blake3: String,
    raw_termination: BackendTerminationReason,
    raw_candidate_count: u32,
    raw_objective_values: Option<Vec<i64>>,
    raw_best_bound_values: Option<Vec<i64>>,
    parent_full_adapter_elapsed_milliseconds: DurationMillis,
    parent_remaining_at_dispatch_milliseconds: DurationMillis,
    parent_backend_limit_milliseconds: DurationMillis,
    parent_first_incumbent_milliseconds: Option<DurationMillis>,
    timings: CompleteTimingEvidence,
    model_counts: BackendModelCountEvidence,
    raw_worker_native_statistics: BackendWorkerStatistics,
    applied_parameters: BackendAppliedParameterEvidence,
    applied_parameters_sha256: String,
    model_fingerprint_sha256: String,
    progress_event_count: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
struct CompleteTimingEvidence {
    translation_serialization_milliseconds: DurationMillis,
    worker_startup_milliseconds: DurationMillis,
    handshake_milliseconds: DurationMillis,
    solver_milliseconds: DurationMillis,
    protocol_decode_milliseconds: DurationMillis,
}

struct DiscardProgress;

impl ProgressSink for DiscardProgress {
    fn emit(&mut self, _event: SolveProgressEvent) -> Result<(), OutputError> {
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("phase-03 benchmark failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> RunnerResult<()> {
    let cli = parse_args(std::env::args_os().skip(1))?;
    let corpus_bytes = read_bounded_file(&cli.corpus, MAX_CORPUS_BYTES).await?;
    let corpus_sha256 = sha256_hex(&corpus_bytes);
    let corpus = parse_and_validate_corpus(&corpus_bytes)?;

    let artifact = VerifiedWorkerArtifact::verify(&cli.artifact_root, cli.manifest_sha256).await?;
    let registry = registry_with_ortools(artifact)?;
    let backend_id = ORTOOLS_BACKEND_ID.parse()?;
    let backend = registry
        .get(&backend_id)
        .ok_or_else(|| invalid("the production OR-Tools backend is absent from its registry"))?;
    let descriptor = backend.descriptor();
    if descriptor.id.as_str() != ORTOOLS_BACKEND_ID
        || descriptor.version != ORTOOLS_VERSION
        || descriptor.adapter_version != ORTOOLS_ADAPTER_VERSION
    {
        return Err(invalid(
            "the production backend descriptor identity changed",
        ));
    }

    let protocol = checked_in_policy()?;
    let protocol_major = protocol.protocol_major();
    let protocol_minor = protocol.protocol_minor();
    let options = fixed_solve_options()?;
    let mut cases = Vec::with_capacity(corpus.cases.len());
    for case in corpus.cases {
        cases.push(
            run_case(
                case,
                backend.as_ref(),
                &options,
                protocol_major,
                protocol_minor,
            )
            .await?,
        );
    }

    let evidence = BenchmarkEvidence {
        format: EVIDENCE_FORMAT,
        schema_version: EVIDENCE_SCHEMA_VERSION,
        evidence_status: "raw-unverified",
        independent_verification_performed: false,
        manifest_sha256: hex_encode(&cli.manifest_sha256),
        corpus_sha256,
        corpus: CorpusIdentity {
            format: CORPUS_FORMAT,
            schema_version: CORPUS_SCHEMA_VERSION,
            version: CORPUS_VERSION,
            planning_ir_schema_version: PLANNING_IR_SCHEMA_VERSION,
        },
        identities: execution_identities(protocol_major, protocol_minor),
        fixed_solve_options: options,
        cases,
    };
    let output = deterministic_pretty_json(&evidence)?;
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(invalid("benchmark evidence exceeds its output byte limit"));
    }
    write_atomic(&cli.output, &output)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn run_case(
    case: ValidatedCase,
    backend: &dyn SolverBackend,
    options: &SolveOptions,
    protocol_major: u32,
    protocol_minor: u32,
) -> RunnerResult<CaseEvidence> {
    if !backend.compatibility(&case.summary, options).compatible() {
        return Err(case_error(
            &case.id,
            "is incompatible with the production backend",
        ));
    }

    let problem = Arc::new(case.problem);
    let parent_budget = ParentSolveBudget::new(
        options.time_limit_milliseconds,
        Arc::new(SystemMonotonicClock::new()),
        CancellationToken::new(),
    )?;
    let request = SolveRequest::new(
        ORTOOLS_BACKEND_ID.parse()?,
        ORTOOLS_VERSION,
        ORTOOLS_ADAPTER_VERSION,
        Arc::clone(&problem),
        case.summary,
        options.clone(),
        &parent_budget,
        None,
        DurationMillis::ZERO,
    )?;
    let mut progress = DiscardProgress;
    let mut output = BoundedBackendOutput::new(
        &problem,
        &mut progress,
        request.dispatch_budget(),
        OUTPUT_LIMITS,
    )?;
    let outcome = backend
        .solve(&request, &mut output as &mut dyn BackendOutputSink)
        .await?;
    let result = output.into_result(outcome);
    validate_outcome(&request, &result.outcome, &result.candidates, OUTPUT_LIMITS)?;

    let candidate_count = u32::try_from(result.candidates.len())
        .map_err(|_| case_error(&case.id, "returned too many raw candidates"))?;
    let evidence = &result.outcome.evidence;
    let (objective_values, best_bound_values) = evidence.objective.as_ref().map_or_else(
        || (None, None),
        |objective| {
            (
                Some(objective.objective_values.clone()),
                objective.best_bound_values.clone(),
            )
        },
    );
    let execution = evidence
        .execution
        .as_ref()
        .ok_or_else(|| case_error(&case.id, "omitted adapter execution evidence"))?;
    let reproducibility = &execution.reproducibility;

    if result.outcome.termination != case.expected.raw_termination {
        return Err(case_error(
            &case.id,
            "raw termination differs from the corpus expectation",
        ));
    }
    if candidate_count != case.expected.raw_candidate_count {
        return Err(case_error(
            &case.id,
            "raw candidate count differs from the corpus expectation",
        ));
    }
    if objective_values != case.expected.raw_objective_values {
        return Err(case_error(
            &case.id,
            "raw objective differs from the corpus expectation",
        ));
    }
    if best_bound_values != case.expected.raw_best_bound_values {
        return Err(case_error(
            &case.id,
            "raw objective bound differs from the corpus expectation",
        ));
    }
    if execution.model_counts != case.expected.model_counts {
        return Err(case_error(
            &case.id,
            "model counts differ from the corpus expectation",
        ));
    }
    if reproducibility.worker_version != WORKER_VERSION
        || reproducibility.engine_version != ORTOOLS_VERSION
        || reproducibility.protocol_major != protocol_major
        || reproducibility.protocol_minor != protocol_minor
    {
        return Err(case_error(
            &case.id,
            "worker, engine, or protocol identity changed",
        ));
    }
    validate_applied_parameters(
        &case.id,
        &reproducibility.applied_parameters,
        !problem.objectives.levels.is_empty(),
    )?;
    if decode_sha256(&reproducibility.model_fingerprint_sha256).is_err() {
        return Err(case_error(&case.id, "has an invalid model fingerprint"));
    }
    let applied_parameters_sha256 = reproducibility
        .applied_parameters_sha256
        .clone()
        .ok_or_else(|| {
            case_error(
                &case.id,
                "omitted the corroborated applied-parameter digest",
            )
        })?;
    if decode_sha256(&applied_parameters_sha256).is_err() {
        return Err(case_error(
            &case.id,
            "has an invalid applied-parameter digest",
        ));
    }
    let timings = complete_timings(&case.id, &execution.timings)?;
    let worker_statistics = execution
        .worker_statistics
        .clone()
        .ok_or_else(|| case_error(&case.id, "omitted bounded worker-native statistics"))?;
    if worker_statistics.wall_time_milliseconds.is_none()
        || worker_statistics.user_time_milliseconds.is_none()
        || worker_statistics.deterministic_time_milliseconds.is_none()
        || worker_statistics.conflicts.is_none()
        || worker_statistics.branches.is_none()
        || worker_statistics.binary_propagations.is_none()
        || worker_statistics.integer_propagations.is_none()
    {
        return Err(case_error(
            &case.id,
            "omitted required worker-native timing or model-search counts",
        ));
    }

    Ok(CaseEvidence {
        id: case.id,
        evidence_status: "raw-unverified",
        canonical_ir_blake3: result.outcome.model_hash.clone(),
        solve_fingerprint_blake3: result.outcome.solve_fingerprint.clone(),
        raw_termination: result.outcome.termination,
        raw_candidate_count: candidate_count,
        raw_objective_values: objective_values,
        raw_best_bound_values: best_bound_values,
        parent_full_adapter_elapsed_milliseconds: evidence.elapsed_milliseconds,
        parent_remaining_at_dispatch_milliseconds: evidence.remaining_at_dispatch_milliseconds,
        parent_backend_limit_milliseconds: evidence.backend_limit_milliseconds,
        parent_first_incumbent_milliseconds: evidence.first_incumbent_milliseconds,
        timings,
        model_counts: execution.model_counts.clone(),
        raw_worker_native_statistics: worker_statistics,
        applied_parameters: reproducibility.applied_parameters.clone(),
        applied_parameters_sha256,
        model_fingerprint_sha256: reproducibility.model_fingerprint_sha256.clone(),
        progress_event_count: result.progress_event_count,
    })
}

fn validate_applied_parameters(
    case_id: &str,
    applied: &BackendAppliedParameterEvidence,
    has_objective: bool,
) -> RunnerResult<()> {
    if applied.memory_limit_bytes.is_some()
        || applied.worker_threads != 1
        || applied.random_seed != 1
        || applied.stop_after_first_feasible
        || applied.emit_intermediate_solutions
        || applied.log_search_progress != has_objective
        || !applied.deterministic_test_profile
        || applied.wall_time_milliseconds.is_none()
    {
        Err(case_error(
            case_id,
            "did not apply the fixed deterministic worker parameters",
        ))
    } else {
        Ok(())
    }
}

fn complete_timings(
    case_id: &str,
    timings: &BackendTimingEvidence,
) -> RunnerResult<CompleteTimingEvidence> {
    Ok(CompleteTimingEvidence {
        translation_serialization_milliseconds: timings.translation_serialization_milliseconds,
        worker_startup_milliseconds: timings
            .worker_startup_milliseconds
            .ok_or_else(|| case_error(case_id, "omitted worker startup timing"))?,
        handshake_milliseconds: timings
            .handshake_milliseconds
            .ok_or_else(|| case_error(case_id, "omitted handshake timing"))?,
        solver_milliseconds: timings
            .solver_milliseconds
            .ok_or_else(|| case_error(case_id, "omitted solver timing"))?,
        protocol_decode_milliseconds: timings
            .protocol_decode_milliseconds
            .ok_or_else(|| case_error(case_id, "omitted protocol decode timing"))?,
    })
}

fn fixed_solve_options() -> RunnerResult<SolveOptions> {
    Ok(SolveOptions {
        backend: BackendSelection::Specific(ORTOOLS_BACKEND_ID.parse()?),
        mode: SolveMode::Custom,
        time_limit_milliseconds: DurationMillis::new(WALL_BUDGET_MILLISECONDS)?,
        memory_limit_bytes: None,
        worker_threads: WorkerThreadPolicy::Exact(1),
        random_seed: 1,
        solution_limit: None,
        stop_after_first_feasible: false,
        collect_intermediate_solutions: false,
        explanation_mode: ExplanationMode::None,
        preserve_existing: PreservationPolicy::None,
        reproducibility: ReproducibilityMode::Deterministic,
        resource_limits: ResourceLimits {
            max_entities: 128,
            max_rules: 128,
            max_variables: IR_LIMITS.max_variables,
            max_constraints: IR_LIMITS.max_constraints,
        },
    })
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> RunnerResult<Cli> {
    let arguments: Vec<_> = arguments.into_iter().collect();
    if arguments.len() != 8 {
        return Err(invalid("expected exactly four flag-value pairs"));
    }

    let mut artifact_root = None;
    let mut manifest_sha256 = None;
    let mut corpus = None;
    let mut output = None;
    for pair in arguments.chunks_exact(2) {
        let flag = pair[0]
            .to_str()
            .ok_or_else(|| invalid("CLI flags must be valid UTF-8"))?;
        match flag {
            "--artifact-root" => {
                if artifact_root.replace(PathBuf::from(&pair[1])).is_some() {
                    return Err(invalid("--artifact-root may be supplied only once"));
                }
            }
            "--manifest-sha256" => {
                let value = pair[1]
                    .to_str()
                    .ok_or_else(|| invalid("--manifest-sha256 must be valid UTF-8"))?;
                if manifest_sha256.replace(decode_sha256(value)?).is_some() {
                    return Err(invalid("--manifest-sha256 may be supplied only once"));
                }
            }
            "--corpus" => {
                if corpus.replace(PathBuf::from(&pair[1])).is_some() {
                    return Err(invalid("--corpus may be supplied only once"));
                }
            }
            "--output" => {
                if output.replace(PathBuf::from(&pair[1])).is_some() {
                    return Err(invalid("--output may be supplied only once"));
                }
            }
            _ => return Err(invalid("unknown CLI flag")),
        }
    }

    let artifact_root = artifact_root.ok_or_else(|| invalid("missing --artifact-root"))?;
    let manifest_sha256 = manifest_sha256.ok_or_else(|| invalid("missing --manifest-sha256"))?;
    let corpus = corpus.ok_or_else(|| invalid("missing --corpus"))?;
    let output = output.ok_or_else(|| invalid("missing --output"))?;
    if !artifact_root.is_absolute() {
        return Err(invalid("--artifact-root must be an absolute path"));
    }
    if corpus == output {
        return Err(invalid(
            "--corpus and --output must identify different paths",
        ));
    }
    if output.file_name().is_none() {
        return Err(invalid("--output must name a file"));
    }
    Ok(Cli {
        artifact_root,
        manifest_sha256,
        corpus,
        output,
    })
}

async fn read_bounded_file(path: &Path, byte_limit: u64) -> RunnerResult<Vec<u8>> {
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(invalid("--corpus must identify a regular file"));
    }
    if metadata.len() > byte_limit {
        return Err(invalid("corpus exceeds its byte limit"));
    }
    let file = tokio::fs::File::open(path).await?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
    file.take(byte_limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .await?;
    if u64::try_from(bytes.len())? > byte_limit {
        return Err(invalid("corpus exceeds its byte limit"));
    }
    Ok(bytes)
}

fn parse_and_validate_corpus(bytes: &[u8]) -> RunnerResult<ValidatedCorpus> {
    if u64::try_from(bytes.len())? > MAX_CORPUS_BYTES {
        return Err(invalid("corpus exceeds its byte limit"));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let corpus = Corpus::deserialize(&mut deserializer)?;
    deserializer.end()?;
    if corpus.format != CORPUS_FORMAT
        || corpus.schema_version != CORPUS_SCHEMA_VERSION
        || corpus.version != CORPUS_VERSION
        || corpus.planning_ir_schema_version != PLANNING_IR_SCHEMA_VERSION
    {
        return Err(invalid("corpus format or version is unsupported"));
    }
    if corpus.provenance.source != CORPUS_PROVENANCE_SOURCE
        || corpus.provenance.method != CORPUS_PROVENANCE_METHOD
    {
        return Err(invalid(
            "corpus provenance is not the version-1 checked-in contract",
        ));
    }
    if corpus.license.spdx_expression != CORPUS_LICENSE {
        return Err(invalid(
            "corpus license is not the version-1 license contract",
        ));
    }
    if corpus.cases.is_empty() || corpus.cases.len() > MAX_CASES {
        return Err(invalid("corpus must contain between one and 32 cases"));
    }

    let mut prior_id: Option<String> = None;
    let mut validated = Vec::with_capacity(corpus.cases.len());
    for case in corpus.cases {
        validate_case_id(&case.id)?;
        if prior_id
            .as_deref()
            .is_some_and(|prior| prior >= case.id.as_str())
        {
            return Err(invalid(
                "corpus case IDs must be strictly sorted and unique",
            ));
        }
        let encoded_problem = serde_json::to_vec(&case.problem)?;
        if u64::try_from(encoded_problem.len())? > MAX_CASE_IR_BYTES {
            return Err(case_error(
                &case.id,
                "exceeds the per-case Planning IR byte limit",
            ));
        }
        let summary = summarize(&case.problem, IR_LIMITS)
            .map_err(|error| case_error(&case.id, &format!("has invalid Planning IR: {error}")))?;
        validate_expected(&case.id, &case.problem, &summary, &case.expected)?;
        prior_id = Some(case.id.clone());
        validated.push(ValidatedCase {
            id: case.id,
            problem: case.problem,
            summary,
            expected: case.expected,
        });
    }
    Ok(ValidatedCorpus { cases: validated })
}

fn validate_expected(
    case_id: &str,
    problem: &PlanningProblem,
    summary: &PlanningProblemSummary,
    expected: &ExpectedEvidence,
) -> RunnerResult<()> {
    let objective_count = problem.objectives.levels.len();
    let objective_shape_valid = if objective_count == 0 {
        expected.raw_objective_values.is_none() && expected.raw_best_bound_values.is_none()
    } else {
        expected
            .raw_objective_values
            .as_ref()
            .zip(expected.raw_best_bound_values.as_ref())
            .is_some_and(|(objectives, bounds)| {
                objectives.len() == objective_count
                    && bounds.len() == objective_count
                    && objectives == bounds
                    && problem
                        .objectives
                        .levels
                        .iter()
                        .zip(objectives)
                        .all(|(level, value)| {
                            (level.lower_bound..=level.upper_bound).contains(value)
                        })
            })
    };
    if !objective_shape_valid {
        return Err(case_error(
            case_id,
            "has inconsistent expected objective evidence",
        ));
    }
    match expected.raw_termination {
        BackendTerminationReason::OptimalityClaimed if expected.raw_candidate_count == 1 => {}
        BackendTerminationReason::InfeasibilityClaimed if expected.raw_candidate_count == 0 => {}
        _ => {
            return Err(case_error(
                case_id,
                "must expect one optimal raw candidate or a raw infeasibility claim",
            ));
        }
    }
    if expected.model_counts.planning_variable_count != summary.variable_count
        || expected.model_counts.planning_constraint_count != summary.constraint_count
        || expected.model_counts.translated_variable_count > 256
        || expected.model_counts.translated_constraint_count > 512
    {
        return Err(case_error(
            case_id,
            "has inconsistent or excessive expected model counts",
        ));
    }
    Ok(())
}

fn validate_case_id(value: &str) -> RunnerResult<()> {
    let valid = value.len() <= MAX_CASE_ID_BYTES
        && value.split('.').count() >= 2
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-')
                })
                && segment
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && segment
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        });
    if valid {
        Ok(())
    } else {
        Err(invalid("corpus contains an invalid case ID"))
    }
}

fn decode_sha256(value: &str) -> RunnerResult<[u8; 32]> {
    let bytes = value.as_bytes();
    if bytes.len() != 64
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(invalid(
            "SHA-256 values must be exactly 64 lowercase hexadecimal digits",
        ));
    }
    let mut decoded = [0_u8; 32];
    for (index, output) in decoded.iter_mut().enumerate() {
        *output = (hex_nibble(bytes[index * 2]) << 4) | hex_nibble(bytes[index * 2 + 1]);
    }
    Ok(decoded)
}

const fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => 0,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn deterministic_pretty_json(value: &impl Serialize) -> RunnerResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn write_atomic(output: &Path, bytes: &[u8]) -> RunnerResult<()> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = std::fs::metadata(parent)?;
    if !metadata.is_dir() {
        return Err(invalid("--output parent must be a directory"));
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".eutheto-phase03-")
        .tempfile_in(parent)?;
    temporary.as_file_mut().write_all(bytes)?;
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(output)
        .map_err(|error| Box::new(error.error) as Box<dyn Error + Send + Sync>)?;
    Ok(())
}

fn execution_identities(protocol_major: u32, protocol_minor: u32) -> ExecutionIdentities {
    ExecutionIdentities {
        target: TargetIdentity {
            triple: current_target_triple(),
            architecture: std::env::consts::ARCH,
            operating_system: std::env::consts::OS,
        },
        runner: VersionedIdentity {
            id: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        },
        backend: VersionedIdentity {
            id: ORTOOLS_BACKEND_ID,
            version: ORTOOLS_VERSION,
        },
        adapter: VersionedIdentity {
            id: "eutheto-solver-ortools",
            version: ORTOOLS_ADAPTER_VERSION,
        },
        worker: VersionedIdentity {
            id: WORKER_IDENTITY,
            version: WORKER_VERSION,
        },
        engine: VersionedIdentity {
            id: ENGINE_IDENTITY,
            version: ORTOOLS_VERSION,
        },
        protocol: ProtocolIdentity {
            id: PROTOCOL_IDENTITY,
            major: protocol_major,
            minor: protocol_minor,
            wire_version: PROTOCOL_WIRE_VERSION,
        },
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
const fn current_target_triple() -> &'static str {
    "x86_64-unknown-linux-gnu"
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const fn current_target_triple() -> &'static str {
    "x86_64-pc-windows-msvc"
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const fn current_target_triple() -> &'static str {
    "x86_64-apple-darwin"
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const fn current_target_triple() -> &'static str {
    "aarch64-apple-darwin"
}

#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"),
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
const fn current_target_triple() -> &'static str {
    "unsupported"
}

fn case_error(case_id: &str, message: &str) -> Box<dyn Error + Send + Sync> {
    invalid(format!("case {case_id} {message}"))
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidData, message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_parser_requires_exact_lowercase_sha256() -> RunnerResult<()> {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(hex_encode(&decode_sha256(digest)?), digest);
        assert!(decode_sha256(&digest.to_uppercase()).is_err());
        assert!(decode_sha256(&digest[..63]).is_err());
        Ok(())
    }

    #[test]
    fn cli_parser_requires_each_exact_flag_once_and_an_absolute_artifact_root() -> RunnerResult<()>
    {
        let root = std::env::current_dir()?.join("artifact");
        let digest = "0".repeat(64);
        let arguments = [
            OsString::from("--artifact-root"),
            root.into_os_string(),
            OsString::from("--manifest-sha256"),
            OsString::from(digest),
            OsString::from("--corpus"),
            OsString::from("corpus.json"),
            OsString::from("--output"),
            OsString::from("evidence.json"),
        ];
        let parsed = parse_args(arguments)?;
        assert!(parsed.artifact_root.is_absolute());

        let duplicate = [
            OsString::from("--artifact-root"),
            OsString::from("/one"),
            OsString::from("--artifact-root"),
            OsString::from("/two"),
            OsString::from("--corpus"),
            OsString::from("corpus.json"),
            OsString::from("--output"),
            OsString::from("evidence.json"),
        ];
        assert!(parse_args(duplicate).is_err());
        Ok(())
    }

    #[test]
    fn corpus_contract_rejects_an_empty_case_set() {
        let bytes = br#"{
            "format":"eutheto/planning-ir-benchmark-corpus",
            "schemaVersion":1,
            "corpusVersion":1,
            "planningIrSchemaVersion":2,
            "provenance":{
                "source":"benchmarks/corpus/solver/phase03-primitives.json",
                "method":"checked-in-hand-authored-planning-ir"
            },
            "license":{"spdxExpression":"Apache-2.0"},
            "cases":[]
        }"#;
        assert!(parse_and_validate_corpus(bytes).is_err());
    }
}

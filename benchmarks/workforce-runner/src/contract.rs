use eutheto_types::{AssignmentId, RuleId, ScenarioSnapshotV1, SolveMode, SolveStatus};
use eutheto_workforce::commands::EntityPayload;
use serde::{Deserialize, Serialize};

pub(crate) const FORMAT_VERSION: u32 = 1;
pub(crate) const CORPUS_VERSION: u32 = 1;
pub(crate) const CORPUS_FORMAT: &str = "eutheto/workforce-corpus";
pub(crate) const EXPECTED_FORMAT: &str = "eutheto/workforce-corpus-expectations";
pub(crate) const REPAIR_FORMAT: &str = "eutheto/workforce-repair-fixture";
pub(crate) const EVIDENCE_FORMAT: &str = "eutheto/workforce-benchmark-evidence";
pub(crate) const CLI_EVIDENCE_FORMAT: &str = "eutheto/workforce-cli-corpus-evidence";
pub(crate) const FIXTURE_ROOT: &str = "domains/workforce/fixtures/v1";
pub(crate) const CORPUS_PATH: &str = "benchmarks/corpus/workforce/v1.json";
pub(crate) const EXPECTED_PATH: &str = "benchmarks/expected/workforce/v1.json";
pub(crate) const REPAIR_PATH: &str = "domains/workforce/fixtures/v1/repair-callout.json";
pub(crate) const FIXTURE_CLOCK: &str = "2026-08-28T23:00:00Z";
pub(crate) const MAX_INDEX_BYTES: u64 = 256 * 1024;
pub(crate) const MAX_EXPECTED_BYTES: u64 = 256 * 1024;
pub(crate) const MAX_EVIDENCE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CaseId {
    AppendixF,
    ClinicTiny,
    ClinicInitial,
    ClinicFull,
    ClinicOvernight,
    RollingHours,
    SpecialistCoverage,
    RepairBefore,
    InfeasibleCoverage,
    DstSpring,
    DstFall,
    LargeSupported,
    LargePressure,
}

impl CaseId {
    pub(crate) const ALL: [Self; 13] = [
        Self::AppendixF,
        Self::ClinicTiny,
        Self::ClinicInitial,
        Self::ClinicFull,
        Self::ClinicOvernight,
        Self::RollingHours,
        Self::SpecialistCoverage,
        Self::RepairBefore,
        Self::InfeasibleCoverage,
        Self::DstSpring,
        Self::DstFall,
        Self::LargeSupported,
        Self::LargePressure,
    ];

    pub(crate) const fn slug(self) -> &'static str {
        match self {
            Self::AppendixF => "appendix-f",
            Self::ClinicTiny => "clinic-tiny",
            Self::ClinicInitial => "clinic-initial",
            Self::ClinicFull => "clinic-full",
            Self::ClinicOvernight => "clinic-overnight",
            Self::RollingHours => "rolling-hours",
            Self::SpecialistCoverage => "specialist-coverage",
            Self::RepairBefore => "repair-before",
            Self::InfeasibleCoverage => "infeasible-coverage",
            Self::DstSpring => "dst-spring",
            Self::DstFall => "dst-fall",
            Self::LargeSupported => "large-supported",
            Self::LargePressure => "large-pressure",
        }
    }

    pub(crate) fn fixture_path(self) -> String {
        format!("{FIXTURE_ROOT}/{}.json", self.slug())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CorpusFamily {
    ClinicSmall,
    ClinicOvernight,
    RollingHours,
    SpecialistCoverage,
    RepairCallout,
    InfeasibleCoverage,
    Dst,
    LargeSynthetic,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum WorkloadClass {
    A,
    B,
    C,
    D,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ExpectedDisposition {
    Accepted,
    Infeasible,
    CompileRejected,
    ResourceLimit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum PreservedObligation {
    Rule {
        rule_id: RuleId,
        rule_kind: String,
    },
    Preference {
        rule_id: RuleId,
        preference_kind: String,
    },
    Lock {
        assignment_id: AssignmentId,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelEnvelope {
    pub minimum_variables: u32,
    pub maximum_variables: u32,
    pub maximum_constraints: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScoreExpectation {
    pub feasibility: i64,
    pub minimum_stable_rank: i64,
    pub maximum_stable_rank: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExpectedCase {
    pub id: CaseId,
    pub disposition: ExpectedDisposition,
    pub people: u32,
    pub horizon_days: u32,
    pub resolved_shifts: u32,
    pub selected_assignments: Option<u32>,
    pub score: Option<ScoreExpectation>,
    pub model_envelope: Option<ModelEnvelope>,
    pub deferred_obligations: Vec<PreservedObligation>,
    /// Reviewed descriptions, not an executable assertion language.
    pub semantics: Vec<String>,
}

pub(crate) struct FixtureDefinition {
    pub id: CaseId,
    pub family: Option<CorpusFamily>,
    pub workload_class: WorkloadClass,
    pub snapshot: ScenarioSnapshotV1,
    pub expected: ExpectedCase,
}

pub(crate) struct CorpusDefinitions {
    pub fixtures: Vec<FixtureDefinition>,
    pub repair: RepairFixture,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LargeParameters {
    pub people: u16,
    pub shifts: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RepairFixture {
    pub format: String,
    pub schema_version: u32,
    pub corpus_version: u32,
    pub before_case: CaseId,
    pub execution_phase: u32,
    pub baseline_requirement: String,
    pub command_id: String,
    pub payload: EntityPayload,
    pub repair_requirement: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FileBinding {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GeneratorIdentity {
    pub method: String,
    pub source_sha256: String,
    pub sources: Vec<FileBinding>,
    pub seed: u64,
    pub license: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MeasurementProfile {
    pub budget_milliseconds: u64,
    pub mode: SolveMode,
    pub random_seed: u64,
    pub worker_threads: u16,
    pub stop_after_first_feasible: bool,
    pub first_operation_samples: u16,
    pub warmup_runs: u16,
    pub post_warmup_samples: u16,
}

impl MeasurementProfile {
    pub(crate) const REVIEWED: Self = Self {
        budget_milliseconds: 60_000,
        mode: SolveMode::Deep,
        random_seed: 1,
        worker_threads: 1,
        stop_after_first_feasible: true,
        first_operation_samples: 1,
        warmup_runs: 1,
        post_warmup_samples: 3,
    };
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CaseManifest {
    pub id: CaseId,
    pub family: Option<CorpusFamily>,
    pub workload_class: WorkloadClass,
    pub input: FileBinding,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CorpusManifest {
    pub format: String,
    pub schema_version: u32,
    pub corpus_version: u32,
    pub generator: GeneratorIdentity,
    pub fixture_clock: String,
    pub profile: MeasurementProfile,
    pub cases: Vec<CaseManifest>,
    pub expectations: FileBinding,
    pub repair: FileBinding,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExpectedCorpus {
    pub format: String,
    pub schema_version: u32,
    pub corpus_version: u32,
    pub generator_source_sha256: String,
    pub cases: Vec<ExpectedCaseBinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExpectedCaseBinding {
    pub input_sha256: String,
    pub expected: ExpectedCase,
}

pub(crate) struct LoadedFixture {
    pub manifest: CaseManifest,
    pub expected: ExpectedCase,
    /// The same bounded, hashed bytes that were decoded. CLI staging uses these bytes.
    pub bytes: Vec<u8>,
    pub snapshot: ScenarioSnapshotV1,
}

pub(crate) struct LoadedCorpus {
    pub manifest: CorpusManifest,
    pub manifest_sha256: String,
    pub fixtures: Vec<LoadedFixture>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CliEvidence {
    pub format: String,
    pub schema_version: u32,
    pub corpus_version: u32,
    pub corpus_sha256: String,
    pub cli_sha256: String,
    pub worker_manifest_sha256: String,
    pub checks: Vec<CliCheck>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CliCheck {
    pub case_id: CaseId,
    pub operation: String,
    pub exit_code: i32,
    pub observed_status: Option<SolveStatus>,
    pub observed_code: Option<String>,
    pub artifact_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RunnerProcessState {
    FirstOperation,
    PostWarmup,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TimingMeasurements {
    pub milliseconds: std::collections::BTreeMap<String, u64>,
    pub unavailable: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelMeasurements {
    pub variables: u64,
    pub constraints: u64,
    pub canonical_ir_blake3: String,
    pub domain_filter_counts: std::collections::BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BackendMeasurements {
    pub backend_id: String,
    pub termination: String,
    pub remaining_at_dispatch_milliseconds: u64,
    pub backend_limit_milliseconds: u64,
    pub versions: std::collections::BTreeMap<String, String>,
    pub timings: TimingMeasurements,
    pub translated_variables: u64,
    pub translated_constraints: u64,
    pub applied_parameters_sha256: String,
    pub model_fingerprint_sha256: String,
    pub backend_objective_values: Option<Vec<i64>>,
    pub backend_best_bound_values: Option<Vec<i64>>,
    pub infeasibility_core: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AcceptanceMeasurements {
    pub portable_sha256: String,
    pub selected_assignments: u32,
    pub feasibility: i64,
    pub stable_rank: i64,
    pub consumer_checks: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BenchmarkSample {
    pub case_id: CaseId,
    pub runner_process_state: RunnerProcessState,
    pub ordinal: u16,
    pub status: SolveStatus,
    pub preparation_error_code: Option<String>,
    pub model: Option<ModelMeasurements>,
    pub timings: TimingMeasurements,
    pub backend: Option<BackendMeasurements>,
    pub acceptance: Option<AcceptanceMeasurements>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SampleBatch {
    pub schema_version: u32,
    pub corpus_sha256: String,
    pub worker_manifest_sha256: String,
    pub runner_sha256: String,
    pub samples: Vec<BenchmarkSample>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TimingAggregate {
    pub case_id: CaseId,
    pub runner_process_state: RunnerProcessState,
    pub metric: String,
    pub samples: u16,
    pub minimum_milliseconds: u64,
    pub median_milliseconds: u64,
    pub maximum_milliseconds: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunnerIdentity {
    pub source_sha256: String,
    pub executable_sha256: String,
    pub target: String,
    pub rustc: String,
    pub build_profile: String,
    pub runner_class: String,
    pub logical_parallelism: Option<u32>,
    /// Checkout observation, not an attestation of the executable's complete build graph.
    pub checkout_commit: String,
    pub checkout_dirty: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BenchmarkEvidence {
    pub format: String,
    pub schema_version: u32,
    pub corpus_version: u32,
    pub corpus_sha256: String,
    pub worker_manifest_sha256: String,
    pub runner: RunnerIdentity,
    pub profile: MeasurementProfile,
    pub worker_lifecycle: String,
    pub os_cache_state: String,
    pub enrichment_state: String,
    pub evidence_class: String,
    pub aggregation_method: String,
    pub variance_policy: String,
    pub samples: Vec<BenchmarkSample>,
    pub aggregates: Vec<TimingAggregate>,
}

//! Headless command-line adapter over [`eutheto_core::EuthetoApp`].

mod bundled_solver;
mod files;
mod native_file;
mod progress;
mod scenario_files;
mod solutions;
mod solve;

use clap::{Args, Parser, Subcommand, ValueEnum, error::ErrorKind};
use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult,
    AppliedPortableScenario, BackupAssetSelection, BackupSelection, DeferredCapability, EuthetoApp,
    FoundationStatusService, ProjectScope, SOLUTION_API_SCHEMA_VERSION, SolutionCompareRequestV1,
    SolutionExplainRequestV1, SolutionListRequestV1, SolutionSummaryRequestV1,
    SolutionVerifyRequestV1, SolverSupportMatrixMetadata, SupportCell,
};
use eutheto_domain_ir::{
    AcceptedResultRefV1, DomainAssignmentId, ExplanationRequestSubjectV1, ExplanationRequestV1,
};
use eutheto_export::FixedExclusion;
use eutheto_import::{
    CollisionAction, CollisionPlan, ImportOptions, PreservedBundleMetadata, RestoreAuthorization,
    RestoreMode, SafetyBackupEvidence,
};
use eutheto_types::{
    ActorRef, AppError, BackendId, CancellationToken, CommandBatch, CommandEnvelope, CommandId,
    CommandSource, DomainPackRef, GapPolicy, Horizon, IanaTimeZone, LocaleTag, PackId, RequestId,
    Revision, ScenarioCommand, ScenarioId, ScenarioSettings, SolutionId, SystemClock,
    SystemIdGenerator, UnitSystem,
};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value, json};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const API_VERSION: &str = "eutheto/cli-result/v1";
const COMMAND_JSON_LIMIT: u64 = 16 * 1024 * 1024;
const BUNDLE_LIMIT: u64 = 64 * 1024 * 1024;
const HUMAN_SOLUTION_LIST_LIMIT: usize = 20;

/// Stable process exit codes for the working CLI contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CliExitCode {
    Success = 0,
    Application = 1,
    Usage = 2,
    Validation = 3,
    Infeasible = 4,
    NoVerifiedSolution = 5,
    Unavailable = 6,
    Verification = 7,
    Storage = 8,
    ArtificialIntelligence = 9,
    Conflict = 10,
    Cancelled = 130,
}

impl CliExitCode {
    #[must_use]
    pub const fn value(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    #[default]
    Human,
    Json,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum ProjectScopeArg {
    #[default]
    Active,
    Archived,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum UnitsArg {
    Metric,
    UsCustomary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum RestoreModeArg {
    Add,
    Replace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum SolveModeArg {
    Quick,
    Balanced,
    Deep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ProgressArg {
    Human,
    Jsonl,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum SolutionExportFormat {
    Csv,
    Ics,
    Svg,
    Html,
    Pdf,
    Json,
}

#[derive(Debug, Parser)]
#[command(
    name = "optimizer",
    version,
    about = "Provisional development CLI for Eutheto",
    long_about = "Provisional development-only CLI. The optimizer name and .eutheto extension are not final public identities."
)]
struct Cli {
    /// Select human-readable output or one versioned JSON result envelope.
    #[arg(long, value_enum, default_value = "human")]
    format: OutputFormat,
    /// Select diagnostic verbosity. Diagnostics never use stdout.
    #[arg(long, value_enum, default_value = "info")]
    log_level: LogLevel,
    /// Disable terminal colors.
    #[arg(long)]
    no_color: bool,
    /// Use this directory for the application database and private safety backups.
    #[arg(long, value_name = "PATH")]
    data_dir: Option<PathBuf>,
    /// Use an explicit configuration path (reserved; no Phase-01 settings are read from it).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Prohibit network use. Phase-01 operations are already local-only.
    #[arg(long)]
    offline: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Report whether the local Phase-01 application can open.
    Doctor,
    /// Report build and application capability information.
    Info,
    /// Report bundled project licensing information.
    Licenses,
    /// Retained truthful alias for Phase-01 capability information.
    Status(StatusArgs),
    /// Inspect the domain-pack catalog.
    Packs(PacksArgs),
    /// Safely inspect or exactly re-export an unopened portable bundle.
    Bundle(BundleArgs),
    /// Manage persisted projects and portable scenario bundles.
    Projects(ProjectsArgs),
    /// Inspect, create, and restore full-library backups.
    Backup(BackupArgs),
    /// View, validate, mutate, and inspect scenario history.
    Scenario(ScenarioArgs),
    /// Read and mutate application settings.
    Settings(SettingsArgs),
    /// Optimize a stored scenario or an explicit portable scenario file.
    Solve(SolveArgs),
    /// Inspect and independently verify accepted solutions.
    Solutions(SolutionsArgs),
    /// Inspect registered solver backends and the Phase-02 support matrix.
    Solvers(SolversArgs),
    /// Optional AI commands (catalogued but unavailable in Phase 01).
    Ai(AiArgs),
    /// Post-MVP authenticated service mode (unavailable).
    Serve,
}

#[derive(Debug, Args)]
struct StatusArgs {
    /// Legacy spelling for `--format json`; output still uses the CLI result envelope.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct PacksArgs {
    #[command(subcommand)]
    command: PacksCommand,
}
#[derive(Debug, Subcommand)]
enum PacksCommand {
    /// List compiled-in domain packs.
    List,
    /// Describe an installed pack.
    Describe { pack_id: String },
}

#[derive(Debug, Args)]
struct BundleArgs {
    #[command(subcommand)]
    command: BundleCommand,
}

#[derive(Debug, Subcommand)]
enum BundleCommand {
    /// Inspect allowlisted metadata without importing or accepting the bundle.
    Inspect {
        #[arg(value_name = "BUNDLE")]
        bundle: PathBuf,
    },
    /// Inspect and atomically publish the exact original bytes without overwriting.
    ExactReexport {
        #[arg(value_name = "BUNDLE")]
        bundle: PathBuf,
        #[arg(value_name = "OUTPUT")]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct ProjectsArgs {
    #[command(subcommand)]
    command: ProjectsCommand,
}

#[derive(Debug, Subcommand)]
enum ProjectsCommand {
    /// List persisted projects.
    List {
        #[arg(long, value_enum, default_value = "active")]
        scope: ProjectScopeArg,
    },
    /// Create a project using explicit, deterministic scenario settings.
    Create(CreateProjectArgs),
    /// Duplicate a project and its authoritative scenario.
    Duplicate {
        source_id: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        title: String,
    },
    /// Archive a project without deleting it.
    Archive {
        scenario_id: String,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Return an archived project to the active list.
    Unarchive {
        scenario_id: String,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Preview and atomically import a portable scenario bundle.
    Import {
        bundle: PathBuf,
        /// Exclude bundled retained results after reviewing the import selection.
        #[arg(long)]
        exclude_results: bool,
        /// Exclude bundled referenced assets after reviewing the import selection.
        #[arg(long)]
        exclude_assets: bool,
        /// Exact JSON: `{"scenarios":{"UUID":"create-copy"},"supplementalChoices":[{"section":"assets","key":"logo.png","action":"skip"}]}`.
        #[arg(long)]
        collision_plan: Option<String>,
    },
    /// Export a persisted scenario as a portable bundle.
    Export {
        scenario_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Permanently delete a project.
    Delete {
        scenario_id: String,
        #[arg(long)]
        expected_revision: u64,
    },
}

#[derive(Debug, Args)]
struct CreateProjectArgs {
    #[arg(long)]
    pack: String,
    #[arg(long, default_value_t = 1)]
    pack_schema: u32,
    #[arg(long)]
    title: String,
    #[arg(long, default_value = "")]
    description: String,
    #[arg(long, default_value = "UTC")]
    time_zone: String,
    #[arg(long, default_value = "en-US")]
    locale: String,
    #[arg(long, value_enum, default_value = "metric")]
    units: UnitsArg,
    #[arg(long, default_value = "2020-01-01T00:00:00Z")]
    horizon_start: String,
    #[arg(long, default_value = "2100-01-01T00:00:00Z")]
    horizon_end: String,
    /// Create a standalone portable scenario without opening the project database.
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct BackupArgs {
    #[command(subcommand)]
    command: BackupCommand,
}

#[derive(Debug, Subcommand)]
enum BackupCommand {
    /// Safely inspect and preview a full backup without mutation.
    Inspect {
        bundle: PathBuf,
        /// Build removal scope against the current library for this restore mode.
        #[arg(long, value_enum, default_value = "add")]
        mode: RestoreModeArg,
    },
    /// Create a full-library portable backup.
    Create {
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = "Eutheto backup")]
        title: String,
        #[arg(long)]
        exclude_results: bool,
        /// Exclude only assets above the version-1 portable large-asset threshold.
        #[arg(long)]
        exclude_large_assets: bool,
    },
    /// Preview and atomically restore a full backup.
    Restore {
        bundle: PathBuf,
        #[arg(long, value_enum)]
        mode: RestoreModeArg,
        /// Exact JSON: `{"scenarios":{"UUID":"replace"},"supplementalChoices":[{"section":"assets","key":"logo.png","action":"skip"}]}`; replace mode requires both collections empty.
        #[arg(long)]
        collision_plan: Option<String>,
        /// Required acknowledgement for replace-library mode.
        #[arg(long)]
        confirm_replace: bool,
        /// Token emitted by a prior review-only invocation for this exact preview and plan.
        #[arg(long, value_name = "TOKEN")]
        review_token: Option<String>,
        /// Receipt emitted only after an attempted safety backup actually failed.
        #[arg(long, value_name = "TOKEN")]
        without_backup_token: Option<String>,
    },
}

#[derive(Debug, Args)]
struct ScenarioArgs {
    #[command(subcommand)]
    command: ScenarioCommandArgs,
}

#[derive(Debug, Subcommand)]
enum ScenarioCommandArgs {
    /// Open and print a stored scenario or an explicit portable scenario file.
    Show { input: String },
    /// Standalone migration command (unavailable; checked loading handles supported migrations).
    Migrate {
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Fully validate a stored scenario or an explicit portable scenario file.
    Validate { input: String },
    /// Apply one strict command JSON document, or an array as one atomic batch.
    Apply(ApplyArgs),
    /// Apply a strict JSON array as one atomic command batch.
    Batch(ApplyArgs),
    /// Undo the currently applied history head.
    Undo {
        scenario_id: String,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Redo the next command on the current history branch.
    Redo {
        scenario_id: String,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Print the complete persisted command journal.
    History { scenario_id: String },
}

#[derive(Debug, Args)]
struct ApplyArgs {
    input: String,
    /// Path to a strict JSON command envelope, command object, or command array.
    #[arg(long)]
    commands: PathBuf,
    /// Required for command objects/arrays; full envelopes carry their own revision.
    #[arg(long)]
    expected_revision: Option<u64>,
    /// Export the committed scenario bundle to this explicit path.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Explicitly discard a redo branch after a new command.
    #[arg(long)]
    truncate_redo: bool,
}

#[derive(Debug, Args)]
struct SettingsArgs {
    #[command(subcommand)]
    command: SettingsCommand,
}

#[derive(Debug, Subcommand)]
enum SettingsCommand {
    Get {
        key: String,
    },
    Set {
        key: String,
        /// Strict inline JSON value.
        value: String,
    },
    Delete {
        key: String,
    },
}

#[derive(Debug, Args)]
struct SolveArgs {
    #[command(subcommand)]
    command: Option<SolveCommand>,
    #[arg(value_name = "INPUT")]
    input: Option<PathBuf>,
    #[arg(long)]
    backend: Option<String>,
    #[arg(long, value_enum)]
    mode: Option<SolveModeArg>,
    #[arg(long)]
    max_time: Option<String>,
    #[arg(long)]
    threads: Option<u16>,
    #[arg(long)]
    seed: Option<u64>,
    #[arg(long)]
    first_feasible: bool,
    #[arg(long)]
    repair_from: Option<PathBuf>,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long)]
    include_diagnostics: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "none")]
    progress: ProgressArg,
}

#[derive(Debug, Args)]
struct AiArgs {
    /// Reserved later-phase AI command and arguments.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    arguments: Vec<OsString>,
}

#[derive(Debug, Subcommand)]
enum SolveCommand {
    /// Future service-mode cancellation command.
    Cancel { job_id: String },
}

#[derive(Debug, Args)]
struct SolutionsArgs {
    #[command(subcommand)]
    command: SolutionsCommand,
}

#[derive(Debug, Subcommand)]
enum SolutionsCommand {
    List {
        #[arg(value_name = "SCENARIO")]
        scenario: String,
    },
    Verify {
        #[arg(value_name = "SCENARIO")]
        scenario: String,
        #[arg(value_name = "SOLUTION")]
        solution: String,
    },
    Compare {
        #[arg(value_name = "SCENARIO")]
        scenario: String,
        #[arg(value_name = "BASE_SOLUTION")]
        solution_a: String,
        #[arg(value_name = "CANDIDATE_SOLUTION")]
        solution_b: String,
    },
    Explain {
        #[arg(value_name = "SCENARIO")]
        scenario: String,
        #[arg(value_name = "SOLUTION")]
        solution: String,
        /// Stable assignment identity to explain within the accepted solution.
        #[arg(long = "assignment-id", value_name = "ASSIGNMENT")]
        assignment: String,
    },
    Export(ExportSolutionArgs),
}

#[derive(Debug, Args)]
struct ExportSolutionArgs {
    scenario: PathBuf,
    solution: PathBuf,
    #[arg(long, value_enum)]
    format: SolutionExportFormat,
    /// Publish to an explicit no-clobber destination; otherwise human mode writes raw stdout.
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct SolversArgs {
    #[command(subcommand)]
    command: SolversCommand,
}

#[derive(Debug, Subcommand)]
enum SolversCommand {
    List,
    Describe { solver_id: String },
    Check { solver_id: String },
}

/// Pure support-matrix rendering used by the `solvers list` adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SolverSupportMatrixPresentation {
    pub json: Value,
    pub human: Vec<String>,
}

/// Renders validated core metadata without providing a registry or solver-injection path.
#[must_use]
pub fn present_solver_support_matrix(
    matrix: &SolverSupportMatrixMetadata,
) -> SolverSupportMatrixPresentation {
    let mut human = vec![format!(
        "Support matrix: schema {}, planning IR schema {}, {} features, {} production backends.",
        matrix.schema_version,
        matrix.planning_ir_schema_version,
        matrix.features.len(),
        matrix.backend_columns.len(),
    )];
    let backend_columns = matrix
        .backend_columns
        .iter()
        .map(|column| {
            let cells = column
                .cells
                .iter()
                .map(|(feature_id, cell)| match cell {
                    SupportCell::Supported { fixture_id } => json!({
                        "featureId": feature_id,
                        "support": "supported",
                        "fixtureId": fixture_id,
                    }),
                    SupportCell::Degraded {
                        restriction_id,
                        reason,
                        remediation,
                        fixture_id,
                    } => {
                        human.push(format!(
                            "Warning: backend {} feature {} is degraded ({}): {} Remediation: {} Fixture: {}.",
                            column.backend_id,
                            feature_id,
                            restriction_id,
                            reason,
                            remediation,
                            fixture_id,
                        ));
                        json!({
                            "featureId": feature_id,
                            "support": "degraded",
                            "restrictionId": restriction_id,
                            "reason": reason,
                            "remediation": remediation,
                            "fixtureId": fixture_id,
                        })
                    }
                    SupportCell::Unsupported {
                        reason,
                        remediation,
                        fixture_id,
                    } => {
                        human.push(format!(
                            "Warning: backend {} feature {} is unsupported: {} Remediation: {} Fixture: {}.",
                            column.backend_id,
                            feature_id,
                            reason,
                            remediation,
                            fixture_id,
                        ));
                        json!({
                            "featureId": feature_id,
                            "support": "unsupported",
                            "reason": reason,
                            "remediation": remediation,
                            "fixtureId": fixture_id,
                        })
                    }
                })
                .collect::<Vec<_>>();
            json!({
                "backendId": column.backend_id,
                "backendVersion": column.backend_version,
                "adapterVersion": column.adapter_version,
                "cells": cells,
            })
        })
        .collect::<Vec<_>>();
    SolverSupportMatrixPresentation {
        json: json!({
            "schemaVersion": matrix.schema_version,
            "planningIrSchemaVersion": matrix.planning_ir_schema_version,
            "featureCount": matrix.features.len(),
            "productionBackendCount": matrix.backend_columns.len(),
            "features": matrix.features,
            "productionBackendIds": matrix.production_backend_ids,
            "backendColumns": backend_columns,
        }),
        human,
    }
}

struct Outcome {
    command: &'static str,
    status: &'static str,
    result: Value,
    human: Vec<String>,
    warnings: Vec<SafeCliWarning>,
    exit: CliExitCode,
    raw: Option<Vec<u8>>,
}

impl Outcome {
    fn new(command: &'static str, status: &'static str, result: Value, human: Vec<String>) -> Self {
        Self {
            command,
            status,
            result,
            human,
            warnings: Vec::new(),
            exit: CliExitCode::Success,
            raw: None,
        }
    }

    fn with_warning(mut self, warning: SafeCliWarning) -> Self {
        self.warnings.push(warning);
        self
    }

    fn with_exit(mut self, exit: CliExitCode) -> Self {
        self.exit = exit;
        self
    }

    fn with_raw(mut self, bytes: Vec<u8>) -> Self {
        self.raw = Some(bytes);
        self
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SafeCliWarning {
    code: String,
    message: String,
    details: Option<Value>,
}

impl SafeCliWarning {
    fn artifact_publication(error: AppError) -> Self {
        let cause = app_error(error);
        Self {
            code: "scenario.output_publication_failed".to_owned(),
            message: "The scenario command committed, but its requested output artifact could not be published. Do not reapply the command.".to_owned(),
            details: Some(json!({"causeCode": cause.code.clone()})),
        }
    }
}

#[derive(Debug)]
struct SafeCliError(Box<SafeCliErrorData>);

#[derive(Debug)]
struct SafeCliErrorData {
    exit: CliExitCode,
    code: String,
    message: String,
    details: Option<Value>,
    human_details: Vec<String>,
    diagnostic_id: Option<String>,
}

impl std::ops::Deref for SafeCliError {
    type Target = SafeCliErrorData;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for SafeCliError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl SafeCliError {
    fn new(exit: CliExitCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self(Box::new(SafeCliErrorData {
            exit,
            code: code.into(),
            message: message.into(),
            details: None,
            human_details: Vec::new(),
            diagnostic_id: None,
        }))
    }

    fn validation(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(CliExitCode::Validation, code, message)
    }

    fn storage(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(CliExitCode::Storage, code, message)
    }

    fn cancelled() -> Self {
        Self::new(
            CliExitCode::Cancelled,
            "operation.cancelled",
            "The operation was cancelled.",
        )
    }

    fn unavailable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(CliExitCode::Unavailable, code, message)
    }
}

/// Parse and execute one CLI invocation, writing data only to `stdout` and diagnostics only to
/// `stderr`. The returned value is one stable process exit code.
pub fn run_from<I, T, O, E>(args: I, mut stdout: O, mut stderr: E) -> CliExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    O: Write,
    E: Write,
{
    let raw_args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let requested_json = requests_json(&raw_args);
    let mut cli = match Cli::try_parse_from(raw_args) {
        Ok(cli) => cli,
        Err(error) => {
            if error.use_stderr() {
                let safe = SafeCliError::new(
                    CliExitCode::Usage,
                    "cli.usage",
                    "Invalid command-line arguments; use --help for the command catalog.",
                );
                if requested_json {
                    let _ = write_error_json(&mut stderr, "usage", &safe);
                } else {
                    let _ = writeln!(stderr, "optimizer: {}: {}", safe.code, safe.message);
                }
                return CliExitCode::Usage;
            }
            if error.exit_code() == 0 {
                if requested_json {
                    let command = match error.kind() {
                        ErrorKind::DisplayVersion => "version",
                        _ => "help",
                    };
                    let outcome = Outcome::new(
                        command,
                        "displayed",
                        json!({"text": error.to_string()}),
                        Vec::new(),
                    );
                    let _ = render_success(&mut stdout, &mut stderr, OutputFormat::Json, &outcome);
                } else {
                    let _ = write!(stdout, "{error}");
                }
                return CliExitCode::Success;
            }
            let _ = write!(stderr, "{error}");
            return CliExitCode::Usage;
        }
    };
    if matches!(&cli.command, Command::Status(StatusArgs { json: true })) {
        cli.format = OutputFormat::Json;
    }
    let format = cli.format;
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        let error = SafeCliError::new(
            CliExitCode::Application,
            "cli.runtime_unavailable",
            "The command runtime could not be started.",
        );
        let _ = render_error(&mut stdout, &mut stderr, format, "startup", &error);
        return error.exit;
    };
    let cancellation = CancellationToken::new();
    let interrupt_signal = cancellation.clone();
    let result = runtime.block_on(async {
        let watcher = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                interrupt_signal.cancel();
            }
        });
        let result = progress::run(cli, cancellation, &mut stderr).await;
        watcher.abort();
        result
    });
    match result {
        Ok(outcome) => {
            if render_success(&mut stdout, &mut stderr, format, &outcome).is_err() {
                CliExitCode::Application
            } else {
                outcome.exit
            }
        }
        Err((command, error)) => {
            let _ = render_error(&mut stdout, &mut stderr, format, command, &error);
            error.exit
        }
    }
}

fn requests_json(args: &[OsString]) -> bool {
    args.windows(2).any(|pair| {
        pair[0] == OsStr::new("--format")
            && pair[1]
                .to_str()
                .is_some_and(|value| value.eq_ignore_ascii_case("json"))
    }) || args.iter().any(|arg| arg == OsStr::new("--format=json"))
        || args
            .iter()
            .position(|arg| arg == OsStr::new("status"))
            .is_some_and(|status| {
                args[status.saturating_add(1)..]
                    .iter()
                    .any(|arg| arg == OsStr::new("--json"))
            })
}

async fn execute_cli(
    cli: Cli,
    cancellation: CancellationToken,
    progress: &mut dyn eutheto_solver_api::ProgressSink,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let _ = (cli.log_level, cli.no_color, &cli.config, cli.offline);
    match cli.command {
        Command::Info => Ok(info_outcome("info")),
        Command::Status(_) => Ok(info_outcome("status")),
        Command::Licenses => Ok(Outcome::new(
            "licenses",
            "available",
            json!({"packages": [{"name": "eutheto-cli", "license": "Apache-2.0"}]}),
            vec!["eutheto-cli: Apache-2.0".to_owned()],
        )),
        Command::Packs(args) => {
            let app = open_app(cli.data_dir, &cancellation)
                .await
                .map_err(|error| ("packs", error))?;
            execute_packs(&app, args).await
        }
        Command::Serve => Err((
            "serve",
            SafeCliError::unavailable(
                "feature.serve_post_mvp",
                "Service mode is a post-MVP capability and is unavailable.",
            ),
        )),
        Command::Solutions(args) => {
            solutions::execute(
                cli.data_dir,
                args,
                &cancellation,
                cli.format,
                cli.config.is_some(),
            )
            .await
        }
        Command::Solvers(args) => {
            let app = open_app(cli.data_dir, &cancellation)
                .await
                .map_err(|error| ("solvers", error))?;
            execute_solvers(&app, args).await
        }
        Command::Ai(_) => {
            execute_deferred(
                cli.data_dir,
                "ai",
                DeferredCapability::ArtificialIntelligence,
                &cancellation,
            )
            .await
        }
        Command::Solve(args) => {
            solve::execute(
                cli.data_dir,
                args,
                &cancellation,
                cli.config.is_some(),
                progress,
            )
            .await
        }
        Command::Bundle(args) => {
            let app = open_app(cli.data_dir, &cancellation)
                .await
                .map_err(|error| ("bundle", error))?;
            execute_bundle(&app, args, &cancellation).await
        }
        Command::Doctor => execute_doctor(cli.data_dir, &cancellation).await,
        Command::Projects(args) => {
            execute_projects_cli(cli.data_dir, args, &cancellation, cli.config.is_some()).await
        }
        Command::Backup(args) => {
            let app = open_app(cli.data_dir, &cancellation)
                .await
                .map_err(|error| ("backup", error))?;
            execute_backup(&app, args, &cancellation).await
        }
        Command::Scenario(args) => {
            scenario_files::execute(cli.data_dir, args, &cancellation, cli.config.is_some()).await
        }
        Command::Settings(args) => {
            let app = open_app(cli.data_dir, &cancellation)
                .await
                .map_err(|error| ("settings", error))?;
            execute_settings(&app, args).await
        }
    }
}

async fn execute_doctor(
    data_dir: Option<PathBuf>,
    cancellation: &CancellationToken,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    open_app(data_dir, cancellation)
        .await
        .map_err(|error| ("doctor", error))?;
    Ok(Outcome::new(
        "doctor",
        "ready",
        json!({"ready": true, "capability": "phase_01_core"}),
        vec!["Phase-01 application storage is ready.".to_owned()],
    ))
}

async fn execute_projects_cli(
    data_dir: Option<PathBuf>,
    args: ProjectsArgs,
    cancellation: &CancellationToken,
    has_config: bool,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let command = match args.command {
        ProjectsCommand::Create(create) if create.output.is_some() => {
            scenario_files::reject_config(has_config)
                .map_err(|error| ("projects.create", error))?;
            return scenario_files::create(create, cancellation);
        }
        command => command,
    };
    let args = ProjectsArgs { command };
    if cancellation.is_cancelled() && matches!(&args.command, ProjectsCommand::Export { .. }) {
        return Err(("projects.export", SafeCliError::cancelled()));
    }
    let app = open_app(data_dir, cancellation)
        .await
        .map_err(|error| ("projects", error))?;
    execute_projects(&app, args, cancellation).await
}

fn info_outcome(command: &'static str) -> Outcome {
    let service = FoundationStatusService::current();
    let status = service.status();
    let status_value = match serde_json::to_value(status) {
        Ok(value) => value,
        Err(_) => Value::Null,
    };
    let package_name = service.package_name();
    let package_version = service.package_version();
    let schema_version = status.schema_version;
    let capability = status.capability;
    Outcome::new(
        command,
        "available",
        status_value,
        vec![
            "optimizer (provisional development name)".to_owned(),
            format!("foundation: {package_name} {package_version}"),
            format!("schema version: {schema_version}"),
            format!("capability: {capability}"),
        ],
    )
}

async fn execute_packs(
    app: &EuthetoApp,
    args: PacksArgs,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    match args.command {
        PacksCommand::List => {
            let result = app
                .query(AppQuery::ListDomainPacks)
                .await
                .map_err(|error| ("packs.list", app_error(error)))?;
            let AppQueryResult::DomainPacks(packs) = result else {
                return Err(("packs.list", unexpected_result()));
            };
            let human = if packs.is_empty() {
                vec!["No domain packs are registered.".to_owned()]
            } else {
                packs
                    .iter()
                    .map(|pack| {
                        format!(
                            "{} (pack {}, scenario schema {})",
                            pack.id, pack.pack_version, pack.scenario_versions.latest
                        )
                    })
                    .collect()
            };
            Ok(Outcome::new(
                "packs.list",
                "available",
                json!({"packs": packs}),
                human,
            ))
        }
        PacksCommand::Describe { pack_id } => {
            let pack_id = pack_id.parse::<PackId>().map_err(|_| {
                (
                    "packs.describe",
                    SafeCliError::validation(
                        "pack.id_invalid",
                        "The domain-pack identifier is invalid.",
                    ),
                )
            })?;
            let result = app
                .query(AppQuery::DescribeDomainPack(pack_id))
                .await
                .map_err(|error| ("packs.describe", app_error(error)))?;
            let AppQueryResult::DomainPack(metadata) = result else {
                return Err(("packs.describe", unexpected_result()));
            };
            let metadata = *metadata;
            let descriptor = metadata.descriptor;
            let catalog = metadata.catalog;
            let human = vec![
                format!("{} (pack {})", descriptor.id, descriptor.pack_version),
                format!(
                    "Scenario schemas: latest {}, migratable from {}.",
                    descriptor.scenario_versions.latest,
                    descriptor.scenario_versions.migratable_from.len()
                ),
                format!(
                    "Catalog: {} commands, {} AI tools, {} setup steps, {} entity kinds, {} rule kinds, {} goal kinds, {} result views.",
                    catalog.commands.len(),
                    catalog.ai_tools.len(),
                    catalog.ui.setup_steps.len(),
                    catalog.ui.entity_kinds.len(),
                    catalog.ui.rule_kinds.len(),
                    catalog.ui.goal_kinds.len(),
                    catalog.ui.result_views.len(),
                ),
            ];
            Ok(Outcome::new(
                "packs.describe",
                "available",
                json!({"descriptor": descriptor, "catalog": catalog}),
                human,
            ))
        }
    }
}

async fn execute_solvers(
    app: &EuthetoApp,
    args: SolversArgs,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    match args.command {
        SolversCommand::List => {
            let AppQueryResult::Solvers(solvers) = app
                .query(AppQuery::ListSolvers)
                .await
                .map_err(|error| ("solvers.list", app_error(error)))?
            else {
                return Err(("solvers.list", unexpected_result()));
            };
            let AppQueryResult::SolverSupportMatrix(matrix) = app
                .query(AppQuery::SolverSupportMatrix)
                .await
                .map_err(|error| ("solvers.list", app_error(error)))?
            else {
                return Err(("solvers.list", unexpected_result()));
            };
            let AppQueryResult::DeferredSolverGates(deferred_gates) = app
                .query(AppQuery::DeferredSolverGates)
                .await
                .map_err(|error| ("solvers.list", app_error(error)))?
            else {
                return Err(("solvers.list", unexpected_result()));
            };
            let mut human = if solvers.is_empty() {
                vec!["Production solvers: none.".to_owned()]
            } else {
                solvers
                    .iter()
                    .map(|solver| {
                        format!(
                            "{} {} (adapter {})",
                            solver.id, solver.version, solver.adapter_version
                        )
                    })
                    .collect()
            };
            let matrix_presentation = present_solver_support_matrix(&matrix);
            human.extend(matrix_presentation.human);
            human.extend(deferred_gates.iter().map(|gate| {
                format!(
                    "Deferred unclaimed candidate: {} {} (phase {}).",
                    gate.backend_id, gate.candidate_version, gate.owning_phase
                )
            }));
            Ok(Outcome::new(
                "solvers.list",
                "available",
                json!({
                    "solvers": solvers,
                    "supportMatrix": matrix_presentation.json,
                    "deferredGates": deferred_gates,
                }),
                human,
            ))
        }
        SolversCommand::Describe { solver_id } => {
            describe_solver(app, solver_id, "solvers.describe", false).await
        }
        SolversCommand::Check { solver_id } => {
            describe_solver(app, solver_id, "solvers.check", true).await
        }
    }
}

async fn execute_solution_list(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let AppQueryResult::SolutionList(list) = app
        .query(AppQuery::SolutionList(SolutionListRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id,
        }))
        .await
        .map_err(|error| ("solutions.list", app_error(error)))?
    else {
        return Err(("solutions.list", unexpected_result()));
    };
    let mut human = list
        .solutions
        .iter()
        .take(HUMAN_SOLUTION_LIST_LIMIT)
        .map(|solution| {
            format!(
                "{}\trevision {}{}{}",
                solution.result.solution_id,
                solution.scenario_revision.value(),
                if solution.stale { "\tstale" } else { "" },
                if solution.selected { "\tselected" } else { "" },
            )
        })
        .collect::<Vec<_>>();
    if list.solutions.is_empty() {
        human.push(format!("No accepted solutions for scenario {scenario_id}."));
    } else if list.solutions.len() > HUMAN_SOLUTION_LIST_LIMIT {
        human.push(format!(
            "{} additional accepted solutions omitted.",
            list.solutions.len() - HUMAN_SOLUTION_LIST_LIMIT
        ));
    }
    Ok(Outcome::new(
        "solutions.list",
        "listed",
        to_value(list).map_err(|error| ("solutions.list", error))?,
        human,
    ))
}

async fn execute_solution_verify(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    solution_id: SolutionId,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let AppQueryResult::SolutionVerification(verified) = app
        .query(AppQuery::SolutionVerify(SolutionVerifyRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id,
            solution_id,
        }))
        .await
        .map_err(|error| ("solutions.verify", app_error(error)))?
    else {
        return Err(("solutions.verify", unexpected_result()));
    };
    let mut warnings: Vec<SafeCliWarning> = verified
        .verification
        .warnings
        .iter()
        .map(|warning| SafeCliWarning {
            code: warning.id.to_string(),
            message: warning.message_key.clone(),
            details: Some(json!({
                "affectedEntities": warning.affected_entities.clone(),
                "facts": warning.facts.clone(),
            })),
        })
        .collect();
    warnings.extend(solutions::stale_warning(
        verified.scenario_revision,
        verified.current_revision,
    ));
    let human = vec![format!(
        "Solution {} independently verified for scenario {} revision {}.",
        verified.result.solution_id,
        verified.scenario_id,
        verified.scenario_revision.value(),
    )];
    let mut outcome = Outcome::new(
        "solutions.verify",
        "verified",
        to_value(&verified).map_err(|error| ("solutions.verify", error))?,
        human,
    );
    outcome.warnings = warnings;
    Ok(outcome)
}

async fn execute_solution_compare(
    app: &EuthetoApp,
    scenario: &str,
    solution_a: &str,
    solution_b: &str,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let scenario_id = scenario_id(scenario).map_err(|error| ("solutions.compare", error))?;
    let base_solution_id = solution_id(solution_a).map_err(|error| ("solutions.compare", error))?;
    let candidate_solution_id =
        solution_id(solution_b).map_err(|error| ("solutions.compare", error))?;
    let AppQueryResult::SolutionComparison(comparison) = app
        .query(AppQuery::SolutionCompare(SolutionCompareRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id,
            base_solution_id,
            candidate_solution_id,
        }))
        .await
        .map_err(|error| ("solutions.compare", app_error(error)))?
    else {
        return Err(("solutions.compare", unexpected_result()));
    };
    let detail = &comparison.comparison;
    let human = vec![
        format!(
            "Compared candidate {} with base {} for scenario {}.",
            detail.candidate.accepted_result.solution_id,
            detail.base.accepted_result.solution_id,
            comparison.scenario_id,
        ),
        format!(
            "{} assignment changes, {} required-rule changes, {} affected entities.",
            detail.assignments.len(),
            detail.rules.len(),
            detail.affected_entities.len(),
        ),
    ];
    Ok(Outcome::new(
        "solutions.compare",
        "compared",
        to_value(&comparison).map_err(|error| ("solutions.compare", error))?,
        human,
    ))
}

async fn execute_solution_explain(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    solution_id: SolutionId,
    assignment_id: DomainAssignmentId,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let AppQueryResult::SolutionSummary(detail) = app
        .query(AppQuery::SolutionGetSummary(SolutionSummaryRequestV1 {
            schema_version: SOLUTION_API_SCHEMA_VERSION,
            scenario_id,
            solution_id,
        }))
        .await
        .map_err(|error| ("solutions.explain", app_error(error)))?
    else {
        return Err(("solutions.explain", unexpected_result()));
    };
    let result = AcceptedResultRefV1::from_result(&detail.result)
        .map_err(|_| ("solutions.explain", unexpected_result()))?;
    let request = SolutionExplainRequestV1 {
        schema_version: SOLUTION_API_SCHEMA_VERSION,
        scenario_id,
        request: ExplanationRequestV1::new(ExplanationRequestSubjectV1::Assignment {
            result,
            assignment_id,
        })
        .map_err(|_| {
            (
                "solutions.explain",
                SafeCliError::validation(
                    "solution.explanation_request_invalid",
                    "The explanation request is invalid.",
                ),
            )
        })?,
    };
    let AppQueryResult::SolutionExplanation(explanation) = app
        .query(AppQuery::SolutionExplain(request))
        .await
        .map_err(|error| ("solutions.explain", app_error(error)))?
    else {
        return Err(("solutions.explain", unexpected_result()));
    };
    let human = solutions::explanation_human(&explanation.explanation.rendered)
        .map_err(|error| ("solutions.explain", error))?;
    let mut outcome = Outcome::new(
        "solutions.explain",
        "explained",
        to_value(&explanation).map_err(|error| ("solutions.explain", error))?,
        human,
    );
    for revision in &explanation.scenario_revisions {
        outcome.warnings.extend(solutions::stale_warning(
            *revision,
            explanation.current_revision,
        ));
    }
    Ok(outcome)
}

async fn describe_solver(
    app: &EuthetoApp,
    solver_id: String,
    command: &'static str,
    checked: bool,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let solver_id = solver_id.parse::<BackendId>().map_err(|_| {
        (
            command,
            SafeCliError::validation(
                "solver.id_invalid",
                "The solver-backend identifier is invalid.",
            ),
        )
    })?;
    let result = app
        .query(AppQuery::DescribeSolver(solver_id))
        .await
        .map_err(|error| (command, app_error(error)))?;
    let AppQueryResult::Solver(solver) = result else {
        return Err((command, unexpected_result()));
    };
    let human = if checked {
        vec![format!("{} is registered and available.", solver.id)]
    } else {
        vec![
            format!("{} {}", solver.id, solver.version),
            format!("Adapter version: {}.", solver.adapter_version),
        ]
    };
    Ok(Outcome::new(
        command,
        if checked { "available" } else { "described" },
        if checked {
            json!({"available": true, "solver": solver})
        } else {
            json!({"solver": solver})
        },
        human,
    ))
}

async fn execute_bundle(
    app: &EuthetoApp,
    args: BundleArgs,
    cancellation: &CancellationToken,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    match args.command {
        BundleCommand::Inspect { bundle } => {
            let (preview_id, metadata) =
                inspect_unopened_bundle(app, &bundle, "bundle.inspect", cancellation).await?;
            let result = app
                .execute(AppCommand::CancelPortablePreview { preview_id })
                .await
                .map_err(|error| ("bundle.inspect", app_error(error)))?;
            if !matches!(result, AppCommandResult::PortablePreviewCancelled) {
                return Err(("bundle.inspect", unexpected_result()));
            }
            Ok(unopened_bundle_outcome(
                "bundle.inspect",
                "inspected",
                &metadata,
                false,
            ))
        }
        BundleCommand::ExactReexport { bundle, output } => {
            let (preview_id, metadata) =
                inspect_unopened_bundle(app, &bundle, "bundle.exact-reexport", cancellation)
                    .await?;
            let result = app
                .execute(AppCommand::ExactReexportUnopenedBundle {
                    preview_id,
                    destination: output,
                })
                .await
                .map_err(|error| ("bundle.exact-reexport", app_error(error)))?;
            if !matches!(result, AppCommandResult::UnopenedBundleReexported) {
                return Err(("bundle.exact-reexport", unexpected_result()));
            }
            Ok(unopened_bundle_outcome(
                "bundle.exact-reexport",
                "reexported",
                &metadata,
                true,
            ))
        }
    }
}

async fn inspect_unopened_bundle(
    app: &EuthetoApp,
    bundle: &Path,
    command: &'static str,
    cancellation: &CancellationToken,
) -> Result<(RequestId, PreservedBundleMetadata), (&'static str, SafeCliError)> {
    let bytes = read_bounded(bundle, BUNDLE_LIMIT, "bundle.too_large", cancellation)
        .await
        .map_err(|error| (command, error))?;
    match app
        .query(AppQuery::InspectUnopenedBundle { bytes })
        .await
        .map_err(|error| (command, app_error(error)))?
    {
        AppQueryResult::UnopenedBundlePreview {
            preview_id,
            metadata,
        } => Ok((preview_id, metadata)),
        _ => Err((command, unexpected_result())),
    }
}

fn unopened_bundle_outcome(
    command: &'static str,
    status: &'static str,
    metadata: &PreservedBundleMetadata,
    reexported: bool,
) -> Outcome {
    let metadata_value = preserved_bundle_metadata_value(metadata);
    let human = vec![
        if reexported {
            "The exact unopened bundle bytes were re-exported.".to_owned()
        } else {
            "The bundle was inspected without importing it.".to_owned()
        },
        format!("SHA-256: {}.", metadata.file_sha256),
        format!(
            "Format version {}; portable schema {}.",
            metadata.format_version, metadata.portable_schema_version
        ),
        format!(
            "Required semantic capabilities: {}.",
            metadata.required_capabilities.len()
        ),
        format!("Scenarios: {}.", metadata.scenarios.len()),
    ];
    Outcome::new(
        command,
        status,
        json!({"reexported": reexported, "metadata": metadata_value}),
        human,
    )
}

fn preserved_bundle_metadata_value(metadata: &PreservedBundleMetadata) -> Value {
    json!({
        "fileSha256": metadata.file_sha256,
        "format": metadata.format,
        "formatVersion": metadata.format_version,
        "portableSchemaVersion": metadata.portable_schema_version,
        "bundleKind": metadata.bundle_kind,
        "title": metadata.title,
        "requiredCapabilities": metadata.required_capabilities.iter().map(|capability| {
            json!({"id": capability.id, "version": capability.version})
        }).collect::<Vec<_>>(),
        "scenarios": metadata.scenarios.iter().map(|scenario| {
            json!({
                "path": scenario.path,
                "scenarioId": scenario.scenario_id,
                "packId": scenario.pack_id,
                "internalPackSchemaVersion": scenario.internal_pack_schema_version,
                "portablePackSchemaVersion": scenario.portable_pack_schema_version,
            })
        }).collect::<Vec<_>>(),
    })
}

async fn execute_deferred(
    data_dir: Option<PathBuf>,
    command: &'static str,
    capability: DeferredCapability,
    cancellation: &CancellationToken,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let app = open_app(data_dir, cancellation)
        .await
        .map_err(|error| (command, error))?;
    match app.query(AppQuery::Deferred(capability)).await {
        Ok(_) => Err((
            command,
            SafeCliError::new(
                CliExitCode::Application,
                "cli.unexpected_result",
                "The application returned an unexpected result.",
            ),
        )),
        Err(error) => Err((command, app_error(error))),
    }
}

async fn open_app(
    data_dir: Option<PathBuf>,
    cancellation: &CancellationToken,
) -> Result<EuthetoApp, SafeCliError> {
    EuthetoApp::open(app_dependencies(data_dir, cancellation)?)
        .await
        .map_err(app_error)
}

fn app_dependencies(
    data_dir: Option<PathBuf>,
    cancellation: &CancellationToken,
) -> Result<AppDependencies, SafeCliError> {
    let root = match data_dir {
        Some(path) => path,
        None => dirs::data_local_dir()
            .map(|path| path.join("eutheto-development"))
            .ok_or_else(|| {
                SafeCliError::storage(
                    "storage.data_directory_unavailable",
                    "A local data directory could not be resolved; pass --data-dir explicitly.",
                )
            })?,
    };
    Ok(AppDependencies {
        paths: AppPaths {
            database: root.join("eutheto.sqlite"),
            safety_backups: root.join("safety-backups"),
        },
        clock: Arc::new(SystemClock),
        monotonic_clock: Arc::new(eutheto_types::SystemMonotonicClock::new()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: cancellation.clone(),
    })
}

async fn execute_projects(
    app: &EuthetoApp,
    args: ProjectsArgs,
    cancellation: &CancellationToken,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    match args.command {
        ProjectsCommand::List { scope } => list_projects(app, scope).await,
        ProjectsCommand::Create(args) => create_project(app, args).await,
        ProjectsCommand::Duplicate {
            source_id,
            expected_revision,
            title,
        } => duplicate_project(app, &source_id, expected_revision, title).await,
        ProjectsCommand::Archive {
            scenario_id,
            expected_revision,
        } => {
            mutate_project(
                app,
                &scenario_id,
                expected_revision,
                "projects.archive",
                "archived",
                |request_id, scenario_id, expected_revision| AppCommand::ArchiveProject {
                    request_id,
                    scenario_id,
                    expected_revision,
                },
            )
            .await
        }
        ProjectsCommand::Unarchive {
            scenario_id,
            expected_revision,
        } => {
            mutate_project(
                app,
                &scenario_id,
                expected_revision,
                "projects.unarchive",
                "unarchived",
                |request_id, scenario_id, expected_revision| AppCommand::UnarchiveProject {
                    request_id,
                    scenario_id,
                    expected_revision,
                },
            )
            .await
        }
        ProjectsCommand::Delete {
            scenario_id,
            expected_revision,
        } => {
            mutate_project(
                app,
                &scenario_id,
                expected_revision,
                "projects.delete",
                "deleted",
                |request_id, scenario_id, expected_revision| AppCommand::DeleteProject {
                    request_id,
                    scenario_id,
                    expected_revision,
                },
            )
            .await
        }
        ProjectsCommand::Export {
            scenario_id,
            output,
        } => export_project(app, &scenario_id, output).await,
        ProjectsCommand::Import {
            bundle,
            exclude_results,
            exclude_assets,
            collision_plan,
        } => {
            import_project(
                app,
                &bundle,
                !exclude_results,
                !exclude_assets,
                collision_plan.as_deref(),
                cancellation,
            )
            .await
        }
    }
}

async fn list_projects(
    app: &EuthetoApp,
    scope: ProjectScopeArg,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let scope = match scope {
        ProjectScopeArg::Active => ProjectScope::Active,
        ProjectScopeArg::Archived => ProjectScope::Archived,
        ProjectScopeArg::All => ProjectScope::All,
    };
    let result = app
        .query(AppQuery::ListProjects(scope))
        .await
        .map_err(|error| ("projects.list", app_error(error)))?;
    let AppQueryResult::Projects(projects) = result else {
        return Err(("projects.list", unexpected_result()));
    };
    let human = if projects.is_empty() {
        vec!["No projects.".to_owned()]
    } else {
        projects
            .iter()
            .map(|project| {
                format!(
                    "{}\t{}\trevision {}{}",
                    project.scenario_id,
                    project.title,
                    project.revision.value(),
                    if project.archived { "\tarchived" } else { "" }
                )
            })
            .collect()
    };
    Ok(Outcome::new(
        "projects.list",
        "ok",
        to_value(projects).map_err(|error| ("projects.list", error))?,
        human,
    ))
}

async fn create_project(
    app: &EuthetoApp,
    args: CreateProjectArgs,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let settings = create_settings(&args).map_err(|error| ("projects.create", error))?;
    let pack_id = args.pack.parse().map_err(|_| {
        (
            "projects.create",
            SafeCliError::validation("pack.id_invalid", "The domain-pack identifier is invalid."),
        )
    })?;
    let request_id = operation_request_id().map_err(|error| ("projects.create", error))?;
    let result = app
        .execute(AppCommand::CreateProject {
            request_id,
            title: args.title,
            description: args.description,
            domain_pack: DomainPackRef {
                id: pack_id,
                schema_version: args.pack_schema,
            },
            settings,
        })
        .await
        .map_err(|error| ("projects.create", app_error(error)))?;
    project_outcome("projects.create", "created", result)
}

async fn duplicate_project(
    app: &EuthetoApp,
    source_id: &str,
    expected_revision: u64,
    title: String,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let source_id = scenario_id(source_id).map_err(|error| ("projects.duplicate", error))?;
    let expected_revision =
        revision(expected_revision).map_err(|error| ("projects.duplicate", error))?;
    let request_id = operation_request_id().map_err(|error| ("projects.duplicate", error))?;
    let result = app
        .execute(AppCommand::DuplicateProject {
            request_id,
            source_id,
            expected_revision,
            title,
        })
        .await
        .map_err(|error| ("projects.duplicate", app_error(error)))?;
    project_outcome("projects.duplicate", "duplicated", result)
}

async fn mutate_project(
    app: &EuthetoApp,
    input: &str,
    expected_revision: u64,
    command: &'static str,
    status: &'static str,
    app_command: impl FnOnce(RequestId, ScenarioId, Revision) -> AppCommand,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let expected_revision = revision(expected_revision).map_err(|error| (command, error))?;
    let id = scenario_id(input).map_err(|error| (command, error))?;
    let request_id = operation_request_id().map_err(|error| (command, error))?;
    app.execute(app_command(request_id, id, expected_revision))
        .await
        .map_err(|error| (command, app_error(error)))?;
    Ok(id_outcome(command, status, id))
}

async fn export_project(
    app: &EuthetoApp,
    input: &str,
    output: PathBuf,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let id = scenario_id(input).map_err(|error| ("projects.export", error))?;
    app.execute(AppCommand::ExportScenario {
        scenario_id: id,
        destination: output,
    })
    .await
    .map_err(|error| ("projects.export", app_error(error)))?;
    Ok(id_outcome("projects.export", "exported", id))
}

async fn import_project(
    app: &EuthetoApp,
    bundle: &Path,
    include_results: bool,
    include_assets: bool,
    collision_plan: Option<&str>,
    cancellation: &CancellationToken,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let bytes = read_bounded(bundle, BUNDLE_LIMIT, "bundle.too_large", cancellation)
        .await
        .map_err(|error| ("projects.import", error))?;
    let options = ImportOptions {
        restore_mode: RestoreMode::ImportScenario,
        include_results,
        include_assets,
    };
    let (preview_id, preview) = portable_preview(app, AppQuery::PreviewImport { bytes, options })
        .await
        .map_err(|error| ("projects.import", error))?;
    let has_collisions = preview.scenarios.iter().any(|scenario| scenario.collides)
        || !preview.supplemental_collisions.is_empty();
    if collision_plan.is_none() && has_collisions {
        return Ok(import_review_outcome(&preview));
    }
    let plan = match collision_plan_value(collision_plan)
        .and_then(|plan| validated_collision_plan(plan, &preview, RestoreMode::ImportScenario))
    {
        Ok(plan) => plan,
        Err(mut error) => {
            error.details = Some(json!({"preview": preview_value(&preview)}));
            error.human_details = portable_preview_human(&preview);
            return Err(("projects.import", error));
        }
    };
    let request_id = operation_request_id().map_err(|error| ("projects.import", error))?;
    let result = app
        .execute(AppCommand::ApplyImport {
            request_id,
            preview_id,
            collision_plan: plan.clone(),
        })
        .await
        .map_err(|error| ("projects.import", app_error(error)))?;
    portable_applied_outcome(
        "projects.import",
        &preview,
        &plan,
        RestoreMode::ImportScenario,
        result,
    )
}

fn create_settings(args: &CreateProjectArgs) -> Result<ScenarioSettings, SafeCliError> {
    let time_zone = args.time_zone.parse::<IanaTimeZone>().map_err(|_| {
        SafeCliError::validation(
            "scenario.time_zone_invalid",
            "The time-zone identifier is invalid.",
        )
    })?;
    let locale = args.locale.parse::<LocaleTag>().map_err(|_| {
        SafeCliError::validation(
            "scenario.locale_invalid",
            "The locale identifier is invalid.",
        )
    })?;
    let start = args.horizon_start.parse().map_err(|_| {
        SafeCliError::validation(
            "scenario.horizon_start_invalid",
            "The horizon start must be an RFC 3339 timestamp.",
        )
    })?;
    let end = args.horizon_end.parse().map_err(|_| {
        SafeCliError::validation(
            "scenario.horizon_end_invalid",
            "The horizon end must be an RFC 3339 timestamp.",
        )
    })?;
    let horizon = Horizon::new(start, end).map_err(|_| {
        SafeCliError::validation(
            "scenario.horizon_invalid",
            "The horizon end must be after its start.",
        )
    })?;
    Ok(ScenarioSettings {
        time_zone,
        locale,
        units: match args.units {
            UnitsArg::Metric => UnitSystem::Metric,
            UnitsArg::UsCustomary => UnitSystem::UsCustomary,
        },
        horizon,
        gap_policy: GapPolicy::Reject,
        overlap_policy: eutheto_types::OverlapPolicy::Earlier,
    })
}

fn project_outcome(
    command: &'static str,
    status: &'static str,
    result: AppCommandResult,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let AppCommandResult::Project(project) = result else {
        return Err((command, unexpected_result()));
    };
    let project_title = &project.title;
    let project_id = project.scenario_id;
    let human = vec![format!("{status}: {project_title} ({project_id})")];
    Ok(Outcome::new(
        command,
        status,
        to_value(project).map_err(|error| (command, error))?,
        human,
    ))
}

fn id_outcome(command: &'static str, status: &'static str, id: ScenarioId) -> Outcome {
    Outcome::new(
        command,
        status,
        json!({"scenarioId": id}),
        vec![format!("{status}: {id}")],
    )
}

async fn execute_backup(
    app: &EuthetoApp,
    args: BackupArgs,
    cancellation: &CancellationToken,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    match args.command {
        BackupCommand::Inspect { bundle, mode } => {
            let bytes = read_bounded(&bundle, BUNDLE_LIMIT, "bundle.too_large", cancellation)
                .await
                .map_err(|error| ("backup.inspect", error))?;
            let options = ImportOptions {
                restore_mode: match mode {
                    RestoreModeArg::Add => RestoreMode::AddBackup,
                    RestoreModeArg::Replace => RestoreMode::ReplaceLibrary,
                },
                include_results: true,
                include_assets: true,
            };
            let (_, preview) = portable_preview(app, AppQuery::PreviewRestore { bytes, options })
                .await
                .map_err(|error| ("backup.inspect", error))?;
            let result = preview_value(&preview);
            Ok(Outcome::new(
                "backup.inspect",
                "valid",
                result,
                portable_preview_human(&preview),
            ))
        }
        BackupCommand::Create {
            output,
            title,
            exclude_results,
            exclude_large_assets,
        } => execute_backup_create(app, output, title, exclude_results, exclude_large_assets).await,
        BackupCommand::Restore {
            bundle,
            mode,
            collision_plan,
            confirm_replace,
            review_token,
            without_backup_token,
        } => {
            let bytes = read_bounded(&bundle, BUNDLE_LIMIT, "bundle.too_large", cancellation)
                .await
                .map_err(|error| ("backup.restore", error))?;
            execute_backup_restore(
                app,
                bytes,
                mode,
                collision_plan,
                confirm_replace,
                review_token,
                without_backup_token,
            )
            .await
        }
    }
}

async fn execute_backup_create(
    app: &EuthetoApp,
    output: PathBuf,
    title: String,
    exclude_results: bool,
    exclude_large_assets: bool,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let result = app
        .execute(AppCommand::CreateBackup {
            title,
            destination: output,
            selection: BackupSelection {
                include_results: !exclude_results,
                assets: if exclude_large_assets {
                    BackupAssetSelection::IncludeUnderThreshold
                } else {
                    BackupAssetSelection::IncludeAll
                },
                include_audit: false,
            },
        })
        .await
        .map_err(|error| ("backup.create", app_error(error)))?;
    let AppCommandResult::BackupWritten(summary) = result else {
        return Err(("backup.create", unexpected_result()));
    };
    let excluded_count = summary.excluded_asset_count;
    let excluded_ids = summary.excluded_asset_ids;
    let scope = summary.exclusion_scope;
    let threshold = eutheto_types::PORTABLE_LARGE_ASSET_BYTES_V1;
    let asset_selection = match summary.asset_selection {
        BackupAssetSelection::ExcludeAll => "exclude-all",
        BackupAssetSelection::IncludeAll => "all",
        BackupAssetSelection::IncludeUnderThreshold => "v1-threshold",
    };
    let exclusion_line = if excluded_ids.is_empty() {
        "Asset omission placeholders: none.".to_owned()
    } else {
        format!(
            "Asset omission placeholders: {} asset(s): {}.",
            excluded_count,
            excluded_ids.join(", ")
        )
    };
    let fixed_exclusions = summary.fixed_exclusions;
    let fixed_exclusion_labels = fixed_exclusions
        .iter()
        .copied()
        .map(fixed_exclusion_human_label)
        .collect::<Vec<_>>();
    Ok(Outcome::new(
        "backup.create",
        "created",
        json!({
            "written": true,
            "includeResults": summary.include_results,
            "assetSelection": asset_selection,
            "excludedAssetCount": excluded_count,
            "excludedAssetIds": excluded_ids,
            "exclusionScope": scope,
            "fixedExclusions": fixed_exclusions,
            "largeAssetThresholdBytes": exclude_large_assets.then_some(threshold),
        }),
        vec![
            "Backup created.".to_owned(),
            exclusion_line,
            format!("Fixed exclusions: {}.", fixed_exclusion_labels.join("; ")),
        ],
    ))
}

async fn execute_backup_restore(
    app: &EuthetoApp,
    bytes: Vec<u8>,
    mode: RestoreModeArg,
    collision_plan: Option<String>,
    confirm_replace: bool,
    review_token: Option<String>,
    without_backup_token: Option<String>,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let restore_mode = match mode {
        RestoreModeArg::Add => RestoreMode::AddBackup,
        RestoreModeArg::Replace => RestoreMode::ReplaceLibrary,
    };
    let options = ImportOptions {
        restore_mode,
        include_results: true,
        include_assets: true,
    };
    let (preview_id, preview) = portable_preview(app, AppQuery::PreviewRestore { bytes, options })
        .await
        .map_err(|error| ("backup.restore", error))?;
    let plan = collision_plan_value(collision_plan.as_deref())
        .and_then(|plan| validated_collision_plan(plan, &preview, restore_mode))
        .map_err(|error| ("backup.restore", error))?;
    if let Some(outcome) = restore_review_gate(
        mode,
        review_token.as_deref(),
        without_backup_token.as_deref(),
        &preview,
        &plan,
    )
    .map_err(|error| ("backup.restore", error))?
    {
        return Ok(outcome);
    }
    let (authorization, prospective_receipt_token) =
        restore_authorization(mode, confirm_replace, without_backup_token)
            .map_err(|error| ("backup.restore", error))?;
    let request_id = operation_request_id().map_err(|error| ("backup.restore", error))?;
    let result = app
        .execute(AppCommand::ApplyRestore {
            request_id,
            preview_id,
            collision_plan: plan.clone(),
            authorization,
        })
        .await;
    match (result, prospective_receipt_token) {
        (Ok(result), _) => {
            portable_applied_outcome("backup.restore", &preview, &plan, restore_mode, result)
        }
        (Err(error), Some(receipt)) if is_safety_backup_failure(&error) => {
            let mut safe = app_error(error);
            let review = replace_review_token(&preview.binding, &plan)
                .map_err(|error| ("backup.restore", error))?;
            safe.human_details = vec![
                format!("Safety-backup failure receipt: {receipt}"),
                format!("Review token: {review}"),
                "Start a distinct invocation with the same --review-token and --without-backup-token receipt after reviewing the unchanged preview below.".to_owned(),
                "Unchanged preview:".to_owned(),
            ];
            safe.human_details.extend(portable_preview_human_with_plan(
                &preview,
                Some(&plan),
                Some(RestoreMode::ReplaceLibrary),
            ));
            safe.details = Some(json!({
                "failureReceiptToken": receipt,
                "reviewToken": review,
                "instruction": "Start a distinct invocation with the same --review-token and --without-backup-token receipt after reviewing this unchanged preview.",
                "preview": preview_value(&preview),
            }));
            Err(("backup.restore", safe))
        }
        (Err(error), _) => Err(("backup.restore", app_error(error))),
    }
}

fn restore_review_gate(
    mode: RestoreModeArg,
    supplied_review_token: Option<&str>,
    supplied_receipt: Option<&str>,
    preview: &eutheto_import::ImportPreview,
    plan: &CollisionPlan,
) -> Result<Option<Outcome>, SafeCliError> {
    if mode == RestoreModeArg::Add {
        if supplied_review_token.is_some() || supplied_receipt.is_some() {
            return Err(SafeCliError::validation(
                "restore.replace_token_not_applicable",
                "Replace review and safety-backup receipt tokens are not valid in add mode.",
            ));
        }
        return Ok(None);
    }
    let expected = replace_review_token(&preview.binding, plan)?;
    match (supplied_review_token, supplied_receipt) {
        (None, None) => Ok(Some(restore_review_outcome(preview, &expected))),
        (None, Some(_)) => Err(SafeCliError::validation(
            "restore.safety_backup_receipt_unreviewed",
            "A safety-backup failure receipt cannot be used before a matching restore review token.",
        )),
        (Some(supplied), _) if supplied != expected => {
            let mut error = SafeCliError::validation(
                "restore.review_token_invalid",
                "The restore review token does not match this bundle, library revision, options, and collision plan.",
            );
            error.details = Some(json!({"preview": preview_value(preview)}));
            Err(error)
        }
        (Some(_), _) => Ok(None),
    }
}

fn restore_authorization(
    mode: RestoreModeArg,
    confirm_replace: bool,
    failure_receipt: Option<String>,
) -> Result<(RestoreAuthorization, Option<String>), SafeCliError> {
    if mode == RestoreModeArg::Add {
        return Ok((
            RestoreAuthorization {
                destructive_action_confirmed: false,
                safety_backup: SafetyBackupEvidence::NotRequired,
                prospective_failure_receipt_token: None,
                collision_plan_sha256: None,
            },
            None,
        ));
    }
    if !confirm_replace {
        return Err(SafeCliError::validation(
            "restore.confirmation_required",
            "Replace mode requires --confirm-replace after preview review.",
        ));
    }
    if let Some(proof) = failure_receipt {
        return Ok((
            RestoreAuthorization {
                destructive_action_confirmed: true,
                safety_backup: SafetyBackupEvidence::FailedWithStrongConfirmation { proof },
                prospective_failure_receipt_token: None,
                collision_plan_sha256: None,
            },
            None,
        ));
    }
    let prospective = operation_request_id()?.to_string();
    Ok((
        RestoreAuthorization {
            destructive_action_confirmed: true,
            safety_backup: SafetyBackupEvidence::NotRequired,
            prospective_failure_receipt_token: Some(prospective.clone()),
            collision_plan_sha256: None,
        },
        Some(prospective),
    ))
}

fn replace_review_token(
    binding: &eutheto_import::PreviewBinding,
    plan: &CollisionPlan,
) -> Result<String, SafeCliError> {
    eutheto_import::portable_review_token(binding, plan).map_err(|_| {
        SafeCliError::new(
            CliExitCode::Application,
            "restore.review_token_failed",
            "The restore review token could not be generated.",
        )
    })
}

fn restore_review_outcome(preview: &eutheto_import::ImportPreview, review_token: &str) -> Outcome {
    let mut result = preview_value(preview);
    if let Value::Object(object) = &mut result {
        object.insert("reviewToken".to_owned(), json!(review_token));
        object.insert(
            "instruction".to_owned(),
            json!("Review this complete preview, then start a distinct invocation with --review-token TOKEN and --confirm-replace."),
        );
    }
    let plan = CollisionPlan::default();
    let mut human =
        portable_preview_human_with_plan(preview, Some(&plan), Some(RestoreMode::ReplaceLibrary));
    human.push(format!("Review token: {review_token}"));
    human.push(
        "Review the complete scope above, then start a distinct invocation with --review-token TOKEN and --confirm-replace.".to_owned(),
    );
    Outcome::new("backup.restore", "review-required", result, human)
}

fn is_safety_backup_failure(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Protocol(failure) if failure.code == "restore.safety_backup_failed"
    )
}

async fn portable_preview(
    app: &EuthetoApp,
    query: AppQuery,
) -> Result<(RequestId, eutheto_import::ImportPreview), SafeCliError> {
    let result = app.query(query).await.map_err(app_error)?;

    match result {
        AppQueryResult::PortablePreview {
            preview_id,
            preview,
        } => Ok((preview_id, *preview)),
        _ => Err(unexpected_result()),
    }
}
fn import_review_outcome(preview: &eutheto_import::ImportPreview) -> Outcome {
    let required_scenarios = preview
        .scenarios
        .iter()
        .filter(|scenario| scenario.collides)
        .map(|scenario| scenario.scenario_id)
        .collect::<Vec<_>>();
    let scenario_choices = required_scenarios
        .iter()
        .map(|scenario_id| (scenario_id.to_string(), json!("create-copy")))
        .collect::<Map<_, _>>();
    let supplemental_choices = preview
        .supplemental_collisions
        .iter()
        .map(|identity| {
            json!({
                "section": identity.section,
                "key": identity.key,
                "action": "skip",
            })
        })
        .collect::<Vec<_>>();
    let mut result = preview_value(preview);
    if let Value::Object(object) = &mut result {
        object.insert(
            "requiredCollisionPlan".to_owned(),
            json!({
                "scenarios": scenario_choices,
                "supplementalChoices": supplemental_choices,
            }),
        );
        object.insert(
            "collisionActionChoices".to_owned(),
            json!({
                "scenarios": ["create-copy", "replace", "skip"],
                "supplementalChoices": ["replace", "skip"],
            }),
        );
        object.insert(
            "instruction".to_owned(),
            json!("Review this preview, then repeat the import with an exact --collision-plan."),
        );
    }
    let mut human = portable_preview_human(preview);
    human.push(
        "Collision plan required. Repeat the import with one action for every collision shown above."
            .to_owned(),
    );
    Outcome::new("projects.import", "review-required", result, human)
}

fn portable_applied_outcome(
    command: &'static str,
    preview: &eutheto_import::ImportPreview,
    plan: &CollisionPlan,
    mode: RestoreMode,
    result: AppCommandResult,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let AppCommandResult::PortableApplied { scenarios } = result else {
        return Err((command, unexpected_result()));
    };
    let scenario_count = scenarios.len();
    let scenario_ids = scenarios
        .iter()
        .map(|scenario| scenario.scenario_id)
        .collect::<Vec<_>>();
    let mut human = portable_preview_human_with_plan(preview, Some(plan), Some(mode));
    human.push(format!("Applied {scenario_count} scenario(s)."));
    Ok(Outcome::new(
        command,
        "applied",
        json!({
            "preview": preview_value(preview),
            "scenarioIds": scenario_ids,
            "scenarioOutcomes": portable_scenario_outcomes(preview, plan, mode, &scenarios),
        }),
        human,
    ))
}

fn portable_scenario_outcomes(
    preview: &eutheto_import::ImportPreview,
    plan: &CollisionPlan,
    mode: RestoreMode,
    applied_scenarios: &[AppliedPortableScenario],
) -> Vec<Value> {
    let mut remaining_applied_scenarios = applied_scenarios.iter();
    preview
        .scenarios
        .iter()
        .map(|scenario| {
            let action = plan.scenarios.get(&scenario.scenario_id);
            let skipped = scenario.collides && matches!(action, Some(CollisionAction::Skip));
            let replaced = scenario.collides && matches!(action, Some(CollisionAction::Replace));
            let same_identity =
                !scenario.collides || matches!(mode, RestoreMode::ReplaceLibrary) || replaced;
            let (selected_action, revision, warning) = if skipped {
                ("skip", None, None)
            } else if replaced {
                (
                    "replace",
                    Some(scenario.same_identity_revision),
                    scenario.same_identity_revision_warning.as_deref(),
                )
            } else if same_identity {
                (
                    "same-identity",
                    Some(scenario.same_identity_revision),
                    scenario.same_identity_revision_warning.as_deref(),
                )
            } else {
                ("create-copy", Some(scenario.source_revision), None)
            };
            let persisted_scenario_id = if skipped {
                None
            } else {
                remaining_applied_scenarios
                    .find(|applied| applied.source_scenario_id == scenario.scenario_id)
                    .map(|applied| applied.scenario_id)
            };
            json!({
                "sourceScenarioId": scenario.scenario_id,
                "scenarioId": persisted_scenario_id,
                "selectedAction": selected_action,
                "revision": revision,
                "warning": warning,
            })
        })
        .collect()
}

fn portable_preview_human(preview: &eutheto_import::ImportPreview) -> Vec<String> {
    portable_preview_human_with_plan(preview, None, None)
}

fn portable_preview_human_with_plan(
    preview: &eutheto_import::ImportPreview,
    plan: Option<&CollisionPlan>,
    mode: Option<RestoreMode>,
) -> Vec<String> {
    let mut human = portable_preview_metadata_human(preview);
    append_portable_omissions_human(preview, &mut human);
    append_portable_compatibility_human(preview, &mut human);
    append_portable_scenario_scope_human(preview, plan, mode, &mut human);
    append_portable_removal_scope_human(preview, &mut human);
    human
}

fn portable_preview_metadata_human(preview: &eutheto_import::ImportPreview) -> Vec<String> {
    let counts = &preview.counts;
    vec![
        format!(
            "{}: {} scenario(s).",
            preview.title,
            preview.scenarios.len()
        ),
        format!(
            "Source: {} {} (format {}, schema {}).",
            preview.source_application.name,
            preview.source_application.version,
            preview.source_format_version,
            preview.source_schema_version
        ),
        format!("Bundle created: {}.", preview.created_at),
        format!(
            "Previewed library revision: {}.",
            preview.binding.local_library_revision.value()
        ),
        format!(
            "Counts: {} scenarios, {} historical revisions, {} results, {} shared records, {} preferences, {} assets.",
            counts.scenarios,
            counts.scenario_revisions,
            counts.results,
            counts.shared_records,
            counts.preferences,
            counts.assets
        ),
        format!(
            "Included sections: {}.",
            join_sections(&preview.included_sections)
        ),
        format!(
            "Excluded sections: {}.",
            join_sections(&preview.excluded_sections)
        ),
    ]
}

fn fixed_exclusion_human_label(exclusion: FixedExclusion) -> &'static str {
    match exclusion {
        FixedExclusion::LocalUndoAndAuditHistory => "local undo and audit history",
        FixedExclusion::SqliteAndDatabaseInternals => "SQLite and database internals",
        FixedExclusion::CredentialsTokensAndKeychainReferences => {
            "credentials, tokens, and keychain references"
        }
        FixedExclusion::DeviceLocalPathsAndWindowState => "device-local paths and window state",
        FixedExclusion::LogsCachesAndTemporaryData => "logs, caches, and temporary data",
        FixedExclusion::RedistributionProhibitedProviderData => {
            "redistribution-prohibited provider data"
        }
        FixedExclusion::ExecutableContent => "executable content",
    }
}

fn append_portable_omissions_human(
    preview: &eutheto_import::ImportPreview,
    human: &mut Vec<String>,
) {
    if let Some(selection) = &preview.source_backup_selection {
        let asset_selection = serialized_enum_label(&selection.asset_selection);
        let scope = serialized_enum_label(&selection.scope);
        let excluded_ids = if selection.excluded_asset_ids.is_empty() {
            "none".to_owned()
        } else {
            selection
                .excluded_asset_ids
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };
        human.push(format!(
            "Source selection: results {}; assets {}; {} excluded asset(s) [{}]; scope {}; threshold version {:?}, bytes {:?}.",
            if selection.include_results {
                "included"
            } else {
                "explicitly excluded"
            },
            asset_selection,
            selection.excluded_asset_count,
            excluded_ids,
            scope,
            selection.threshold_version,
            selection.threshold_bytes
        ));
        let fixed_exclusions = selection
            .fixed_exclusions
            .iter()
            .copied()
            .map(fixed_exclusion_human_label)
            .collect::<Vec<_>>();
        human.push(format!(
            "Fixed exclusions: {}.",
            if fixed_exclusions.is_empty() {
                "none".to_owned()
            } else {
                fixed_exclusions.join("; ")
            }
        ));
    }
    for (asset_id, placeholder) in &preview.omitted_assets {
        human.push(format!(
            "Omitted asset: {} (reason {}, original type {}, {} bytes).",
            asset_id,
            serialized_enum_label(&placeholder.reason),
            placeholder.original_media_type,
            placeholder.original_size
        ));
    }
}

fn serialized_enum_label(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|encoded| encoded.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn append_portable_compatibility_human(
    preview: &eutheto_import::ImportPreview,
    human: &mut Vec<String>,
) {
    if preview.required_capabilities.is_empty() {
        human.push("Required capabilities: none.".to_owned());
    } else {
        human.push(format!(
            "Required capabilities: {}.",
            preview
                .required_capabilities
                .iter()
                .map(|capability| format!("{} v{}", capability.id, capability.version))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    human.push(format!(
        "Preserved extensions: {}.",
        if preview.preserved_extensions.is_empty() {
            "none".to_owned()
        } else {
            preview
                .preserved_extensions
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    for migration in &preview.applied_migrations {
        let registry = match &migration.registry {
            eutheto_import::MigrationRegistryKind::Outer => "outer",
            eutheto_import::MigrationRegistryKind::Portable => "portable",
        };
        human.push(format!(
            "Applied migration: {} / {} ({} -> {}).",
            registry, migration.name, migration.from_version, migration.to_version
        ));
    }
}

fn append_portable_scenario_scope_human(
    preview: &eutheto_import::ImportPreview,
    plan: Option<&CollisionPlan>,
    mode: Option<RestoreMode>,
    human: &mut Vec<String>,
) {
    for scenario in &preview.scenarios {
        let action = plan.and_then(|plan| plan.scenarios.get(&scenario.scenario_id));
        let replace_identity = !scenario.collides
            || matches!(mode, Some(RestoreMode::ReplaceLibrary))
            || matches!(action, Some(CollisionAction::Replace));
        let outcome = if scenario.collides && matches!(action, Some(CollisionAction::Skip)) {
            "no resulting project (skip selected)".to_owned()
        } else if replace_identity {
            format!(
                "same-identity revision {}",
                scenario.same_identity_revision.value()
            )
        } else if matches!(action, Some(CollisionAction::CreateCopy)) {
            format!("create-copy revision {}", scenario.source_revision.value())
        } else {
            format!(
                "action required: create-copy revision {}, replace revision {}, or skip",
                scenario.source_revision.value(),
                scenario.same_identity_revision.value()
            )
        };
        human.push(format!(
            "Scenario: {} ({}){}; source revision {}; selected outcome: {}.",
            scenario.title,
            scenario.scenario_id,
            if scenario.collides {
                " [collision]"
            } else {
                ""
            },
            scenario.source_revision.value(),
            outcome,
        ));
        if replace_identity && let Some(warning) = &scenario.same_identity_revision_warning {
            human.push(format!("Same-identity revision warning: {warning}"));
        }
    }
    for identity in &preview.supplemental_collisions {
        human.push(format!(
            "Supplemental collision: {:?}/{}.",
            identity.section, identity.key
        ));
    }
}

fn append_portable_removal_scope_human(
    preview: &eutheto_import::ImportPreview,
    human: &mut Vec<String>,
) {
    for scenario in &preview.removed_scenarios {
        human.push(format!(
            "Remove scenario: {} ({}, revision {}).",
            scenario.title,
            scenario.scenario_id,
            scenario.revision.value()
        ));
    }
    for identity in &preview.removed_supplemental {
        human.push(format!(
            "Remove supplemental: {:?}/{}.",
            identity.section, identity.key
        ));
    }
    if !preview.settings_changed.is_empty() {
        human.push(format!(
            "Application settings changed: {}.",
            preview.settings_changed.join(", ")
        ));
    }
    if !preview.settings_removed.is_empty() {
        human.push(format!(
            "Application settings removed: {}.",
            preview.settings_removed.join(", ")
        ));
    }
}

fn join_sections(sections: &std::collections::BTreeSet<String>) -> String {
    if sections.is_empty() {
        "none".to_owned()
    } else {
        sections.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

fn preview_value(preview: &eutheto_import::ImportPreview) -> Value {
    let scenarios = preview
        .scenarios
        .iter()
        .map(|scenario| {
            json!({
                "scenarioId": scenario.scenario_id,
                "title": scenario.title,
                "collides": scenario.collides,
                "sourceRevision": scenario.source_revision,
                "sameIdentityRevision": scenario.same_identity_revision,
                "sameIdentityRevisionWarning": scenario.same_identity_revision_warning,
            })
        })
        .collect::<Vec<_>>();
    let removed_scenarios = preview
        .removed_scenarios
        .iter()
        .map(|scenario| {
            json!({
                "scenarioId": scenario.scenario_id,
                "title": scenario.title,
                "revision": scenario.revision,
                "archived": scenario.archived,
            })
        })
        .collect::<Vec<_>>();
    let omitted_assets = preview
        .omitted_assets
        .iter()
        .map(|(asset_id, placeholder)| {
            json!({
                "assetId": asset_id,
                "format": placeholder.format,
                "version": placeholder.version,
                "reason": placeholder.reason,
                "originalMediaType": placeholder.original_media_type,
                "originalSize": placeholder.original_size,
                "contentSha256": placeholder.content_sha256,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "bundleId": preview.bundle_id,
        "bundleKind": preview.bundle_kind,
        "title": preview.title,
        "createdAt": preview.created_at,
        "sourceApplication": preview.source_application,
        "sourceFormatVersion": preview.source_format_version,
        "sourceSchemaVersion": preview.source_schema_version,
        "localLibraryRevision": preview.binding.local_library_revision,
        "supplementalCollisions": preview.supplemental_collisions,
        "removedSupplemental": preview.removed_supplemental,
        "counts": preview.counts,
        "requiredCapabilities": preview.required_capabilities,
        "preservedExtensions": preview.preserved_extensions,
        "includedSections": preview.included_sections,
        "excludedSections": preview.excluded_sections,
        "sourceBackupSelection": preview.source_backup_selection,
        "omittedAssets": omitted_assets,
        "scenarioCollisionChoices": ["create-copy", "replace", "skip"],
        "supplementalCollisionChoices": ["replace", "skip"],
        "scenarios": scenarios,
        "removedScenarios": removed_scenarios,
        "settingsChanged": preview.settings_changed,
        "settingsRemoved": preview.settings_removed,
        "appliedMigrations": preview.applied_migrations,
    })
}

async fn execute_scenario(
    app: &EuthetoApp,
    args: ScenarioArgs,
    cancellation: &CancellationToken,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    match args.command {
        ScenarioCommandArgs::Show { input } => show_scenario(app, &input).await,
        ScenarioCommandArgs::Validate { input } => validate_scenario(app, &input).await,
        ScenarioCommandArgs::Apply(apply) => apply_commands(app, apply, false, cancellation).await,
        ScenarioCommandArgs::Batch(apply) => apply_commands(app, apply, true, cancellation).await,
        ScenarioCommandArgs::Undo {
            scenario_id,
            expected_revision,
        } => history_move(app, "scenario.undo", &scenario_id, expected_revision, true).await,
        ScenarioCommandArgs::Redo {
            scenario_id,
            expected_revision,
        } => history_move(app, "scenario.redo", &scenario_id, expected_revision, false).await,
        ScenarioCommandArgs::History { scenario_id } => scenario_history(app, &scenario_id).await,
        ScenarioCommandArgs::Migrate { input, output } => {
            let _ = (input, output);
            Err((
                "scenario.migrate",
                SafeCliError::unavailable(
                    "scenario.migrate_unavailable",
                    "Use checked scenario loading for supported migrations; standalone migration is unavailable.",
                ),
            ))
        }
    }
}

async fn show_scenario(
    app: &EuthetoApp,
    input: &str,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let id = scenario_id(input).map_err(|error| ("scenario.show", error))?;
    let result = app
        .query(AppQuery::OpenProject(id))
        .await
        .map_err(|error| ("scenario.show", app_error(error)))?;
    let AppQueryResult::Scenario(view) = result else {
        return Err(("scenario.show", unexpected_result()));
    };
    let title = &view.document.metadata.title;
    let revision = view.revision.value();
    let human = vec![
        format!("{title} (revision {revision})"),
        serde_json::to_string_pretty(&view.document)
            .map_err(|_| ("scenario.show", serialization_error()))?,
    ];
    Ok(Outcome::new(
        "scenario.show",
        "ok",
        to_value(view).map_err(|error| ("scenario.show", error))?,
        human,
    ))
}

async fn validate_scenario(
    app: &EuthetoApp,
    input: &str,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let id = scenario_id(input).map_err(|error| ("scenario.validate", error))?;
    let (revision, report) = app
        .validate_stored_full(id)
        .await
        .map_err(|error| ("scenario.validate", app_error(error)))?;
    Ok(scenario_files::validation_outcome(revision, &report.issues))
}

async fn scenario_history(
    app: &EuthetoApp,
    input: &str,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let id = scenario_id(input).map_err(|error| ("scenario.history", error))?;
    let result = app
        .query(AppQuery::History(id))
        .await
        .map_err(|error| ("scenario.history", app_error(error)))?;
    let AppQueryResult::History(entries) = result else {
        return Err(("scenario.history", unexpected_result()));
    };
    let values = entries
        .iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "revisionBefore": entry.revision_before,
                "revisionAfter": entry.revision_after,
                "commandType": entry.command_type,
                "command": entry.command,
                "inverse": entry.inverse,
                "actor": entry.actor,
                "source": entry.source,
                "summary": entry.summary,
                "createdAt": entry.created_at,
                "historySequence": entry.history_sequence,
                "branchGeneration": entry.branch_generation,
                "applied": entry.applied,
            })
        })
        .collect::<Vec<_>>();
    let human = if entries.is_empty() {
        vec!["No history.".to_owned()]
    } else {
        entries
            .iter()
            .map(|entry| {
                let id = entry.id;
                let before = entry.revision_before.value();
                let after = entry.revision_after.value();
                let summary = &entry.summary;
                format!("{id}\t{before} -> {after}\t{summary}")
            })
            .collect()
    };
    Ok(Outcome::new(
        "scenario.history",
        "ok",
        Value::Array(values),
        human,
    ))
}

async fn apply_commands(
    app: &EuthetoApp,
    args: ApplyArgs,
    require_batch: bool,
    cancellation: &CancellationToken,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let command_name = if require_batch {
        "scenario.batch"
    } else {
        "scenario.apply"
    };
    let id = scenario_id(&args.input).map_err(|error| (command_name, error))?;
    let bytes = read_bounded(
        &args.commands,
        COMMAND_JSON_LIMIT,
        "commands.too_large",
        cancellation,
    )
    .await
    .map_err(|error| (command_name, error))?;
    let value = parse_strict_json(&bytes).map_err(|error| (command_name, error))?;
    let envelope = command_envelope(id, args.expected_revision, value, require_batch)
        .map_err(|error| (command_name, error))?;
    let request_id = operation_request_id().map_err(|error| (command_name, error))?;
    let result = app
        .execute(AppCommand::ApplyScenario {
            request_id,
            envelope,
            truncate_redo: args.truncate_redo,
        })
        .await
        .map_err(|error| (command_name, app_error(error)))?;
    let AppCommandResult::ScenarioCommand(command_result) = result else {
        return Err((command_name, unexpected_result()));
    };
    let publication_warning = if let Some(output) = args.output {
        app.execute(AppCommand::ExportScenario {
            scenario_id: id,
            destination: output,
        })
        .await
        .err()
        .map(SafeCliWarning::artifact_publication)
    } else {
        None
    };
    let revision = command_result.new_revision.value();
    let human = vec![format!("Applied command; revision {revision}.")];
    let outcome = Outcome::new(
        command_name,
        "applied",
        to_value(command_result).map_err(|error| (command_name, error))?,
        human,
    );
    Ok(match publication_warning {
        Some(warning) => outcome.with_warning(warning),
        None => outcome,
    })
}

fn command_envelope(
    scenario_id: ScenarioId,
    expected_revision: Option<u64>,
    value: Value,
    require_batch: bool,
) -> Result<CommandEnvelope, SafeCliError> {
    if value.get("commandId").is_some() {
        if require_batch {
            return Err(SafeCliError::validation(
                "commands.batch_required",
                "The batch command requires a JSON array of scenario commands.",
            ));
        }
        let envelope: CommandEnvelope = serde_json::from_value(value).map_err(|_| {
            SafeCliError::validation(
                "commands.invalid",
                "The command envelope does not match the strict Phase-01 schema.",
            )
        })?;
        if envelope.scenario_id != scenario_id {
            return Err(SafeCliError::validation(
                "commands.scenario_mismatch",
                "The command envelope targets a different scenario.",
            ));
        }
        return Ok(envelope);
    }
    let expected = expected_revision.ok_or_else(|| {
        SafeCliError::validation(
            "commands.expected_revision_required",
            "Command objects and arrays require --expected-revision.",
        )
    })?;
    let command = match value {
        Value::Array(values) => {
            let commands = values
                .into_iter()
                .map(|item| {
                    serde_json::from_value(item).map_err(|_| {
                        SafeCliError::validation(
                            "commands.invalid",
                            "A command does not match the strict Phase-01 schema.",
                        )
                    })
                })
                .collect::<Result<Vec<ScenarioCommand>, _>>()?;
            if commands.is_empty() {
                return Err(SafeCliError::validation(
                    "commands.empty",
                    "A command batch must not be empty.",
                ));
            }
            ScenarioCommand::ApplyBatch(CommandBatch {
                label: None,
                commands,
            })
        }
        object if !require_batch => serde_json::from_value(object).map_err(|_| {
            SafeCliError::validation(
                "commands.invalid",
                "The command does not match the strict Phase-01 schema.",
            )
        })?,
        _ => {
            return Err(SafeCliError::validation(
                "commands.batch_required",
                "The batch command requires a JSON array of scenario commands.",
            ));
        }
    };
    let ids = SystemIdGenerator;
    let command_id = CommandId::new(&ids).map_err(|_| {
        SafeCliError::new(
            CliExitCode::Application,
            "identity.unavailable",
            "A command identifier could not be created.",
        )
    })?;
    Ok(CommandEnvelope {
        command_id,
        scenario_id,
        expected_revision: revision(expected)?,
        actor: ActorRef {
            actor_id: None,
            display_name: "optimizer CLI".to_owned(),
        },
        source: CommandSource::Cli,
        command,
    })
}

async fn history_move(
    app: &EuthetoApp,
    command: &'static str,
    input: &str,
    expected_revision: u64,
    undo: bool,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let id = scenario_id(input).map_err(|error| (command, error))?;
    let request_id = operation_request_id().map_err(|error| (command, error))?;
    let expected_revision = revision(expected_revision).map_err(|error| (command, error))?;
    let app_command = if undo {
        AppCommand::Undo {
            request_id,
            scenario_id: id,
            expected_revision,
        }
    } else {
        AppCommand::Redo {
            request_id,
            scenario_id: id,
            expected_revision,
        }
    };
    let result = app
        .execute(app_command)
        .await
        .map_err(|error| (command, app_error(error)))?;
    let AppCommandResult::ScenarioCommand(command_result) = result else {
        return Err((command, unexpected_result()));
    };
    let revision = command_result.new_revision.value();
    Ok(Outcome::new(
        command,
        if undo { "undone" } else { "redone" },
        to_value(&command_result).map_err(|error| (command, error))?,
        vec![format!("Revision {revision}.")],
    ))
}

async fn execute_settings(
    app: &EuthetoApp,
    args: SettingsArgs,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    match args.command {
        SettingsCommand::Get { key } => {
            let result = app
                .query(AppQuery::Setting(key))
                .await
                .map_err(|error| ("settings.get", app_error(error)))?;
            let AppQueryResult::Setting(setting) = result else {
                return Err(("settings.get", unexpected_result()));
            };
            let value = setting.map_or(
                Value::Null,
                |setting| json!({"value": setting.value, "updatedAt": setting.updated_at}),
            );
            Ok(Outcome::new(
                "settings.get",
                "ok",
                value.clone(),
                vec![value.to_string()],
            ))
        }
        SettingsCommand::Set { key, value } => {
            let value =
                parse_strict_json(value.as_bytes()).map_err(|error| ("settings.set", error))?;
            let request_id = operation_request_id().map_err(|error| ("settings.set", error))?;
            app.execute(AppCommand::SetSetting {
                request_id,
                key,
                value,
            })
            .await
            .map_err(|error| ("settings.set", app_error(error)))?;
            Ok(Outcome::new(
                "settings.set",
                "updated",
                json!({"updated": true}),
                vec!["Setting updated.".to_owned()],
            ))
        }
        SettingsCommand::Delete { key } => {
            let request_id = operation_request_id().map_err(|error| ("settings.delete", error))?;
            let result = app
                .execute(AppCommand::DeleteSetting { request_id, key })
                .await
                .map_err(|error| ("settings.delete", app_error(error)))?;
            let AppCommandResult::SettingDeleted(deleted) = result else {
                return Err(("settings.delete", unexpected_result()));
            };
            Ok(Outcome::new(
                "settings.delete",
                "deleted",
                json!({"deleted": deleted}),
                vec![if deleted {
                    "Setting deleted.".to_owned()
                } else {
                    "Setting was not present.".to_owned()
                }],
            ))
        }
    }
}

fn collision_plan_value(input: Option<&str>) -> Result<CollisionPlan, SafeCliError> {
    match input {
        None => Ok(CollisionPlan::default()),
        Some(input) => {
            let value = parse_strict_json(input.as_bytes())?;
            serde_json::from_value(value).map_err(|_| {
                SafeCliError::validation(
                    "collision_plan.invalid",
                    "The collision plan does not match the strict Phase-01 schema.",
                )
            })
        }
    }
}

fn validated_collision_plan(
    plan: CollisionPlan,
    preview: &eutheto_import::ImportPreview,
    mode: RestoreMode,
) -> Result<CollisionPlan, SafeCliError> {
    let invalid = || {
        SafeCliError::validation(
            "collision_plan.invalid",
            "The collision plan must contain exactly the choices shown in the portable preview.",
        )
    };
    if matches!(mode, RestoreMode::ReplaceLibrary) {
        if plan.scenarios.is_empty() && plan.supplemental.is_empty() {
            return Ok(CollisionPlan::default());
        }
        return Err(invalid());
    }
    let expected_scenarios = preview
        .scenarios
        .iter()
        .filter(|scenario| scenario.collides)
        .map(|scenario| scenario.scenario_id)
        .collect::<Vec<_>>();
    if plan.scenarios.len() != expected_scenarios.len()
        || expected_scenarios
            .iter()
            .any(|scenario_id| !plan.scenarios.contains_key(scenario_id))
        || plan.supplemental.len() != preview.supplemental_collisions.len()
        || preview
            .supplemental_collisions
            .iter()
            .any(|identity| !plan.supplemental.contains_key(identity))
    {
        return Err(invalid());
    }
    Ok(plan)
}

fn revision(value: u64) -> Result<Revision, SafeCliError> {
    Revision::try_new(value).map_err(|_| {
        SafeCliError::validation(
            "revision.out_of_range",
            "Revision must be a non-negative JavaScript safe integer.",
        )
    })
}

fn scenario_id(input: &str) -> Result<ScenarioId, SafeCliError> {
    input.parse().map_err(|_| {
        SafeCliError::validation(
            "scenario.id_invalid",
            "The scenario identifier must be a UUIDv7.",
        )
    })
}

fn solution_id(input: &str) -> Result<SolutionId, SafeCliError> {
    input.parse().map_err(|_| {
        SafeCliError::validation(
            "solution.id_invalid",
            "The solution identifier must be a UUIDv7.",
        )
    })
}

fn operation_request_id() -> Result<RequestId, SafeCliError> {
    RequestId::new(&SystemIdGenerator).map_err(|_| {
        SafeCliError::new(
            CliExitCode::Application,
            "identity.unavailable",
            "A request identifier could not be created.",
        )
    })
}

async fn read_bounded(
    path: &Path,
    limit: u64,
    code: &'static str,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, SafeCliError> {
    files::read_controlled(
        path,
        limit,
        code,
        &eutheto_types::OperationControl::Cancellation(cancellation.clone()),
    )
    .await
}

fn parse_strict_json(bytes: &[u8]) -> Result<Value, SafeCliError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValueSeed
        .deserialize(&mut deserializer)
        .map_err(|_| {
            SafeCliError::validation(
                "json.invalid",
                "The JSON input is malformed, duplicated, or too deeply structured.",
            )
        })?;
    deserializer.end().map_err(|_| {
        SafeCliError::validation(
            "json.trailing_data",
            "The JSON input contains trailing data.",
        )
    })?;
    Ok(value)
}

struct StrictValueSeed;

impl<'de> DeserializeSeed<'de> for StrictValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E>(self, value: f64) -> Result<Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }
    fn visit_str<E>(self, value: &str) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::String(value.to_owned()))
    }
    fn visit_string<E>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(StrictValueSeed)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            let value = object.next_value_seed(StrictValueSeed)?;
            if values.insert(key.clone(), value).is_some() {
                return Err(de::Error::custom(format_args!(
                    "duplicate object key {key:?}"
                )));
            }
        }
        Ok(Value::Object(values))
    }
}

fn is_portable_artifact_error_code(code: &str) -> bool {
    code.starts_with("portable.version_")
        || code.starts_with("portable.limit.")
        || matches!(
            code,
            "portable.archive_unreadable"
                | "portable.content_invalid"
                | "portable.kind_unsupported"
                | "portable.capability_unsupported"
                | "portable.migration_registry_invalid"
                | "restore.safety_backup_failed"
                | "scenario.single_export_required"
        )
}

fn app_error(error: AppError) -> SafeCliError {
    match error {
        AppError::Validation(report) => {
            let first = report.issues.first();
            let code = first.map_or("validation.failed", |issue| issue.code.as_str());
            let message = first.map_or("The request failed validation.", |issue| {
                issue.message.as_str()
            });
            let mut error = if is_portable_artifact_error_code(code) {
                SafeCliError::storage(code, message)
            } else {
                SafeCliError::validation(code, message)
            };
            error.details = serde_json::to_value(report).ok();
            error
        }
        AppError::Conflict {
            expected_revision,
            actual_revision,
        } => {
            let mut error = SafeCliError::new(
                CliExitCode::Conflict,
                "state.revision_conflict",
                "The authoritative state changed; reload it and retry.",
            );
            error.details = Some(
                json!({"expectedRevision": expected_revision, "actualRevision": actual_revision}),
            );
            error
        }
        AppError::NotFound(_) => SafeCliError::validation(
            "resource.not_found",
            "The requested resource does not exist.",
        ),
        AppError::Unsupported(feature) => {
            let capability = feature.capability;
            let message = format!("{capability} is unavailable in this build.");
            if is_portable_artifact_error_code(&feature.code) {
                SafeCliError::storage(feature.code, message)
            } else {
                SafeCliError::unavailable(feature.code, message)
            }
        }
        AppError::Solver(failure) => failure_error(
            CliExitCode::Unavailable,
            failure.code,
            failure.message,
            failure.diagnostic_id,
        ),
        AppError::Verification(failure) => failure_error(
            CliExitCode::Verification,
            failure.code,
            failure.message,
            failure.diagnostic_id,
        ),
        AppError::Storage(failure) => failure_error(
            CliExitCode::Storage,
            failure.code,
            failure.message,
            failure.diagnostic_id,
        ),
        AppError::Protocol(failure) => {
            let exit = if failure.code == "operation.cancelled" {
                CliExitCode::Cancelled
            } else if matches!(
                failure.code.as_str(),
                "operation.deadline_exceeded" | "operation.resource_limit"
            ) {
                CliExitCode::NoVerifiedSolution
            } else if is_portable_artifact_error_code(&failure.code) {
                CliExitCode::Storage
            } else {
                CliExitCode::Application
            };
            failure_error(exit, failure.code, failure.message, failure.diagnostic_id)
        }
        AppError::Ai(failure) => failure_error(
            CliExitCode::ArtificialIntelligence,
            failure.code,
            failure.message,
            failure.diagnostic_id,
        ),
        AppError::Internal { incident_id } => {
            let mut error = SafeCliError::new(
                CliExitCode::Application,
                "application.internal",
                "The application could not complete the request.",
            );
            error.diagnostic_id = Some(incident_id.to_string());
            error
        }
    }
}

fn failure_error(
    exit: CliExitCode,
    code: String,
    message: String,
    diagnostic_id: Option<RequestId>,
) -> SafeCliError {
    let mut error = SafeCliError::new(exit, code, message);
    error.diagnostic_id = diagnostic_id.map(|id| id.to_string());
    error
}
fn unexpected_result() -> SafeCliError {
    SafeCliError::new(
        CliExitCode::Application,
        "cli.unexpected_result",
        "The application returned an unexpected result.",
    )
}

fn serialization_error() -> SafeCliError {
    SafeCliError::new(
        CliExitCode::Application,
        "cli.serialization_failed",
        "The command result could not be serialized.",
    )
}

fn to_value(value: impl serde::Serialize) -> Result<Value, SafeCliError> {
    serde_json::to_value(value).map_err(|_| serialization_error())
}

fn render_success(
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    format: OutputFormat,
    outcome: &Outcome,
) -> std::io::Result<()> {
    match format {
        OutputFormat::Human => {
            if let Some(bytes) = &outcome.raw {
                stdout.write_all(bytes)?;
            } else {
                for line in &outcome.human {
                    writeln!(stdout, "{line}")?;
                }
            }
            for warning in &outcome.warnings {
                // A diagnostic sink cannot undo the successful data/commit disposition.
                let _ = writeln!(
                    stderr,
                    "optimizer: warning: {}: {}",
                    warning.code, warning.message
                );
            }
            Ok(())
        }
        OutputFormat::Json => {
            let envelope = json!({
                "apiVersion": API_VERSION,
                "command": outcome.command,
                "ok": true,
                "status": outcome.status,
                "result": outcome.result,
                "warnings": outcome.warnings,
                "diagnosticId": Value::Null,
            });
            serde_json::to_writer(&mut *stdout, &envelope).map_err(std::io::Error::other)?;
            writeln!(stdout)
        }
    }
}

fn render_error(
    _stdout: &mut impl Write,
    stderr: &mut impl Write,
    format: OutputFormat,
    command: &'static str,
    error: &SafeCliError,
) -> std::io::Result<()> {
    match format {
        OutputFormat::Human => {
            writeln!(stderr, "optimizer: {}: {}", error.code, error.message)?;
            for detail in &error.human_details {
                writeln!(stderr, "{detail}")?;
            }
            Ok(())
        }
        OutputFormat::Json => write_error_json(stderr, command, error),
    }
}

fn write_error_json(
    output: &mut impl Write,
    command: &'static str,
    error: &SafeCliError,
) -> std::io::Result<()> {
    let envelope = json!({
        "apiVersion": API_VERSION,
        "command": command,
        "ok": false,
        "status": "error",
        "result": Value::Null,
        "warnings": [],
        "diagnosticId": error.diagnostic_id,
        "error": {
            "code": error.code,
            "message": error.message,
            "details": error.details,
        },
    });
    serde_json::to_writer(&mut *output, &envelope).map_err(std::io::Error::other)?;
    writeln!(output)
}

#[cfg(test)]
mod tests {
    use super::{
        Cli, CliExitCode, OutputFormat, RestoreModeArg, SafeCliError, app_error,
        collision_plan_value, is_safety_backup_failure, progress, render_error,
        replace_review_token, restore_authorization,
    };
    use clap::Parser;
    use eutheto_import::{
        CollisionPlan, PreviewBinding, SafetyBackupEvidence, SupplementalCollisionAction,
    };
    use eutheto_types::CancellationToken;
    use eutheto_types::{
        AppError, ProtocolFailure, RequestId, Revision, StorageFailure, ValidationIssue,
        ValidationReport, ValidationSeverity, VerificationFailure,
    };
    use std::error::Error;

    #[test]
    fn restore_review_and_receipt_tokens_are_bound_to_distinct_phases() -> Result<(), Box<dyn Error>>
    {
        let binding = PreviewBinding {
            file_sha256: "a".repeat(64),
            options_sha256: "b".repeat(64),
            local_library_revision: Revision::try_new(7)?,
            format_version: 1,
            schema_version: 1,
        };
        let plan = CollisionPlan::default();
        let Ok(review) = replace_review_token(&binding, &plan) else {
            return Err("review token generation failed".into());
        };
        let Ok(same_review) = replace_review_token(&binding, &plan) else {
            return Err("repeat review token generation failed".into());
        };
        assert_eq!(review, same_review);
        let mut stale_binding = binding;
        stale_binding.local_library_revision = Revision::try_new(8)?;
        let Ok(stale_review) = replace_review_token(&stale_binding, &plan) else {
            return Err("stale review token generation failed".into());
        };
        assert_ne!(review, stale_review);

        let Ok((first_attempt, Some(prospective_receipt))) =
            restore_authorization(RestoreModeArg::Replace, true, None)
        else {
            return Err("first replace authorization was not prospective".into());
        };
        assert_eq!(
            first_attempt.safety_backup,
            SafetyBackupEvidence::NotRequired
        );
        assert!(first_attempt.collision_plan_sha256.is_none());
        assert!(prospective_receipt.parse::<RequestId>().is_ok());

        let Ok((later_attempt, None)) = restore_authorization(
            RestoreModeArg::Replace,
            true,
            Some(prospective_receipt.clone()),
        ) else {
            return Err("later replace authorization did not use receipt proof".into());
        };
        assert!(matches!(
            later_attempt.safety_backup,
            SafetyBackupEvidence::FailedWithStrongConfirmation { proof }
                if proof == prospective_receipt
        ));
        assert!(later_attempt.prospective_failure_receipt_token.is_none());
        assert!(later_attempt.collision_plan_sha256.is_none());
        Ok(())
    }

    #[test]
    fn supplemental_collision_plans_accept_only_replace_or_skip() -> Result<(), Box<dyn Error>> {
        let create_copy = collision_plan_value(Some(
            r#"{"scenarios":{},"supplementalChoices":[{"section":"assets","key":"logo.png","action":"create-copy"}]}"#,
        ));
        let Err(error) = create_copy else {
            return Err("supplemental create-copy must be rejected".into());
        };
        assert_eq!(error.code, "collision_plan.invalid");

        let Ok(plan) = collision_plan_value(Some(
            r#"{"scenarios":{},"supplementalChoices":[{"section":"assets","key":"logo.png","action":"skip"}]}"#,
        )) else {
            return Err("supplemental skip must be accepted".into());
        };
        assert!(matches!(
            plan.supplemental.values().next(),
            Some(SupplementalCollisionAction::Skip)
        ));
        Ok(())
    }

    #[test]
    fn injected_cancellation_prevents_export_publication_and_leaves_no_temp()
    -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let data_dir = directory.path().to_string_lossy().into_owned();
        let scenario_id = "018f1e2d-3c4b-7a69-8def-012345678940";
        let destination = directory.path().join("cancelled.eutheto");
        let destination_text = destination.to_string_lossy().into_owned();
        let cli = Cli::try_parse_from([
            "optimizer",
            "--format",
            "json",
            "--data-dir",
            &data_dir,
            "projects",
            "export",
            scenario_id,
            "--output",
            &destination_text,
        ])?;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let Err((command, error)) =
            runtime.block_on(progress::run(cli, cancellation, &mut std::io::sink()))
        else {
            return Err("pre-cancelled export unexpectedly succeeded".into());
        };
        assert_eq!(command, "projects.export");
        assert_eq!(error.exit, CliExitCode::Cancelled);
        assert!(!destination.exists());
        for entry in std::fs::read_dir(directory.path())? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            assert!(!name.contains(".tmp"), "temporary file leaked: {name}");
            assert!(
                !name.contains("secret"),
                "secret-bearing file leaked: {name}"
            );
        }
        Ok(())
    }

    #[test]
    fn cancellation_is_exit_130_with_one_safe_json_or_human_error() -> Result<(), Box<dyn Error>> {
        let cancelled = app_error(AppError::Protocol(ProtocolFailure {
            code: "operation.cancelled".to_owned(),
            message: "The operation was cancelled.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        }));
        assert_eq!(cancelled.exit, CliExitCode::Cancelled);
        assert_eq!(cancelled.code, "operation.cancelled");
        assert_eq!(cancelled.message, "The operation was cancelled.");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        render_error(
            &mut stdout,
            &mut stderr,
            OutputFormat::Json,
            "projects.export",
            &cancelled,
        )?;
        assert!(stdout.is_empty());
        let rendered = String::from_utf8(stderr)?;
        assert_eq!(rendered.lines().count(), 1);
        let envelope: serde_json::Value = serde_json::from_str(&rendered)?;
        assert_eq!(envelope["error"]["code"], "operation.cancelled");
        assert_eq!(envelope["error"]["message"], "The operation was cancelled.");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        render_error(
            &mut stdout,
            &mut stderr,
            OutputFormat::Human,
            "projects.export",
            &cancelled,
        )?;
        assert!(stdout.is_empty());
        assert_eq!(
            String::from_utf8(stderr)?,
            "optimizer: operation.cancelled: The operation was cancelled.\n"
        );
        Ok(())
    }

    #[test]
    fn independent_verification_alarm_is_exit_7_on_stderr() -> Result<(), Box<dyn Error>> {
        let alarm = app_error(AppError::Verification(VerificationFailure {
            code: "solution.reverification_mismatch".to_owned(),
            message: "Fresh independent verification did not match the retained accepted result."
                .to_owned(),
            retryable: false,
            diagnostic_id: None,
        }));
        assert_eq!(alarm.exit, CliExitCode::Verification);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        render_error(
            &mut stdout,
            &mut stderr,
            OutputFormat::Json,
            "solutions.verify",
            &alarm,
        )?;
        assert!(stdout.is_empty());
        let rendered = String::from_utf8(stderr)?;
        assert_eq!(rendered.lines().count(), 1);
        let envelope: serde_json::Value = serde_json::from_str(&rendered)?;
        assert_eq!(envelope["command"], "solutions.verify");
        assert_eq!(
            envelope["error"]["code"],
            "solution.reverification_mismatch"
        );
        Ok(())
    }

    #[test]
    fn safety_backup_failure_detection_is_exact_and_typed() {
        let safety_failure = AppError::Protocol(ProtocolFailure {
            code: "restore.safety_backup_failed".to_owned(),
            message: "The automatic safety backup failed.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        });
        assert!(is_safety_backup_failure(&safety_failure));
        assert!(!is_safety_backup_failure(&AppError::Storage(
            StorageFailure {
                code: "restore.safety_backup_failed".to_owned(),
                message: "Storage failed.".to_owned(),
                retryable: false,
                diagnostic_id: None,
            }
        )));
    }

    #[test]
    fn human_errors_print_follow_up_tokens_and_preview_scope_on_stderr()
    -> Result<(), Box<dyn Error>> {
        let mut error = SafeCliError::storage(
            "restore.safety_backup_failed",
            "The automatic safety backup failed.",
        );
        error.human_details = vec![
            "Safety-backup failure receipt: bound-token".to_owned(),
            "Unchanged preview:".to_owned(),
            "Previewed library revision: 7.".to_owned(),
        ];
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        render_error(
            &mut stdout,
            &mut stderr,
            OutputFormat::Human,
            "backup.restore",
            &error,
        )?;
        assert!(stdout.is_empty());
        let rendered = String::from_utf8(stderr)?;
        assert!(rendered.contains("Safety-backup failure receipt: bound-token"));
        assert!(rendered.contains("Previewed library revision: 7."));
        Ok(())
    }

    #[test]
    fn portable_artifact_failures_use_storage_exit_without_reclassifying_commands() {
        let issue = |code: &str| ValidationIssue {
            code: code.to_owned(),
            severity: ValidationSeverity::Error,
            message: "Safe validation message.".to_owned(),
            field_path: Some("/bundle".to_owned()),
            resource: None,
        };
        let portable = app_error(AppError::Validation(ValidationReport {
            issues: vec![issue("portable.version_newer")],
        }));
        assert_eq!(portable.exit, CliExitCode::Storage);
        let command = app_error(AppError::Validation(ValidationReport {
            issues: vec![issue("scenario.title_invalid")],
        }));
        assert_eq!(command.exit, CliExitCode::Validation);
        let migration = app_error(AppError::Protocol(ProtocolFailure {
            code: "portable.migration_registry_invalid".to_owned(),
            message: "Migration registry invalid.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        }));
        assert_eq!(migration.exit, CliExitCode::Storage);
        let safety_backup = app_error(AppError::Protocol(ProtocolFailure {
            code: "restore.safety_backup_failed".to_owned(),
            message: "The automatic safety backup failed.".to_owned(),
            retryable: false,
            diagnostic_id: None,
        }));
        assert_eq!(safety_backup.exit, CliExitCode::Storage);
    }
}

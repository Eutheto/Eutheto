//! File/ID dispatch is syntactic and precedes every application-directory access.

use super::{
    ApplyArgs, CliExitCode, CreateProjectArgs, Outcome, SafeCliError, ScenarioArgs,
    ScenarioCommandArgs, app_error, command_envelope, create_settings, execute_scenario, files,
    open_app, parse_strict_json, to_value, unexpected_result,
};
use eutheto_core::HeadlessService;
use eutheto_export::PORTABLE_LIMITS;
use eutheto_import::InspectedBundle;
use eutheto_types::{
    CancellationToken, DomainPackRef, OperationControl, Revision, ScenarioSnapshotV1, SystemClock,
    SystemIdGenerator, SystemMonotonicClock, ValidationIssue, ValidationSeverity,
};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) enum LoadedScenario {
    Standalone(ScenarioSnapshotV1),
    Bundle(InspectedBundle),
}

impl LoadedScenario {
    pub(super) fn snapshot(&self) -> Result<&ScenarioSnapshotV1, SafeCliError> {
        match self {
            Self::Standalone(snapshot) => Ok(snapshot),
            Self::Bundle(bundle) => bundle.scenarios.first().ok_or_else(unexpected_result),
        }
    }
}

pub(super) fn service() -> Result<HeadlessService, SafeCliError> {
    HeadlessService::new(
        Arc::new(SystemClock),
        Arc::new(SystemMonotonicClock::new()),
        Arc::new(SystemIdGenerator),
    )
    .map_err(app_error)
}

pub(super) async fn load(
    service: &HeadlessService,
    path: &Path,
    allow_bundle: bool,
    control: &OperationControl,
) -> Result<LoadedScenario, SafeCliError> {
    let limit = if allow_bundle {
        PORTABLE_LIMITS.max_archive_bytes
    } else {
        PORTABLE_LIMITS.max_json_bytes
    };
    let bytes = files::read_controlled(path, limit, "scenario.input_invalid", control).await?;
    if bytes.starts_with(b"PK\x03\x04") {
        if !allow_bundle {
            return Err(SafeCliError::unavailable(
                "scenario.bundle_mutation_unavailable",
                "File commands require standalone portable JSON; import a bundle for persisted editing.",
            ));
        }
        service
            .decode_scenario_bundle(&bytes, control)
            .map(LoadedScenario::Bundle)
            .map_err(app_error)
    } else {
        service
            .decode_scenario(&bytes, control)
            .map(|inspected| LoadedScenario::Standalone(inspected.scenario))
            .map_err(app_error)
    }
}

pub(super) fn reject_config(has_config: bool) -> Result<(), SafeCliError> {
    if has_config {
        Err(SafeCliError::unavailable(
            "cli.config_unavailable",
            "Configuration files are unavailable for this operation; pass its supported options explicitly.",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn create(
    args: CreateProjectArgs,
    cancellation: &CancellationToken,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    const COMMAND: &str = "projects.create";
    let output = args
        .output
        .as_ref()
        .ok_or_else(|| (COMMAND, unexpected_result()))?;
    let settings = create_settings(&args).map_err(|error| (COMMAND, error))?;
    let pack = args.pack.parse().map_err(|_| {
        (
            COMMAND,
            SafeCliError::validation("pack.id_invalid", "The domain-pack identifier is invalid."),
        )
    })?;
    let control = OperationControl::Cancellation(cancellation.clone());
    let service = service().map_err(|error| (COMMAND, error))?;
    let snapshot = service
        .create_scenario(
            args.title,
            args.description,
            DomainPackRef {
                id: pack,
                schema_version: args.pack_schema,
            },
            settings,
            &control,
        )
        .map_err(|error| (COMMAND, app_error(error)))?;
    let bytes = service
        .encode_scenario(&snapshot, &control)
        .map_err(|error| (COMMAND, app_error(error)))?;
    files::publish_text(output, &bytes, &control).map_err(|error| (COMMAND, error))?;
    Ok(Outcome::new(
        COMMAND,
        "created",
        json!({
            "scenarioId": snapshot.document.scenario_id,
            "revision": snapshot.revision,
            "output": output,
        }),
        vec![format!(
            "Created standalone scenario {} (revision {}).",
            snapshot.document.scenario_id,
            snapshot.revision.value()
        )],
    ))
}

pub(super) async fn execute(
    data_dir: Option<PathBuf>,
    args: ScenarioArgs,
    cancellation: &CancellationToken,
    has_config: bool,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let (command, input, file_supported) = match &args.command {
        ScenarioCommandArgs::Show { input } => ("scenario.show", input, true),
        ScenarioCommandArgs::Validate { input } => ("scenario.validate", input, true),
        ScenarioCommandArgs::Apply(args) => ("scenario.apply", &args.input, true),
        ScenarioCommandArgs::Batch(args) => ("scenario.batch", &args.input, true),
        ScenarioCommandArgs::Undo { scenario_id, .. } => ("scenario.undo", scenario_id, false),
        ScenarioCommandArgs::Redo { scenario_id, .. } => ("scenario.redo", scenario_id, false),
        ScenarioCommandArgs::History { scenario_id } => ("scenario.history", scenario_id, false),
        ScenarioCommandArgs::Migrate { .. } => {
            return Err((
                "scenario.migrate",
                SafeCliError::unavailable(
                    "scenario.migrate_unavailable",
                    "Standalone migration commands are unavailable; checked loading performs supported portable migrations.",
                ),
            ));
        }
    };
    if files::scenario_id_or_file(Path::new(input))
        .map_err(|error| (command, error))?
        .is_some()
    {
        let app = open_app(data_dir, cancellation)
            .await
            .map_err(|error| (command, error))?;
        return execute_scenario(&app, args, cancellation).await;
    }
    if !file_supported {
        return Err((
            command,
            SafeCliError::unavailable(
                "scenario.stored_history_required",
                "History operations require a stored scenario identifier.",
            ),
        ));
    }
    reject_config(has_config).map_err(|error| (command, error))?;
    let control = OperationControl::Cancellation(cancellation.clone());
    let service = service().map_err(|error| (command, error))?;
    match args.command {
        ScenarioCommandArgs::Show { input } => {
            let loaded = load(&service, Path::new(&input), true, &control)
                .await
                .map_err(|error| (command, error))?;
            let snapshot = loaded.snapshot().map_err(|error| (command, error))?;
            let human = serde_json::to_string_pretty(snapshot)
                .map_err(|_| (command, super::serialization_error()))?;
            Ok(Outcome::new(
                command,
                "ok",
                to_value(snapshot).map_err(|error| (command, error))?,
                vec![human],
            ))
        }
        ScenarioCommandArgs::Validate { input } => {
            let loaded = load(&service, Path::new(&input), true, &control)
                .await
                .map_err(|error| (command, error))?;
            let snapshot = loaded.snapshot().map_err(|error| (command, error))?;
            let report = service
                .validate_full(snapshot, &control)
                .map_err(|error| (command, app_error(error)))?;
            Ok(validation_outcome(snapshot.revision, &report.issues))
        }
        ScenarioCommandArgs::Apply(args) => apply(&service, args, false, cancellation).await,
        ScenarioCommandArgs::Batch(args) => apply(&service, args, true, cancellation).await,
        _ => Err((command, unexpected_result())),
    }
}

async fn apply(
    service: &HeadlessService,
    args: ApplyArgs,
    require_batch: bool,
    cancellation: &CancellationToken,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let command = if require_batch {
        "scenario.batch"
    } else {
        "scenario.apply"
    };
    let output = args.output.as_ref().ok_or_else(|| {
        (
            command,
            SafeCliError::new(
                CliExitCode::Usage,
                "scenario.output_required",
                "File commands require an explicit --output destination.",
            ),
        )
    })?;
    if args.truncate_redo {
        return Err((
            command,
            SafeCliError::unavailable(
                "scenario.file_history_unavailable",
                "Standalone files do not have a redo journal to truncate.",
            ),
        ));
    }
    let control = OperationControl::Cancellation(cancellation.clone());
    let loaded = load(service, Path::new(&args.input), false, &control)
        .await
        .map_err(|error| (command, error))?;
    let LoadedScenario::Standalone(snapshot) = loaded else {
        return Err((command, unexpected_result()));
    };
    let bytes = files::read_controlled(
        &args.commands,
        super::COMMAND_JSON_LIMIT,
        "commands.too_large",
        &control,
    )
    .await
    .map_err(|error| (command, error))?;
    let value = parse_strict_json(&bytes).map_err(|error| (command, error))?;
    let envelope = command_envelope(
        snapshot.document.scenario_id,
        args.expected_revision,
        value,
        require_batch,
    )
    .map_err(|error| (command, error))?;
    let applied = service
        .apply_scenario(snapshot, &envelope, cancellation)
        .map_err(|error| (command, app_error(error)))?;
    let encoded = service
        .encode_scenario(&applied.snapshot, &control)
        .map_err(|error| (command, app_error(error)))?;
    files::publish_text(output, &encoded, &control).map_err(|error| (command, error))?;
    let revision = applied.result.new_revision.value();
    Ok(Outcome::new(
        command,
        "applied",
        to_value(applied.result).map_err(|error| (command, error))?,
        vec![format!("Applied command; published revision {revision}.")],
    ))
}

pub(super) fn validation_outcome(revision: Revision, issues: &[ValidationIssue]) -> Outcome {
    let invalid = issues
        .iter()
        .any(|issue| issue.severity == ValidationSeverity::Error);
    let human = if issues.is_empty() {
        vec!["Scenario is valid.".to_owned()]
    } else {
        issues
            .iter()
            .map(|issue| format!("{:?}: {}: {}", issue.severity, issue.code, issue.message))
            .collect()
    };
    Outcome::new(
        "scenario.validate",
        if invalid { "invalid" } else { "valid" },
        json!({ "revision": revision, "issues": issues }),
        human,
    )
    .with_exit(if invalid {
        CliExitCode::Validation
    } else {
        CliExitCode::Success
    })
}

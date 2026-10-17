//! File results are untrusted historical records until freshly verified against their source.

use super::{
    CliExitCode, ExportSolutionArgs, Outcome, OutputFormat, SafeCliError, SafeCliWarning,
    SolutionExportFormat, SolutionsArgs, SolutionsCommand, app_error, execute_solution_compare,
    execute_solution_explain, execute_solution_list, execute_solution_verify, files, open_app,
    scenario_files,
};
use eutheto_core::{AppQuery, AppQueryResult, RecipientReportRequestV1};
use eutheto_domain_ir::{DomainAssignmentId, PortableAcceptedResult};
use eutheto_export::PORTABLE_LIMITS;
use eutheto_types::{
    CancellationToken, DurationMillis, OperationControl, ParentSolveBudget, PersonId,
    RecipientOriginV1, RecipientReportOptionsV1, RecipientSourceV1, Revision, ScenarioId,
    SolutionId, SystemMonotonicClock,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) async fn execute(
    data_dir: Option<PathBuf>,
    args: SolutionsArgs,
    cancellation: &CancellationToken,
    output_format: OutputFormat,
    has_config: bool,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    match args.command {
        SolutionsCommand::List { scenario } => {
            let id = files::scenario_id_or_file(Path::new(&scenario))
                .map_err(|error| ("solutions.list", error))?
                .ok_or_else(|| {
                    (
                        "solutions.list",
                        SafeCliError::unavailable(
                            "solution.stored_list_required",
                            "Listing retained solutions requires a stored scenario identifier.",
                        ),
                    )
                })?;
            let app = open_app(data_dir, cancellation)
                .await
                .map_err(|error| ("solutions.list", error))?;
            execute_solution_list(&app, id).await
        }
        SolutionsCommand::Verify { scenario, solution } => {
            verify(data_dir, &scenario, &solution, cancellation, has_config)
                .await
                .map_err(|error| ("solutions.verify", error))
        }
        SolutionsCommand::Explain {
            scenario,
            solution,
            assignment,
        } => explain(
            data_dir,
            &scenario,
            &solution,
            &assignment,
            cancellation,
            has_config,
        )
        .await
        .map_err(|error| ("solutions.explain", error)),
        SolutionsCommand::Export(args) => {
            export(data_dir, args, cancellation, output_format, has_config)
                .await
                .map_err(|error| ("solutions.export", error))
        }
        SolutionsCommand::Compare {
            scenario,
            solution_a,
            solution_b,
        } => {
            let left = paired_ids(Path::new(&scenario), Path::new(&solution_a))
                .map_err(|error| ("solutions.compare", error))?;
            let right = paired_ids(Path::new(&scenario), Path::new(&solution_b))
                .map_err(|error| ("solutions.compare", error))?;
            if left.is_none() || right.is_none() {
                return Err((
                    "solutions.compare",
                    SafeCliError::unavailable(
                        "solution.file_comparison_unavailable",
                        "Comparison currently requires retained stored solutions.",
                    ),
                ));
            }
            let app = open_app(data_dir, cancellation)
                .await
                .map_err(|error| ("solutions.compare", error))?;
            execute_solution_compare(&app, &scenario, &solution_a, &solution_b).await
        }
    }
}

fn paired_ids(
    scenario: &Path,
    solution: &Path,
) -> Result<Option<(ScenarioId, SolutionId)>, SafeCliError> {
    match (
        files::scenario_id_or_file(scenario)?,
        files::solution_id_or_file(solution)?,
    ) {
        (Some(scenario), Some(solution)) => Ok(Some((scenario, solution))),
        (None, None) => Ok(None),
        _ => Err(SafeCliError::new(
            CliExitCode::Usage,
            "solution.mixed_input_modes",
            "Use two stored identifiers or two explicit files; mixed inputs are unavailable.",
        )),
    }
}

async fn load_result(
    path: &Path,
    control: &OperationControl,
) -> Result<PortableAcceptedResult, SafeCliError> {
    let bytes = files::read_controlled(
        path,
        PORTABLE_LIMITS.max_json_bytes,
        "solution.input_invalid",
        control,
    )
    .await?;
    let portable = PortableAcceptedResult::from_json(&bytes).map_err(|_| SafeCliError::storage(
        "solution.portable_invalid", "The accepted-result file is malformed, incompatible, oversized or internally inconsistent.",
    ))?;
    files::check_control(control)?;
    Ok(portable)
}

async fn verify(
    data_dir: Option<PathBuf>,
    scenario: &str,
    solution: &str,
    cancellation: &CancellationToken,
    has_config: bool,
) -> Result<Outcome, SafeCliError> {
    if let Some((scenario_id, solution_id)) = paired_ids(Path::new(scenario), Path::new(solution))?
    {
        let app = open_app(data_dir, cancellation).await?;
        return execute_solution_verify(&app, scenario_id, solution_id)
            .await
            .map_err(|(_, error)| error);
    }
    scenario_files::reject_config(has_config)?;
    let control = OperationControl::Cancellation(cancellation.clone());
    let service = scenario_files::service()?;
    let loaded = scenario_files::load(&service, Path::new(scenario), true, &control).await?;
    let snapshot = loaded.snapshot()?;
    let portable = load_result(Path::new(solution), &control).await?;
    let verification = service
        .verify_result(snapshot, &portable, &control)
        .map_err(app_error)?;
    Ok(Outcome::new("solutions.verify", "verified", json!({
        "resultId": portable.result_id, "scenarioRevision": snapshot.revision,
        "verification": verification, "historicalRunMetadata": "sourceProvidedUnverified",
    }), vec!["The supplied schedule and score were freshly independently verified. External solver history, optimality and timings remain unverified.".to_owned()]))
}

async fn explain(
    data_dir: Option<PathBuf>,
    scenario: &str,
    solution: &str,
    assignment: &str,
    cancellation: &CancellationToken,
    has_config: bool,
) -> Result<Outcome, SafeCliError> {
    let ids = paired_ids(Path::new(scenario), Path::new(solution))?;
    let assignment_id = assignment.parse::<DomainAssignmentId>().map_err(|_| {
        SafeCliError::validation(
            "solution.explanation_request_invalid",
            "The explanation request contains an invalid assignment identifier.",
        )
    })?;
    if let Some((scenario_id, solution_id)) = ids {
        let app = open_app(data_dir, cancellation).await?;
        return execute_solution_explain(&app, scenario_id, solution_id, assignment_id)
            .await
            .map_err(|(_, error)| error);
    }
    scenario_files::reject_config(has_config)?;
    let control = OperationControl::Cancellation(cancellation.clone());
    let service = scenario_files::service()?;
    let loaded = scenario_files::load(&service, Path::new(scenario), true, &control).await?;
    let portable = load_result(Path::new(solution), &control).await?;
    let explanation = service
        .explain_assignment(loaded.snapshot()?, &portable, &assignment_id, &control)
        .map_err(app_error)?;
    let human = explanation_human(&explanation.rendered)?;
    Ok(Outcome::new(
        "solutions.explain",
        "explained",
        json!({
            "resultId": portable.result_id, "explanation": explanation,
            "historicalRunMetadata": "sourceProvidedUnverified",
        }),
        human,
    )
    .with_warning(external_history_warning()))
}
#[allow(clippy::too_many_lines)]
async fn export(
    data_dir: Option<PathBuf>,
    args: ExportSolutionArgs,
    cancellation: &CancellationToken,
    output_format: OutputFormat,
    has_config: bool,
) -> Result<Outcome, SafeCliError> {
    let report_options = RecipientReportOptionsV1 {
        include_person_names: args.include_person_names,
        include_authored_labels: args.include_authored_labels,
        include_staffing: args.include_staffing,
        include_changes: args.include_changes,
        include_fairness_actuals: args.include_fairness_actuals,
    };
    if args.format != SolutionExportFormat::Html
        && report_options != RecipientReportOptionsV1::default()
    {
        return Err(SafeCliError::new(
            CliExitCode::Usage,
            "solution.report_options_invalid",
            "Report disclosure switches are supported only with --format html.",
        ));
    }
    let format = match args.format {
        SolutionExportFormat::Csv => "csv",
        SolutionExportFormat::SummaryCsv => "summary-csv",
        SolutionExportFormat::Ics => "ics",
        SolutionExportFormat::Json => "json",
        SolutionExportFormat::Html => "html",
        _ => {
            return Err(SafeCliError::unavailable(
                "solution.export_format_unavailable",
                "This export format is unavailable; use JSON, assignment CSV, summary CSV, Workforce iCalendar or recipient HTML.",
            ));
        }
    };
    if args.person_id.is_some() && args.format != SolutionExportFormat::Ics {
        return Err(SafeCliError::new(
            CliExitCode::Usage,
            "solution.person_filter_invalid",
            "--person-id is supported only with --format ics.",
        ));
    }
    let person_filter = args
        .person_id
        .as_deref()
        .map(str::parse::<PersonId>)
        .transpose()
        .map_err(|_| {
            SafeCliError::validation(
                "solution.person_id_invalid",
                "The Workforce person filter is not a valid stable person UUID.",
            )
        })?;
    if output_format == OutputFormat::Json && args.output.is_none() {
        return Err(SafeCliError::new(
            CliExitCode::Usage,
            "solution.export_output_required",
            "JSON result-envelope mode requires --output; raw data cannot share stdout with the envelope.",
        ));
    }
    if args.format == SolutionExportFormat::Html {
        return recipient_html(data_dir, args, report_options, cancellation, has_config).await;
    }
    let ids = paired_ids(&args.scenario, &args.solution)?;
    let control = OperationControl::Cancellation(cancellation.clone());
    let mut warnings = Vec::new();
    let (bytes, metadata) = if let Some((scenario_id, solution_id)) = ids {
        let app = open_app(data_dir, cancellation).await?;
        let exported = match args.format {
            SolutionExportFormat::Csv => {
                app.export_solution_assignments_csv(scenario_id, solution_id)
                    .await
            }
            SolutionExportFormat::SummaryCsv => {
                app.export_solution_summary_csv(scenario_id, solution_id)
                    .await
            }
            SolutionExportFormat::Ics => {
                app.export_solution_ics(scenario_id, solution_id, person_filter)
                    .await
            }
            SolutionExportFormat::Json => app.export_solution_json(scenario_id, solution_id).await,
            _ => return Err(super::unexpected_result()),
        }
        .map_err(app_error)?;
        // The core held the mutation boundary through read/verification/encoding. This is
        // deliberately a revision-as-of observation; publication occurs after the lock releases.
        let stale_at_read = exported.scenario_revision != exported.current_revision;
        if stale_at_read && !args.allow_stale {
            return Err(stale_confirmation_error(
                exported.scenario_revision,
                exported.current_revision,
            ));
        }
        if stale_at_read
            && let Some(warning) =
                stale_warning(exported.scenario_revision, exported.current_revision)
        {
            warnings.push(warning);
        }
        (
            exported.bytes,
            json!({
                "scenarioId": scenario_id, "resultId": solution_id,
                "scenarioRevision": exported.scenario_revision,
                "currentRevision": exported.current_revision,
                "revisionAsOf": exported.current_revision,
                "staleAtRevisionRead": stale_at_read,
                "format": format, "output": args.output,
                "personId": person_filter.map(|id| id.to_string()),
            }),
        )
    } else {
        scenario_files::reject_config(has_config)?;
        let service = scenario_files::service()?;
        let loaded = scenario_files::load(&service, &args.scenario, true, &control).await?;
        let snapshot = loaded.snapshot()?;
        let portable = load_result(&args.solution, &control).await?;
        let bytes = match args.format {
            SolutionExportFormat::Csv => {
                service.export_assignments_csv(snapshot, &portable, &control)
            }
            SolutionExportFormat::SummaryCsv => {
                service.export_result_summary_csv(snapshot, &portable, &control)
            }
            SolutionExportFormat::Ics => {
                service.export_result_ics(snapshot, &portable, person_filter, &control)
            }
            SolutionExportFormat::Json => service.export_result_json(snapshot, &portable, &control),
            _ => return Err(super::unexpected_result()),
        }
        .map_err(app_error)?;
        warnings.push(external_history_warning());
        (
            bytes,
            json!({
                "scenarioId": snapshot.document.scenario_id, "resultId": portable.result_id,
                "scenarioRevision": snapshot.revision, "currentRevision": snapshot.revision,
                "revisionAsOf": snapshot.revision,
                "staleAtRevisionRead": false,
                "format": format, "output": args.output,
                "personId": person_filter.map(|id| id.to_string()),
                "historicalRunMetadata": "sourceProvidedUnverified",
            }),
        )
    };
    let mut outcome = export_output(bytes, metadata, args.output.as_deref(), &control)?;
    outcome.warnings.extend(warnings);
    Ok(outcome)
}

async fn recipient_html(
    data_dir: Option<PathBuf>,
    args: ExportSolutionArgs,
    options: RecipientReportOptionsV1,
    cancellation: &CancellationToken,
    has_config: bool,
) -> Result<Outcome, SafeCliError> {
    let output = args.output.ok_or_else(|| {
        SafeCliError::new(
            CliExitCode::Usage,
            "solution.export_output_required",
            "Recipient HTML requires --output for atomic no-clobber publication.",
        )
    })?;
    let budget = ParentSolveBudget::new(
        DurationMillis::new(30_000).map_err(|_| super::unexpected_result())?,
        Arc::new(SystemMonotonicClock::new()),
        cancellation.clone(),
    )
    .map_err(|_| {
        SafeCliError::new(
            CliExitCode::Application,
            "operation.clock_invalid",
            "The operation timing cannot be represented safely.",
        )
    })?;
    let control = OperationControl::Solve(budget.phase_view());
    if let Some((scenario_id, solution_id)) = paired_ids(&args.scenario, &args.solution)? {
        let app = recipient_source_wait(open_app(data_dir, cancellation), &control).await??;
        let metadata =
            recipient_source_wait(app.query(AppQuery::ProjectMetadata(scenario_id)), &control)
                .await?
                .map_err(app_error)?;
        let expected_revision = match metadata {
            AppQueryResult::Project(project) => project.revision,
            _ => return Err(super::unexpected_result()),
        };
        files::check_control(&control)?;
        // Capture and publication own their alarm/commit settlement. Do not timeout them or
        // replace their result with a post-await cancellation check.
        let (capture, _) = app
            .prepare_recipient_report(
                RecipientReportRequestV1 {
                    schema_version: 1,
                    scenario_id,
                    solution_id,
                    expected_revision,
                    options,
                },
                control.clone(),
            )
            .await
            .map_err(app_error)?;
        let outcome = recipient_outcome(
            &capture.metadata().source,
            options,
            &output,
            capture.model_sha256(),
            capture.html_sha256(),
        );
        capture
            .publish_html(output, args.allow_stale, control)
            .await
            .map_err(app_error)?;
        Ok(outcome)
    } else {
        scenario_files::reject_config(has_config)?;
        files::check_control(&control)?;
        let service = scenario_files::service()?;
        let loaded = recipient_source_wait(
            scenario_files::load(&service, &args.scenario, true, &control),
            &control,
        )
        .await??;
        let portable =
            recipient_source_wait(load_result(&args.solution, &control), &control).await??;
        let (encoded, report) = service
            .export_recipient_html(loaded.snapshot()?, &portable, options, &control)
            .map_err(app_error)?;
        let outcome = recipient_outcome(
            &report.source,
            options,
            &output,
            encoded.model_sha256(),
            encoded.html_sha256(),
        );
        drop(report);
        drop(portable);
        drop(loaded);
        encoded
            .prepare_destination(&output, &control)
            .map_err(|error| files::publication_error(&error, &control))?
            .publish_controlled(&control)
            .map_err(|error| files::publication_error(&error, &control))?;
        Ok(outcome)
    }
}

// Only pre-capture source reads belong here, never native capture/publication settlement.
fn recipient_source_wait<F: Future>(
    future: F,
    control: &OperationControl,
) -> impl Future<Output = Result<F::Output, SafeCliError>> {
    // Box before constructing the wait future so large source-read frames do not multiply
    // through the caller's async state.
    let mut future = Box::pin(future);
    async move {
        loop {
            files::check_control(control)?;
            tokio::select! {
                result = &mut future => {
                    files::check_control(control)?;
                    return Ok(result);
                }
                () = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
            }
        }
    }
}

fn recipient_outcome(
    source: &RecipientSourceV1,
    options: RecipientReportOptionsV1,
    output: &Path,
    model_sha256: &str,
    html_sha256: &str,
) -> Outcome {
    let mut outcome = Outcome::new(
        "solutions.export",
        "exported",
        json!({
            "scenarioId": source.scenario_id,
            "resultId": source.solution_id,
            "scenarioRevision": source.scenario_revision,
            "currentRevision": source.observed_current_revision,
            "revisionAsOf": source.observed_current_revision,
            "staleAtRevisionRead": source.observed_current_revision
                .map(|head| head != source.scenario_revision),
            "format": "html",
            "output": output,
            "source": source,
            "privacy": options,
            "modelSha256": model_sha256,
            "htmlSha256": html_sha256,
        }),
        vec!["Published freshly verified recipient HTML.".to_owned()],
    );
    outcome.warnings.push(SafeCliWarning {
        code: "solution.recipient_linkability".to_owned(),
        message: "Generated aliases are not anonymity: stable identities and schedule patterns can link people across reports.".to_owned(),
        details: None,
    });
    if source.origin == RecipientOriginV1::VerifiedFile {
        outcome.result["historicalRunMetadata"] = json!("sourceProvidedUnverified");
        outcome.warnings.push(external_history_warning());
    } else if let Some(current) = source.observed_current_revision
        && current != source.scenario_revision
    {
        outcome.warnings.push(SafeCliWarning {
            code: "solution.stale".to_owned(),
            message: format!(
                "The shared plan belongs to revision {}; revision {} was current at publication. The report is explicitly labeled as an older plan.",
                source.scenario_revision.value(), current.value(),
            ),
            details: Some(json!({
                "scenarioRevision": source.scenario_revision,
                "currentRevision": current,
                "staleAtRevisionRead": true,
            })),
        });
    }
    outcome
}

fn export_output(
    bytes: Vec<u8>,
    metadata: Value,
    output: Option<&Path>,
    control: &OperationControl,
) -> Result<Outcome, SafeCliError> {
    let outcome = Outcome::new(
        "solutions.export",
        "exported",
        metadata,
        vec!["Published freshly verified result export.".to_owned()],
    );
    if let Some(output) = output {
        files::publish_text(output, &bytes, control)?;
        Ok(outcome)
    } else {
        files::check_control(control)?;
        Ok(outcome.with_raw(bytes))
    }
}

pub(super) fn explanation_human(
    rendered: &eutheto_domain_ir::EvidenceRenderResultV1,
) -> Result<Vec<String>, SafeCliError> {
    Ok(vec![
        serde_json::to_string_pretty(rendered).map_err(|_| super::serialization_error())?,
    ])
}

fn external_history_warning() -> SafeCliWarning {
    SafeCliWarning {
        code: "solution.external_history_unverified".to_owned(),
        message: "The schedule was freshly independently verified; source-provided solver history, optimality and timings remain unverified.".to_owned(),
        details: None,
    }
}

fn stale_confirmation_error(solved: Revision, current: Revision) -> SafeCliError {
    let mut error = SafeCliError::validation(
        "solution.stale_confirmation_required",
        format!(
            "WARNING: this accepted result belongs to revision {}, while revision {} was current when it was read and verified. No output was published; re-run with --allow-stale to authorize an explicitly stale export.",
            solved.value(),
            current.value(),
        ),
    );
    error.details = Some(json!({
        "warningCode": "solution.stale",
        "scenarioRevision": solved,
        "currentRevision": current,
        "revisionAsOf": current,
        "staleAtRevisionRead": true,
        "requires": "--allow-stale",
    }));
    error
}

pub(super) fn stale_warning(solved: Revision, current: Revision) -> Option<SafeCliWarning> {
    (solved != current).then(|| SafeCliWarning {
        code: "solution.stale".to_owned(),
        message: format!(
            "This result belongs to revision {}; revision {} was current when it was read and independently verified. The acknowledged export is labeled stale as of that revision read; a concurrent edit may occur before publication.",
            solved.value(), current.value(),
        ),
        details: Some(json!({
            "scenarioRevision": solved,
            "currentRevision": current,
            "revisionAsOf": current,
            "staleAtRevisionRead": true,
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recipient_source_wait_charges_elapsed_time_and_interrupts_pending_reads()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let input = directory.path().join("source.json");
        std::fs::write(&input, b"{}")?;
        let clock = Arc::new(eutheto_types::FixedMonotonicClock::default());
        let budget = ParentSolveBudget::new(
            DurationMillis::new(30_000)?,
            clock.clone(),
            CancellationToken::new(),
        )?;
        let control = OperationControl::Solve(budget.phase_view());
        clock.advance(std::time::Duration::from_secs(29))?;
        let bytes = recipient_source_wait(
            files::read_controlled(&input, 2, "test.input_too_large", &control),
            &control,
        )
        .await
        .and_then(std::convert::identity)
        .map_err(|error| format!("{error:?}"))?;
        assert_eq!(bytes, b"{}");
        assert_eq!(
            budget
                .phase_view()
                .snapshot()
                .remaining_milliseconds
                .value(),
            1000
        );
        let wait = recipient_source_wait(std::future::pending::<()>(), &control);
        let expire = async {
            tokio::task::yield_now().await;
            clock.advance(std::time::Duration::from_secs(1))
        };
        let (result, advanced) = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::join!(wait, expire)
        })
        .await?;
        advanced?;
        assert_eq!(
            result.err().ok_or("pending read ignored deadline")?.code,
            "operation.deadline_exceeded"
        );
        Ok(())
    }
}

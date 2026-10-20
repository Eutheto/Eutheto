use super::*;
use eutheto_core::{
    CapturedScheduleExport, MAX_SCHEDULE_EXPORT_SAMPLE_BYTES, RecipientReportRequestV1,
    ScheduleExportFormatV1, ScheduleExportPreviewV1, ScheduleExportRequestV1,
};
use eutheto_types::{
    FixedMonotonicClock, MonotonicClock, ParentSolveBudget, RecipientChangeBasisV1,
    RecipientContentV1, RecipientHistoricalEndpointV1, RecipientLabelV1, RecipientOriginV1,
    RecipientOriginalAdmissionV1, RecipientPairChangeV1, RecipientReportOptionsV1,
};

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_recipient_capture_is_private_immutable_and_revision_leased() -> TestResult {
    let mut source = snapshot(1)?;
    source.document.metadata.title = "PRIVATE-AUTHORED-TITLE".to_owned();
    source.document.metadata.description = "PRIVATE-DESCRIPTION".to_owned();
    let scenario_id = source.document.scenario_id;
    let (directory, store, outcome, _, app) =
        stored_real_run(source, ExplanationMode::None).await?;
    let solution_id = outcome
        .accepted_result_id()
        .ok_or("missing accepted result")?;
    let control = OperationControl::Cancellation(CancellationToken::new());
    let mut request = RecipientReportRequestV1 {
        schema_version: 1,
        scenario_id,
        solution_id,
        expected_revision: Revision::INITIAL,
        options: RecipientReportOptionsV1::default(),
    };
    let preview_token = CancellationToken::new();
    let (capture, report) = app
        .prepare_recipient_report(
            request.clone(),
            OperationControl::Cancellation(preview_token.clone()),
        )
        .await
        .map_err(boxed)?;
    let RecipientContentV1::Workforce(content) = &report.content;
    assert_eq!(content.assignments.len(), 1);
    let html = capture.html_bytes().to_vec();
    let text = std::str::from_utf8(&html)?;
    assert!(!text.contains("PRIVATE-AUTHORED-TITLE"));
    assert!(!text.contains("PRIVATE-DESCRIPTION"));
    preview_token.cancel();
    let destination = directory.path().join("recipient.html");
    let receipt = capture
        .publish_html(destination.clone(), false, control.clone())
        .await
        .map_err(boxed)?;
    assert_eq!(std::fs::read(&destination)?, html);
    assert_eq!(receipt.solution_id, solution_id);
    assert_eq!(receipt.source_revision, Revision::INITIAL);

    let (collision, _) = app
        .prepare_recipient_report(request.clone(), control.clone())
        .await
        .map_err(boxed)?;
    assert!(
        collision
            .publish_html(destination.clone(), false, control.clone())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&destination)?, html);
    assert_recipient_budget(&app, &request, directory.path()).await?;
    let (obsolete, _) = app
        .prepare_recipient_report(request.clone(), control.clone())
        .await
        .map_err(boxed)?;
    let changed = serde_json::to_value(snapshot(2)?.document)?;
    let edit = app
        .execute(AppCommand::ApplyScenario {
            request_id: RequestId::new(&SystemIdGenerator)?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Recipient regression".to_owned(),
                },
                source: CommandSource::System,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: eutheto_workforce::commands::UPDATE_ENTITY.to_owned(),
                    payload: json!({"entity": changed["domain"]["entities"][id(8)]}),
                }),
            },
            truncate_redo: false,
        })
        .await
        .map_err(boxed)?;
    let AppCommandResult::ScenarioCommand(edit) = edit else {
        return Err("missing committed edit".into());
    };
    let refused = directory.path().join("obsolete.html");
    assert!(
        obsolete
            .publish_html(refused.clone(), true, control.clone())
            .await
            .is_err()
    );
    assert!(!refused.exists());
    assert!(
        app.prepare_recipient_report(request.clone(), control.clone())
            .await
            .is_err()
    );
    request.expected_revision = edit.new_revision;
    let (stale, report) = app
        .prepare_recipient_report(request.clone(), control.clone())
        .await
        .map_err(boxed)?;
    assert_eq!(report.source.scenario_revision, Revision::INITIAL);
    assert_eq!(
        report.source.observed_current_revision,
        Some(edit.new_revision)
    );
    let RecipientContentV1::Workforce(content) = &report.content;
    assert_eq!(content.assignments.len(), 1);
    assert!(
        stale
            .publish_html(refused.clone(), false, control.clone())
            .await
            .is_err()
    );
    assert!(!refused.exists());
    let (approved, _) = app
        .prepare_recipient_report(request.clone(), control.clone())
        .await
        .map_err(boxed)?;
    let expected = approved.html_bytes().to_vec();
    let receipt = approved
        .publish_html(refused.clone(), true, control.clone())
        .await
        .map_err(boxed)?;
    assert_eq!(std::fs::read(refused)?, expected);
    assert_eq!(receipt.current_revision, edit.new_revision);
    assert_eq!(
        store.get_project(scenario_id).await?.summary.revision,
        edit.new_revision
    );
    let (library_stale, _) = app
        .prepare_recipient_report(request.clone(), control.clone())
        .await
        .map_err(boxed)?;
    let template = snapshot(1)?.document;
    let another = app
        .headless_service()
        .create_scenario(
            "Unrelated library addition".to_owned(),
            String::new(),
            template.domain_pack,
            template.settings,
            &control,
        )
        .map_err(boxed)?
        .document;
    store
        .create_project(NewProject { document: another })
        .await?;
    let library_destination = directory.path().join("library-changed.html");
    assert!(
        library_stale
            .publish_html(library_destination.clone(), true, control.clone())
            .await
            .is_err()
    );
    assert!(!library_destination.exists());
    let (cancelled, _) = app
        .prepare_recipient_report(request, control)
        .await
        .map_err(boxed)?;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(
        cancelled
            .publish_html(
                library_destination.clone(),
                true,
                OperationControl::Cancellation(cancellation)
            )
            .await
            .is_err()
    );
    assert!(!library_destination.exists());
    Ok(())
}

const SCHEDULE_FORMATS: [ScheduleExportFormatV1; 3] = [
    ScheduleExportFormatV1::AssignmentsCsv,
    ScheduleExportFormatV1::SummaryCsv,
    ScheduleExportFormatV1::Ics,
];

fn schedule_request(
    scenario_id: ScenarioId,
    solution_id: SolutionId,
    expected_revision: Revision,
    format: ScheduleExportFormatV1,
) -> ScheduleExportRequestV1 {
    ScheduleExportRequestV1 {
        schema_version: 1,
        scenario_id,
        solution_id,
        expected_revision,
        format,
        person_id: None,
    }
}

fn assert_schedule_preview(
    capture: &CapturedScheduleExport,
    preview: &ScheduleExportPreviewV1,
) -> TestResult {
    let bytes = capture.bytes();
    let text = std::str::from_utf8(bytes)?;
    assert_eq!(preview.schema_version, 1);
    assert_eq!(preview.format, capture.format());
    assert_eq!(usize::try_from(preview.byte_count)?, bytes.len());
    assert_eq!(preview.sha256, format!("{:x}", Sha256::digest(bytes)));
    assert!(text.starts_with(&preview.sample));
    assert!(preview.sample.len() <= MAX_SCHEDULE_EXPORT_SAMPLE_BYTES);
    assert_eq!(
        preview.sample_truncated,
        bytes.len() > MAX_SCHEDULE_EXPORT_SAMPLE_BYTES
    );
    if preview.sample_truncated {
        // A UTF-8 character occupies at most four bytes; no complete character is omitted.
        assert!(preview.sample.len() >= MAX_SCHEDULE_EXPORT_SAMPLE_BYTES - 3);
    } else {
        assert_eq!(preview.sample, text);
    }
    let retained = capture
        .retained_bytes(&OperationControl::Cancellation(CancellationToken::new()))
        .map_err(boxed)?;
    assert!(retained >= bytes.len());
    assert!(
        retained
            + preview
                .retained_bytes()
                .ok_or("preview capacity overflow")?
            < 64 * 1024 * 1024
    );
    Ok(())
}

pub(super) async fn publish_schedule_exact(
    capture: CapturedScheduleExport,
    preview: ScheduleExportPreviewV1,
    destination: PathBuf,
    allow_stale: bool,
    control: OperationControl,
) -> TestResult {
    let expected = capture.bytes().to_vec();
    let receipt = capture
        .publish(destination.clone(), allow_stale, control)
        .await
        .map_err(boxed)?;
    assert_eq!(std::fs::read(destination)?, expected);
    assert_eq!(receipt.schema_version, 1);
    assert_eq!(receipt.solution_id, preview.source.solution_id);
    assert_eq!(receipt.source_revision, preview.source.scenario_revision);
    assert_eq!(
        Some(receipt.current_revision),
        preview.source.observed_current_revision
    );
    assert_eq!(receipt.format, preview.format);
    assert_eq!(receipt.person_id, preview.person_id);
    assert_eq!(receipt.sha256, preview.sha256);
    assert_eq!(receipt.byte_count, preview.byte_count);
    Ok(())
}

#[tokio::test]
#[ignore = "requires EUTHETO_TEST_ORTOOLS_ARTIFACT pointing to an approved installed real worker; no helper fallback"]
#[allow(clippy::too_many_lines)]
async fn actual_worker_schedule_capture_is_complete_exact_and_revision_leased() -> TestResult {
    let mut source = snapshot(1)?;
    let person_template = source.document.domain.entities[&id(1).parse()?].clone();
    // The accepted census exceeds the result view's first fifty people. Only River is active.
    for index in 200..255 {
        let mut person = person_template.clone();
        person["id"] = json!(id(index));
        person["name"] = json!(format!("Unassigned {index} {}", "雪".repeat(40)));
        person["externalId"] = json!(format!("staff-{index}"));
        person["activeRange"] = json!({
            "kind":"dateRange", "startDate":"2026-12-01", "endDateExclusive":"2026-12-02"
        });
        source
            .document
            .domain
            .entities
            .insert(id(index).parse()?, person);
    }
    let scenario_id = source.document.scenario_id;
    let (directory, store, outcome, _, app) =
        stored_real_run(source, ExplanationMode::None).await?;
    let solution_id = outcome
        .accepted_result_id()
        .ok_or("missing accepted result")?;
    let accepted = store.load_accepted_result(solution_id).await?;
    let control = OperationControl::Cancellation(CancellationToken::new());
    let first_page = app
        .solution_view_with_cancellation(
            SolutionViewRequestV1 {
                schema_version: 1,
                scenario_id,
                solution_id,
                view_id: "official.workforce.result.schedule".to_owned(),
                parameters: json!({"kind":"people","limit":50}),
                continuation: None,
            },
            CancellationToken::new(),
        )
        .await
        .map_err(boxed)?;
    assert_eq!(first_page.view.data["result"]["totalItems"], 56);
    let people = first_page.view.data["result"]["items"]
        .as_array()
        .ok_or("missing accepted people page")?;
    assert_eq!(people.len(), 50);
    assert!(!people.iter().any(|person| person["personId"] == id(254)));
    let mut pending = Vec::new();
    for format in SCHEDULE_FORMATS {
        let request = schedule_request(scenario_id, solution_id, Revision::INITIAL, format);
        let preview_token = CancellationToken::new();
        let (capture, preview) = app
            .prepare_schedule_export(
                request.clone(),
                OperationControl::Cancellation(preview_token.clone()),
            )
            .await
            .map_err(boxed)?;
        assert_schedule_preview(&capture, &preview)?;
        assert_eq!(preview.person_id, None);
        assert_eq!(preview.source.origin, RecipientOriginV1::StoredTerminal);
        assert_eq!(
            preview.source.accepted_result_checksum,
            accepted.portable.accepted_result.checksum
        );
        assert_eq!(
            capture.metadata().run_input_checksum,
            accepted.portable.run_input.checksum
        );
        assert_eq!(
            capture.metadata().run_manifest_checksum.as_deref(),
            Some(accepted.portable.run_manifest.checksum.as_str())
        );
        let text = std::str::from_utf8(capture.bytes())?;
        match format {
            ScheduleExportFormatV1::AssignmentsCsv => assert_eq!(
                text,
                format!(
                    "eutheto/assignments,1\nassignment_id,person_id,shift_id\n\
                     official.workforce.assignment.{}.{},{},{}\n",
                    id(1),
                    id(8),
                    id(1),
                    id(8)
                )
            ),
            ScheduleExportFormatV1::SummaryCsv => {
                assert!(text.starts_with("eutheto/workforce-summary,1\nkind,person_id,"));
                assert_eq!(
                    text.lines()
                        .filter(|line| line.starts_with("person,"))
                        .count(),
                    56
                );
                assert_eq!(
                    text.lines()
                        .filter(|line| line.starts_with("shift,"))
                        .count(),
                    1
                );
                assert!(text.contains(&format!("person,{},Unassigned 254", id(254))));
                assert!(preview.sample_truncated);
            }
            ScheduleExportFormatV1::Ics => {
                assert!(text.starts_with("BEGIN:VCALENDAR\r\n"));
                assert!(text.ends_with("END:VCALENDAR\r\n"));
                assert_eq!(text.matches("BEGIN:VEVENT\r\n").count(), 1);
                assert!(text.contains("DTSTART:20261101T080000Z\r\n"));
                assert!(text.contains("DTEND:20261101T100000Z\r\n"));
                assert!(text.contains(&format!("X-EUTHETO-PERSON-ID:{}\r\n", id(1))));
            }
        }
        // Review retains bytes, not the preparation token or its deadline.
        preview_token.cancel();
        let destination = directory.path().join(format!("schedule-{format:?}"));
        publish_schedule_exact(
            capture,
            preview,
            destination.clone(),
            false,
            control.clone(),
        )
        .await?;
        let before = std::fs::read(&destination)?;
        let (collision, _) = app
            .prepare_schedule_export(request.clone(), control.clone())
            .await
            .map_err(boxed)?;
        assert!(
            collision
                .publish(destination.clone(), false, control.clone())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(destination)?, before);
        let (cancelled, _) = app
            .prepare_schedule_export(request.clone(), control.clone())
            .await
            .map_err(boxed)?;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let refused = directory.path().join(format!("cancelled-{format:?}"));
        assert!(
            cancelled
                .publish(
                    refused.clone(),
                    false,
                    OperationControl::Cancellation(cancellation)
                )
                .await
                .is_err()
        );
        assert!(!refused.exists());
        assert_schedule_budget(&app, &request, directory.path()).await?;
        let (obsolete, _) = app
            .prepare_schedule_export(request.clone(), control.clone())
            .await
            .map_err(boxed)?;
        pending.push((request, obsolete));
    }
    for (person, events) in [(1, 1), (254, 0)] {
        let mut request = schedule_request(
            scenario_id,
            solution_id,
            Revision::INITIAL,
            ScheduleExportFormatV1::Ics,
        );
        request.person_id = Some(id(person).parse()?);
        let (capture, preview) = app
            .prepare_schedule_export(request, control.clone())
            .await
            .map_err(boxed)?;
        assert_schedule_preview(&capture, &preview)?;
        assert_eq!(preview.person_id, Some(id(person).parse()?));
        let text = std::str::from_utf8(capture.bytes())?;
        assert_eq!(text.matches("BEGIN:VEVENT\r\n").count(), events);
        assert!(text.contains(&format!("X-EUTHETO-PERSON-ID:{}\r\n", id(person))));
        assert!(text.ends_with("END:VCALENDAR\r\n"));
        if events == 0 {
            assert!(!text.contains("DTSTART:"));
            assert!(!text.contains(&format!("X-EUTHETO-PERSON-ID:{}\r\n", id(1))));
        }
        publish_schedule_exact(
            capture,
            preview,
            directory.path().join(format!("person-{person}.ics")),
            false,
            control.clone(),
        )
        .await?;
    }
    for format in SCHEDULE_FORMATS {
        let mut invalid = schedule_request(scenario_id, solution_id, Revision::INITIAL, format);
        invalid.person_id = Some(
            id(if format == ScheduleExportFormatV1::Ics {
                999
            } else {
                1
            })
            .parse()?,
        );
        assert!(
            app.prepare_schedule_export(invalid, control.clone())
                .await
                .is_err()
        );
    }
    let changed = serde_json::to_value(snapshot(2)?.document)?;
    let edit = app
        .execute(AppCommand::ApplyScenario {
            request_id: RequestId::new(&SystemIdGenerator)?,
            envelope: CommandEnvelope {
                command_id: CommandId::new(&SystemIdGenerator)?,
                scenario_id,
                expected_revision: Revision::INITIAL,
                actor: ActorRef {
                    actor_id: None,
                    display_name: "Schedule export regression".to_owned(),
                },
                source: CommandSource::System,
                command: ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
                    command_type: eutheto_workforce::commands::UPDATE_ENTITY.to_owned(),
                    payload: json!({"entity": changed["domain"]["entities"][id(8)]}),
                }),
            },
            truncate_redo: false,
        })
        .await
        .map_err(boxed)?;
    let AppCommandResult::ScenarioCommand(edit) = edit else {
        return Err("missing committed edit".into());
    };
    let mut library_pending = Vec::new();
    for (mut request, obsolete) in pending {
        let refused = directory
            .path()
            .join(format!("obsolete-{:?}", request.format));
        assert!(
            obsolete
                .publish(refused.clone(), true, control.clone())
                .await
                .is_err()
        );
        assert!(!refused.exists());
        assert!(
            app.prepare_schedule_export(request.clone(), control.clone())
                .await
                .is_err()
        );
        request.expected_revision = edit.new_revision;
        let (stale, preview) = app
            .prepare_schedule_export(request.clone(), control.clone())
            .await
            .map_err(boxed)?;
        assert_schedule_preview(&stale, &preview)?;
        assert_eq!(preview.source.scenario_revision, Revision::INITIAL);
        assert_eq!(
            preview.source.observed_current_revision,
            Some(edit.new_revision)
        );
        let text = std::str::from_utf8(stale.bytes())?;
        match request.format {
            ScheduleExportFormatV1::AssignmentsCsv => assert_eq!(text.lines().count(), 3),
            ScheduleExportFormatV1::SummaryCsv => {
                let shift = text
                    .lines()
                    .find(|line| line.starts_with("shift,"))
                    .ok_or("missing historical shift summary")?;
                let columns = shift.split(',').collect::<Vec<_>>();
                assert_eq!(columns[6], Revision::INITIAL.value().to_string());
                assert_eq!(columns[7], edit.new_revision.value().to_string());
                assert_eq!(columns[8], "old-revision");
                assert_eq!(columns[9], "1");
                assert_eq!(
                    columns[10], "1",
                    "head now requires two, but the source required one"
                );
            }
            ScheduleExportFormatV1::Ics => assert!(text.contains("X-EUTHETO-STALE-AS-OF:TRUE\r\n")),
        }
        assert!(
            stale
                .publish(refused.clone(), false, control.clone())
                .await
                .is_err()
        );
        assert!(!refused.exists());
        let (approved, preview) = app
            .prepare_schedule_export(request.clone(), control.clone())
            .await
            .map_err(boxed)?;
        publish_schedule_exact(approved, preview, refused, true, control.clone()).await?;
        let (capture, _) = app
            .prepare_schedule_export(request, control.clone())
            .await
            .map_err(boxed)?;
        library_pending.push(capture);
    }
    let template = snapshot(1)?.document;
    let another = app
        .headless_service()
        .create_scenario(
            "Unrelated export library addition".to_owned(),
            String::new(),
            template.domain_pack,
            template.settings,
            &control,
        )
        .map_err(boxed)?
        .document;
    store
        .create_project(NewProject { document: another })
        .await?;
    for capture in library_pending {
        let refused = directory
            .path()
            .join(format!("library-changed-{:?}", capture.format()));
        assert!(
            capture
                .publish(refused.clone(), true, control.clone())
                .await
                .is_err()
        );
        assert!(!refused.exists());
    }
    Ok(())
}

async fn assert_schedule_budget(
    app: &EuthetoApp,
    request: &ScheduleExportRequestV1,
    directory: &Path,
) -> TestResult {
    let clock = Arc::new(FixedMonotonicClock::default());
    let budget = ParentSolveBudget::new(
        DurationMillis::new(30_000)?,
        clock.clone(),
        CancellationToken::new(),
    )?;
    clock.advance(Duration::from_secs(29))?;
    let control = OperationControl::Solve(budget.phase_view());
    let (capture, _) = app
        .prepare_schedule_export(request.clone(), control.clone())
        .await
        .map_err(boxed)?;
    clock.advance(Duration::from_secs(1))?;
    let expired = directory.join(format!("expired-{:?}", request.format));
    assert!(
        capture
            .publish(expired.clone(), false, control)
            .await
            .is_err()
    );
    assert!(!expired.exists());

    let destination = directory.join(format!(
        "confirmed-before-interruption-{:?}",
        request.format
    ));
    let cancellation = CancellationToken::new();
    let clock = Arc::new(DeadlineAtPublication {
        destination: destination.clone(),
        cancellation: cancellation.clone(),
    });
    let budget = ParentSolveBudget::new(DurationMillis::new(30_000)?, clock, cancellation.clone())?;
    let control = OperationControl::Solve(budget.phase_view());
    let (capture, preview) = app
        .prepare_schedule_export(request.clone(), control.clone())
        .await
        .map_err(boxed)?;
    publish_schedule_exact(capture, preview, destination, false, control.clone()).await?;
    assert!(control.check().is_err());
    assert!(cancellation.is_cancelled());
    Ok(())
}

pub(super) async fn assert_recipient_repair_history(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    solution_id: SolutionId,
    expected_revision: Revision,
    base_pair: &eutheto_workforce::model::AssignmentPair,
    manual_pair: &eutheto_workforce::model::AssignmentPair,
    manual_available: bool,
) -> TestResult {
    use eutheto_types::{
        RecipientChangeBasisV1, RecipientHistoricalEndpointV1, RecipientOriginalAdmissionV1,
        RecipientPairChangeV1,
    };
    let (_, report) = app
        .prepare_recipient_report(
            RecipientReportRequestV1 {
                schema_version: 1,
                scenario_id,
                solution_id,
                expected_revision,
                options: RecipientReportOptionsV1 {
                    include_changes: true,
                    ..RecipientReportOptionsV1::default()
                },
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)?;
    let RecipientContentV1::Workforce(content) = report.content;
    let changes = content.changes.ok_or("missing requested comparisons")?;
    let original = changes
        .iter()
        .find(|value| value.basis == RecipientChangeBasisV1::RecordedOriginal)
        .ok_or("missing original comparison")?;
    assert_eq!(
        original.original.as_ref().ok_or("missing A")?.admission,
        RecipientOriginalAdmissionV1::FreshlyReadmittedNative
    );
    let original_row = original
        .rows
        .iter()
        .find(|row| {
            row.person_id == base_pair.person_id && row.shift_id == base_pair.shift_id.into()
        })
        .ok_or("missing original pair")?;
    assert_eq!(original_row.membership, RecipientPairChangeV1::Retained);
    assert!(matches!(
        original_row.previous,
        Some(RecipientHistoricalEndpointV1::Available(_))
    ));
    let manual = changes
        .iter()
        .find(|value| value.basis == RecipientChangeBasisV1::RecordedManual)
        .ok_or("missing manual comparison")?;
    assert!(manual.original.is_none());
    let manual_row = manual
        .rows
        .iter()
        .find(|row| {
            row.person_id == manual_pair.person_id && row.shift_id == manual_pair.shift_id.into()
        })
        .ok_or("missing recorded manual pair")?;
    assert_eq!(manual_row.membership, RecipientPairChangeV1::Removed);
    if manual_available {
        assert!(matches!(
            manual_row.previous,
            Some(RecipientHistoricalEndpointV1::Available(_))
        ));
    } else {
        assert_eq!(
            manual_row.previous,
            Some(RecipientHistoricalEndpointV1::Unavailable)
        );
    }
    Ok(())
}

pub(super) async fn capture_checkpoint_report(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    checkpoint: &VerifiedCheckpoint,
) -> TestResult<eutheto_core::CapturedRecipientReport> {
    let (capture, report) = app
        .prepare_recipient_report(
            RecipientReportRequestV1 {
                schema_version: 1,
                scenario_id,
                solution_id: checkpoint.accepted_result.solution.solution_id,
                expected_revision: Revision::new(checkpoint.run_input.scenario_revision),
                options: RecipientReportOptionsV1::default(),
            },
            OperationControl::Cancellation(CancellationToken::new()),
        )
        .await
        .map_err(boxed)?;
    let retained = capture
        .retained_bytes(&OperationControl::Cancellation(CancellationToken::new()))
        .map_err(boxed)?;
    assert!(
        retained + report.retained_bytes().ok_or("report capacity overflow")? < 64 * 1024 * 1024
    );
    assert_eq!(report.source.origin, RecipientOriginV1::StoredCheckpoint);
    assert_eq!(
        report.source.verification,
        eutheto_types::RecipientVerificationV1::VerifiedFeasible
    );
    assert_eq!(capture.metadata().run_manifest_checksum, None);
    assert_eq!(
        report.source.accepted_result_checksum,
        checkpoint.accepted_result.checksum
    );
    Ok(capture)
}

pub(super) async fn capture_checkpoint_schedule_exports(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    checkpoint: &VerifiedCheckpoint,
) -> TestResult<Vec<(CapturedScheduleExport, ScheduleExportPreviewV1)>> {
    let mut captures = Vec::new();
    for format in SCHEDULE_FORMATS {
        let (capture, preview) = app
            .prepare_schedule_export(
                schedule_request(
                    scenario_id,
                    checkpoint.accepted_result.solution.solution_id,
                    Revision::new(checkpoint.run_input.scenario_revision),
                    format,
                ),
                OperationControl::Cancellation(CancellationToken::new()),
            )
            .await
            .map_err(boxed)?;
        assert_schedule_preview(&capture, &preview)?;
        assert_eq!(preview.source.origin, RecipientOriginV1::StoredCheckpoint);
        assert_eq!(
            preview.source.verification,
            eutheto_types::RecipientVerificationV1::VerifiedFeasible
        );
        assert_eq!(capture.metadata().run_manifest_checksum, None);
        assert_eq!(capture.metadata().run_id, checkpoint.run_input.run_id);
        assert_eq!(
            capture.metadata().run_input_checksum,
            checkpoint.run_input.checksum
        );
        assert_eq!(
            preview.source.accepted_result_checksum,
            checkpoint.accepted_result.checksum
        );
        assert_eq!(
            preview.source.verification_report_checksum,
            checkpoint.accepted_result.verification.checksum
        );
        assert_eq!(
            preview.source.source_document_hash,
            checkpoint.accepted_result.verification.document_hash
        );
        let text = std::str::from_utf8(capture.bytes())?;
        match format {
            ScheduleExportFormatV1::AssignmentsCsv => assert_eq!(
                text,
                format!(
                    "eutheto/assignments,1\nassignment_id,person_id,shift_id\n\
                     official.workforce.assignment.{}.{},{},{}\n",
                    id(1),
                    id(8),
                    id(1),
                    id(8)
                )
            ),
            ScheduleExportFormatV1::SummaryCsv => {
                assert_eq!(
                    text.lines()
                        .filter(|line| line.starts_with("person,"))
                        .count(),
                    1
                );
                assert_eq!(
                    text.lines()
                        .filter(|line| line.starts_with("shift,"))
                        .count(),
                    1
                );
                assert!(text.contains(",current,"));
            }
            ScheduleExportFormatV1::Ics => {
                assert!(text.contains("X-EUTHETO-STATUS:feasible\r\n"));
                assert!(!text.contains("X-EUTHETO-STATUS:accepted\r\n"));
                assert_eq!(text.matches("BEGIN:VEVENT\r\n").count(), 1);
                assert!(text.contains("DTSTART:20261101T080000Z\r\n"));
                assert!(text.contains("DTEND:20261101T100000Z\r\n"));
            }
        }
        captures.push((capture, preview));
    }
    Ok(captures)
}

pub(super) fn assert_file_recipient_report(
    service: &HeadlessService,
    source: &ScenarioSnapshotV1,
    portable: &PortableAcceptedResult,
    control: &OperationControl,
) -> TestResult {
    let (encoded, report) = service
        .export_recipient_html(
            source,
            portable,
            RecipientReportOptionsV1::default(),
            control,
        )
        .map_err(boxed)?;
    assert_eq!(report.source.origin, RecipientOriginV1::VerifiedFile);
    assert_eq!(report.source.observed_current_revision, None);
    assert_eq!(
        report.source.accepted_result_checksum,
        portable.accepted_result.checksum
    );
    assert!(matches!(report.title, RecipientLabelV1::Generated(_)));
    let RecipientContentV1::Workforce(content) = &report.content;
    assert_eq!(
        content
            .people
            .iter()
            .map(|person| person.person_id.to_string())
            .collect::<Vec<_>>(),
        vec![id(1)],
    );
    assert!(matches!(
        content.people[0].label,
        RecipientLabelV1::Generated(_)
    ));
    assert_eq!(
        content
            .shifts
            .iter()
            .map(|shift| shift.shift_id.to_string())
            .collect::<Vec<_>>(),
        vec![id(8)],
    );
    assert_eq!(
        content
            .assignments
            .iter()
            .map(|pair| (pair.person_id.to_string(), pair.shift_id.to_string()))
            .collect::<Vec<_>>(),
        vec![(id(1), id(8))],
    );
    assert!(
        content.staffing.is_none()
            && content.changes.is_none()
            && content.fairness_actuals.is_none()
    );
    let directory = tempfile::tempdir()?;
    let destination = directory.path().join("file-recipient.html");
    encoded
        .prepare_destination(&destination, control)?
        .publish_controlled(control)?;
    assert_eq!(std::fs::read(&destination)?, encoded.as_bytes());
    assert!(
        encoded
            .prepare_destination(&destination, control)?
            .publish_controlled(control)
            .is_err()
    );
    assert_eq!(std::fs::read(&destination)?, encoded.as_bytes());

    let options = RecipientReportOptionsV1 {
        include_person_names: true,
        include_authored_labels: true,
        include_staffing: true,
        include_changes: true,
        include_fairness_actuals: true,
    };
    let (_, disclosed) = service
        .export_recipient_html(source, portable, options, control)
        .map_err(boxed)?;
    assert_eq!(
        disclosed.title,
        RecipientLabelV1::Authored(source.document.metadata.title.clone())
    );
    let RecipientContentV1::Workforce(content) = disclosed.content;
    let name = source.document.domain.entities[&id(1).parse()?]["name"]
        .as_str()
        .ok_or("missing source person name")?;
    assert_eq!(
        content.people[0].label,
        RecipientLabelV1::Authored(name.to_owned())
    );
    let staffing = content.staffing.ok_or("missing opted-in staffing")?;
    assert_eq!(
        staffing
            .iter()
            .map(|row| (row.shift_id.to_string(), row.assigned_count))
            .collect::<Vec<_>>(),
        vec![(id(8), 1)],
    );
    let actuals = content.fairness_actuals.ok_or("missing opted-in actuals")?;
    // This source defines a policy but no active workload-balance preference.
    assert!(actuals.is_empty());
    assert_file_recipient_admission(service, source, portable, control)?;
    Ok(())
}

fn assert_file_recipient_admission(
    service: &HeadlessService,
    source: &ScenarioSnapshotV1,
    portable: &PortableAcceptedResult,
    control: &OperationControl,
) -> TestResult {
    let options = RecipientReportOptionsV1::default();
    let mut changed = source.clone();
    changed.revision = Revision::new(source.revision.value() + 1);
    assert!(
        service
            .export_recipient_html(&changed, portable, options, control)
            .is_err()
    );
    changed.revision = source.revision;
    changed
        .document
        .metadata
        .description
        .push_str("Changed after acceptance");
    assert!(
        service
            .export_recipient_html(&changed, portable, options, control)
            .is_err()
    );
    for field in ["documentHash", "verificationScopeChecksum"] {
        let mut value = serde_json::to_value(portable)?;
        let forged_hash = eutheto_domain_ir::blake3_hex(field.as_bytes());
        value["acceptedResult"]["verification"][field] = json!(forged_hash);
        if field == "documentHash" {
            value["runInput"]["snapshotDocumentHash"] = json!(forged_hash);
        }
        let forged = reseal_recipient_source(value)?;
        forged.validate()?;
        assert!(
            service
                .export_recipient_html(source, &forged, options, control)
                .is_err()
        );
    }
    let mut value = serde_json::to_value(portable)?;
    value["runInput"]["modelHash"] = json!(eutheto_domain_ir::blake3_hex(b"wrong model binding"));
    let forged = reseal_recipient_source(value)?;
    assert!(
        service
            .export_recipient_html(source, &forged, options, control)
            .is_err()
    );
    let clock = Arc::new(FixedMonotonicClock::default());
    let budget = ParentSolveBudget::new(
        DurationMillis::new(30_000)?,
        clock.clone(),
        CancellationToken::new(),
    )?;
    clock.advance(Duration::from_secs(29))?;
    let control = OperationControl::Solve(budget.phase_view());
    let (encoded, _) = service
        .export_recipient_html(source, portable, options, &control)
        .map_err(boxed)?;
    clock.advance(Duration::from_secs(1))?;
    let directory = tempfile::tempdir()?;
    let destination = directory.path().join("expired.html");
    assert!(matches!(
        encoded.prepare_destination(&destination, &control),
        Err(eutheto_export::ExportError::DeadlineExceeded)
    ));
    assert!(!destination.exists());
    Ok(())
}

fn reseal_recipient_source(mut value: Value) -> TestResult<PortableAcceptedResult> {
    fn seal(value: &mut Value) -> TestResult {
        value
            .as_object_mut()
            .and_then(|object| object.remove("checksum"))
            .ok_or("missing checksum")?;
        value["checksum"] = json!(eutheto_domain_ir::blake3_hex(&serde_json::to_vec(value)?));
        Ok(())
    }
    seal(&mut value["runInput"])?;
    seal(&mut value["acceptedResult"]["verification"])?;
    seal(&mut value["acceptedResult"])?;
    value["runManifest"]["runInputChecksum"] = value["runInput"]["checksum"].clone();
    value["runManifest"]["outcome"]["acceptedResultChecksum"] =
        value["acceptedResult"]["checksum"].clone();
    value["runManifest"]["outcome"]["verificationChecksum"] =
        value["acceptedResult"]["verification"]["checksum"].clone();
    seal(&mut value["runManifest"])?;
    seal(&mut value)?;
    Ok(serde_json::from_value(value)?)
}

pub(super) fn assert_file_recipient_repair(
    service: &HeadlessService,
    stored: &eutheto_store::StoredAcceptedResultV2,
    base_pair: &eutheto_workforce::model::AssignmentPair,
    manual_pair: &eutheto_workforce::model::AssignmentPair,
) -> TestResult {
    let source = ScenarioSnapshotV1::current(
        Revision::new(stored.portable.scenario_revision),
        stored.document.clone(),
        BTreeSet::new(),
    );
    let (_, report) = service
        .export_recipient_html(
            &source,
            &stored.portable,
            RecipientReportOptionsV1 {
                include_person_names: true,
                include_authored_labels: true,
                include_changes: true,
                ..RecipientReportOptionsV1::default()
            },
            &OperationControl::Cancellation(CancellationToken::new()),
        )
        .map_err(boxed)?;
    assert_eq!(report.source.origin, RecipientOriginV1::VerifiedFile);
    assert_eq!(report.source.observed_current_revision, None);
    let RecipientContentV1::Workforce(content) = report.content;
    let changes = content.changes.ok_or("missing recorded comparisons")?;
    assert_eq!(changes.len(), 2);
    let original = changes
        .iter()
        .find(|change| change.basis == RecipientChangeBasisV1::RecordedOriginal)
        .ok_or("missing recorded A")?;
    assert_eq!(
        original
            .original
            .as_ref()
            .ok_or("missing A reference")?
            .admission,
        RecipientOriginalAdmissionV1::NotEstablished
    );
    let [original_row] = original.rows.as_slice() else {
        return Err("incorrect complete A→C census".into());
    };
    assert_eq!(
        (original_row.person_id, original_row.shift_id),
        (base_pair.person_id, base_pair.shift_id.into())
    );
    assert_eq!(original_row.membership, RecipientPairChangeV1::Retained);
    assert_eq!(
        original_row.previous,
        Some(RecipientHistoricalEndpointV1::Unavailable)
    );
    let manual = changes
        .iter()
        .find(|change| change.basis == RecipientChangeBasisV1::RecordedManual)
        .ok_or("missing recorded B")?;
    assert!(manual.original.is_none());
    assert_eq!(manual.rows.len(), 2);
    let added = manual
        .rows
        .iter()
        .find(|row| {
            row.person_id == base_pair.person_id && row.shift_id == base_pair.shift_id.into()
        })
        .ok_or("missing B→C addition")?;
    assert_eq!(added.membership, RecipientPairChangeV1::Added);
    assert_eq!(added.previous, None);
    let removed = manual
        .rows
        .iter()
        .find(|row| {
            row.person_id == manual_pair.person_id && row.shift_id == manual_pair.shift_id.into()
        })
        .ok_or("missing B→C removal")?;
    assert_eq!(removed.membership, RecipientPairChangeV1::Removed);
    assert_eq!(
        removed.previous,
        Some(RecipientHistoricalEndpointV1::Unavailable)
    );
    Ok(())
}

struct DeadlineAtPublication {
    destination: PathBuf,
    cancellation: CancellationToken,
}

impl MonotonicClock for DeadlineAtPublication {
    fn now(&self) -> Duration {
        if self.destination.exists() {
            self.cancellation.cancel();
            Duration::from_secs(30)
        } else {
            Duration::ZERO
        }
    }
}

async fn assert_recipient_budget(
    app: &EuthetoApp,
    request: &RecipientReportRequestV1,
    directory: &Path,
) -> TestResult {
    let clock = Arc::new(FixedMonotonicClock::default());
    let budget = ParentSolveBudget::new(
        DurationMillis::new(30_000)?,
        clock.clone(),
        CancellationToken::new(),
    )?;
    clock.advance(Duration::from_secs(29))?;
    let control = OperationControl::Solve(budget.phase_view());
    let (capture, _) = app
        .prepare_recipient_report(request.clone(), control.clone())
        .await
        .map_err(boxed)?;
    clock.advance(Duration::from_secs(1))?;
    let expired = directory.join("expired.html");
    assert!(
        capture
            .publish_html(expired.clone(), false, control)
            .await
            .is_err()
    );
    assert!(!expired.exists());

    let destination = directory.join("confirmed-before-interruption.html");
    let cancellation = CancellationToken::new();
    let clock = Arc::new(DeadlineAtPublication {
        destination: destination.clone(),
        cancellation: cancellation.clone(),
    });
    let budget = ParentSolveBudget::new(DurationMillis::new(30_000)?, clock, cancellation.clone())?;
    let control = OperationControl::Solve(budget.phase_view());
    let (capture, _) = app
        .prepare_recipient_report(request.clone(), control.clone())
        .await
        .map_err(boxed)?;
    let receipt = capture
        .publish_html(destination.clone(), false, control.clone())
        .await
        .map_err(boxed)?;
    assert!(control.check().is_err());
    assert!(cancellation.is_cancelled());
    assert_eq!(
        receipt.html_sha256,
        format!("{:x}", Sha256::digest(std::fs::read(destination)?))
    );
    Ok(())
}

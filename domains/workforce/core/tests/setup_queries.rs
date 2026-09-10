mod support;

use eutheto_domain_api::{
    DomainPack, DomainPackError, DomainSetupQueryV1, DomainViewInput, SetupViewContext,
};
use eutheto_types::{CancellationToken, EntityId, OperationControl, Revision, ScenarioDocument};
use eutheto_workforce::{
    WorkforcePack,
    setup::contracts::{WorkforceSetupResultV1, WorkforceSetupViewDataV1},
};
use serde_json::{Value, json};
use std::error::Error;

fn context() -> SetupViewContext {
    SetupViewContext {
        revision: Revision::INITIAL,
        query_fingerprint: [7; 32],
    }
}

fn query(view_id: &str, parameters: Value) -> DomainSetupQueryV1 {
    DomainSetupQueryV1 {
        schema_version: 1,
        view_id: view_id.to_owned(),
        parameters,
        continuation: None,
    }
}

fn view(
    document: &ScenarioDocument,
    query: &DomainSetupQueryV1,
    context: SetupViewContext,
) -> Result<WorkforceSetupViewDataV1, Box<dyn Error>> {
    let output = WorkforcePack.build_view(
        DomainViewInput::StoredSetup {
            document,
            query,
            context,
        },
        &OperationControl::Cancellation(CancellationToken::new()),
    )?;
    if output.reconciliation.is_some() {
        return Err("query unexpectedly proposed a mutation".into());
    }
    Ok(serde_json::from_value::<WorkforceSetupResultV1>(output.view.data)?.result)
}

fn people_fixture() -> Result<ScenarioDocument, Box<dyn Error>> {
    let mut document = support::fixture()?;
    let person = document
        .domain
        .entities
        .get(&support::id(1).parse()?)
        .ok_or("missing person")?
        .clone();
    for index in [503, 501, 502] {
        let mut record = person.clone();
        record["id"] = json!(support::id(index));
        record["name"] = json!("Équipe / Nord [A]");
        document
            .domain
            .entities
            .insert(support::id(index).parse()?, record);
    }
    Ok(document)
}

#[test]
fn literal_unicode_search_pages_duplicate_names_by_stable_identity() -> Result<(), Box<dyn Error>> {
    let document = people_fixture()?;
    let mut request = query(
        "eutheto.setup.entity_page",
        json!({"entityKind":"person", "search":"éQUIPE /", "limit":2}),
    );
    let WorkforceSetupViewDataV1::EntityPage(first) = view(&document, &request, context())? else {
        return Err("wrong result family".into());
    };
    assert_eq!(first.total_items, 3);
    assert_eq!(
        first
            .items
            .iter()
            .map(|row| row.entity_id)
            .collect::<Vec<_>>(),
        [support::id(501).parse()?, support::id(502).parse()?]
    );
    assert!(
        first
            .items
            .iter()
            .all(|row| row.name.as_deref() == Some("Équipe / Nord [A]"))
    );
    request.continuation = Some(first.continuation.ok_or("missing continuation")?);
    let WorkforceSetupViewDataV1::EntityPage(last) = view(&document, &request, context())? else {
        return Err("wrong result family".into());
    };
    assert_eq!(last.total_items, 3);
    assert_eq!(
        last.items
            .iter()
            .map(|row| row.entity_id)
            .collect::<Vec<_>>(),
        [support::id(503).parse()?]
    );
    assert!(last.continuation.is_none());
    Ok(())
}

#[test]
fn entity_continuation_cannot_change_authority_or_skip_a_missing_identity()
-> Result<(), Box<dyn Error>> {
    let document = people_fixture()?;
    let mut request = query(
        "eutheto.setup.entity_page",
        json!({"entityKind":"person", "search":"équipe /", "limit":1}),
    );
    let WorkforceSetupViewDataV1::EntityPage(first) = view(&document, &request, context())? else {
        return Err("wrong result family".into());
    };
    let cursor = first.continuation.ok_or("missing continuation")?;
    let mut invalid_cursors = Vec::new();
    let mut changed = cursor.clone();
    changed.revision = Revision::new(1);
    invalid_cursors.push(changed);
    let mut changed = cursor.clone();
    changed.query_fingerprint = [8; 32];
    invalid_cursors.push(changed);
    let mut changed = cursor.clone();
    changed.scenario_id = support::id(999).parse()?;
    invalid_cursors.push(changed);
    let mut changed = cursor.clone();
    changed.schema_version = 2;
    invalid_cursors.push(changed);
    for position in [
        json!({"kind":"entity", "entityId":support::id(999)}),
        json!({"kind":"entity", "entityId":support::id(11)}),
        json!({"kind":"ordinal", "nextOrdinal":1}),
    ] {
        let mut changed = cursor.clone();
        changed.position = position;
        invalid_cursors.push(changed);
    }
    let control = OperationControl::Cancellation(CancellationToken::new());
    for cursor in invalid_cursors {
        request.continuation = Some(cursor);
        assert!(matches!(
            WorkforcePack.build_view(
                DomainViewInput::StoredSetup {
                    document: &document,
                    query: &request,
                    context: context()
                },
                &control
            ),
            Err(DomainPackError::InvalidPayload { .. })
        ));
    }
    Ok(())
}

#[test]
fn entity_detail_preserves_complete_record_and_honors_cancellation() -> Result<(), Box<dyn Error>> {
    let document = support::fixture()?;
    let entity_id: EntityId = support::id(1).parse()?;
    let request = query(
        "eutheto.setup.entity_detail",
        json!({"entityKind":"person", "entityId":entity_id}),
    );
    let WorkforceSetupViewDataV1::EntityDetail(record) = view(&document, &request, context())?
    else {
        return Err("wrong result family".into());
    };
    assert_eq!(
        serde_json::to_value(record)?,
        document.domain.entities[&entity_id]
    );
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        WorkforcePack.build_view(
            DomainViewInput::StoredSetup {
                document: &document,
                query: &request,
                context: context()
            },
            &OperationControl::Cancellation(token)
        ),
        Err(DomainPackError::Cancelled)
    ));
    Ok(())
}

struct ValidationClock {
    ticks: std::sync::atomic::AtomicU64,
    cancellation: Option<CancellationToken>,
}

impl eutheto_types::MonotonicClock for ValidationClock {
    fn now(&self) -> std::time::Duration {
        let tick = self
            .ticks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(token) = &self.cancellation {
            if tick == 1_000 {
                token.cancel();
            }
            std::time::Duration::ZERO
        } else {
            std::time::Duration::from_millis(tick)
        }
    }
}

#[test]
fn work_detail_validation_observes_live_cancellation_and_deadlines() -> Result<(), Box<dyn Error>> {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    document
        .domain
        .entities
        .get_mut(&support::id(1).parse()?)
        .ok_or("person")?["tags"] = json!(
        (0..5_000)
            .map(|index| format!("tag-{index}"))
            .collect::<Vec<_>>()
    );
    let policy = &mut document
        .domain
        .entities
        .get_mut(&support::id(9).parse()?)
        .ok_or("score policy")?["workloadPolicies"][support::id(10)];
    policy["peerGroup"]["people"] = json!({"kind":"filter", "allTags":["absent"], "anyTags":[]});
    policy["targetMode"] = json!({"kind":"explicit", "targets":[]});
    let request = query(
        "official.workforce.setup.work_detail",
        json!({"shiftId":support::id(7)}),
    );
    assert!(matches!(
        view(&document, &request, context())?,
        WorkforceSetupViewDataV1::WorkDetail(_)
    ));
    for cancel in [false, true] {
        let token = CancellationToken::new();
        let clock = std::sync::Arc::new(ValidationClock {
            ticks: std::sync::atomic::AtomicU64::new(0),
            cancellation: cancel.then(|| token.clone()),
        });
        let parent = eutheto_types::ParentSolveBudget::new(
            eutheto_types::DurationMillis::new(1_000)?,
            clock,
            token,
        )?;
        let control = OperationControl::Solve(parent.phase_view());
        let result = WorkforcePack.build_view(
            DomainViewInput::StoredSetup {
                document: &document,
                query: &request,
                context: context(),
            },
            &control,
        );
        assert!(matches!(
            (cancel, result),
            (true, Err(DomainPackError::Cancelled)) | (false, Err(DomainPackError::BudgetExpired))
        ));
    }
    Ok(())
}

#[test]
fn deeply_nested_query_values_are_rejected_without_stack_exhaustion() -> Result<(), Box<dyn Error>>
{
    const CHILD: &str = "EUTHETO_SETUP_DEPTH_REGRESSION";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "deeply_nested_query_values_are_rejected_without_stack_exhaustion",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()?;
        assert!(
            output.status.success(),
            "malformed query terminated its process: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }
    let document = support::fixture()?;
    for cursor_position in [false, true] {
        let mut nested = Value::Null;
        for _ in 0..20_000 {
            nested = Value::Array(vec![nested]);
        }
        let mut request = query("official.workforce.setup.overview", json!({}));
        if cursor_position {
            request.continuation = Some(eutheto_domain_api::SetupContinuationV1 {
                schema_version: 1,
                scenario_id: document.scenario_id,
                revision: context().revision,
                query_fingerprint: context().query_fingerprint,
                position: nested,
            });
        } else {
            request.parameters = nested;
        }
        let result = WorkforcePack.build_view(
            DomainViewInput::StoredSetup {
                document: &document,
                query: &request,
                context: context(),
            },
            &OperationControl::Cancellation(CancellationToken::new()),
        );
        // The malformed Value belongs to this caller; dismantle it without recursive Drop.
        let mut nested = if cursor_position {
            request
                .continuation
                .take()
                .ok_or("missing cursor")?
                .position
        } else {
            std::mem::take(&mut request.parameters)
        };
        while let Value::Array(mut values) = nested {
            nested = values.pop().ok_or("missing nested value")?;
        }
        assert!(matches!(
            result,
            Err(DomainPackError::InvalidPayload { .. })
        ));
    }
    Ok(())
}

#[test]
fn overview_counts_configured_facts_and_catalog_labels_actual_support() -> Result<(), Box<dyn Error>>
{
    use eutheto_workforce::setup::contracts::{RuleSupportV1, WorkforceEntityKindV1};
    let document = people_fixture()?;
    let WorkforceSetupViewDataV1::Overview(facts) = view(
        &document,
        &query("official.workforce.setup.overview", json!({})),
        context(),
    )?
    else {
        return Err("wrong result family".into());
    };
    assert_eq!(
        facts
            .entities
            .iter()
            .find(|row| row.kind == WorkforceEntityKindV1::Person)
            .ok_or("missing person count")?
            .count,
        4
    );
    assert_eq!(
        facts
            .entities
            .iter()
            .find(|row| row.kind == WorkforceEntityKindV1::Team)
            .ok_or("missing zero team count")?
            .count,
        0
    );
    assert_eq!(facts.configured_type_memberships, 4);
    let WorkforceSetupViewDataV1::RuleCatalog(catalog) = view(
        &document,
        &query("eutheto.setup.rule_catalog", json!({})),
        context(),
    )?
    else {
        return Err("wrong result family".into());
    };
    assert!(matches!(
        catalog
            .required
            .iter()
            .find(|row| row.descriptor.id == "official.workforce.rule.minimum-rest")
            .ok_or("missing rest descriptor")?
            .support,
        RuleSupportV1::Implemented
    ));
    assert!(matches!(
        catalog
            .required
            .iter()
            .find(|row| row.descriptor.id == "official.workforce.rule.maximum-hours")
            .ok_or("missing hours descriptor")?
            .support,
        RuleSupportV1::NotImplemented
    ));
    assert!(matches!(
        catalog
            .preferences
            .iter()
            .find(|row| row.descriptor.id == "official.workforce.preference.time")
            .ok_or("missing time descriptor")?
            .support,
        RuleSupportV1::NotImplemented
    ));
    Ok(())
}

#[test]
fn settings_preparation_respects_midnight_boundaries_without_a_display_window_cap()
-> Result<(), Box<dyn Error>> {
    let document = support::fixture()?;
    let mut request = query(
        "official.workforce.setup.settings_preparation",
        json!({
            "timeZone":"America/Sao_Paulo", "locale":"en-US", "units":"metric",
            "dates":{"startDate":"2018-11-04", "endDateExclusive":"2020-11-05"},
            "gapPolicy":"reject", "overlapPolicy":"reject"
        }),
    );
    let control = OperationControl::Cancellation(CancellationToken::new());
    assert!(matches!(
        WorkforcePack.build_view(
            DomainViewInput::StoredSetup {
                document: &document,
                query: &request,
                context: context()
            },
            &control
        ),
        Err(DomainPackError::InvalidPayload { .. })
    ));
    request.parameters["gapPolicy"] = json!("moveForward");
    assert!(view(&document, &request, context()).is_err());
    request.parameters["dates"]["startDate"] = json!("2018-11-05");
    let WorkforceSetupViewDataV1::SettingsPreparation(settings) =
        view(&document, &request, context())?
    else {
        return Err("wrong result family".into());
    };
    assert_eq!(settings.horizon.start, "2018-11-05T02:00:00Z".parse()?);
    assert_eq!(settings.horizon.end, "2020-11-05T03:00:00Z".parse()?);
    assert_eq!(settings.time_zone.as_str(), "America/Sao_Paulo");
    Ok(())
}

#[test]
fn command_change_pages_keep_repeated_path_edits_and_complete_records() -> Result<(), Box<dyn Error>>
{
    use eutheto_domain_api::{DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand};
    use eutheto_types::{
        Change, ChangeKind, CommandBatch, DomainCommandEnvelope, PackId, ScenarioCommand,
    };
    use eutheto_workforce::commands::{UPDATE_ENTITY, apply_batch};
    let original = support::fixture()?;
    let person_id = support::id(1).parse()?;
    let mut first = original.domain.entities[&person_id].clone();
    first["name"] = json!("First edited name");
    let mut second = first.clone();
    second["name"] = json!("Second edited name");
    let batch = DomainBatchCommand {
        schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: PackId::new("official.workforce")?,
        scenario_schema_version: 1,
        label: None,
        commands: vec![
            DomainCommandEnvelope {
                command_type: UPDATE_ENTITY.to_owned(),
                payload: json!({"entity":first}),
            },
            DomainCommandEnvelope {
                command_type: UPDATE_ENTITY.to_owned(),
                payload: json!({"entity":second}),
            },
        ],
    };
    let mutation = apply_batch(&original, &batch)?;
    let changes = mutation
        .changes
        .iter()
        .map(|row| {
            Ok(Change {
                kind: ChangeKind::Updated,
                path: row.value["path"]
                    .as_str()
                    .ok_or("missing change path")?
                    .to_owned(),
                before: Some(row.value["before"].clone()),
                after: Some(row.value["after"].clone()),
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let command = ScenarioCommand::ApplyBatch(CommandBatch {
        label: None,
        commands: batch
            .commands
            .into_iter()
            .map(ScenarioCommand::ApplyDomainCommand)
            .collect(),
    });
    let control = OperationControl::Cancellation(CancellationToken::new());
    let mut request = query("eutheto.setup.command_changes", json!({"limit":1}));
    assert!(
        WorkforcePack
            .build_view(
                DomainViewInput::StoredSetup {
                    document: &original,
                    query: &request,
                    context: context()
                },
                &control
            )
            .is_err()
    );
    for (ordinal, (before, after)) in [
        (&original.domain.entities[&person_id], &first),
        (&first, &second),
    ]
    .into_iter()
    .enumerate()
    {
        let output = WorkforcePack.build_view(
            DomainViewInput::CommandPreviewSetup {
                original: &original,
                prospective: &mutation.document,
                command: &command,
                changes: &changes,
                query: &request,
                context: context(),
            },
            &control,
        )?;
        let WorkforceSetupViewDataV1::CommandChanges(page) =
            serde_json::from_value::<WorkforceSetupResultV1>(output.view.data)?.result
        else {
            return Err("wrong result family".into());
        };
        assert_eq!(page.total_items, 2);
        assert_eq!(page.items[0].ordinal, u32::try_from(ordinal)?);
        assert_eq!(page.items[0].change.path, changes[0].path);
        assert_eq!(page.items[0].change.before.as_ref(), Some(before));
        assert_eq!(page.items[0].change.after.as_ref(), Some(after));
        request.continuation = page.continuation;
    }
    assert!(request.continuation.is_none());
    Ok(())
}

#[test]
fn temporal_views_keep_elapsed_and_wall_durations_distinct_across_a_fold()
-> Result<(), Box<dyn Error>> {
    let mut document = support::fixture()?;
    document.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    let request = query(
        "official.workforce.setup.work_detail",
        json!({"shiftId":support::id(8)}),
    );
    let WorkforceSetupViewDataV1::WorkDetail(detail) = view(&document, &request, context())? else {
        return Err("wrong result family".into());
    };
    assert_eq!(detail.shift.elapsed.seconds, "7200");
    assert_eq!(detail.shift.scheduled.seconds, "3600");
    assert_eq!(
        detail.shift.interval.starts_at.instant,
        "2026-11-01T05:30:00Z".parse()?
    );
    assert_eq!(
        detail.shift.interval.ends_at.instant,
        "2026-11-01T07:30:00Z".parse()?
    );
    let mut request = query(
        "official.workforce.setup.work_window",
        json!({
            "dates":{"startDate":"2026-11-01", "endDateExclusive":"2026-11-02"}, "limit":1
        }),
    );
    for expected in [7, 8] {
        let WorkforceSetupViewDataV1::WorkWindow(page) = view(&document, &request, context())?
        else {
            return Err("wrong result family".into());
        };
        assert_eq!(page.total_items, 2);
        assert_eq!(page.items[0].shift_id, support::id(expected).parse()?);
        request.continuation = page.continuation;
    }
    assert!(request.continuation.is_none());
    Ok(())
}

#[test]
fn filtered_generation_review_reconciles_the_entire_prospective_horizon()
-> Result<(), Box<dyn Error>> {
    use eutheto_types::{Change, ChangeKind, ScenarioCommand, SetScenarioSettings};
    let mut original = support::fixture()?;
    original.settings.overlap_policy = eutheto_types::OverlapPolicy::Earlier;
    let token = CancellationToken::new();
    let control = OperationControl::Cancellation(token.clone());
    let mut prospective = original.clone();
    prospective.settings.horizon.end = "2026-11-09T05:00:00Z".parse()?;
    prospective.domain = WorkforcePack
        .reconcile_settings(&original, &prospective.settings, None, &control)?
        .domain;
    let command = ScenarioCommand::SetScenarioSettings(Box::new(SetScenarioSettings {
        settings: prospective.settings.clone(),
        restoration: None,
    }));
    let changes = [Change {
        kind: ChangeKind::Updated,
        path: "/settings".to_owned(),
        before: Some(serde_json::to_value(&original.settings)?),
        after: Some(serde_json::to_value(&prospective.settings)?),
    }];
    let request = query(
        "official.workforce.setup.generation_review",
        json!({
            "dates":{"startDate":"2026-11-01", "endDateExclusive":"2026-11-02"},
            "changesOnly":true, "limit":1
        }),
    );
    let output = WorkforcePack.build_view(
        DomainViewInput::CommandPreviewSetup {
            original: &original,
            prospective: &prospective,
            command: &command,
            changes: &changes,
            query: &request,
            context: context(),
        },
        &control,
    )?;
    let WorkforceSetupViewDataV1::GenerationReview(review) =
        serde_json::from_value::<WorkforceSetupResultV1>(output.view.data)?.result
    else {
        return Err("wrong result family".into());
    };
    assert_eq!(
        (
            review.total_added,
            review.total_changed,
            review.total_removed
        ),
        (1, 0, 0)
    );
    assert!(review.reconciliation_required);
    assert_eq!(review.page.total_items, 0);
    assert!(review.page.items.is_empty());
    let reconciliation = output
        .reconciliation
        .ok_or("missing whole-horizon reconciliation")?;
    let reconciled = eutheto_workforce::commands::apply_batch(&prospective, &reconciliation)?;
    let shifts = eutheto_workforce::temporal::resolve_shifts(&reconciled.document, &token)?;
    assert_eq!(shifts.len(), 3);
    let added_date = jiff::civil::Date::new(2026, 11, 8)?;
    assert!(
        shifts
            .iter()
            .any(|shift| shift.interval.starts_at.local.as_datetime().date() == added_date)
    );
    Ok(())
}

#![cfg(debug_assertions)]

use eutheto_core::{
    AppCommand, AppCommandResult, AppDependencies, AppPaths, AppQuery, AppQueryResult, EuthetoApp,
};
use eutheto_domain_api::{
    ContractJsonLimits, DOMAIN_BATCH_SCHEMA_VERSION, DomainBatchCommand, DomainPack,
    validate_contract_value,
};
use eutheto_types::{
    ActorRef, AddEntity, AppError, CancellationToken, CommandBatch, CommandEnvelope, CommandId,
    CommandResult, CommandSource, DomainCommandEnvelope, DomainPackRef, EntityId, FixedClock,
    FixedMonotonicClock, RequestId, Revision, ScenarioCommand, ScenarioDocument, ScenarioId,
    ScenarioSettings, ScenarioViewDto, SystemIdGenerator, UpdateEntity,
};
use eutheto_workforce::{WorkforcePack, commands};
use serde_json::{Value, json};
use std::error::Error;
use std::sync::Arc;
use tempfile::TempDir;

fn boxed(error: &AppError) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}

fn dependencies() -> Result<(TempDir, AppDependencies), Box<dyn Error>> {
    let directory = tempfile::Builder::new()
        .prefix("eutheto-batch-dispatch-")
        .tempdir_in(dirs::home_dir().ok_or("missing home directory")?)?;
    let dependencies = AppDependencies {
        paths: AppPaths {
            database: directory.path().join("eutheto.sqlite"),
            safety_backups: directory.path().join("backups"),
        },
        clock: Arc::new(FixedClock::new("2026-09-01T00:00:00Z".parse()?)),
        monotonic_clock: Arc::new(FixedMonotonicClock::default()),
        ids: Arc::new(SystemIdGenerator),
        cancellation: CancellationToken::new(),
    };
    Ok((directory, dependencies))
}

fn request_id() -> Result<RequestId, Box<dyn Error>> {
    Ok(RequestId::new(&SystemIdGenerator)?)
}

async fn create(app: &EuthetoApp, pack: &str) -> Result<ScenarioId, Box<dyn Error>> {
    let settings: ScenarioSettings = serde_json::from_value(json!({
        "timeZone":"UTC", "locale":"en-US", "units":"metric",
        "horizon":{"start":"2026-09-01T00:00:00Z","end":"2026-09-08T00:00:00Z"},
        "gapPolicy":"reject", "overlapPolicy":"earlier"
    }))?;
    match app
        .execute(AppCommand::CreateProject {
            request_id: request_id()?,
            title: "Batch dispatch".to_owned(),
            description: String::new(),
            domain_pack: DomainPackRef {
                id: pack.parse()?,
                schema_version: 1,
            },
            settings,
        })
        .await
        .map_err(|error| boxed(&error))?
    {
        AppCommandResult::Project(project) => Ok(project.scenario_id),
        other => Err(format!("unexpected project result: {other:?}").into()),
    }
}

async fn view(app: &EuthetoApp, id: ScenarioId) -> Result<ScenarioViewDto, Box<dyn Error>> {
    match app
        .query(AppQuery::ScenarioView(id))
        .await
        .map_err(|error| boxed(&error))?
    {
        AppQueryResult::Scenario(view) => Ok(*view),
        other => Err(format!("unexpected view result: {other:?}").into()),
    }
}

fn envelope(
    id: ScenarioId,
    revision: Revision,
    command: ScenarioCommand,
) -> Result<CommandEnvelope, Box<dyn Error>> {
    Ok(CommandEnvelope {
        command_id: CommandId::new(&SystemIdGenerator)?,
        scenario_id: id,
        expected_revision: revision,
        actor: ActorRef {
            actor_id: None,
            display_name: "Batch dispatch test".to_owned(),
        },
        source: CommandSource::System,
        command,
    })
}

async fn apply(
    app: &EuthetoApp,
    envelope: CommandEnvelope,
) -> Result<CommandResult, Box<dyn Error>> {
    match app
        .execute(AppCommand::ApplyScenario {
            request_id: request_id()?,
            envelope,
            truncate_redo: false,
        })
        .await
        .map_err(|error| boxed(&error))?
    {
        AppCommandResult::ScenarioCommand(result) => Ok(result),
        other => Err(format!("unexpected apply result: {other:?}").into()),
    }
}

fn batch(label: &str, commands: Vec<ScenarioCommand>) -> ScenarioCommand {
    ScenarioCommand::ApplyBatch(CommandBatch {
        label: Some(label.to_owned()),
        commands,
    })
}

fn domain(command_type: &str, payload: Value) -> ScenarioCommand {
    ScenarioCommand::ApplyDomainCommand(DomainCommandEnvelope {
        command_type: command_type.to_owned(),
        payload,
    })
}

fn configure(id: EntityId, target: u32) -> ScenarioCommand {
    domain(
        "official.test.configure_entity",
        json!({"entityId":id,"enabled":target > 0,"target":target}),
    )
}

async fn assert_journal(
    app: &EuthetoApp,
    id: ScenarioId,
    command_id: CommandId,
    submitted: &Value,
    inverse: &Value,
) -> Result<(), Box<dyn Error>> {
    let AppQueryResult::History(history) = app
        .query(AppQuery::History(id))
        .await
        .map_err(|error| boxed(&error))?
    else {
        return Err("expected history".into());
    };
    let entry = history
        .iter()
        .find(|entry| entry.id == command_id)
        .ok_or("missing journal entry")?;
    assert_eq!(&entry.command, submitted);
    assert_eq!(entry.inverse.as_ref(), Some(inverse));
    Ok(())
}

fn configured_record(id: EntityId, target: u32) -> Value {
    json!({"id":id,"enabled":target > 0,"target":target})
}

fn mixed_nested_commands(id: EntityId) -> (ScenarioCommand, ScenarioCommand) {
    let record = |target| configured_record(id, target);
    let nested = batch("nested", vec![configure(id, 3), configure(id, 4)]);
    let mut submitted = batch(
        "outer",
        vec![
            ScenarioCommand::AddEntity(AddEntity {
                entity_id: id,
                value: record(0),
            }),
            configure(id, 1),
            configure(id, 2),
            nested,
            ScenarioCommand::UpdateEntity(UpdateEntity {
                entity_id: id,
                value: record(5),
            }),
            configure(id, 6),
            configure(id, 7),
        ],
    );
    let mut inverse = batch(
        "outer",
        vec![
            configure(id, 6),
            configure(id, 5),
            ScenarioCommand::UpdateEntity(UpdateEntity {
                entity_id: id,
                value: record(4),
            }),
            batch("nested", vec![configure(id, 3), configure(id, 2)]),
            configure(id, 1),
            configure(id, 0),
            ScenarioCommand::RemoveEntity(eutheto_types::RemoveEntity { entity_id: id }),
        ],
    );
    // Six wrappers plus outer and nested exercise the maximum depth on both replays.
    for depth in 0..6 {
        submitted = batch(&format!("wrapper {depth}"), vec![submitted]);
        inverse = batch(&format!("wrapper {depth}"), vec![inverse]);
    }
    (submitted, inverse)
}

#[tokio::test]
async fn mixed_nested_journal_preserves_exact_ast_and_reopens_for_undo_redo()
-> Result<(), Box<dyn Error>> {
    let (_directory, dependencies) = dependencies()?;
    let app = EuthetoApp::open(dependencies.clone())
        .await
        .map_err(|error| boxed(&error))?;
    let scenario_id = create(&app, "official.test").await?;
    let initial = view(&app, scenario_id).await?.document;
    let id = EntityId::new(&SystemIdGenerator)?;
    let record = |target| configured_record(id, target);
    let (submitted, inverse) = mixed_nested_commands(id);
    let submitted_json = serde_json::to_value(&submitted)?;
    let inverse_json = serde_json::to_value(&inverse)?;
    let input = envelope(scenario_id, Revision::INITIAL, submitted)?;
    let command_id = input.command_id;
    let result = apply(&app, input).await?;
    assert_eq!(result.inverse, Some(inverse));
    assert_eq!(result.change_set.changes.len(), 8);
    let mut expected_before = None;
    for (target, change) in (0_u32..8).zip(&result.change_set.changes) {
        assert_eq!(change.path, format!("/domain/entities/{id}"));
        assert_eq!(change.before, expected_before);
        assert_eq!(change.after, Some(record(target)));
        expected_before = Some(record(target));
    }
    let applied = view(&app, scenario_id).await?.document;
    assert_journal(
        &app,
        scenario_id,
        command_id,
        &submitted_json,
        &inverse_json,
    )
    .await?;
    drop(app);
    let reopened = EuthetoApp::open(dependencies.clone())
        .await
        .map_err(|error| boxed(&error))?;
    reopened
        .execute(AppCommand::Undo {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(1),
        })
        .await
        .map_err(|error| boxed(&error))?;
    assert_eq!(view(&reopened, scenario_id).await?.document, initial);
    assert_journal(
        &reopened,
        scenario_id,
        command_id,
        &submitted_json,
        &inverse_json,
    )
    .await?;
    drop(reopened);
    let reopened = EuthetoApp::open(dependencies)
        .await
        .map_err(|error| boxed(&error))?;
    reopened
        .execute(AppCommand::Redo {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(2),
        })
        .await
        .map_err(|error| boxed(&error))?;
    let replayed = view(&reopened, scenario_id).await?;
    assert_eq!(replayed.revision, Revision::new(3));
    assert_eq!(replayed.document, applied);
    assert_journal(
        &reopened,
        scenario_id,
        command_id,
        &submitted_json,
        &inverse_json,
    )
    .await?;
    assert_history_len(&reopened, scenario_id, 1).await?;
    Ok(())
}

async fn assert_history_len(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    expected: usize,
) -> Result<(), Box<dyn Error>> {
    let AppQueryResult::History(history) = app
        .query(AppQuery::History(scenario_id))
        .await
        .map_err(|error| boxed(&error))?
    else {
        return Err("expected history".into());
    };
    assert_eq!(history.len(), expected);
    Ok(())
}

fn rest_rules(id: eutheto_types::RuleId) -> (Value, Value) {
    let tags: Vec<String> = (0..10_000).map(|index| format!("t{index}")).collect();
    let full_scope = json!({"people":{"kind":"filter","allTags":tags,"anyTags":tags}});
    let smaller_scope = json!({"people":{"kind":"filter","allTags":tags,"anyTags":[]}});
    let full = json!({"kind":"minimumRest","id":id,"active":false,"strength":"required",
        "scope":full_scope,"afterScope":full_scope,"beforeScope":full_scope,"minimumMinutes":600});
    let mut smaller = full.clone();
    for scope in ["scope", "afterScope", "beforeScope"] {
        smaller[scope] = smaller_scope.clone();
    }
    (full, smaller)
}

fn domain_batch(
    document: &ScenarioDocument,
    commands: Vec<DomainCommandEnvelope>,
) -> DomainBatchCommand {
    DomainBatchCommand {
        schema_version: DOMAIN_BATCH_SCHEMA_VERSION,
        pack_id: document.domain_pack.id.clone(),
        scenario_schema_version: document.domain_pack.schema_version,
        label: None,
        commands,
    }
}

fn workforce_node_commands(
    full: &Value,
    smaller: &Value,
) -> (Vec<DomainCommandEnvelope>, ScenarioCommand) {
    // Each update carries 30,000 tags: four updates exceed the 100,000-node
    // pack-call limit while both documents and each individual update fit.
    let commands = vec![
        DomainCommandEnvelope {
            command_type: commands::UPDATE_RULE.to_owned(),
            payload: json!({"rule":smaller}),
        };
        4
    ];
    let inverse = batch(
        "node bounded",
        (0..4)
            .map(|index| {
                domain(
                    commands::UPDATE_RULE,
                    json!({"rule": if index == 3 { full } else { smaller }}),
                )
            })
            .collect(),
    );
    (commands, inverse)
}

fn assert_node_bound_rejection(
    original: &ScenarioDocument,
    direct: &DomainBatchCommand,
) -> Result<(), Box<dyn Error>> {
    direct.validate_bounds()?;
    let direct_json = serde_json::to_value(direct)?;
    assert!(
        validate_contract_value(&json!({}), &direct_json, ContractJsonLimits::DEFAULT).is_err()
    );
    validate_contract_value(
        &json!({}),
        &direct_json,
        ContractJsonLimits {
            max_collection_items: 1_000_000,
            ..ContractJsonLimits::DEFAULT
        },
    )?;
    assert!(
        WorkforcePack
            .apply_batch(original, direct, &CancellationToken::new())
            .is_err()
    );
    Ok(())
}

async fn add_workforce_rule(
    app: &EuthetoApp,
    scenario_id: ScenarioId,
    rule: &Value,
) -> Result<ScenarioDocument, Box<dyn Error>> {
    apply(
        app,
        envelope(
            scenario_id,
            Revision::INITIAL,
            domain(commands::ADD_RULE, json!({"rule":rule})),
        )?,
    )
    .await?;
    Ok(view(app, scenario_id).await?.document)
}

#[tokio::test]
async fn workforce_node_bounded_batch_preserves_journal_and_survives_restart()
-> Result<(), Box<dyn Error>> {
    let (_directory, dependencies) = dependencies()?;
    let app = EuthetoApp::open(dependencies.clone())
        .await
        .map_err(|error| boxed(&error))?;
    let scenario_id = create(&app, "official.workforce").await?;
    let rule_id = eutheto_types::RuleId::new(&SystemIdGenerator)?;
    let (full, smaller) = rest_rules(rule_id);
    let original = add_workforce_rule(&app, scenario_id, &full).await?;
    let (commands, expected_inverse) = workforce_node_commands(&full, &smaller);
    let direct = domain_batch(&original, commands);
    assert_node_bound_rejection(&original, &direct)?;
    let submitted = batch(
        "node bounded",
        direct
            .commands
            .into_iter()
            .map(ScenarioCommand::ApplyDomainCommand)
            .collect(),
    );
    let submitted_json = serde_json::to_value(&submitted)?;
    let inverse_json = serde_json::to_value(&expected_inverse)?;
    let input = envelope(scenario_id, Revision::new(1), submitted)?;
    let command_id = input.command_id;
    let result = apply(&app, input).await?;
    assert_eq!(result.inverse, Some(expected_inverse));
    assert_eq!(result.new_revision, Revision::new(2));
    assert_eq!(result.change_set.changes.len(), 4);
    for (index, change) in result.change_set.changes.iter().enumerate() {
        assert_eq!(
            change.before.as_ref(),
            Some(if index == 0 { &full } else { &smaller })
        );
        assert_eq!(change.after.as_ref(), Some(&smaller));
    }
    let changed = view(&app, scenario_id).await?.document;
    assert_journal(
        &app,
        scenario_id,
        command_id,
        &submitted_json,
        &inverse_json,
    )
    .await?;
    drop(app);
    let reopened = EuthetoApp::open(dependencies.clone())
        .await
        .map_err(|error| boxed(&error))?;
    reopened
        .execute(AppCommand::Undo {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(2),
        })
        .await
        .map_err(|error| boxed(&error))?;
    let undone = view(&reopened, scenario_id).await?;
    assert_eq!(undone.revision, Revision::new(3));
    assert_eq!(undone.document, original);
    assert_journal(
        &reopened,
        scenario_id,
        command_id,
        &submitted_json,
        &inverse_json,
    )
    .await?;
    drop(reopened);
    let reopened = EuthetoApp::open(dependencies)
        .await
        .map_err(|error| boxed(&error))?;
    reopened
        .execute(AppCommand::Redo {
            request_id: request_id()?,
            scenario_id,
            expected_revision: Revision::new(3),
        })
        .await
        .map_err(|error| boxed(&error))?;
    let replayed = view(&reopened, scenario_id).await?;
    assert_eq!(replayed.revision, Revision::new(4));
    assert_eq!(replayed.document, changed);
    assert_journal(
        &reopened,
        scenario_id,
        command_id,
        &submitted_json,
        &inverse_json,
    )
    .await?;
    assert_history_len(&reopened, scenario_id, 2).await?;
    Ok(())
}
